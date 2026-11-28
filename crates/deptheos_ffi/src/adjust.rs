//! Adjust for the iOS app, over the shared session
//! (`deptheos_app::adjust::AdjustSession`) as the desktop drives it
//! (ADR-153): the sliders and the reframe of one render at a time, its drag
//! preview, histogram and settled picture, undo, paste and the
//! eyedropper, with the panel's sliders worded by the engine so the app
//! needs no code per slider.
//!
//! Every call only queues work, so is safe on the main thread. The
//! observer is called in order on whichever thread the session reports
//! from.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use deptheos_app::Engine as AppEngine;
use deptheos_app::adjust::preview::{FlatPicture, Shown};
use deptheos_app::adjust::{Clipping, Counted, Group, Histogram, Panel, Slider, Update};
use deptheos_app::engine::work::Lane;
use deptheos_app::library::adjustments::RenderAdjustments;
use deptheos_app::library::adjustments::rules::StackKind;
use deptheos_app::library::history::RenderId;
use deptheos_app::library::thumbnails::rewrite::Rewrote;
use deptheos_app::view::{Framing, Pane, Placement, Tool};
use deptheos_render::dng_render::Region;
use deptheos_render::reframe::{Flip, Rect, Size, Turn};
use deptheos_render::{Reframe, slider::Track};

use crate::{DisplayImage, Engine, Picture, Result, StackResult, Told, level_image};

/// An adjustment slider (`deptheos_app::adjust::Slider`).
#[uniffi::remote(Enum)]
pub enum Slider {
    Temperature,
    Tint,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Clarity,
    Texture,
    Vibrance,
    Saturation,
    Sharpening,
    SharpenRadius,
    SharpenDetail,
    SharpenMasking,
    Vignette,
    VignetteMidpoint,
}

/// A heading of the panel (`deptheos_app::adjust::Group`).
#[uniffi::remote(Enum)]
pub enum Group {
    WhiteBalance,
    Tone,
    Presence,
    Sharpening,
    Vignette,
}

/// Quarter-turns clockwise, after the flip (`deptheos_adjust::Turn`).
#[uniffi::remote(Enum)]
pub enum Turn {
    None,
    Quarter,
    Half,
    ThreeQuarters,
}

/// A mirror left to right, before the turn (`deptheos_adjust::Flip`).
#[uniffi::remote(Enum)]
pub enum Flip {
    None,
    Horizontal,
}

/// A rectangle of the stored picture, in its pixels.
#[uniffi::remote(Record)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A render's reframe: the rectangle kept (`None`: the whole picture),
/// then the flip and the turn.
#[uniffi::remote(Record)]
pub struct Reframe {
    pub crop: Option<Rect>,
    pub flip: Flip,
    pub turn: Turn,
}

/// What a pane shows of a render (`deptheos_app::view::Pane`).
#[uniffi::remote(Enum)]
pub enum Pane {
    Result,
    Uncropped,
    Retouch,
    Stored,
}

/// Adjust's tools (`deptheos_app::view::Tool`).
#[uniffi::remote(Enum)]
pub enum Tool {
    Sliders,
    Crop,
}

/// One slider as the panel draws it: its words and numbers. `kelvin`: the
/// track is even to 7000 K and logarithmic above
/// ([`AdjustValue::position`]).
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AdjustSliderInfo {
    pub slider: Slider,
    pub label: String,
    pub help: String,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub default_value: f64,
    pub kelvin: bool,
}

/// A heading and its sliders, in order.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AdjustGroupInfo {
    pub group: Group,
    pub label: String,
    pub sliders: Vec<AdjustSliderInfo>,
}

/// The panel of a render: its headings in order, those its stack shows (no
/// color sliders on a monochrome one), and whether its white balance is in
/// kelvins.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AdjustPanel {
    pub groups: Vec<AdjustGroupInfo>,
    pub camera_raw: bool,
}

