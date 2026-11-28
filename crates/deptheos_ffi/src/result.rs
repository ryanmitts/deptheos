//! A stack as it runs ([`StackObserver`]) and what it makes ([`StackResult`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use deptheos_app::engine::work::Lane;
use deptheos_app::library::adjustments::RenderAdjustments;
use deptheos_app::library::export::{self, ExportFormat, NameParts};
use deptheos_app::library::history::{History, RenderId};
use deptheos_app::{
    Adjustments, CompletedStack, Engine as AppEngine, Look, PresentedImage, PresentedStack,
    SaveOptions,
};

use crate::request::{Ask, on_pool};
use crate::{DisplayImage, ExportPreferences, Request, Result, Told};

/// What a stack is doing, sent once per frame per phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StackPhase {
    Started,
    Aligning,
    Finalizing,
    /// The reverse check tripped: pass 1 restarts with the frames reversed.
    Reversed,
    Blending,
    Completed,
}

/// A running stack's progress after each event: the engine's
/// [`deptheos_app::runs::session::Progress`].
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct StackProgress {
    pub phase: StackPhase,
    /// The phase's count of `total`: "Aligning and stacking 12 of 66". 0
    /// while it (re)starts, `total` once finalizing.
    pub current: u32,
    /// The run's frame count.
    pub total: u32,
    /// Frames taken in so far, in run order: the rail's key to the live map;
    /// all from finalizing on.
    pub taken: u32,
    /// The run restarted from the last frame.
    pub reversed: bool,
    /// 0 to 1. It goes back only on a reversed restart; a two-pass run fills
    /// to half taking frames in, the rest blending.
    pub fraction: f64,
}

impl From<&deptheos_app::runs::session::Progress> for StackProgress {
    fn from(p: &deptheos_app::runs::session::Progress) -> Self {
        use deptheos_app::runs::session::Step;
        let total = p.frames();
        let (phase, current) = match p.step() {
            Step::Started => (StackPhase::Started, 0),
            Step::Aligning { current } => (StackPhase::Aligning, current),
            Step::Reversed => (StackPhase::Reversed, 0),
            Step::Finalized => (StackPhase::Finalizing, total),
            Step::Blending { current } => (StackPhase::Blending, current),
            Step::Completed => (StackPhase::Completed, total),
        };
        Self {
            phase,
            current,
            total,
            taken: p.taken(),
            reversed: p.reversed(),
            fraction: p.fraction() as f64,
        }
    }
}

/// The app's view of a running stack, called from the stacking thread
/// (progress) and a readback thread (live map), never the main thread.
#[uniffi::export(foreign)]
pub trait StackObserver: Send + Sync {
    /// After each of the run's events.
    fn progress(&self, progress: StackProgress);
    /// The focus or depth map so far, at the stack's `live_side`.
    fn live_map(&self, image: DisplayImage);
}

/// What a result is exported as (`deptheos_app::library::export::ExportFormat`).
/// DNG needs a linear (DNG-source) stack.
#[uniffi::remote(Enum)]
pub enum ExportFormat {
    Tiff,
    Jpeg,
    Png,
    Dng,
}

/// "TIFF", "JPEG": a format as the Export sheet names it.
#[uniffi::export]
pub fn export_format_label(format: ExportFormat) -> String {
    format.label().into()
}

/// Whether an export in `format` has 16 bits per channel, when `asked` and
/// the stack has them (`source`): JPEG never, DNG always
/// ([`ExportFormat::depth`]).
#[uniffi::export]
pub fn export_sixteen_bit(format: ExportFormat, asked: bool, source: bool) -> bool {
    format.depth(asked, source)
}

/// Whether the bit depth in `format` is fixed, not a choice: by JPEG and
/// DNG, or a stack without 16 bits (`source`).
#[uniffi::export]
pub fn export_depth_fixed(format: ExportFormat, source: bool) -> bool {
    format.is_depth_fixed(source)
}

/// Where an export of `stack` as `name` goes in `directory`: the name as a
/// file stem ([`export::clean_stem`]) with the format's extension. DNG is
/// refused for a non-linear stack.
pub(crate) fn export_target(
    stack: &CompletedStack,
    directory: &Path,
    name: &str,
    format: ExportFormat,
) -> std::result::Result<PathBuf, deptheos_app::Error> {
    if !format.is_available(stack.is_linear()) {
        return Err(deptheos_app::Error::NotRaw);
    }
    let file = format!("{}.{}", export::clean_stem(name), format.extension());
    Ok(directory.join(file))
}

