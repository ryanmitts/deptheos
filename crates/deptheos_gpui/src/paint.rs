//! Paint mode on the desktop: the brush, the panels' source names, and the
//! shared `deptheos_app::paint::PaintSession` (open, selection, scrub timing,
//! worker), whose reports reach the viewer here and whose status drives the
//! panels. Whether Retouch is open is `AppView`'s `Mode` (`app/mode.rs`),
//! whose Retouch variant holds a [`Painting`].

use std::cell::Cell;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use deptheos_app::engine::generation::Generation;
use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::History;
use deptheos_app::paint::{
    Brush, CanvasState, DEFAULT_TOLERANCE, FLOW_RANGE, Failure, Flattened, HARDNESS_RANGE,
    OPACITY_RANGE, Observer, PaintSession, RADIUS_RANGE, Report, SourceKind, Status,
    TOLERANCE_RANGE, Told,
};
use deptheos_app::view::Retouched;
use deptheos_app::{CompletedStack, Engine, Explain, Moves, PresentedImage, RenderId};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, SharedString, Task, Window};
use gpui_kit::component::slider::{SliderEvent, SliderState};

use crate::images::TiledImage;
use crate::viewer::{CompareLabels, PaintInput, Ring, ViewEvent, Viewer};
use crate::words::{DesktopWords, Wording as _, say};

/// The session's observer: its reports, in order, to the UI thread.
struct ToUi(mpsc::UnboundedSender<Report>);

impl Observer for ToUi {
    fn report(&self, report: Report) {
        let _ = self.0.unbounded_send(report);
    }
}

/// How long quitting waits for the retouching to be saved.
pub(crate) const SAVE_ON_QUIT: Duration = Duration::from_secs(10);
/// Labels of render sources kept, for naming the source pane.
const LABELS_KEPT: usize = 8;

/// Why a source couldn't load, worded for after the notice's lead.
fn why(words: &DesktopWords, error: &dyn Explain) -> Failure {
    let explained = crate::notices::explain(error, words.shared());
    let said =
        crate::notices::sentences(words, None, &explained, crate::notices::Details::Diagnostics);
    Failure { text: said.text, fault: said.fault }
}

/// [`why`], for the session's loaders.
fn told(words: &DesktopWords) -> Told {
    let words = words.clone();
    Arc::new(move |error| why(&words, error))
}

/// Brush radius slider range, as log2 of the radius in image pixels.
fn size_log2() -> RangeInclusive<f32> {
    RADIUS_RANGE.start().log2()..=RADIUS_RANGE.end().log2()
}

pub enum PaintEvent {
    Failed(crate::notices::Worded),
    /// Something to say briefly: the render can't tell the sharpest frame.
    Notice(String),
    /// [`Paint::present`]'s answer: the painted display, or `None` when it
    /// shows as stacked. The subscriber takes it (no full-size copy).
    Presented {
        key: RenderId,
        display: Cell<Option<PresentedImage>>,
    },
    /// The source painted from, for the viewer's source pane, which
    /// names it from Retouch's picks. Taken once.
    Source {
        key: RenderId,
        image: Cell<Option<TiledImage>>,
    },
}

/// The brush sliders.
pub struct BrushControls {
    pub size: Entity<SliderState>,
    pub hardness: Entity<SliderState>,
    pub opacity: Entity<SliderState>,
    pub flow: Entity<SliderState>,
    pub tolerance: Entity<SliderState>,
    pub tolerance_on: bool,
}

/// The desktop's part of the render painted: the Retouch variant of
/// `AppView`'s `Mode`. Inside it, [`SourcesTab`] changes only by
/// `AppView::set_sources_tab`.
pub struct Painting {
    pub render: RenderId,
    pub stack: Arc<CompletedStack>,
    /// The frames to step through, in order.
    pub frames: Vec<PathBuf>,
    /// The frame painted from first.
    pub first_source: Option<PathBuf>,
    /// Its stored picture's generation as it opened: another as it closes
    /// means it was painted.
    pub opened: Generation,
    /// What the left panel lists to paint from.
    pub sources: SourcesTab,
    /// Render source names from picks ("#3 ★"), richer than [`Paint::name_of`]'s.
    pub labels: Vec<(SourceKind, SharedString)>,
}

impl Painting {
    /// The source name picked for `kind`, newest first, at most a few kept.
    pub fn label(&mut self, kind: SourceKind, label: SharedString) {
        self.labels.retain(|(k, _)| *k != kind);
        self.labels.insert(0, (kind, label));
        self.labels.truncate(LABELS_KEPT);
    }
}

/// What Retouch's left panel lists: frames, or the stack's other renders.
/// Each opening starts on frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcesTab {
    Frames,
    Renders,
}