fn panel_info(panel: &Panel, engine: &AppEngine) -> AdjustPanel {
    let words = engine.words();
    let groups = panel
        .groups()
        .into_iter()
        .map(|(group, sliders)| AdjustGroupInfo {
            group,
            label: group.label(words),
            sliders: sliders
                .into_iter()
                .map(|slider| {
                    let spec = slider.spec(panel);
                    AdjustSliderInfo {
                        slider,
                        label: slider.label(words),
                        help: slider.help(words),
                        min: spec.min,
                        max: spec.max,
                        step: spec.step,
                        default_value: slider.neutral(panel),
                        kelvin: spec.track == Track::Kelvin,
                    }
                })
                .collect(),
        })
        .collect();
    AdjustPanel { groups, camera_raw: panel.kind == StackKind::CameraRaw }
}

/// A render's adjustments: what Copy Adjustments holds and the status
/// shows, read slider by slider against the render's panel.
#[derive(uniffi::Object)]
pub struct AdjustValue {
    engine: Arc<AppEngine>,
    pub(crate) value: RenderAdjustments,
    panel: Panel,
}

impl AdjustValue {
    fn of(engine: &Arc<AppEngine>, value: RenderAdjustments, panel: Panel) -> Arc<Self> {
        Arc::new(Self { engine: engine.clone(), value, panel })
    }
}

#[uniffi::export]
impl AdjustValue {
    /// `slider`'s value; a camera RAW stack's as-shot white reads as its
    /// kelvins and tint.
    pub fn get(&self, slider: Slider) -> f64 {
        slider.get(&self.value.adjustments, &self.panel)
    }

    /// `slider`'s value as the panel writes it: "+0.35 EV", "5200 K".
    pub fn text(&self, slider: Slider) -> String {
        slider.value_text(self.get(slider), &self.panel, self.engine.words())
    }

    /// `value` of `slider` as the panel writes it.
    pub fn text_of(&self, slider: Slider, value: f64) -> String {
        slider.value_text(value, &self.panel, self.engine.words())
    }

    /// A typed value of `slider`, on its range and step; `None` if it
    /// isn't a number.
    pub fn parse(&self, slider: Slider, text: String) -> Option<f64> {
        Some(slider.spec(&self.panel).snap(slider.parse(&text)?))
    }

    /// Where `value` sits on `slider`'s track, 0 to 1.
    pub fn position(&self, slider: Slider, value: f64) -> f64 {
        slider.spec(&self.panel).position(value)
    }

    /// The value at track position `p` (0 to 1), on a step.
    pub fn value_at(&self, slider: Slider, p: f64) -> f64 {
        let spec = slider.spec(&self.panel);
        spec.snap(spec.value_at(p))
    }

    /// Whether `slider` is at its default: its reset does nothing.
    pub fn is_default(&self, slider: Slider) -> bool {
        slider.is_neutral(&self.value.adjustments, &self.panel)
    }

    /// Whether every slider is at its default.
    pub fn is_neutral(&self) -> bool {
        self.value.adjustments.is_neutral()
    }

    pub fn reframe(&self) -> Reframe {
        self.value.reframe
    }

    /// The process version the values are read in.
    pub fn process_version(&self) -> u32 {
        self.value.adjustments.process().number()
    }
}

/// Where Adjust stands (`deptheos_app::adjust::machine::Phase`).
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum AdjustPhase {
    /// The GPU copies being made: the sliders set, nothing drawn yet.
    Opening,
    Ready,
    /// In the background or under memory pressure, the GPU copies freed.
    Away,
    /// [`AdjustSession::retry`] opens it again.
    Failed {
        message: String,
    },
}

/// Adjust on one render, as the app shows it.
#[derive(uniffi::Record)]
pub struct AdjustStatus {
    pub render_id: String,
    pub phase: AdjustPhase,
    /// The value shown, a change in progress included.
    pub value: Arc<AdjustValue>,
    pub undo: u32,
    pub redo: u32,
}