/// A finished stack: its pixels on the CPU for every save, and its display
/// and map while the app takes the images it shows.
#[derive(uniffi::Object)]
pub struct StackResult {
    pub(crate) engine: Arc<AppEngine>,
    /// Shared with the paint worker while the result is retouched.
    pub(crate) stack: Arc<CompletedStack>,
    displays: Mutex<Displays>,
    /// The render it is in the Library, once saved or loaded: its export
    /// draws under that render's adjustments.
    saved: std::sync::OnceLock<(Arc<History>, RenderId)>,
    /// The reframe its pictures were handed over under, turned and
    /// flipped; the app draws the crop.
    reframe: deptheos_render::Reframe,
}

/// A result's display and map levels, full size down: for 44 MP about
/// 235 MB and 58 MB. Each is shared with the [`Picture`]s handed out, so
/// releasing here drops only the result's reference: a shown picture keeps
/// its pyramid.
///
/// `Held` (a stack, or [`Engine::load_render`]) → `Held` without its map
/// ([`Engine::save_render`], before the encode; the app remakes the map's
/// picture on the CPU when shown) → `Released`
/// ([`StackResult::release_displays`], once the app has its images). One
/// loaded for pixels ([`Engine::load_render_pixels`]) starts `Released`. A
/// picture asked of one not held is remade from the stack.
enum Displays {
    Held {
        /// `None` when its readback failed.
        display: Option<Arc<PresentedImage>>,
        /// `None` for a stack with no map, and once a save let go of it.
        map: Option<Arc<PresentedImage>>,
    },
    Released,
}

impl StackResult {
    pub(crate) fn new(engine: Arc<AppEngine>, presented: PresentedStack) -> Arc<Self> {
        let PresentedStack { stack, display, map, reframe } = presented;
        Arc::new(Self {
            engine,
            stack: Arc::new(stack),
            displays: Mutex::new(Displays::Held {
                display: display.map(Arc::new),
                map: map.map(Arc::new),
            }),
            saved: std::sync::OnceLock::new(),
            reframe,
        })
    }

    /// It is render `id` of `history`.
    pub(crate) fn saved_as(self: Arc<Self>, history: &Arc<History>, id: RenderId) -> Arc<Self> {
        if self.saved.set((history.clone(), id)).is_err() {
            tracing::debug!("a result saved twice keeps its first render");
        }
        self
    }

    /// No display or map: only its pixels are wanted.
    pub(crate) fn pixels_only(engine: Arc<AppEngine>, stack: CompletedStack) -> Arc<Self> {
        Arc::new(Self {
            engine,
            stack: Arc::new(stack),
            displays: Mutex::new(Displays::Released),
            saved: std::sync::OnceLock::new(),
            reframe: deptheos_render::Reframe::NONE,
        })
    }

    /// The display (`map` false) or the map, while held: shared, not
    /// copied.
    pub(crate) fn held(&self, map: bool) -> Option<Arc<PresentedImage>> {
        match &*crate::lock(&self.displays) {
            Displays::Held { display, map: depth } => if map { depth } else { display }.clone(),
            Displays::Released => None,
        }
    }

    /// Lit for the frame at `frame`, made from the stack; `None` when it
    /// can't be lit ([`PictureOf::LitMap`]).
    pub(crate) fn lit_map(&self, frame: &Path) -> Option<PresentedImage> {
        self.stack.frame_map.as_ref()?;
        let index = self.stack.paths.iter().position(|p| p == frame)?;
        if self.stack.geometry.is_left_out(index) {
            return None;
        }
        self.stack.present_map(Some(index))
    }

    /// In this result's geometry, as [`PictureOf::ComparedFrame`] shows it,
    /// every level.
    pub(crate) fn compared_frame(&self, path: &Path, contribution: bool) -> Result<PresentedImage> {
        let index = self.stack.paths.iter().position(|p| p == path);
        let overlay = match contribution {
            true => deptheos_app::Overlay::Contribution,
            false => deptheos_app::Overlay::None,
        };
        self.engine
            .load_compare(path, index, Look::stored(&self.stack), overlay)
            .told(self.engine.words())
    }

    /// In this result's geometry, as [`PictureOf::ComparedRender`] shows
    /// it, every level.
    pub(crate) fn compared_render(&self, other: &StackResult) -> Result<PresentedImage> {
        self.engine
            .load_render_compare(&self.stack, Look::shown(&other.stack, &Adjustments::NEUTRAL))
            .told(self.engine.words())
    }