pub struct Paint {
    session: PaintSession,
    engine: Arc<Engine>,
    /// The session's status as last read, after each call and report.
    status: Option<Status>,
    viewer: Entity<Viewer>,
    /// The render whose canvas is in the viewer (open, or just left while its
    /// last tiles come); only its patches apply.
    canvas: Option<RenderId>,
    /// What Retouch last showed and its layout, kept as it closes for the
    /// next opening (ADR-149).
    retouched: Retouched<RenderId>,
    pub brush: BrushControls,
    _reports: Task<()>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl EventEmitter<PaintEvent> for Paint {}

impl Paint {
    pub fn new(
        engine: Arc<Engine>,
        viewer: Entity<Viewer>,
        history: Arc<History>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded();
        let observer = Arc::new(ToUi(tx.clone()));
        let session = PaintSession::new(engine.clone(), history, observer, told(cx.wording()));
        let _reports = cx.spawn(async move |this, cx| {
            while let Some(report) = rx.next().await {
                let handled = this.update(cx, |this, cx| this.on_report(report, cx));
                if handled.is_err() {
                    return;
                }
            }
        });
        let defaults = Brush::default();
        let slider = |cx: &mut App, range: RangeInclusive<f32>, value: f32| {
            let (min, max) = range.into_inner();
            cx.new(|_| SliderState::new().min(min).max(max).step(0.01).default_value(value))
        };
        let brush = BrushControls {
            size: slider(cx, size_log2(), defaults.radius.log2()),
            hardness: slider(cx, HARDNESS_RANGE, defaults.hardness),
            opacity: slider(cx, OPACITY_RANGE, defaults.opacity),
            flow: slider(cx, FLOW_RANGE, defaults.flow),
            tolerance: slider(cx, TOLERANCE_RANGE, DEFAULT_TOLERANCE),
            tolerance_on: false,
        };
        let ring_follows = |cx: &mut Context<Self>, state: &Entity<SliderState>| {
            cx.subscribe(state, |this: &mut Self, _, _: &SliderEvent, cx| this.sync_ring(cx))
        };
        let _subscriptions = vec![ring_follows(cx, &brush.size), ring_follows(cx, &brush.hardness)];
        Self {
            session,
            engine,
            status: None,
            viewer,
            canvas: None,
            retouched: Retouched::default(),
            brush,
            _reports,
            _subscriptions,
        }
    }

    /// The brush the sliders describe.
    pub fn current_brush(&self, cx: &App) -> Brush {
        let v = |s: &Entity<SliderState>| s.read(cx).value().start();
        Brush {
            radius: 2f32.powf(v(&self.brush.size)),
            hardness: v(&self.brush.hardness),
            opacity: v(&self.brush.opacity),
            flow: v(&self.brush.flow),
            tolerance: self.brush.tolerance_on.then(|| v(&self.brush.tolerance)),
        }
        .clamped()
    }

    fn sync_ring(&mut self, cx: &mut Context<Self>) {
        let brush = self.current_brush(cx);
        let ring = Ring { radius: brush.radius, hardness: brush.hardness };
        self.viewer.update(cx, |v, cx| v.set_ring(ring, cx));
        cx.notify();
    }

    /// A report, as if the session had sent it.
    #[cfg(test)]
    pub fn hear(&mut self, report: Report, cx: &mut Context<Self>) {
        self.on_report(report, cx);
    }

    /// Save all paint without waiting, as quit does; the receiver says
    /// whether every change is on disk.
    pub fn start_save(&self) -> std::sync::mpsc::Receiver<bool> {
        self.session.start_save()
    }

    /// Save all paint now, waiting up to [`SAVE_ON_QUIT`]: the quit hook's
    /// when quit didn't already (`app::quit::on_quit`). The process exits
    /// without dropping the session.
    pub fn save(&self) -> bool {
        let saved = self.session.save(SAVE_ON_QUIT);
        if !saved {
            tracing::error!("quit with the retouching not all saved");
        }
        saved
    }

    /// Read the session's status again, after a call or a report.
    fn refresh(&mut self) {
        self.status = self.session.status();
    }

    /// Call the session while paint mode is on, then read its status.
    fn call(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&PaintSession)) {
        if self.state().is_none() {
            return;
        }
        f(&self.session);
        self.refresh();
        self.light_map(cx);
        cx.notify();
    }

    /// Whether `key` is the render painted.
    fn paints(&self, key: &RenderId) -> bool {
        self.state().is_some_and(|s| s.key == *key)
    }