/// A histogram of the picture under its crop, the same at any zoom
/// (`deptheos_app::adjust::Histogram`): 256 bins of 8-bit display values
/// each, and the pixels at 0 and 255 per channel (red, green, blue).
/// `counted_full`: the clipping was counted over every pixel of the
/// settled picture, not the small draw.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AdjustHistogram {
    pub red: Vec<u32>,
    pub green: Vec<u32>,
    pub blue: Vec<u32>,
    pub luminance: Vec<u32>,
    pub pixels: u64,
    pub shadows: Vec<u64>,
    pub highlights: Vec<u64>,
    pub shadows_clipped: bool,
    pub highlights_clipped: bool,
    pub counted_full: bool,
}

impl From<&Histogram> for AdjustHistogram {
    fn from(h: &Histogram) -> Self {
        let Clipping { shadows, highlights, counted } = h.clipping;
        Self {
            red: h.channels[0].to_vec(),
            green: h.channels[1].to_vec(),
            blue: h.channels[2].to_vec(),
            luminance: h.luminance.to_vec(),
            pixels: h.pixels,
            shadows: shadows.to_vec(),
            highlights: highlights.to_vec(),
            shadows_clipped: h.clipping.shadows_clipped(),
            highlights_clipped: h.clipping.highlights_clipped(),
            counted_full: counted == Counted::Full,
        }
    }
}

/// A rectangle of the stored picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PictureRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl From<Region> for PictureRegion {
    fn from(r: Region) -> Self {
        Self { x: r.x, y: r.y, width: r.width, height: r.height }
    }
}

/// The colors a picture for the screen is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenSpace {
    /// sRGB, and a standard stack's own values with Color Management off,
    /// shown as they are.
    Srgb,
    DisplayP3,
}

impl From<deptheos_render::OutputSpace> for ScreenSpace {
    fn from(space: deptheos_render::OutputSpace) -> Self {
        match space {
            deptheos_render::OutputSpace::DisplayP3 => Self::DisplayP3,
            _ => Self::Srgb,
        }
    }
}

/// One flat picture of the drag preview, or of the view on screen as it
/// settles: `width` × `height` 8-bit BGRA, covering `covers` of the stored
/// picture turned and flipped as the render's reframe says (the crop left
/// to the app). The app reads its pixels where they are
/// ([`Self::bgra_address`], ADR-139) for as long as it holds it; its buffer
/// then goes back to the session for the next tick, so let go of each once
/// it is replaced on screen.
#[derive(uniffi::Object)]
pub struct AdjustPreview {
    session: Arc<deptheos_app::adjust::AdjustSession>,
    picture: Mutex<Option<FlatPicture>>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    covers: Region,
    space: ScreenSpace,
}

impl AdjustPreview {
    fn of(session: &Arc<deptheos_app::adjust::AdjustSession>, picture: FlatPicture) -> Arc<Self> {
        let (width, height, covers) = (picture.width, picture.height, picture.covers);
        let space = picture.space.into();
        Arc::new(Self {
            session: session.clone(),
            picture: Mutex::new(Some(picture)),
            width,
            height,
            covers,
            space,
        })
    }
}

impl Drop for AdjustPreview {
    fn drop(&mut self) {
        if let Some(picture) = crate::lock(&self.picture).take() {
            self.session.give_back(picture);
        }
    }
}

#[uniffi::export]
impl AdjustPreview {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The stored pixels it shows: the whole picture at fit.
    pub fn covers(&self) -> PictureRegion {
        self.covers.into()
    }

    pub fn space(&self) -> ScreenSpace {
        self.space
    }

    /// Where its pixels are: `width` × `height` × 4 bytes, rows tightly
    /// packed, BGRA. They stay there, unchanged, until this preview is let
    /// go of, so the app reads them in place while it holds it (a
    /// `CGDataProvider` whose release lets go). Nothing writes them while
    /// it lives: the buffer goes back to the session only on its drop.
    pub fn bgra_address(&self) -> u64 {
        crate::lock(&self.picture).as_ref().map_or(0, |p| p.pixels.as_ptr() as u64)
    }

    /// The bytes at [`Self::bgra_address`].
    pub fn byte_count(&self) -> u64 {
        crate::lock(&self.picture).as_ref().map_or(0, |p| p.pixels.len() as u64)
    }
}

