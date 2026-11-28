//! The image viewport: pan/zoom, the compare divider and the side-by-side
//! panes, painted directly into GPUI's frame; its loads (`loads.rs`) run on
//! the engine's pool.
//!
//! The view transform (`transform.rs`) places the image, drawn as GPUI
//! image tiles culled to the pane (`images.rs`), so pan and zoom are pure
//! layout. Two slots hold what the panes show: `primary` (the result or a
//! preview) and `secondary` (the compare source, or render B). In paint
//! mode they hold the canvas and the paint source, and drags become
//! [`ViewerEvent::Paint`] strokes.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

mod adjusted;
mod draw;
mod framed;
mod input;
mod loads;
mod transform;

use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::RenderId;
use deptheos_app::paint::{Dab, DisplayPatch};
use deptheos_app::view::{self, Action, Facts, Light, Mode};
pub use deptheos_app::view::{Layout, Look, Pick};
use deptheos_app::{CompletedStack, Overlay, PresentedImage};
use gpui::prelude::*;
use gpui::{Bounds, Context, EventEmitter, Pixels, Point, SharedString, Window, canvas, div, rgb};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::h_flex;
use gpui_kit::component::spinner::Spinner;
pub use transform::ViewTransformController;

use input::Gesture;
use loads::{FrameLoad, LitLoad, Loads};

use crate::files::FileId;
use crate::images::{Atlas, TiledImage};
use crate::look::{Fill, hud_text, overlay_surface};
use crate::words::say;

/// What the viewport shows: the engine's view (`deptheos_app::view`,
/// ADR-149), its frames named by path. A comparison without its compare
/// image yet draws like the result alone.
///
/// Lifecycle: an alias of the engine's `view::Mode`, read from the `View`
/// the [`Viewer`] holds and changed only by [`Viewer::on`]. The lit map's
/// picture is a `Loads<LitLoad>`; the frame on screen is `Shown`, changed
/// only by `set_shown`.
pub type ViewMode = view::Mode<PathBuf, RenderId>;
/// A change to the view, for [`Viewer::on`].
pub type ViewEvent = view::Event<PathBuf, RenderId>;
/// What the app does after one (`AppView::view`): a frame shown.
pub type ViewAction = view::Action<PathBuf>;

/// Frame `index` of `stack` as an event names it, with whether it supplied
/// part of the result (a frame left out, or any of a stack without a frame
/// map, didn't).
pub fn pick(stack: &CompletedStack, index: usize) -> Option<Pick<PathBuf>> {
    let id = stack.paths.get(index)?.clone();
    let supplied = stack.frame_map.is_some() && !stack.geometry.is_left_out(index);
    Some(Pick { id, supplied })
}

/// What to load.
#[derive(Clone)]
pub enum Source {
    /// The file on its own, before a stack.
    File(PathBuf),
    /// A file brought into a finished stack's geometry and color, as the
    /// compare source; `frame_index` is its position in the stack.
    Compare {
        path: PathBuf,
        frame_index: Option<usize>,
        stack: Arc<CompletedStack>,
        overlay: Overlay,
    },
    /// Another finished stack (a saved render) in `stack`'s geometry.
    Render { stack: Arc<CompletedStack>, other: OtherStack },
}

/// Reads the stack a [`Source::Render`] compares against; runs with the load.
pub type OtherStack = Arc<dyn Fn() -> Result<CompletedStack, deptheos_app::Error> + Send + Sync>;

/// What the two compare panes show: the compare source, then the result.
pub type CompareLabels = (SharedString, SharedString);

/// Turns a [`Source`] into display pixels with its overlay; runs on a
/// worker thread ([`crate::work::run`]).
pub type Loader = Arc<dyn Fn(&Source) -> Result<PresentedImage, deptheos_app::Error> + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub enum ViewerEvent {
    /// A frame (`file`) or a render (`file` is `None`) failed to load;
    /// `message` follows the caller's lead.
    LoadFailed { file: Option<FileId>, message: crate::notices::Worded },
    /// The Paint tool's pointer, in image pixels.
    Paint(PaintInput),
    /// ⌥-click with the Paint tool: paint from the frame sharpest there.
    PickAt { x: u32, y: u32 },
    /// The result's pixel under the pointer on the depth map, or `None`.
    MapPointer(Option<(u32, u32)>),
    /// A click (no pan) on the depth map, in result pixels.
    MapClick { x: u32, y: u32 },
    /// In Adjust: what the viewport shows of the stored picture changed.
    Shown(deptheos_app::adjust::preview::Shown),
    /// A click with the eyedropper, in stored pixels.
    Eyedrop { x: f64, y: f64 },
    /// The crop tool's handle dragged: the reframe it makes.
    Crop(deptheos_app::reframe::Reframe),
    /// The crop tool's drag released.
    CropEnd,
}

/// A stroke as the pointer draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaintInput {
    Begin(Dab),
    Move(Dab),
    End,
}

/// What a left drag does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pan,
    Paint,
    /// Adjust's white balance eyedropper: a click picks the spot.
    Eyedropper,
    /// Adjust's crop tool, its drags shaped by the aspect.
    Crop(deptheos_app::adjust::crop::Aspect),
}

/// The brush ring: radius in image pixels, and hardness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub radius: f32,
    pub hardness: f32,
}

/// Keys that act while held.
///
/// Lifecycle: each key up or down, set by its key-down and key-up
/// ([`Viewer::set_space_pan`], [`Viewer::set_peek`]); a key-up may never
/// arrive, so [`Viewer::release_keys`] releases them all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct HeldKeys {
    /// Space: drags pan even with the Paint tool.
    space: bool,
    /// Backslash, with the Paint tool: the source shown in place of the canvas.
    source: bool,
}

/// The whole depth map's picture: a finished stack's is held only while the
/// map is shown, and made again from its stack when shown again.
///
/// Lifecycle: `None`, `Held` or `Off`, changed only by `set_map`, which
/// frees the replaced picture from the atlas. `None` → `Held` (a live or a
/// result's map); `Held` with its stack → `Off` when the map is left
/// ([`Viewer::fit_map`]); `Off` → `Held` when the picture made again lands
/// ([`Viewer::make_map`]). A new result, run or lone frame replaces any.
enum MapPicture {
    None,
    /// The picture and its stack; none for a run's live map, which is kept.
    Held(TiledImage, Option<Arc<CompletedStack>>),
    /// A finished stack's map, its picture let go of while the map is off.
    Off(Arc<CompletedStack>),
}

impl MapPicture {
    /// A map to show, held or not.
    fn exists(&self) -> bool {
        !matches!(self, Self::None)
    }

    fn image(&self) -> Option<&TiledImage> {
        match self {
            Self::Held(image, _) => Some(image),
            Self::None | Self::Off(_) => None,
        }
    }

    fn image_mut(&mut self) -> Option<&mut TiledImage> {
        match self {
            Self::Held(image, _) => Some(image),
            Self::None | Self::Off(_) => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Held(..) => "held",
            Self::Off(_) => "off",
        }
    }
}