    /// Drop the map's levels, keeping the display: a save, having taken its
    /// thumbnail, holds only the display shown next through the encode. The
    /// map is remade from the frame map on the CPU ([`PictureOf::Map`])
    /// when asked.
    pub(crate) fn release_map(&self) {
        if let Displays::Held { map: map @ Some(_), .. } = &mut *crate::lock(&self.displays) {
            tracing::debug!("result displays: held -> held without its map, for the save's encode");
            *map = None;
        }
    }

    /// While the display is held.
    pub(crate) fn thumbnail(&self) -> Option<image::RgbImage> {
        match &*crate::lock(&self.displays) {
            Displays::Held { display, .. } => display.as_ref()?.thumbnail(),
            Displays::Released => None,
        }
    }

    /// Linear camera RGB from DNG sources: DNG export is available.
    fn is_linear(&self) -> bool {
        self.stack.is_linear()
    }
}

/// A frame a stack left out, and why (`deptheos_app::left_out_message`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LeftOutInfo {
    pub path: PathBuf,
    pub reason: String,
}

/// One included frame's share of a result
/// ([`StackResult::frame_contributions`]).
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct FrameContribution {
    pub path: PathBuf,
    /// Of the result's pixels, 0–1; 0 when left out.
    pub share: f32,
    /// [`StackResult::left_out`] says why.
    pub left_out: bool,
}

/// A frame's color on the depth map, sRGB ([`StackResult::frame_colors`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LegendColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// The rail's key while a stack of `total` frames runs, in the order given
/// (`deptheos_app::engine::present::running_legend`): each frame's map
/// color once taken in, `None` before. `taken` is the count of
/// [`StackPhase::Aligning`]; `reversed` once [`StackPhase::Reversed`] has
/// come, restarting the count. The colors match
/// [`StackResult::frame_colors`] once done. Cheap.
#[uniffi::export]
pub fn running_legend(total: u32, taken: u32, reversed: bool) -> Vec<Option<LegendColor>> {
    deptheos_app::engine::present::running_legend(total as usize, taken as usize, reversed)
        .into_iter()
        .map(|c| c.map(|[red, green, blue]| LegendColor { red, green, blue }))
        .collect()
}

#[uniffi::export]
impl StackResult {
    /// The reframe its display and map were handed over under: they come
    /// turned and flipped, and the viewer's result pane draws the crop as
    /// a window (`PaneFraming`).
    pub fn reframe(&self) -> deptheos_render::Reframe {
        self.reframe
    }

    /// The frames left out and why, then any load notes; `None` when
    /// nothing.
    pub fn notice(&self) -> Option<String> {
        self.stack.notice(self.engine.words())
    }

    /// Each with why.
    pub fn left_out(&self) -> Vec<LeftOutInfo> {
        let words = self.engine.words();
        self.stack
            .left_out_frames()
            .map(|(path, why)| LeftOutInfo {
                path: path.to_path_buf(),
                reason: deptheos_app::left_out_message(words, [(path, why)])
                    .unwrap_or_else(|| why.to_owned()),
            })
            .collect()
    }

    pub fn width(&self) -> u32 {
        self.stack.width
    }

    pub fn height(&self) -> u32 {
        self.stack.height
    }

    /// Whether PNG and TIFF can keep 16 bits (DNG, or 16-bit sources such as
    /// camera RAW and 16-bit TIFF).
    pub fn has_sixteen_bit(&self) -> bool {
        self.stack.has_sixteen_bit()
    }

    /// In the order offered.
    pub fn export_formats(&self) -> Vec<ExportFormat> {
        ExportFormat::offered(self.is_linear())
    }