/// The app's view of Adjust, called in order on whichever thread the
/// session reports from.
#[uniffi::export(foreign)]
pub trait AdjustObserver: Send + Sync {
    /// Adjust's state changed: [`AdjustSession::status`] says how.
    fn state_changed(&self);
    /// A tick of the drag preview: shown over the settled picture until
    /// the next.
    fn previewed(&self, preview: Arc<AdjustPreview>, histogram: AdjustHistogram);
    /// The settled picture of the view on screen, ahead of the whole: in
    /// place of the preview.
    fn on_screen(&self, preview: Arc<AdjustPreview>, histogram: AdjustHistogram);
    /// The whole picture of `render_id` under its value, as `image` at
    /// most the display side and `picture` at every size, the clipping
    /// counted over every pixel: in place of the display and every preview.
    fn settled(
        &self,
        render_id: String,
        image: DisplayImage,
        picture: Arc<Picture>,
        histogram: AdjustHistogram,
    );
    /// The histogram of `render_id` as opened, its whole picture already
    /// on screen; clipping counted on the small draw.
    fn counted(&self, render_id: String, histogram: AdjustHistogram);
    /// Adjust left `render_id`, its last write in. `redraw`: the screen
    /// doesn't show its saved value, so the viewer loads it again.
    fn closed(&self, render_id: String, redraw: bool);
    /// `render_id`'s thumbnail was drawn again: read it again
    /// ([`crate::ThumbnailPolicy::rewritten`]).
    fn rewritten(&self, render_id: String);
    /// Something to say briefly over the photo.
    fn notice(&self, message: String);
}

/// The session's updates to the observer.
struct Forward {
    observer: Arc<dyn AdjustObserver>,
    session: Mutex<std::sync::Weak<deptheos_app::adjust::AdjustSession>>,
    side: u32,
}

impl deptheos_app::adjust::Observer for Forward {
    fn update(&self, update: Update) {
        let session = crate::lock(&self.session).upgrade();
        match update {
            Update::Status(_) => self.observer.state_changed(),
            Update::Preview { picture, histogram } => {
                if let Some(s) = session {
                    self.observer.previewed(AdjustPreview::of(&s, picture), (&histogram).into());
                }
            }
            Update::OnScreen { picture, histogram } => {
                if let Some(s) = session {
                    self.observer.on_screen(AdjustPreview::of(&s, picture), (&histogram).into());
                }
            }
            Update::Settled { key, picture, histogram } => {
                if let Some(image) = level_image(&picture, self.side) {
                    let id = key.as_str().to_string();
                    self.observer.settled(id, image, Picture::of(picture), (&histogram).into());
                }
            }
            Update::Histogram { key, histogram } => {
                self.observer.counted(key.as_str().to_string(), (&histogram).into())
            }
            Update::Closed { key, redraw } => {
                self.observer.closed(key.as_str().to_string(), redraw)
            }
            Update::Rewritten(key) => self.observer.rewritten(key.as_str().to_string()),
            Update::Notice(text) => self.observer.notice(text),
        }
    }
}

/// Adjust, one render at a time. One per app, from
/// [`Engine::adjust_session`].
#[derive(uniffi::Object)]
pub struct AdjustSession {
    engine: Arc<AppEngine>,
    session: Arc<deptheos_app::adjust::AdjustSession>,
}

#[uniffi::export]
impl Engine {
    /// Adjust's session; `display_side` is the longest side
    /// [`AdjustObserver::settled`] sends as an image.
    pub fn adjust_session(
        &self,
        observer: Arc<dyn AdjustObserver>,
        display_side: u32,
    ) -> Arc<AdjustSession> {
        let forward = Arc::new(Forward {
            observer,
            session: Mutex::new(std::sync::Weak::new()),
            side: display_side,
        });
        let session = Arc::new(deptheos_app::adjust::AdjustSession::new(
            self.engine.clone(),
            self.history.clone(),
            forward.clone(),
        ));
        *crate::lock(&forward.session) = Arc::downgrade(&session);
        Arc::new(AdjustSession { engine: self.engine.clone(), session })
    }
}

impl AdjustSession {
    fn id(&self, render_id: &str) -> Result<RenderId> {
        crate::render_id(render_id).told(self.engine.words())
    }
}