pub struct Viewer {
    view: ViewTransformController,
    /// What it shows; changed only by [`Self::on`].
    state: view::View<PathBuf, RenderId>,
    /// The result shown, whose map is lit.
    stack: Option<Arc<CompletedStack>>,
    /// The stored size of the render shown, and its reframe: the pictures
    /// held are turned and flipped as it says (`framed.rs`).
    picture: Option<deptheos_app::reframe::Size>,
    reframe: deptheos_app::reframe::Reframe,
    /// How the pictures held were last framed.
    framed: framed::Framed,
    /// Compare divider, 0..1 of the viewport width.
    split: f32,
    /// The result or the preview.
    primary: Option<TiledImage>,
    /// In Adjust: a drag's preview, or the view on screen settled first,
    /// drawn over `primary` until the settled picture replaces it.
    flat: Option<crate::adjust::Flat>,
    /// In Adjust: the render without its adjustments, drawn while held.
    before: Option<TiledImage>,
    /// What the viewport last showed of the stored picture, for Adjust.
    adjust_view: Option<deptheos_app::adjust::preview::Shown>,
    /// The whole depth map; changed only by [`Self::set_map`].
    map: MapPicture,
    /// The map lit for a frame, drawn while `view::Light::Lit`; the previous
    /// one is held until the newer lands.
    lit_map: Option<TiledImage>,
    /// The compare source.
    secondary: Option<TiledImage>,
    /// What the images hold in GPUI's atlas.
    atlas: Atlas,
    /// Flushes drops waiting while no frame is drawn, after
    /// [`crate::images::DROP_AFTER`]; dropped to cancel.
    flushing: Option<gpui::Task<()>>,
    /// Starts [`Self::flushing`] whenever the viewer changes.
    _flush: gpui::Subscription,
    /// Over the compare panes, when the compare source's loader gave them.
    compare_labels: Option<CompareLabels>,
    /// What the held mouse button is doing, if one is.
    gesture: Gesture,
    /// A button is held. Shared with the mouse-move handler so a plain move
    /// never touches the entity.
    held: Rc<Cell<bool>>,
    /// Every move matters (Paint tool's ring, or the map's readout). Set by
    /// [`Self::sync_tracking`].
    tracking: Rc<Cell<bool>>,
    /// The pointer over the viewport, in window coordinates.
    pointer: Rc<Cell<Option<Point<Pixels>>>>,
    tool: Tool,
    ring: Ring,
    /// The pointer over the viewport, in window coordinates.
    hover: Option<Point<Pixels>>,
    /// The keys that act while they are held.
    held_keys: HeldKeys,
    /// The source frame on screen or loading, reloaded when its overlay
    /// changes. Changed only by [`Self::set_shown`].
    shown: Option<Shown>,
    /// Each pane's clip and image rect as last laid out, in window coordinates.
    panes: Vec<(Bounds<Pixels>, Bounds<Pixels>)>,
    last_frame: Option<Instant>,
    /// The viewport's last layout bounds, in window coordinates.
    bounds: Bounds<Pixels>,
    /// Makes the frames' (and renders') pictures, through the frames' loads.
    loader: Loader,
    /// The frames' loads.
    loads: Loads<FrameLoad>,
    /// The loads that light the depth map.
    lighting: Loads<LitLoad>,
}

/// A source frame's load, as asked for, so it can be asked for again.
#[derive(Clone)]
struct Shown {
    file: Option<FileId>,
    name: String,
    source: Source,
    labels: Option<CompareLabels>,
}

impl EventEmitter<ViewerEvent> for Viewer {}

impl Viewer {
    pub fn new(loader: Loader, cx: &mut Context<Self>) -> Self {
        let flush = cx.observe_self(|this, cx| this.flush_later(cx));
        Self {
            view: ViewTransformController::new(),
            state: view::View::new(),
            stack: None,
            picture: None,
            reframe: deptheos_app::reframe::Reframe::NONE,
            framed: framed::Framed::default(),
            split: 0.5,
            primary: None,
            flat: None,
            before: None,
            adjust_view: None,
            map: MapPicture::None,
            lit_map: None,
            secondary: None,
            atlas: Atlas::default(),
            flushing: None,
            _flush: flush,
            compare_labels: None,
            gesture: Gesture::Idle,
            held: Rc::default(),
            tracking: Rc::default(),
            pointer: Rc::default(),
            tool: Tool::Pan,
            ring: Ring { radius: 30.0, hardness: 0.8 },
            hover: None,
            held_keys: HeldKeys::default(),
            shown: None,
            panes: Vec::new(),
            last_frame: None,
            bounds: Bounds::default(),
            loader,
            loads: Loads::new("load"),
            lighting: Loads::new("light"),
        }
    }