    /// The paint session's status while paint mode is on.
    pub fn state(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// The source picked, while paint mode is on.
    pub fn source(&self) -> Option<&SourceKind> {
        self.state().map(|s| &s.selection)
    }

    /// Whether paint mode is on with something painted.
    pub fn is_painted(&self) -> bool {
        self.state().is_some_and(|s| s.painted)
    }

    /// The sources some painted pixel came from; none outside paint mode.
    pub fn painted_from(&self) -> &[SourceKind] {
        self.state().map_or(&[], |s| &s.painted_from)
    }

    /// How many steps Undo and Redo can take; none outside paint mode.
    pub fn steps(&self) -> [usize; 2] {
        self.state().map_or([0, 0], |s| [s.undo, s.redo])
    }

    /// Why the render couldn't be opened for painting, while it couldn't.
    pub fn open_failure(&self) -> Option<&Failure> {
        match &self.state()?.canvas {
            CanvasState::Failed(failure) => Some(failure),
            CanvasState::Opening | CanvasState::Ready => None,
        }
    }

    /// The canvas is still opening.
    pub fn is_opening(&self) -> bool {
        self.state().is_some_and(|s| s.canvas == CanvasState::Opening)
    }

    /// Light the depth map over the canvas as the session says (the frame
    /// painted from, or whole). Called on any status change and as the map
    /// opens; the viewer's view holds the light.
    pub fn light_map(&self, cx: &mut Context<Self>) {
        let (Some(status), Some((_, stack))) = (self.state(), self.session.painting()) else {
            return;
        };
        let event = match status.lit.and_then(|i| crate::viewer::pick(&stack, i)) {
            Some(frame) => ViewEvent::Light(frame),
            None => ViewEvent::Unlight,
        };
        self.viewer.update(cx, |v, cx| v.on(event, cx));
    }

    /// The source pane's name for `kind`, from its pick in `labels` if any.
    fn name_of(
        &self,
        kind: &SourceKind,
        labels: &[(SourceKind, SharedString)],
        cx: &App,
    ) -> SharedString {
        if let Some((_, label)) = labels.iter().find(|(k, _)| k == kind) {
            return label.clone();
        }
        match kind {
            SourceKind::Original => say!(cx, "paint-source-original").into(),
            SourceKind::Frame { path } => file_label(path).into(),
            SourceKind::Render { .. } => say!(cx, "paint-source-render").into(),
        }
    }

    /// What the source pane shows, by name, while paint mode is on, from
    /// Retouch's picks.
    pub fn source_name(&self, painting: &Painting, cx: &App) -> Option<SharedString> {
        self.source().map(|kind| self.name_of(kind, &painting.labels, cx))
    }

    /// The two panes' labels: the source painted from, and the canvas.
    pub fn source_labels(&self, painting: &Painting, cx: &App) -> CompareLabels {
        let name = self.source_name(painting, cx).unwrap_or_default();
        let source = say!(cx, "paint-source", name = name.as_ref());
        (source.into(), say!(cx, "paint-painted").into())
    }

    /// The stored picture's generation of `render` now.
    pub fn generation(&self, render: &RenderId) -> Generation {
        self.engine.generations().of(render)
    }

    /// What Retouch was left showing, to open the viewer on.
    pub fn retouched(&self) -> Retouched<RenderId> {
        self.retouched.clone()
    }

    /// Retouch closes showing `left`: what it opens on next.
    pub fn remember(&mut self, left: Retouched<RenderId>) {
        self.retouched = left;
    }

    /// Start painting `painting`'s render, from its first source: only
    /// `AppView::set_mode`'s.
    pub(crate) fn open(&mut self, painting: &Painting, cx: &mut Context<Self>) {
        self.session.open(deptheos_app::paint::OpenRender {
            key: painting.render.clone(),
            stack: painting.stack.clone(),
            frames: painting.frames.clone(),
            first: painting.first_source.clone(),
        });
        self.refresh();
        self.sync_ring(cx);
        cx.notify();
    }

    /// The open failed: open the render again.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::retry);
    }

    /// Folders were relinked ([`Moves`]): the worker's sources follow and its
    /// loaders for moved frames go (paint names frames by root). The open
    /// render is reopened ([`crate::app::AppView::relinked`]).
    pub fn relinked(&self, moves: &Moves) {
        self.session.relink(moves);
    }