#[uniffi::export]
impl AdjustSession {
    /// `None` while off. What [`AdjustObserver::state_changed`] said last,
    /// or newer.
    pub fn status(&self) -> Option<AdjustStatus> {
        let status = self.session.status()?;
        let panel = self.session.panel()?;
        let phase = match status.phase {
            deptheos_app::adjust::machine::Phase::Opening => AdjustPhase::Opening,
            deptheos_app::adjust::machine::Phase::Ready => AdjustPhase::Ready,
            deptheos_app::adjust::machine::Phase::Away => AdjustPhase::Away,
            deptheos_app::adjust::machine::Phase::Failed(message) => {
                AdjustPhase::Failed { message }
            }
        };
        Some(AdjustStatus {
            render_id: status.key.as_str().to_string(),
            phase,
            value: AdjustValue::of(&self.engine, status.value, panel),
            undo: status.undo as u32,
            redo: status.redo as u32,
        })
    }

    /// The panel of the render adjusted.
    pub fn panel(&self) -> Option<AdjustPanel> {
        Some(panel_info(&self.session.panel()?, &self.engine))
    }

    /// The render adjusted, while Adjust is on.
    pub fn adjusting(&self) -> Option<String> {
        self.session.adjusting().map(|k| k.as_str().to_string())
    }

    /// Adjust `result`, saved as render `render_id`, its adjustments read
    /// first. Another render open is left. The status follows, opening,
    /// then ready or failed.
    pub fn open(&self, result: Arc<StackResult>, render_id: String) -> Result<()> {
        let key = self.id(&render_id)?;
        self.session.open(key, result.stack.clone());
        Ok(())
    }

    /// What the viewer shows: `zoom` screen pixels per stored pixel, over
    /// `visible` of the stored picture. Said as it changes, for the next
    /// preview.
    pub fn view(&self, zoom: f32, visible: PictureRegion) {
        let PictureRegion { x, y, width, height } = visible;
        self.session.view(Shown { zoom, visible: Region { x, y, width, height } });
    }

    /// `slider` moves to `value`; a change goes on until [`Self::commit`].
    pub fn set_slider(&self, slider: Slider, value: f64) {
        self.session.set_slider(slider, value);
    }

    /// The reframe moves: a crop handle dragged, a turn or flip, until
    /// [`Self::commit`].
    pub fn set_reframe(&self, reframe: Reframe) {
        self.session.set_reframe(reframe);
    }

    /// The change ends: written and undoable if the value moved. After a
    /// drag, a typed value, an arrow key, a crop.
    pub fn commit(&self) {
        self.session.commit();
    }

    pub fn undo(&self) {
        self.session.undo();
    }

    pub fn redo(&self) {
        self.session.redo();
    }

    /// Every slider to neutral, the reframe kept: one change.
    pub fn reset_all(&self) {
        self.session.reset_all();
    }

    /// `slider` to its default: one change.
    pub fn reset(&self, slider: Slider) {
        self.session.reset(slider);
    }

    /// `copied`'s sliders onto the render adjusted, its reframe and process
    /// version kept: one change.
    pub fn paste(&self, copied: Arc<AdjustValue>) {
        self.session.paste(copied.value.adjustments.clone());
    }

    /// Set the white balance from the stored pixels under `x`, `y` on the
    /// picture as shown under its reframe: one change, or a notice of why
    /// not.
    pub fn eyedrop(&self, x: f64, y: f64) {
        self.session.eyedrop((x, y));
    }

    /// Before: render `render_id` of `result` as stored, with its paint
    /// and none of its adjustments, turned and flipped as `reframe` is,
    /// its base at most `base_side`. Drawn on the pool for `request`.
    pub async fn before(
        self: Arc<Self>,
        request: Arc<crate::Request>,
        result: Arc<StackResult>,
        render_id: String,
        reframe: Reframe,
        base_side: u32,
    ) -> Result<crate::Pictured> {
        let key = self.id(&render_id)?;
        let this = self.clone();
        let draw = move || {
            let image =
                this.session.before(&key, &result.stack, reframe).told(this.engine.words())?;
            Ok(crate::Pictured::of(Picture::of(image), base_side))
        };
        let ask = crate::request::Ask::Own("adjust-before");
        crate::request::on_pool(self.engine.words(), &request, Lane::Interactive, ask, draw).await
    }