    /// Flush the atlas drops even when no frame is drawn (viewer off
    /// screen), or replaced images keep their memory until it is shown.
    fn flush_later(&mut self, cx: &mut Context<Self>) {
        if self.flushing.is_some() || !self.atlas.has_drops() {
            return;
        }
        let mut mark = self.atlas.drop_mark();
        self.flushing = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(crate::images::DROP_AFTER).await;
                let waiting = this.update(cx, |v, cx| {
                    let waiting = v.atlas.flush_before(mark, cx);
                    mark = v.atlas.drop_mark();
                    waiting
                });
                if !waiting.unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |v, _| v.flushing = None);
        }));
    }

    /// An image is shown: there is something to zoom.
    pub fn has_content(&self) -> bool {
        self.primary.is_some() || self.map.exists()
    }

    /// A map is held (a render saved without frame names has none).
    pub fn has_map(&self) -> bool {
        self.map.exists()
    }

    /// The one place the map's picture changes; the replaced one leaves
    /// the atlas.
    fn set_map(&mut self, map: MapPicture) {
        let from = self.map.name();
        if let MapPicture::Held(image, _) = std::mem::replace(&mut self.map, map) {
            self.atlas.replace(&mut Some(image), None);
        }
        let to = self.map.name();
        if from != to {
            tracing::debug!("viewer map: {from} -> {to}");
        }
    }

    /// While the map is off, a finished stack's picture goes
    /// ([`MapPicture::Off`]).
    fn fit_map(&mut self) {
        if !self.state.now().is_map()
            && let MapPicture::Held(_, Some(stack)) = &self.map
        {
            let stack = stack.clone();
            self.set_map(MapPicture::Off(stack));
        }
    }

    /// Make the map's picture again off the UI thread; the result stands in
    /// until it lands.
    fn make_map(&mut self, cx: &mut Context<Self>) {
        if let (
            Mode::Map { light: Light::Whole | Light::Marked { .. }, .. },
            MapPicture::Off(stack),
        ) = (self.state.now(), &self.map)
        {
            let label = say!(cx, "viewer-loading-depth-map").into();
            self.request(label, LitLoad { frame: None, stack: stack.clone() }, cx);
        }
    }

    pub fn has_compare(&self) -> bool {
        self.secondary.is_some()
    }

    pub fn zoom_percent(&self) -> i32 {
        self.view.zoom_percent()
    }

    pub fn is_fitted(&self) -> bool {
        self.view.is_fitted()
    }

    /// The listed file shown (or loading) as a source frame, if any.
    pub fn shown_file(&self) -> Option<FileId> {
        self.shown.as_ref().and_then(|s| s.file)
    }

    /// The stack position of the compare frame shown or loading.
    pub fn shown_frame_index(&self) -> Option<usize> {
        match &self.shown.as_ref()?.source {
            Source::Compare { frame_index, .. } => *frame_index,
            Source::File(_) | Source::Render { .. } => None,
        }
    }

    /// A source frame shown alone (no result yet); its name is drawn top left.
    fn shown_alone(&self) -> bool {
        matches!(self.shown.as_ref().map(|s| &s.source), Some(Source::File(_)))
            && matches!(self.state.now(), Mode::Frame { .. })
            && self.primary.is_some()
            && self.secondary.is_none()
    }

    /// Where the panes were last drawn, in window coordinates.
    #[cfg(test)]
    pub fn pane_clips(&self) -> Vec<Bounds<Pixels>> {
        self.panes.iter().map(|(clip, _)| *clip).collect()
    }

    /// Each pane's clip and where its whole picture lies, as last drawn.
    #[cfg(test)]
    pub fn pane_rects(&self) -> Vec<(Bounds<Pixels>, Bounds<Pixels>)> {
        self.panes.clone()
    }

    /// The labels over the compare panes, when they are drawn.
    #[cfg(test)]
    pub fn shown_labels(&self) -> Option<CompareLabels> {
        self.drawn_labels().map(|(_, labels)| labels)
    }

    /// Zoom by `f`, keeping the pointer's pane-local point in place if given.
    pub fn zoom(
        &mut self,
        f: impl FnOnce(&mut ViewTransformController, Option<(f32, f32)>),
        cx: &mut Context<Self>,
    ) {
        let at = self.zoom_anchor();
        f(&mut self.view, at);
        cx.notify();
    }

    pub fn mode(&self) -> ViewMode {
        self.state.now().clone()
    }

    /// In Retouch, what it would be left showing were it closed now.
    pub fn retouched(&self) -> Option<view::Retouched<RenderId>> {
        self.state.retouched()
    }

    /// In Adjust, what it shows.
    #[cfg(test)]
    pub fn adjusting(&self) -> Option<view::Adjusting> {
        self.state.adjusting()
    }

    /// The one place the view changes (ADR-149). Lit pictures are made off
    /// this thread, the current one kept until the new lands; a frame no
    /// longer shown is not loaded. Returns the app's actions.
    pub fn on(&mut self, event: ViewEvent, cx: &mut Context<Self>) -> Vec<ViewAction> {
        // Adjust's pictures go as it closes.
        if let ViewEvent::Adjust(false) = event {
            self.clear_flat(cx);
            self.atlas.replace(&mut self.before, None);
        }
        let answer = self.state.on(event);
        let mut rest = Vec::new();
        for action in answer.actions {
            match action {
                Action::Light { frame, token } => self.light(&frame, token, cx),
                Action::Unlight => {
                    self.lighting.cancel();
                    self.atlas.replace(&mut self.lit_map, None);
                }
                action @ (Action::Show(_) | Action::Reload) => rest.push(action),
            }
        }
        if !self.state.now().shows_source() {
            self.cancel_loads(cx);
        }
        self.fit_map();
        self.make_map(cx);
        self.sync_tracking();
        cx.notify();
        rest
    }

    /// Make the result's map lit for `frame`; only the newest `token` is made.
    fn light(&mut self, frame: &Path, token: u64, cx: &mut Context<Self>) {
        let Some(stack) = self.stack.clone() else { return };
        let Some(index) = stack.paths.iter().position(|p| p == frame) else { return };
        let label = say!(cx, "viewer-loading", name = file_label(frame)).into();
        self.request(label, LitLoad { frame: Some((index, token)), stack }, cx);
    }

    #[cfg(test)]
    pub fn primary_pixels(&self) -> Option<Vec<u8>> {
        self.primary.as_ref().map(|p| p.pixels())
    }

    #[cfg(test)]
    pub fn compare_pixels(&self) -> Option<Vec<u8>> {
        self.secondary.as_ref().map(|p| p.pixels())
    }

    #[cfg(test)]
    pub fn compare_size(&self) -> Option<(u32, u32)> {
        self.secondary.as_ref().map(|p| (p.width, p.height))
    }

    /// The tool a left drag uses. Leaving Paint ends a stroke in progress.
    pub fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if tool != Tool::Paint
            && let Gesture::Stroke { button, .. } = self.gesture
        {
            self.set_gesture(Gesture::Spent(button));
            cx.emit(ViewerEvent::Paint(PaintInput::End));
        }
        // The crop tool shows the picture uncropped (`view::Tool::Crop`).
        let crop = |t: Tool| matches!(t, Tool::Crop(_));
        let changed = crop(tool) != crop(self.tool);
        self.tool = tool;
        if changed && self.state.adjusting().is_some() {
            let to = if crop(tool) { view::Tool::Crop } else { view::Tool::Sliders };
            self.on(ViewEvent::Tool(to), cx);
        }
        self.sync_tracking();
        cx.notify();
    }

    pub fn set_ring(&mut self, ring: Ring, cx: &mut Context<Self>) {
        if ring != self.ring {
            self.ring = ring;
            cx.notify();
        }
    }

    /// Space held or let go: drags pan while it is held.
    pub fn set_space_pan(&mut self, held: bool) {
        self.held_keys.space = held;
    }

    /// Reserve bottom clearance for the floating View Bar at fit scale.
    pub fn set_fit_bottom_inset(&mut self, inset: f32) {
        self.view.set_fit_bottom_inset(inset);
    }

    /// Release every held key: its key-up may never arrive (focus lost, or
    /// Paint ended with it down).
    pub fn release_keys(&mut self, cx: &mut Context<Self>) {
        if self.held_keys != HeldKeys::default() {
            tracing::debug!(keys = ?self.held_keys, "viewer: held keys released");
            self.held_keys = HeldKeys::default();
            cx.notify();
        }
    }

    /// Show the paint source in place of the canvas, or the canvas again.
    pub fn set_peek(&mut self, peek: bool, cx: &mut Context<Self>) {
        if self.held_keys.source != peek {
            self.held_keys.source = peek;
            cx.notify();
        }
    }

    /// Whether the paint source is shown in place of the canvas.
    pub fn peeking(&self) -> bool {
        self.held_keys.source && self.tool == Tool::Paint
    }

    /// The overlay for the frame on screen: none for the result alone, a
    /// render compared, or the map.
    pub fn overlay(&self) -> Overlay {
        self.state.now().overlay()
    }

    /// Reload the source frame on screen when its overlay differs from the
    /// view's.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let overlay = self.overlay();
        let Some(Shown { file, name, source, labels }) = self.shown.clone() else { return };
        let Source::Compare { path, frame_index, stack, overlay: was } = source else { return };
        if was != overlay {
            let source = Source::Compare { path, frame_index, stack, overlay };
            self.load(file, &name, source, labels, cx);
        }
    }

    /// The one place [`Self::shown`] changes.
    fn set_shown(&mut self, shown: Option<Shown>) {
        let said = |s: &Option<Shown>| match s.as_ref().map(|s| (&s.name, &s.source)) {
            None => "nothing".to_string(),
            Some((name, Source::File(_))) => format!("file {name}"),
            Some((name, Source::Compare { overlay, .. })) => format!("frame {name} ({overlay:?})"),
            Some((name, Source::Render { .. })) => format!("render {name}"),
        };
        let (from, to) = (said(&self.shown), said(&shown));
        if from != to {
            tracing::debug!("viewer source: {from} -> {to}");
        }
        self.shown = shown;
    }

    // ------------------------------------------------------------------
    // Images
    // ------------------------------------------------------------------

    /// The canvas being painted, in place of the result, keeping the mode.
    pub fn show_canvas(&mut self, display: PresentedImage, cx: &mut Context<Self>) {
        self.atlas.replace(&mut self.primary, Some(TiledImage::from_presented(display)));
        cx.notify();
    }

    /// Paint's display updates to the canvas.
    pub fn apply_patches(&mut self, patches: Vec<DisplayPatch>, cx: &mut Context<Self>) {
        let Some(primary) = &mut self.primary else { return };
        for patch in patches {
            primary.apply(&mut self.atlas, patch);
        }
        cx.notify();
    }

    /// The paint source beside the canvas, with the panes' labels.
    pub fn show_source(
        &mut self,
        image: TiledImage,
        labels: CompareLabels,
        cx: &mut Context<Self>,
    ) {
        self.cancel_loads(cx);
        self.set_shown(None);
        self.atlas.replace(&mut self.secondary, Some(image));
        self.compare_labels = Some(labels);
        cx.notify();
    }

    /// Drop every image (stack starting, list emptied); the viewport then
    /// tracks the live map.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.cancel_loads(cx);
        self.set_shown(None);
        self.stack = None;
        self.on(ViewEvent::Shown { result: None, keep: false, supplied: false }, cx);
        self.atlas.replace(&mut self.primary, None);
        self.set_map(MapPicture::None);
        self.atlas.replace(&mut self.lit_map, None);
        self.drop_compare();
        cx.notify();
    }

    /// Forget the compare source and its labels.
    fn drop_compare(&mut self) {
        self.atlas.replace(&mut self.secondary, None);
        self.compare_labels = None;
    }

    /// Drop the compare source and stop comparing: the result alone.
    pub fn clear_compare(&mut self, cx: &mut Context<Self>) {
        self.cancel_loads(cx);
        self.set_shown(None);
        self.drop_compare();
        self.on(ViewEvent::StopCompare, cx);
    }

    /// Whether flicker shows the compare source (else the result).
    #[cfg(test)]
    pub fn flicker_shows_source(&self) -> bool {
        matches!(
            self.state.now().comparing().map(|c| c.layout),
            Some(Layout::Flicker { source: true })
        )
    }

    /// The comparison on screen has its sides swapped.
    #[cfg(test)]
    pub fn sides_swapped(&self) -> bool {
        self.state.now().comparing().is_some_and(|c| c.swapped)
    }

    pub fn show_live_map(&mut self, map: PresentedImage, cx: &mut Context<Self>) {
        self.set_map(MapPicture::Held(TiledImage::from_presented(map), None));
        cx.notify();
    }

    /// A finished stack: its display and map. A preview still loading is
    /// dropped so it can't land over the result; `stack` remakes the map's
    /// picture when needed.
    pub fn show_result(
        &mut self,
        display: Option<PresentedImage>,
        map: Option<PresentedImage>,
        stack: Option<&Arc<CompletedStack>>,
        reframe: deptheos_app::reframe::Reframe,
        cx: &mut Context<Self>,
    ) {
        self.cancel_loads(cx);
        let picture =
            stack.map(|s| deptheos_app::reframe::Size { width: s.width, height: s.height });
        self.set_reframe(picture, reframe, cx);
        // The frame on screen, as part of this result.
        let on_screen = match self.shown.as_ref().map(|s| &s.source) {
            Some(Source::File(path) | Source::Compare { path, .. }) => Some(path),
            Some(Source::Render { .. }) | None => None,
        };
        let supplied = stack
            .zip(on_screen)
            .and_then(|(stack, path)| pick(stack, stack.paths.iter().position(|p| p == path)?))
            .is_some_and(|p| p.supplied);
        self.set_shown(None);
        self.atlas.replace(&mut self.primary, display.map(TiledImage::from_presented));
        let facts =
            Facts { map: map.is_some(), lights: stack.is_some_and(|s| s.frame_map.is_some()) };
        self.set_map(match map {
            Some(map) => MapPicture::Held(TiledImage::from_presented(map), stack.cloned()),
            None => MapPicture::None,
        });
        self.stack = stack.cloned();
        self.drop_compare();
        self.on(ViewEvent::Shown { result: Some(facts), keep: false, supplied }, cx);
    }

    /// How the two images are laid out: a comparison, or the source alone
    /// (a lone frame, or the paint source with `peek`) as Flicker; `None`
    /// for one image.
    fn drawn_comparing(&self, peek: bool) -> Option<(Layout, bool)> {
        let source = (Layout::Flicker { source: true }, false);
        if peek && self.peeking() {
            return Some(source);
        }
        match self.state.now() {
            Mode::Frame { .. } => Some(source),
            mode => mode.comparing().map(|c| (c.layout, c.swapped)),
        }
    }

    /// The labels and their comparison, when a compare source is drawn
    /// (the same test as [`Self::prepaint`]'s).
    fn drawn_labels(&self) -> Option<((Layout, bool), CompareLabels)> {
        let main = self.primary.as_ref()?;
        let compare = self.secondary.as_ref()?;
        // Retouch labels the peeked source itself.
        let comparing = self.drawn_comparing(false)?;
        let drawn = (compare.width, compare.height) == (main.width, main.height);
        drawn.then(|| self.compare_labels.clone().map(|labels| (comparing, labels)))?
    }
}