    /// The stacked frames' paths, in the order given.
    pub fn frames(&self) -> Vec<PathBuf> {
        self.stack
            .paths
            .iter()
            .enumerate()
            .filter(|&(i, _)| !self.stack.geometry.is_left_out(i))
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// Each frame's legend color, in the order given (every frame, as
    /// [`StackResult::frame_contributions`] lists them): `None` for a
    /// left-out frame, which the map never names. Cheap.
    pub fn frame_colors(&self) -> Vec<Option<LegendColor>> {
        (0..self.stack.paths.len())
            .map(|i| {
                self.stack.frame_color(i).map(|[red, green, blue]| LegendColor { red, green, blue })
            })
            .collect()
    }

    /// Each frame's share, in the order of [`StackResult::frames`], left-out
    /// ones at 0; the stacked shares sum to 1. `None` without a frame map.
    /// Counted when stacked or loaded; cheap.
    pub fn frame_contributions(&self) -> Option<Vec<FrameContribution>> {
        let shares = self.stack.frame_contributions()?;
        let stack = &self.stack;
        let frames = stack.paths.iter().zip(shares.iter()).enumerate();
        let contribution = |(i, (path, &share)): (usize, (&PathBuf, &f32))| FrameContribution {
            path: path.clone(),
            share,
            left_out: stack.geometry.is_left_out(i),
        };
        Some(frames.map(contribution).collect())
    }

    /// The frame the pixel at `x`, `y` came from (as
    /// [`crate::paint::PaintSample`]): the sharpest there. `None` off the
    /// image or without a frame map.
    pub fn frame_at(&self, x: f32, y: f32) -> Option<PathBuf> {
        frame_at(&self.stack, x, y)
    }

    /// A frame map is kept, so contribution can be shown
    /// ([`PictureOf::ComparedFrame`]).
    pub fn has_contribution(&self) -> bool {
        self.stack.frame_map.is_some()
    }
}

/// The frame `stack` took the pixel at `x`, `y` from; `None` off the image,
/// at a NaN, or without a frame map.
pub(crate) fn frame_at(stack: &CompletedStack, x: f32, y: f32) -> Option<PathBuf> {
    if !(x >= 0.0 && y >= 0.0 && x < stack.width as f32 && y < stack.height as f32) {
        return None;
    }
    let index = stack.frame_at(x as u32, y as u32)?;
    stack.paths.get(index).cloned()
}

impl StackResult {
    /// Drop the display and map once the app has its images and the render
    /// is saved (its thumbnail taken first). Exports and retouching read the
    /// stack's pixels. A [`Picture`] already handed out
    /// ([`StackResult::picture`]) holds its own reference.
    /// [`StackResult::pictures`] calls it.
    pub(crate) fn release_displays(&self) {
        let mut displays = crate::lock(&self.displays);
        if let Displays::Held { .. } = *displays {
            tracing::debug!("result displays: held -> released");
        }
        *displays = Displays::Released;
    }

    /// The adjustments of the render it is, read after their pending
    /// writes; a result not saved has none.
    pub(crate) fn saved_adjustments(&self) -> Result<RenderAdjustments> {
        match self.saved.get() {
            Some((history, id)) => history.adjustments_to_export(id).told(self.engine.words()),
            None => Ok(RenderAdjustments::NONE),
        }
    }

    /// To `directory` as `<name>.<extension>` with `save`; returns the path.
    fn export_saved(
        &self,
        directory: &Path,
        name: &str,
        format: ExportFormat,
        save: SaveOptions,
    ) -> Result<PathBuf> {
        let words = self.engine.words();
        let path = export_target(&self.stack, directory, name, format).told(words)?;
        let saved = self.saved_adjustments()?;
        self.engine.save(Look::saved(&self.stack, &saved), &path, save).told(words)?;
        Ok(path)
    }
}

#[uniffi::export]
impl StackResult {
    /// The file stem `pattern` makes for render `render` of `stack` exported
    /// on `date` (`yyyy-MM-dd`); {method} is its method in the engine's
    /// language. Pass it as `name` to [`Self::export`].
    pub fn export_file_stem(
        &self,
        pattern: String,
        stack: String,
        render: Option<u32>,
        date: String,
    ) -> String {
        let method = self.stack.config.kind().label(self.engine.words());
        let parts = NameParts { stack: &stack, render, method: &method, date: &date };
        export::file_stem(&pattern, &parts)
    }

    /// Write to `directory` as `<name>.<extension>`, the name cleaned, with
    /// the export preferences: JPEG quality, 16 bits and TIFF compression
    /// (`name` is chosen, from [`Self::export_file_stem`] or typed). Returns
    /// the path. Written on the pool for `request`.
    pub async fn export(
        self: Arc<Self>,
        request: Arc<Request>,
        directory: PathBuf,
        name: String,
        format: ExportFormat,
        preferences: ExportPreferences,
    ) -> Result<PathBuf> {
        let save = preferences.save_options();
        let this = self.clone();
        let write = move || this.export_saved(&directory, &name, format, save);
        on_pool(self.engine.words(), &request, Lane::Background, Ask::Own("export"), write).await
    }
}