    /// Open the render again after its open or a draw failed.
    pub fn retry(&self) {
        self.session.retry();
    }

    /// Leave Adjust, a change in progress committed;
    /// [`AdjustObserver::closed`] follows once its write is in.
    pub fn leave(&self) {
        self.session.leave();
    }

    /// The app goes to the background: the GPU copies are freed, the
    /// values kept (ADR-164, ADR-165).
    pub fn away(&self) {
        self.session.away();
    }

    /// The app is back, or memory is no longer short: the GPU copies are
    /// made again.
    pub fn back(&self) {
        self.session.back();
    }

    /// Memory is short: a moderate shed frees the preview's copies, a
    /// critical one all of them, as [`Self::away`] (ADR-114).
    pub fn shed(&self, level: crate::MemoryShed) {
        self.session.shed(level.into());
    }

    /// The device's thermal state is serious or critical (`hot`), or no
    /// longer: a drag's ticks at fit are drawn smaller while it is.
    pub fn set_hot(&self, hot: bool) {
        self.session.set_hot(hot);
    }

    /// The stored picture's paint changed (Retouch on the render): its
    /// copies are made again.
    pub fn stored_changed(&self) {
        self.session.stored_changed();
    }
}

// ---- outside Adjust: copy, paste, thumbnails

#[uniffi::export]
impl Engine {
    /// Render `render_id`'s adjustments, after every write queued before:
    /// what Copy Adjustments holds. A damaged file fails.
    pub async fn render_adjustments(
        self: Arc<Self>,
        render_id: String,
    ) -> Result<Arc<AdjustValue>> {
        let id = self.told(crate::render_id(&render_id))?;
        let copy = crate::request::Ask::Own("adjust-copy");
        self.pool(&crate::Request::lasting(), Lane::Interactive, copy, move |e| {
            let value = e.told(e.history.adjustments_to_export(&id))?;
            let stack = e.told(e.history.load_render(&id))?;
            Ok(AdjustValue::of(&e.engine, value, Panel::of(&stack)))
        })
        .await
    }

    /// Paste `copied`'s sliders onto render `render_id` (not the one
    /// adjusted: [`AdjustSession::paste`] is), its reframe and process
    /// version kept, written as one change, then its thumbnail drawn again
    /// ([`AdjustObserver`] isn't told: the app reads it again). Never
    /// canceled.
    pub async fn paste_adjustments(
        self: Arc<Self>,
        render_id: String,
        copied: Arc<AdjustValue>,
    ) -> Result<()> {
        let id = self.told(crate::render_id(&render_id))?;
        let copied = copied.value.adjustments.clone();
        let this = self.clone();
        let lasting = crate::Request::lasting();
        let paste = crate::request::Ask::Own("adjust-paste");
        let pasted = self
            .pool(&lasting, Lane::Background, paste, move |e| {
                e.told(e.history.paste_adjustments(&id, &copied)).map(|_| id)
            })
            .await?;
        this.rewrite(pasted).await.map(|_| ())
    }

    /// Draw render `render_id`'s thumbnail again under its adjustments and
    /// paint, after Retouch or Adjust left it changed: whether it was
    /// written (a later one may overtake it). The app then drops the one it
    /// holds ([`crate::ThumbnailPolicy::rewritten`]).
    pub async fn rewrite_thumbnail(self: Arc<Self>, render_id: String) -> Result<bool> {
        let id = self.told(crate::render_id(&render_id))?;
        self.rewrite(id).await
    }
}