    /// Leave paint mode, only `AppView::set_mode`'s; the worker's last
    /// tiles still reach the canvas left on screen.
    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.session.close();
        self.refresh();
        cx.notify();
    }

    /// Show render `key`'s paint outside paint mode; [`PaintEvent::Presented`]
    /// follows, without display if it has none.
    pub fn present(&mut self, key: RenderId, stack: Arc<CompletedStack>) {
        self.session.present(key, stack);
    }

    /// `stack`'s pixels with render `key`'s paint, for saving: `None` when
    /// unpainted, an error when the paint can't be read.
    pub fn flatten(
        &mut self,
        key: RenderId,
        stack: Arc<CompletedStack>,
    ) -> std::sync::mpsc::Receiver<Flattened> {
        self.session.flatten(key, stack)
    }

    // ------------------------------------------------------------------
    // Sources
    // ------------------------------------------------------------------

    /// Paint from the frame at `path`.
    pub fn pick_frame(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.call(cx, |s| s.pick_frame(path.to_path_buf()));
    }

    /// Paint from another render, exposure-matched.
    pub fn pick_render(&mut self, render: &RenderId, cx: &mut Context<Self>) {
        self.call(cx, |s| s.pick_render(render));
    }

    /// The eraser: paint the original back.
    pub fn pick_original(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::pick_original);
    }

    /// The brush again after the eraser: the source before it.
    pub fn pick_brush(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::use_brush);
    }

    pub fn swap_source(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::swap);
    }

    /// Paint From's Frames: the frame last painted from, else the first.
    pub fn use_frames(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::use_frames);
    }

    /// Paint from the frame sharpest at `x`, `y`; [`PaintEvent::Notice`] when
    /// the render can't tell.
    pub fn pick_at(&mut self, x: u32, y: u32, cx: &mut Context<Self>) {
        self.call(cx, |s| s.pick_at(x as f32 + 0.5, y as f32 + 0.5));
    }

    /// Paint from the frame at `path` scrubbed to on the rail: picked at
    /// once, full pixels once it settles.
    pub fn scrub_to(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.call(cx, |s| s.scrub_to(path.to_path_buf()));
    }

    // ------------------------------------------------------------------
    // Strokes and history
    // ------------------------------------------------------------------

    pub fn input(&mut self, input: PaintInput, cx: &mut Context<Self>) {
        let brush = self.current_brush(cx);
        self.call(cx, |s| match input {
            PaintInput::Begin(dab) => s.begin_stroke(brush, Some(dab)),
            PaintInput::Move(dab) => s.dabs(vec![dab]),
            PaintInput::End => s.end_stroke(),
        });
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::undo);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::redo);
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.call(cx, PaintSession::reset);
    }

    // ------------------------------------------------------------------
    // Brush keys
    // ------------------------------------------------------------------

    pub fn scale_size(&mut self, factor: f32, window: &mut Window, cx: &mut Context<Self>) {
        let range = size_log2();
        let v = self.brush.size.read(cx).value().start() + factor.log2();
        let v = v.clamp(*range.start(), *range.end());
        self.brush.size.update(cx, |s, cx| s.set_value(v, window, cx));
        self.sync_ring(cx);
    }

    pub fn nudge_hardness(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let v = self.brush.hardness.read(cx).value().start() + delta;
        let v = v.clamp(*HARDNESS_RANGE.start(), *HARDNESS_RANGE.end());
        self.brush.hardness.update(cx, |s, cx| s.set_value(v, window, cx));
        self.sync_ring(cx);
    }

    pub fn set_opacity(&mut self, opacity: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.brush.opacity.update(cx, |s, cx| s.set_value(opacity, window, cx));
        cx.notify();
    }

    pub fn set_tolerance_on(&mut self, on: bool, cx: &mut Context<Self>) {
        self.brush.tolerance_on = on;
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Reports
    // ------------------------------------------------------------------

    /// The source, ready for the viewer; the session sends only the open
    /// render's.
    fn on_report(&mut self, report: Report, cx: &mut Context<Self>) {
        match report {
            Report::Status(_) | Report::Closed { .. } => {
                self.refresh();
                self.light_map(cx);
            }
            Report::Canvas { key, display } => {
                if self.paints(&key) {
                    self.viewer.update(cx, |v, cx| v.show_canvas(display, cx));
                    self.canvas = Some(key);
                }
            }
            // The open render's or the one just left (canvas still on screen).
            Report::Patches { key, patches } => {
                if self.canvas.as_ref() == Some(&key) {
                    self.viewer.update(cx, |v, cx| v.apply_patches(patches, cx))
                }
            }
            Report::Presented { key, display } => {
                cx.emit(PaintEvent::Presented { key, display: Cell::new(display) });
            }
            // Shared with the worker, which keeps it; the viewer copies only
            // the tiles it paints.
            Report::Source { key, display, .. } => {
                if self.paints(&key) {
                    let image = Cell::new(Some(TiledImage::from_presented(display)));
                    cx.emit(PaintEvent::Source { key, image });
                }
            }
            Report::Failed { message, .. } => cx.emit(PaintEvent::Failed(message.into())),
            Report::Notice { text, .. } => cx.emit(PaintEvent::Notice(text)),
        }
        cx.notify();
    }
}