/// A label over a pane's corner.
fn pane_tag(text: SharedString, cx: &gpui::App) -> gpui::Div {
    pane_tag_with(text, cx)
}

/// [`pane_tag`] around any content.
fn pane_tag_with(content: impl IntoElement, cx: &gpui::App) -> gpui::Div {
    overlay_surface(Fill::Hud, cx).px_1p5().py_0p5().text_xs().child(content)
}

impl Render for Viewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let paint_view = view.clone();
        let viewport = canvas(
            move |bounds, window, cx| view.update(cx, |this, cx| this.prepaint(bounds, window, cx)),
            move |bounds, frame, window, _cx| Self::paint(frame, &paint_view, bounds, window),
        )
        .size_full();

        // A slow load dims the picture it replaces; the pane says what loads.
        let loading = self.dimmed_loading().cloned();
        let dimmed = loading.is_some();
        let labels = self.drawn_labels().filter(|_| !dimmed);
        let alone_name = self
            .shown
            .as_ref()
            .filter(|_| self.shown_alone() && !dimmed)
            .map(|s| SharedString::from(s.name.clone()));
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(rgb(crate::color::brand().viewer_background))
            .child(viewport)
            .when(dimmed, |el| {
                el.child(div().absolute().inset_0().bg(gpui::opaque_grey(0.0, 0.45)))
            })
            .when_some(loading, |el, loading| {
                let tag = h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(Spinner::new().xsmall().color(hud_text()))
                    .child(loading);
                el.child(pane_tag_with(tag, cx).absolute().top_2().left_2())
            })
            .when_some(labels, |el, ((layout, swapped), (compare, result))| {
                // Compare source left (or top), result right (or bottom),
                // unless swapped; Flicker labels its one image, larger.
                if let Layout::Flicker { source } = layout {
                    let tag = if source { compare } else { result };
                    return el.child(pane_tag(tag, cx).text_sm().absolute().top_2().left_2());
                }
                let source = pane_tag(compare, cx);
                let result = pane_tag(result, cx);
                // As the panes are drawn: the Paint tool never swaps.
                let swapped = swapped && self.tool != Tool::Paint;
                let (first, second) = if swapped { (result, source) } else { (source, result) };
                let second = second.absolute().top_2();
                let second = match layout {
                    Layout::SideBySide => second.left(gpui::relative(0.5)).ml_2(),
                    Layout::TopAndBottom => second.top(gpui::relative(0.5)).mt_2().left_2(),
                    Layout::Split | Layout::Flicker { .. } => second.right_2(),
                };
                el.child(first.absolute().top_2().left_2()).child(second)
            })
            .when_some(alone_name, |el, name| {
                el.child(pane_tag(name, cx).absolute().top_2().left_2())
            })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::Mutex;

    use gpui::{Entity, MouseButton, TestAppContext, point, px, size};

    use deptheos_app::CpuImage;
    use deptheos_app::reframe::Reframe;

    use super::*;
    use crate::test_support::{open, wait_until};

    /// What the recording loader was asked for, in order.
    type Loaded = Arc<Mutex<Vec<(PathBuf, Overlay)>>>;

    /// A loader that records what it loads (and its overlay); fails on "bad.jpg".
    fn recording_loader() -> (Loader, Loaded) {
        let loaded = Arc::new(Mutex::new(Vec::new()));
        let log = loaded.clone();
        let loader: Loader = Arc::new(move |source: &Source| {
            let (path, overlay) = match source {
                Source::File(path) => (path.clone(), Overlay::None),
                Source::Compare { path, overlay, .. } => (path.clone(), *overlay),
                Source::Render { .. } => (PathBuf::from("render"), Overlay::None),
            };
            log.lock().unwrap().push((path.clone(), overlay));
            if path == Path::new("bad.jpg") {
                return Err(deptheos_app::Error::InvalidState("cannot decode".into()));
            }
            Ok(PresentedImage::single(CpuImage { width: 4, height: 2, pixels: vec![0; 32] }))
        });
        (loader, loaded)
    }

    struct Fixture {
        viewer: Entity<Viewer>,
        ids: Vec<FileId>,
        loaded: Loaded,
        events: Rc<RefCell<Vec<ViewerEvent>>>,
    }

    fn fixture(cx: &mut TestAppContext) -> Fixture {
        let (loader, loaded) = recording_loader();
        let (_, viewer) = open(cx, |_, cx| cx.new(|cx| Viewer::new(loader, cx)));
        // The viewer only carries file ids.
        let ids = FileId::for_tests(4);
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&viewer, move |_, event: &ViewerEvent, _| {
                events.borrow_mut().push(event.clone())
            })
            .detach();
        });
        Fixture { viewer, ids, loaded, events }
    }

    /// A 4 × 2 picture, as a result's map.
    fn picture() -> PresentedImage {
        PresentedImage::single(CpuImage { width: 4, height: 2, pixels: vec![0; 32] })
    }

    fn load(f: &Fixture, cx: &mut TestAppContext, file: usize, path: &str) {
        let (viewer, id) = (f.viewer.clone(), f.ids[file]);
        viewer.update(cx, |v, cx| {
            v.load(Some(id), path, Source::File(PathBuf::from(path)), None, cx)
        });
    }

    /// A right drag pans with the Paint tool: no stroke, a left press
    /// meanwhile ignored, only the right release ends it.
    #[gpui::test]
    fn right_drag_pans_while_painting(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let at = |x: f32| point(px(x), px(10.0));
        f.viewer.update(cx, |v, cx| {
            v.set_tool(Tool::Paint, cx);
            v.mouse_down(at(10.0), MouseButton::Right, false, cx);
            assert_eq!(v.gesture, Gesture::Pan(MouseButton::Right));
            v.mouse_down(at(10.0), MouseButton::Left, false, cx);
            assert_eq!(v.gesture, Gesture::Pan(MouseButton::Right), "the left press is ignored");
            assert!(!v.mouse_up(MouseButton::Left, cx));
            assert_eq!(v.gesture, Gesture::Pan(MouseButton::Right));
            assert!(v.mouse_up(MouseButton::Right, cx));
            assert!(v.gesture == Gesture::Idle && !v.held.get());
        });
        assert!(f.events.borrow().is_empty(), "no paint input");
    }

    /// A stroke crossing into the other side-by-side pane keeps mapping
    /// through its own pane, so the brush doesn't jump across the image.
    #[gpui::test]
    fn a_stroke_stays_in_its_pane(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let pane = |x: f32| {
            let b = Bounds { origin: point(px(x), px(0.0)), size: size(px(100.0), px(100.0)) };
            (b, b)
        };
        let at = |x: f32| point(px(x), px(50.0));
        f.viewer.update(cx, |v, cx| {
            v.set_tool(Tool::Paint, cx);
            v.framed.shown = (100, 100);
            v.panes = vec![pane(0.0), pane(102.0)];
            v.mouse_down(at(104.0), MouseButton::Left, false, cx);
            v.mouse_move(at(98.0), true, cx);
            v.mouse_up(MouseButton::Left, cx);
        });
        let xs: Vec<f32> = f
            .events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                ViewerEvent::Paint(PaintInput::Begin(d) | PaintInput::Move(d)) => Some(d.x),
                _ => None,
            })
            .collect();
        assert_eq!(xs, vec![2.0, -4.0]);
    }

    /// Leaving the Paint tool mid-stroke ends the stroke; the held button
    /// does nothing more.
    #[gpui::test]
    fn leaving_paint_mid_stroke_ends_it(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let b = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(100.0), px(100.0)) };
        let at = |x: f32| point(px(x), px(50.0));
        f.viewer.update(cx, |v, cx| {
            v.set_tool(Tool::Paint, cx);
            v.framed.shown = (100, 100);
            v.panes = vec![(b, b)];
            v.mouse_down(at(10.0), MouseButton::Left, false, cx);
            v.set_tool(Tool::Pan, cx);
            assert_eq!(v.gesture, Gesture::Spent(MouseButton::Left));
            v.mouse_move(at(20.0), true, cx);
            assert!(!v.mouse_up(MouseButton::Left, cx));
            assert!(v.gesture == Gesture::Idle && !v.held.get());
        });
        let events = f.events.borrow();
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                ViewerEvent::Paint(PaintInput::Begin(_)) => "begin",
                ViewerEvent::Paint(PaintInput::Move(_)) => "move",
                ViewerEvent::Paint(PaintInput::End) => "end",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["begin", "end"]);
    }

    /// Zoom keys anchor at the pointer, else the center; touchpad scroll
    /// pans, ⌘-scroll zooms at the pointer; double-click toggles 100 % and fit.
    #[gpui::test]
    fn zoom_follows_the_pointer(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let pane = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(200.0), px(200.0)) };
        f.viewer.update(cx, |v, _| {
            v.view.set_container_size(200.0, 200.0);
            v.view.set_image_size(2000, 2000);
            v.panes = vec![(pane, pane)];
            v.pointer.set(Some(point(px(50.0), px(150.0))));
            assert_eq!(v.zoom_anchor(), Some((50.0, 150.0)));
            v.pointer.set(Some(point(px(250.0), px(150.0))));
            assert_eq!(v.zoom_anchor(), None, "beside the panes");
            v.pointer.set(None);
            assert_eq!(v.zoom_anchor(), None);

            let at = point(px(50.0), px(150.0));
            let scroll = gpui::ScrollDelta::Pixels(point(px(0.0), px(40.0)));
            let fit = v.view.transform.scale;
            v.scroll(at, scroll, px(20.0), false);
            assert_eq!(v.view.transform.scale, fit, "a plain touchpad scroll doesn't zoom");
            assert!(v.scroll(at, scroll, px(20.0), true));
            while v.view.tick(0.1) {}
            assert!(v.view.transform.scale > fit, "with ⌘ it zooms in");

            v.view.zoom_to_fit();
            while v.view.tick(0.1) {}
            let under = v.view.screen_to_image(50.0, 150.0);
            v.double_click(at);
            while v.view.tick(0.1) {}
            assert_eq!(v.zoom_percent(), 100);
            let after = v.view.screen_to_image(50.0, 150.0);
            assert!((under.0 - after.0).abs() < 1.0 && (under.1 - after.1).abs() < 1.0);
            v.double_click(at);
            while v.view.tick(0.1) {}
            assert!(v.is_fitted());
        });
    }

    /// When only the framing changes (here the crop's window), a zoomed
    /// view keeps the stored point at its center; a fitted one refits.
    #[gpui::test]
    fn a_new_framing_keeps_the_stored_point_in_view(cx: &mut TestAppContext) {
        use deptheos_app::reframe::{Flip, Rect, Size, Turn};
        let f = fixture(cx);
        let picture = Size { width: 400, height: 300 };
        let framed = |x: u32| Reframe {
            crop: Some(Rect { x, y: 50, width: 200, height: 150 }),
            flip: Flip::Horizontal,
            turn: Turn::Quarter,
        };
        f.viewer.update(cx, |v, cx| {
            v.view.set_container_size(100.0, 100.0);
            let facts = Facts { map: false, lights: false };
            v.on(ViewEvent::Shown { result: Some(facts), keep: false, supplied: false }, cx);
            v.set_reframe(Some(picture), framed(100), cx);
            assert_eq!(v.frame((300, 400)), (150, 200), "the crop's window, turned");
            assert!(v.is_fitted());
            v.view.zoom_at(50.0, 50.0, 1.0);
            let center = v.view.pane_center();
            let stored = |v: &Viewer| {
                let (x, y) = v.view.screen_to_image(center.0, center.1);
                let w = v.framed.window.unwrap();
                v.to_stored((x + w.x as f32, y + w.y as f32))
            };
            let before = stored(v);
            assert!(!v.view.is_fitted());
            v.set_reframe(Some(picture), framed(120), cx);
            v.frame((300, 400));
            let after = stored(v);
            assert!(
                (before.0 - after.0).abs() < 0.5 && (before.1 - after.1).abs() < 0.5,
                "{before:?} {after:?}"
            );
            assert_eq!(v.view.transform.scale, 1.0, "at the same zoom");
        });
    }

    /// ⌥-click picks only on the image: not in the pane around it.
    #[gpui::test]
    fn an_alt_click_beside_the_image_picks_nothing(cx: &mut TestAppContext) {
        let f = fixture(cx);
        // A 100 × 50 image letterboxed in a 200 × 200 pane, at 1:1.
        let clip = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(200.0), px(200.0)) };
        let rect = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(100.0), px(50.0)) };
        f.viewer.update(cx, |v, cx| {
            v.framed.shown = (100, 50);
            v.panes = vec![(clip, rect)];
            for (x, y) in [(150.0, 10.0), (10.0, 120.0), (100.0, 10.0), (10.0, 50.0), (10.0, 10.0)]
            {
                v.pick_at(point(px(x), px(y)), cx);
            }
        });
        let picked: Vec<(u32, u32)> = f
            .events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                ViewerEvent::PickAt { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        assert_eq!(picked, [(10, 10)]);
    }

    /// Feed `event`, loading a frame shown again with its overlay, as the app does.
    fn on(v: &mut Viewer, event: ViewEvent, cx: &mut Context<Viewer>) {
        for action in v.on(event, cx) {
            if action == Action::Reload {
                v.reload(cx);
            }
        }
    }

    /// The result shown is `stack`'s, with its map.
    fn with_result(v: &mut Viewer, stack: &Arc<CompletedStack>, cx: &mut Context<Viewer>) {
        v.show_result(Some(picture()), stack.present_map(None), Some(stack), Reframe::NONE, cx);
    }

    const MAP: ViewEvent = ViewEvent::ToggleMap { on_screen: None };

    fn light(stack: &CompletedStack, index: usize) -> ViewEvent {
        ViewEvent::Light(pick(stack, index).unwrap())
    }

    /// The frame lit on the map, by its place in `stack`.
    fn lit(f: &Fixture, stack: &CompletedStack, cx: &mut TestAppContext) -> Option<usize> {
        let mode = f.viewer.read_with(cx, |v, _| v.mode());
        mode.lit().and_then(|lit| stack.paths.iter().position(|p| p == lit))
    }

    /// Whether a lit picture is drawn in place of the whole map.
    fn lit_drawn(f: &Fixture, cx: &mut TestAppContext) -> bool {
        f.viewer.read_with(cx, |v, _| v.lit_map.is_some())
    }

    fn lighting_settled(f: &Fixture, cx: &mut TestAppContext) {
        wait_until(cx, "the lit map", |cx| f.viewer.read_with(cx, |v, _| !v.lighting_busy()));
    }

    /// A lit frame lights the map once made; another replaces it (the old
    /// picture stays until the new lands); put out, the whole map is back;
    /// off the map nothing lights.
    #[gpui::test]
    fn the_map_lights_a_frame(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let stack = stack(true);
        f.viewer.update(cx, |v, cx| {
            with_result(v, &stack, cx);
            on(v, MAP, cx);
            on(v, light(&stack, 1), cx);
        });
        assert_eq!(lit(&f, &stack, cx), Some(1));
        lighting_settled(&f, cx);
        assert!(lit_drawn(&f, cx), "lit");
        f.viewer.update(cx, |v, cx| on(v, light(&stack, 2), cx));
        assert_eq!(lit(&f, &stack, cx), Some(2));
        assert!(lit_drawn(&f, cx), "the picture lit before stays until the new one lands");
        lighting_settled(&f, cx);
        assert!(lit_drawn(&f, cx));
        f.viewer.update(cx, |v, cx| on(v, ViewEvent::Unlight, cx));
        assert_eq!(lit(&f, &stack, cx), None);
        assert!(!lit_drawn(&f, cx), "the whole map");
        f.viewer.update(cx, |v, cx| {
            on(v, light(&stack, 0), cx);
            on(v, MAP, cx);
            assert!(!v.mode().is_map());
            on(v, light(&stack, 1), cx);
            assert!(!v.mode().is_map(), "off the map, nothing to light");
        });
        lighting_settled(&f, cx);
        assert!(!lit_drawn(&f, cx));
    }

    /// A lit picture that fails puts the light out; the whole map stays (ADR-149).
    #[gpui::test]
    fn a_lit_picture_that_fails_puts_the_light_out(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let stack = stack(true);
        f.viewer.update(cx, |v, cx| {
            with_result(v, &stack, cx);
            on(v, MAP, cx);
            on(v, light(&stack, 1), cx);
            let token = v.mode().lit_token().expect("asked for");
            let failed = || Err(deptheos_app::Error::InvalidState("no picture".into()));
            v.finish_light(Some((1, token + 1)), &stack, failed(), cx);
            assert_eq!(v.mode().lit_token(), Some(token), "another picture's failure: nothing");
            v.finish_light(Some((1, token)), &stack, failed(), cx);
            let mode = v.mode();
            assert!(mode.is_map() && mode.lit().is_none(), "the map whole, not {mode:?}");
        });
        assert!(!lit_drawn(&f, cx));
    }

    /// Images replaced while the viewer is off screen leave the atlas
    /// without waiting for it to be shown.
    #[gpui::test]
    fn drops_are_flushed_while_no_frame_is_drawn(cx: &mut TestAppContext) {
        let f = fixture(cx);
        f.viewer.update(cx, |v, cx| {
            v.show_result(Some(picture()), Some(picture()), None, Reframe::NONE, cx);
            v.show_result(Some(picture()), None, None, Reframe::NONE, cx);
            assert!(v.atlas.has_drops(), "the first result's images wait");
        });
        cx.executor().advance_clock(crate::images::DROP_AFTER * 2);
        cx.run_until_parked();
        f.viewer.read_with(cx, |v, _| {
            assert!(!v.atlas.has_drops());
            assert!(v.flushing.is_none(), "nothing more to wait for");
        });
    }

    /// A finished stack's map picture is held only while the map is on;
    /// shown again, the result stands in until it is remade. One remade
    /// after the map was left is dropped. A live map stays.
    #[gpui::test]
    fn the_map_s_picture_is_held_only_while_it_is_shown(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let stack = stack(true);
        let map = |cx: &mut TestAppContext| f.viewer.read_with(cx, |v, _| v.map.name());
        f.viewer.update(cx, |v, cx| {
            with_result(v, &stack, cx);
            assert_eq!(v.map.name(), "off", "not shown");
            assert!(v.has_map(), "still there to turn on");
            on(v, MAP, cx);
            assert_eq!(v.map.name(), "off", "made again");
        });
        lighting_settled(&f, cx);
        assert_eq!(map(cx), "held");
        f.viewer.update(cx, |v, cx| {
            on(v, MAP, cx);
            assert_eq!(v.map.name(), "off");
            on(v, MAP, cx);
            on(v, MAP, cx);
        });
        lighting_settled(&f, cx);
        assert_eq!(map(cx), "off", "it landed after the map was left");
        f.viewer.update(cx, |v, cx| {
            v.show_live_map(picture(), cx);
            on(v, ViewEvent::Look(Look::Result), cx);
            assert_eq!(v.map.name(), "held", "a live map stays");
        });
    }

    /// Lit pictures asked faster than made collapse to the newest; one
    /// landing after the map was left or replaced is dropped.
    #[gpui::test]
    fn a_stale_lit_map_is_dropped(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let stack = stack(true);
        f.viewer.update(cx, |v, cx| {
            with_result(v, &stack, cx);
            on(v, MAP, cx);
            for i in 0..4 {
                on(v, light(&stack, i), cx);
            }
        });
        assert_eq!(lit(&f, &stack, cx), Some(3), "the newest");
        f.viewer.update(cx, |v, cx| on(v, MAP, cx));
        lighting_settled(&f, cx);
        assert!(!lit_drawn(&f, cx), "the map was left");
        f.viewer.update(cx, |v, cx| {
            on(v, MAP, cx);
            on(v, light(&stack, 1), cx);
            v.show_result(None, Some(picture()), None, Reframe::NONE, cx);
            assert!(!v.mode().is_map(), "a new result: back from the map");
        });
        lighting_settled(&f, cx);
        assert!(!lit_drawn(&f, cx), "a new result came");
        // In Retouch a new result keeps the map, unlit, or leaves it if it has none.
        f.viewer.update(cx, |v, cx| {
            with_result(v, &stack, cx);
            v.set_tool(Tool::Paint, cx);
            on(v, ViewEvent::Retouch(Some(Default::default())), cx);
            on(v, MAP, cx);
            on(v, light(&stack, 1), cx);
            v.show_result(None, Some(picture()), None, Reframe::NONE, cx);
            assert_eq!(v.mode(), ViewMode::Map { back: view::Back::Result, light: Light::Whole });
            v.show_result(None, None, None, Reframe::NONE, cx);
            assert_eq!(v.mode(), ViewMode::Result, "no map to show");
        });
    }

    /// A frame left out of the stack is marked on the whole map; Depth Map
    /// off shows it plain, since Contribution would dim all of it (ADR-149).
    #[gpui::test]
    fn a_left_out_frame_lights_nothing(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let paths = ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(PathBuf::from).to_vec();
        let mut left = CompletedStack::srgb(4, 2, vec![0; 24], paths);
        left.frame_map = Some(deptheos_app::Gray16Image::new(4, 2));
        left.geometry.left_out.push(deptheos_app::LeftOut { index: 2, reason: String::new() });
        let left = Arc::new(left);
        f.viewer.update(cx, |v, cx| {
            with_result(v, &left, cx);
            on(v, MAP, cx);
            on(v, light(&left, 1), cx);
        });
        lighting_settled(&f, cx);
        assert!(lit_drawn(&f, cx));
        f.viewer.update(cx, |v, cx| on(v, light(&left, 2), cx));
        assert_eq!(lit(&f, &left, cx), Some(2), "marked");
        assert!(!lit_drawn(&f, cx), "the whole map");
        lighting_settled(&f, cx);
        assert!(!lit_drawn(&f, cx));
        let shown = f.viewer.update(cx, |v, cx| v.on(MAP, cx));
        assert_eq!(shown, [Action::Show(PathBuf::from("c.jpg"))], "the frame marked");
        let mode = f.viewer.read_with(cx, |v, _| v.mode());
        assert_eq!(mode.overlay(), Overlay::None, "plain");
        assert!(!mode.overlaid().unwrap().supplied, "a frame left out");
    }

    /// Requests faster than loads collapse to the newest; earlier files are
    /// never decoded.
    #[gpui::test]
    fn only_the_newest_request_decodes(cx: &mut TestAppContext) {
        let f = fixture(cx);
        load(&f, cx, 0, "a.jpg");
        load(&f, cx, 1, "b.jpg");
        load(&f, cx, 2, "c.jpg");
        assert!(f.viewer.read_with(cx, |v, _| v.is_loading()));
        wait_until(cx, "the load", |cx| !f.viewer.read_with(cx, |v, _| v.is_loading()));
        assert_eq!(*f.loaded.lock().unwrap(), [(PathBuf::from("c.jpg"), Overlay::None)]);
        f.viewer.read_with(cx, |v, _| {
            assert!(v.has_content());
            assert!(!v.is_loading());
            assert!(matches!(v.mode(), ViewMode::Frame { .. }), "a frame alone");
        });

        // A later request loads too.
        load(&f, cx, 3, "d.jpg");
        wait_until(cx, "the second load", |cx| !f.viewer.read_with(cx, |v, _| v.is_loading()));
        assert_eq!(f.loaded.lock().unwrap().len(), 2);
    }

    /// A stack of the four test frames, with a frame map when `mapped`.
    fn stack(mapped: bool) -> Arc<CompletedStack> {
        let paths = ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(PathBuf::from).to_vec();
        let plain = CompletedStack::srgb(4, 2, vec![0; 24], paths);
        let frame_map = mapped.then(|| deptheos_app::Gray16Image::new(4, 2));
        Arc::new(CompletedStack { frame_map, ..plain })
    }

    /// The overlay goes with its frame: changing it reloads the frame, and
    /// a render or the result alone has none. A file before a stack, a frame
    /// outside or left out of it, and a stack without a frame map take no
    /// contribution.
    #[gpui::test]
    fn the_overlay_goes_with_the_frame(cx: &mut TestAppContext) {
        let f = fixture(cx);
        let settled = |cx: &mut TestAppContext| {
            wait_until(cx, "the load", |cx| !f.viewer.read_with(cx, |v, _| v.is_loading()))
        };
        // Contribution turned on or off, as the app asks the view.
        let toggle = |cx: &mut TestAppContext| {
            f.viewer.update(cx, |v, cx| on(v, ViewEvent::ToggleContribution, cx));
        };
        // A compared frame, named to the view, then loaded with its overlay.
        let show_frame = |cx: &mut TestAppContext,
                          path: &str,
                          index: Option<usize>,
                          stack: Arc<CompletedStack>| {
            let viewer = f.viewer.clone();
            let path = path.to_string();
            viewer.update(cx, |v, cx| {
                let frame = index
                    .and_then(|i| pick(&stack, i))
                    .unwrap_or(Pick { id: PathBuf::from(&path), supplied: false });
                on(v, ViewEvent::CompareFrame(frame), cx);
                let overlay = v.overlay();
                let source = Source::Compare {
                    path: PathBuf::from(&path),
                    frame_index: index,
                    stack,
                    overlay,
                };
                v.load(None, &path, source, None, cx)
            });
        };
        let show = |cx: &mut TestAppContext, path: &str, index: Option<usize>, mapped: bool| {
            show_frame(cx, path, index, stack(mapped))
        };
        let can_contribute = |cx: &mut TestAppContext| {
            f.viewer.read_with(cx, |v, _| v.mode().overlaid().is_some_and(|o| o.supplied))
        };
        let overlay = |cx: &mut TestAppContext| f.viewer.read_with(cx, |v, _| v.overlay());

        load(&f, cx, 0, "x.jpg");
        settled(cx);
        toggle(cx);
        assert_eq!(overlay(cx), Overlay::None, "a file before a stack has none");

        f.viewer.update(cx, |v, cx| with_result(v, &stack(true), cx));
        show(cx, "a.jpg", Some(0), true);
        settled(cx);
        toggle(cx);
        settled(cx);
        assert_eq!(overlay(cx), Overlay::Contribution);
        show(cx, "b.jpg", Some(1), true);
        settled(cx);
        toggle(cx);
        settled(cx);
        f.viewer.update(cx, |v, cx| v.reload(cx)); // unchanged: nothing to do
        cx.run_until_parked();
        assert_eq!(
            f.loaded.lock().unwrap()[1..],
            [
                (PathBuf::from("a.jpg"), Overlay::None),
                (PathBuf::from("a.jpg"), Overlay::Contribution),
                (PathBuf::from("b.jpg"), Overlay::Contribution),
                (PathBuf::from("b.jpg"), Overlay::None),
            ]
        );

        // Outside the stack, or without a frame map: no contribution.
        for (index, mapped) in [(None, true), (Some(2), false)] {
            show(cx, "c.jpg", index, mapped);
            settled(cx);
            toggle(cx);
            assert_eq!(overlay(cx), Overlay::None, "{index:?} {mapped}");
            assert!(!can_contribute(cx));
        }
        // A frame left out supplied nothing: no overlay, C does nothing, and
        // Contribution returns on the next frame that supplied part.
        let paths = ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(PathBuf::from).to_vec();
        let mut left = CompletedStack::srgb(4, 2, vec![0; 24], paths);
        left.frame_map = Some(deptheos_app::Gray16Image::new(4, 2));
        left.geometry.left_out.push(deptheos_app::LeftOut { index: 2, reason: String::new() });
        let left = Arc::new(left);
        show_frame(cx, "a.jpg", Some(0), left.clone());
        settled(cx);
        toggle(cx);
        settled(cx);
        assert_eq!(overlay(cx), Overlay::Contribution);
        show_frame(cx, "c.jpg", Some(2), left.clone());
        settled(cx);
        assert_eq!(overlay(cx), Overlay::None, "left out");
        assert!(!can_contribute(cx));
        toggle(cx);
        let chosen = f.viewer.read_with(cx, |v, _| v.mode().overlaid().unwrap().chosen);
        assert_eq!(chosen, Overlay::Contribution, "kept: C does nothing here");
        show_frame(cx, "b.jpg", Some(1), left);
        settled(cx);
        assert_eq!(overlay(cx), Overlay::Contribution, "back on the next frame stacked");
        toggle(cx);
        settled(cx);

        // The result alone again: no frame, no overlay.
        show(cx, "a.jpg", Some(0), true);
        settled(cx);
        toggle(cx);
        settled(cx);
        f.viewer.update(cx, |v, cx| on(v, ViewEvent::Look(Look::Result), cx));
        assert_eq!(overlay(cx), Overlay::None);
        let before = f.loaded.lock().unwrap().len();
        toggle(cx);
        cx.run_until_parked();
        f.viewer.read_with(cx, |v, _| assert!(!v.is_loading()));
        assert_eq!(f.loaded.lock().unwrap().len(), before);
    }

    #[gpui::test]
    fn a_canceled_request_is_never_loaded(cx: &mut TestAppContext) {
        let f = fixture(cx);
        load(&f, cx, 0, "a.jpg");
        f.viewer.update(cx, |v, cx| v.cancel_loads(cx));
        cx.run_until_parked();
        assert!(f.loaded.lock().unwrap().is_empty());
        f.viewer.read_with(cx, |v, _| assert!(!v.has_content() && !v.is_loading()));
    }

    /// A newer request cancels the decode it replaces.
    #[gpui::test]
    fn a_newer_request_cancels_the_decode_before(cx: &mut TestAppContext) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (started_tx, started) = std::sync::mpsc::channel();
        let loader: Loader = Arc::new(move |source: &Source| {
            if matches!(source, Source::File(path) if path == Path::new("slow.jpg")) {
                started_tx.send(()).unwrap();
                let token = deptheos_app::engine::work::current();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while !token.is_canceled() && std::time::Instant::now() < until {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                tx.send(token.is_canceled()).unwrap();
            }
            Ok(PresentedImage::single(CpuImage { width: 4, height: 2, pixels: vec![0; 32] }))
        });
        let (_, viewer) = open(cx, |_, cx| cx.new(|cx| Viewer::new(loader, cx)));
        let ids = FileId::for_tests(2);
        let load = |cx: &mut TestAppContext, id: FileId, path: &str| {
            viewer.update(cx, |v, cx| {
                v.load(Some(id), path, Source::File(PathBuf::from(path)), None, cx)
            })
        };
        load(cx, ids[0], "slow.jpg");
        // The slow decode has started on the pool.
        wait_until(cx, "the slow decode", |_| started.try_recv().is_ok());
        load(cx, ids[1], "fast.jpg");
        // The task dropped lets its future go once the UI thread runs.
        let mut canceled = None;
        wait_until(cx, "the slow decode's end", |_| {
            canceled = rx.try_recv().ok();
            canceled.is_some()
        });
        assert_eq!(canceled, Some(true), "the slow decode was canceled, not waited for");
        wait_until(cx, "the fast frame", |cx| viewer.read_with(cx, |v, _| !v.is_loading()));
        viewer.read_with(cx, |v, _| assert!(v.has_content()));
    }

    #[gpui::test]
    fn a_failed_load_reports_its_file(cx: &mut TestAppContext) {
        let f = fixture(cx);
        load(&f, cx, 1, "bad.jpg");
        wait_until(cx, "the failure", |_| !f.events.borrow().is_empty());
        let events = f.events.borrow();
        assert!(
            matches!(&events[..], [ViewerEvent::LoadFailed { file, .. }] if *file == Some(f.ids[1]))
        );
        f.viewer.read_with(cx, |v, _| assert!(!v.has_content() && !v.is_loading()));
    }
}