impl Engine {
    async fn rewrite(self: &Arc<Self>, id: RenderId) -> Result<bool> {
        let history = self.history.clone();
        let engine = self.engine.clone();
        let load = crate::request::Ask::Own("thumbnail-rewrite");
        let loaded = id.clone();
        let stack = self
            .pool(&crate::Request::lasting(), Lane::Background, load, move |e| {
                e.told(e.history.load_render(&loaded)).map(Arc::new)
            })
            .await?;
        let rewrote = self.told(history.rewrite_thumbnail_painted(engine, id, stack).await)?;
        Ok(matches!(rewrote, Rewrote::Written))
    }
}

// ---- the panes and the pointer

/// A point, in continuous coordinates (pixel centers at half-integers).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct PicturePoint {
    pub x: f64,
    pub y: f64,
}

/// A stored pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PicturePixel {
    pub x: u32,
    pub y: u32,
}

/// Where a pane's picture sits on screen: screen points per shown pixel
/// and the screen point of its top left (`deptheos_app::view::Placement`).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct PicturePlacement {
    pub zoom: f64,
    pub origin: PicturePoint,
}

impl From<PicturePlacement> for Placement {
    fn from(p: PicturePlacement) -> Self {
        Self { zoom: p.zoom, origin: (p.origin.x, p.origin.y) }
    }
}

impl From<Placement> for PicturePlacement {
    fn from(p: Placement) -> Self {
        Self { zoom: p.zoom, origin: PicturePoint { x: p.origin.0, y: p.origin.1 } }
    }
}

/// How a pane shows a render's stored picture under its reframe
/// (`deptheos_app::view::Framing`): the result reframed, every other view
/// turned and flipped but uncropped, the crop marked where it applies.
/// Pointers map back to stored pixels through it.
#[derive(uniffi::Object)]
pub struct PaneFraming {
    framing: Framing,
}

#[uniffi::export]
impl PaneFraming {
    /// A `width` × `height` stored picture under `reframe`, as `pane`
    /// shows it.
    #[uniffi::constructor]
    pub fn new(width: u32, height: u32, reframe: Reframe, pane: Pane) -> Arc<Self> {
        Arc::new(Self { framing: Framing::of(Size { width, height }, reframe, pane) })
    }

    /// What the hand-off applies: the crop only for the result.
    pub fn drawn(&self) -> Reframe {
        self.framing.drawn()
    }

    /// The size shown.
    pub fn size(&self) -> PicturePixel {
        let Size { width, height } = self.framing.size();
        PicturePixel { x: width, y: height }
    }

    /// The stored point under shown point `p`.
    pub fn to_stored(&self, p: PicturePoint) -> PicturePoint {
        let (x, y) = self.framing.to_stored((p.x, p.y));
        PicturePoint { x, y }
    }

    /// The shown point over stored point `p`.
    pub fn to_shown(&self, p: PicturePoint) -> PicturePoint {
        let (x, y) = self.framing.to_shown((p.x, p.y));
        PicturePoint { x, y }
    }

    /// The stored pixel under shown point `p`, if it is on the picture.
    pub fn stored_pixel(&self, p: PicturePoint) -> Option<PicturePixel> {
        let (x, y) = self.framing.stored_pixel((p.x, p.y))?;
        Some(PicturePixel { x, y })
    }

    /// The crop marked, in shown pixels; `None` where it isn't marked.
    pub fn crop_mark(&self) -> Option<Rect> {
        self.framing.crop_mark()
    }

    /// The frame sharpest under shown point `p` of `result` (Sharpest
    /// Frame), by path; `None` off the picture or with no map.
    pub fn frame_at(&self, result: Arc<StackResult>, p: PicturePoint) -> Option<PathBuf> {
        let i = self.framing.frame_at(&result.stack, (p.x, p.y))?;
        result.stack.paths.get(i).cloned()
    }

    /// The placement of `to` at `placement`'s zoom that keeps the stored
    /// point under screen point `screen` where this pane had it: switching
    /// between the reframed and the uncropped view.
    pub fn keeping(
        &self,
        placement: PicturePlacement,
        to: Arc<PaneFraming>,
        screen: PicturePoint,
    ) -> PicturePlacement {
        Placement::from(placement).keeping(&self.framing, &to.framing, (screen.x, screen.y)).into()
    }
}

#[cfg(test)]
#[path = "adjust_tests.rs"]
mod tests;
