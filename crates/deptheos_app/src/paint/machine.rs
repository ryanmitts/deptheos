//! Retouch as one state machine, shared by both apps.
//!
//! The session ([`super::PaintSession`]) feeds each [`Event`] to
//! [`Machine::on`] and carries out the [`Action`]s back, in order. No I/O
//! and no clock: a timer is an [`Action::Schedule`] with a [`Token`], whose
//! [`Event::Fired`] counts only while the state still holds it, so a timer
//! is never canceled, only outdated.
//!
//! ```text
//!            open                         close
//!   Closed ─────────▶ Open(Opening) ─────────────────▶ Leaving
//!     ▲  ▲               │      ▲  ▲ open (another)       │ the worker's
//!     │  │   its canvas  ▼      │  └─────────┐            │ state, none open
//!     │  │       Open(Ready)    │ Retry      │            ▼
//!     │  │               │   Open(Failed) ◀─┘ its       Closed
//!     │  └───────────────┘      open failed
//!     └── Leaving, as it ends
//! ```
//!
//! What it settles, the same way on both apps:
//!
//! - **The selection** is shown at once, ahead of the worker. Picks are
//!   numbered and the worker's state names the last it answered
//!   ([`PaintState::pick`]), so a stale state never moves the selection
//!   back; once all are answered, a pick it couldn't load falls back to
//!   what it kept.
//! - **One scrub load in flight**: frames go as displays only, one at a
//!   time, the newest scrubbed to next. Then the scrub settles
//!   ([`SCRUB_SETTLE`]), the frame is picked in full, and the next one in
//!   the direction stepped is fetched ahead ([`PREFETCH_AFTER`]).
//! - **The canvas**: only a ready canvas takes strokes; a failed one
//!   offers Retry.
//! - **Reports carry their render**; one about a render no longer open is
//!   dropped, but for display tiles of the one just left.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::brush::{Brush, Dab};
use super::display::DisplayPatch;
use super::source::{SourceKind, step_frame};
use super::worker::{Failure, Output, PaintState};
use crate::engine::present::PresentedImage;
use crate::words::{Words, say};
use crate::{CompletedStack, RenderId};

/// How long a frame scrubbed to waits, once shown, before its pixels are
/// fetched.
pub const SCRUB_SETTLE: Duration = Duration::from_millis(300);
/// Quiet time after a full pick before the next frame is fetched ahead.
pub const PREFETCH_AFTER: Duration = Duration::from_millis(500);

/// A timer's name: its firing counts only while the state still holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token(u64);

/// What [`Event::Open`] paints.
pub struct OpenRender {
    pub key: RenderId,
    pub stack: Arc<CompletedStack>,
    /// The frames to step through, in order.
    pub frames: Vec<PathBuf>,
    /// The frame to paint from first; otherwise the first stacked, or the
    /// original.
    pub first: Option<PathBuf>,
}

/// What the session saw: a call from the app, a timer, the worker.
pub enum Event {
    Open(OpenRender),
    /// The open failed: open the render again.
    Retry,
    Close,
    /// Paint from this frame: a tap in a list, a step.
    PickFrame(PathBuf),
    /// The rail scrubbed to this frame.
    ScrubTo(PathBuf),
    /// `n` frames from the one painted from; from a non-frame, the first
    /// or last.
    Step(isize),
    /// The eraser: the original.
    PickOriginal,
    /// Another render of the stack, by id.
    PickRender(String),
    /// Back to the source painted from before the selection.
    Swap,
    /// After the eraser: the source before it, else the first frame.
    UseBrush,
    /// Back to the frames: the last painted from, else the first stacked.
    UseFrames,
    /// The frame sharpest at this point, in image pixels.
    PickAt {
        x: f32,
        y: f32,
    },
    BeginStroke {
        brush: Brush,
        at: Option<Dab>,
    },
    Dabs(Vec<Dab>),
    EndStroke,
    Undo,
    Redo,
    Reset,
    /// An [`Action::Schedule`] timer went off.
    Fired(Token),
    Worker(Output),
}

/// What the session tells the worker; the session builds the loaders.
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    Open(RenderId),
    Close,
    /// Paint from `kind`, numbered `pick`; `scrub` for its display only.
    Pick {
        kind: SourceKind,
        scrub: bool,
        pick: u64,
    },
    Prefetch(SourceKind),
    BeginStroke(Brush),
    Dabs(Vec<Dab>),
    EndStroke,
    Undo,
    Redo,
    Reset,
}

/// The canvas of the render open.
///
/// Lifecycle: opening, ready, failed (Retry opens it again); changed only
/// by [`Machine::on`].
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasState {
    /// No stroke yet.
    Opening,
    Ready,
    /// Retry opens it again.
    Failed(Failure),
}

impl CanvasState {
    fn name(&self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Ready => "ready",
            Self::Failed(_) => "failed",
        }
    }
}

/// Paint mode on one render, as the app shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub key: RenderId,
    pub canvas: CanvasState,
    /// What the brush paints from, as picked.
    pub selection: SourceKind,
    /// The worker has the selection's pixels.
    pub ready: bool,
    /// What Paint From's Frames goes back to.
    pub last_frame: Option<PathBuf>,
    /// The stack index the depth map is lit for, when the render has a
    /// frame map; `None` lights it whole.
    pub lit: Option<usize>,
    pub undo: usize,
    pub redo: usize,
    /// What undo and redo would take back next ("Paint", "Erase").
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub painted: bool,
    /// Why nothing painted can be saved.
    pub unsaveable: Option<String>,
    /// The sources some painted pixel came from, the original aside.
    pub painted_from: Vec<SourceKind>,
}

/// What the app hears, in order. Each names its render.
pub enum Report {
    /// Paint mode's state changed.
    Status(Status),
    /// Paint mode left render `key`.
    Closed { key: RenderId },
    /// The canvas's whole display, as it opened.
    Canvas { key: RenderId, display: PresentedImage },
    /// Display tiles of the render open, or of the one just left.
    Patches { key: RenderId, patches: Vec<DisplayPatch> },
    /// The source painted from, lined up with the render.
    Source { key: RenderId, kind: SourceKind, display: Arc<PresentedImage> },
    /// A present's answer: `None` when it shows as stacked.
    Presented { key: RenderId, display: Option<PresentedImage> },
    /// A failure, with the render paint mode was on, if any.
    Failed { key: Option<RenderId>, message: Failure },
    /// Something to say briefly over the photo.
    Notice { key: RenderId, text: String },
}

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(s) => write!(f, "Status({s:?})"),
            Self::Closed { key } => write!(f, "Closed({})", key.as_str()),
            Self::Canvas { key, .. } => write!(f, "Canvas({})", key.as_str()),
            Self::Patches { key, patches } => {
                write!(f, "Patches({}, {})", key.as_str(), patches.len())
            }
            Self::Source { key, kind, .. } => write!(f, "Source({}, {kind:?})", key.as_str()),
            Self::Presented { key, display } => {
                write!(f, "Presented({}, {})", key.as_str(), display.is_some())
            }
            Self::Failed { key, message } => {
                write!(f, "Failed({:?}, {})", key.as_ref().map(RenderId::as_str), message.text)
            }
            Self::Notice { key, text } => write!(f, "Notice({}, {text})", key.as_str()),
        }
    }
}

/// What the session is to do, in the order given.
#[derive(Debug)]
pub enum Action {
    Send(Order),
    /// Start a timer: [`Event::Fired`] with `token` after `after`.
    Schedule {
        token: Token,
        after: Duration,
    },
    Report(Report),
}

/// What waits on the worker or on time.
///
/// Lifecycle: idle, scrubbing, settling, prefetching; changed only by
/// [`Machine::on`].
///
/// ```text
/// Idle ──a scrub──▶ Scrubbing ──scrubbed again──▶ Scrubbing(next)
/// Scrubbing ──the worker answers, a next──▶ Scrubbing (the next sent)
/// Scrubbing ──the worker answers, none──▶ Settling ──fires──▶ Prefetching
/// any ──a frame picked in full──▶ Prefetching ──fires──▶ Idle (fetched)
/// any ──another pick, a stroke──▶ Idle
/// ```
#[derive(Clone, Debug, PartialEq)]
enum Pace {
    Idle,
    /// Pick `pick` asked for a display; `next` is sent once it answers.
    Scrubbing {
        pick: u64,
        next: Option<PathBuf>,
    },
    /// Its pixels are fetched when this fires.
    Settling(Token),
    /// The next frame is fetched ahead when this fires.
    Prefetching(Token),
}

impl Pace {
    fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Scrubbing { .. } => "scrubbing",
            Self::Settling(_) => "settling",
            Self::Prefetching(_) => "prefetching",
        }
    }
}

/// The render painted.
struct Open {
    key: RenderId,
    stack: Arc<CompletedStack>,
    frames: Vec<PathBuf>,
    canvas: CanvasState,
    selection: SourceKind,
    /// What [`Event::Swap`] goes back to.
    previous: Option<SourceKind>,
    last_frame: Option<PathBuf>,
    /// The direction last stepped, for fetching ahead.
    step: isize,
    sent: u64,
    pace: Pace,
    worker: Option<PaintState>,
}

/// Lifecycle: closed, open, leaving, as the module's diagram shows;
/// changed only by [`Machine::on`].
enum Mode {
    Closed,
    Open(Box<Open>),
    /// Closed on this render; its last display tiles still come.
    Leaving(RenderId),
}

impl Mode {
    fn name(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open(_) => "open",
            Self::Leaving(_) => "leaving",
        }
    }
}

/// Retouch's state, one per app.
///
/// Lifecycle: the render retouched (`Mode`: closed, open, leaving), its
/// canvas ([`CanvasState`]: opening, ready, failed) and its pace (`Pace`:
/// idle, scrubbing, settling, prefetching); changed only by
/// [`Machine::on`], driven by [`super::PaintSession`]. The module's
/// diagram shows the transitions.
pub struct Machine {
    words: Words,
    mode: Mode,
    tokens: u64,
    picks: u64,
    reported: Option<Status>,
}

/// What one event does, gathered.
#[derive(Default)]
struct Out(Vec<Action>);

impl Out {
    fn send(&mut self, order: Order) {
        self.0.push(Action::Send(order));
    }

    fn report(&mut self, report: Report) {
        self.0.push(Action::Report(report));
    }
}

impl Machine {
    pub fn new(words: Words) -> Self {
        Self { words, mode: Mode::Closed, tokens: 0, picks: 0, reported: None }
    }

    /// `None` while paint mode is off.
    pub fn status(&self) -> Option<Status> {
        match &self.mode {
            Mode::Open(open) => Some(open.status()),
            Mode::Closed | Mode::Leaving(_) => None,
        }
    }

    /// The render painted and its stack, while paint mode is on.
    pub fn painting(&self) -> Option<(&RenderId, &Arc<CompletedStack>)> {
        match &self.mode {
            Mode::Open(open) => Some((&open.key, &open.stack)),
            Mode::Closed | Mode::Leaving(_) => None,
        }
    }

    pub fn on(&mut self, event: Event) -> Vec<Action> {
        let mut out = Out::default();
        self.handle(event, &mut out);
        if let Some(status) = self.status()
            && self.reported.as_ref() != Some(&status)
        {
            self.reported = Some(status.clone());
            out.report(Report::Status(status));
        }
        out.0
    }

    fn set_mode(&mut self, mode: Mode) {
        tracing::debug!(from = self.mode.name(), to = mode.name(), "paint session");
        self.mode = mode;
    }

    fn token(&mut self) -> Token {
        self.tokens += 1;
        Token(self.tokens)
    }

    fn open_mut(&mut self) -> Option<&mut Open> {
        match &mut self.mode {
            Mode::Open(open) => Some(open),
            Mode::Closed | Mode::Leaving(_) => None,
        }
    }

    fn handle(&mut self, event: Event, out: &mut Out) {
        match event {
            Event::Open(render) => self.open(render, out),
            Event::Retry => self.retry(out),
            Event::Close => {
                if let Mode::Open(open) = &self.mode {
                    let key = open.key.clone();
                    out.send(Order::Close);
                    out.report(Report::Closed { key: key.clone() });
                    self.reported = None;
                    self.set_mode(Mode::Leaving(key));
                }
            }
            Event::PickFrame(path) => self.pick_frame(path, out),
            Event::ScrubTo(path) => self.scrub_to(path, out),
            Event::Step(by) => {
                let Some(open) = self.open_mut() else { return };
                let frames = &open.frames;
                if frames.is_empty() || by == 0 {
                    return;
                }
                let at = match &open.selection {
                    SourceKind::Frame { path } => frames.iter().position(|f| f == path),
                    _ => None,
                };
                let at = at.map_or(if by > 0 { -1 } else { frames.len() as isize }, |i| i as isize);
                let next = (at + by).clamp(0, frames.len() as isize - 1) as usize;
                let next = frames[next].clone();
                self.pick_frame(next, out);
            }
            Event::PickOriginal => self.pick(SourceKind::Original, out),
            Event::PickRender(render) => self.pick(SourceKind::Render { render }, out),
            Event::Swap => {
                let Some(open) = self.open_mut() else { return };
                if let Some(previous) = open.previous.clone() {
                    self.pick(previous, out);
                }
            }
            Event::UseBrush => {
                let Some(open) = self.open_mut() else { return };
                if open.selection != SourceKind::Original {
                    return;
                }
                match open.previous.clone().filter(|p| *p != SourceKind::Original) {
                    Some(previous) => self.pick(previous, out),
                    None => {
                        if let Some(first) = open.first_stacked() {
                            self.pick_frame(first, out);
                        }
                    }
                }
            }
            Event::UseFrames => {
                let Some(open) = self.open_mut() else { return };
                if matches!(open.selection, SourceKind::Frame { .. }) {
                    return;
                }
                let last = open.last_frame.clone().filter(|f| open.frames.contains(f));
                if let Some(frame) = last.or_else(|| open.first_stacked()) {
                    tracing::debug!("paint: back to the frames");
                    self.pick_frame(frame, out);
                }
            }
            Event::PickAt { x, y } => self.pick_at(x, y, out),
            Event::BeginStroke { brush, at } => {
                let Some(open) = self.open_mut() else { return };
                // It would paint unseen on a canvas not yet in.
                if open.canvas != CanvasState::Ready {
                    return;
                }
                // Send a frame scrubbed to first: the stroke paints from
                // what is on screen.
                if let Pace::Scrubbing { next: Some(_), .. } = open.pace {
                    let kind = open.selection.clone();
                    self.send_pick(kind, false, out);
                }
                let Some(open) = self.open_mut() else { return };
                open.set_pace(Pace::Idle);
                out.send(Order::BeginStroke(brush));
                if let Some(dab) = at {
                    out.send(Order::Dabs(vec![dab]));
                }
            }
            Event::Dabs(dabs) => self.pass(Order::Dabs(dabs), out),
            Event::EndStroke => self.pass(Order::EndStroke, out),
            Event::Undo => self.pass(Order::Undo, out),
            Event::Redo => self.pass(Order::Redo, out),
            Event::Reset => self.pass(Order::Reset, out),
            Event::Fired(token) => self.fired(token, out),
            Event::Worker(output) => self.output(output, out),
        }
    }

    fn pass(&mut self, order: Order, out: &mut Out) {
        if self.open_mut().is_some() {
            out.send(order);
        }
    }

    fn open(&mut self, render: OpenRender, out: &mut Out) {
        let OpenRender { key, stack, frames, first } = render;
        let first = first
            .filter(|p| frames.contains(p) && stack.stacked(p))
            .or_else(|| first_stacked(&stack, &frames));
        let selection =
            first.clone().map_or(SourceKind::Original, |path| SourceKind::Frame { path });
        let open = Open {
            key: key.clone(),
            stack,
            frames,
            canvas: CanvasState::Opening,
            selection: selection.clone(),
            previous: None,
            last_frame: first,
            step: 1,
            sent: 0,
            pace: Pace::Idle,
            worker: None,
        };
        tracing::debug!(selection = kind_name(&selection), "paint: opening");
        self.set_mode(Mode::Open(Box::new(open)));
        out.send(Order::Open(key));
        self.send_pick(selection.clone(), false, out);
        if matches!(selection, SourceKind::Frame { .. }) {
            self.prefetch_later(out);
        }
    }

    fn retry(&mut self, out: &mut Out) {
        let Some(open) = self.open_mut() else { return };
        if !matches!(open.canvas, CanvasState::Failed(_)) {
            return;
        }
        open.set_canvas(CanvasState::Opening);
        open.set_pace(Pace::Idle);
        let (key, selection) = (open.key.clone(), open.selection.clone());
        out.send(Order::Open(key));
        self.send_pick(selection, false, out);
    }

    fn send_pick(&mut self, kind: SourceKind, scrub: bool, out: &mut Out) {
        self.picks += 1;
        let pick = self.picks;
        if let Some(open) = self.open_mut() {
            open.sent = pick;
        }
        out.send(Order::Pick { kind, scrub, pick });
    }

    /// Paint from `kind` in full.
    fn pick(&mut self, kind: SourceKind, out: &mut Out) {
        let Some(open) = self.open_mut() else { return };
        open.select(kind.clone(), false);
        open.set_pace(Pace::Idle);
        self.send_pick(kind.clone(), false, out);
        if matches!(kind, SourceKind::Frame { .. }) {
            self.prefetch_later(out);
        }
    }

    fn pick_frame(&mut self, path: PathBuf, out: &mut Out) {
        let Some(open) = self.open_mut() else { return };
        if !open.frames.contains(&path) {
            return;
        }
        self.pick(SourceKind::Frame { path }, out);
    }

    fn scrub_to(&mut self, path: PathBuf, out: &mut Out) {
        let Some(open) = self.open_mut() else { return };
        let kind = SourceKind::Frame { path: path.clone() };
        if !open.frames.contains(&path) || open.selection == kind {
            return;
        }
        open.select(kind.clone(), true);
        if let Pace::Scrubbing { pick, .. } = open.pace {
            open.set_pace(Pace::Scrubbing { pick, next: Some(path) });
            return;
        }
        self.send_scrub(kind, out);
    }

    fn send_scrub(&mut self, kind: SourceKind, out: &mut Out) {
        self.send_pick(kind, true, out);
        let pick = self.picks;
        if let Some(open) = self.open_mut() {
            open.set_pace(Pace::Scrubbing { pick, next: None });
        }
    }

    fn prefetch_later(&mut self, out: &mut Out) {
        let token = self.token();
        if let Some(open) = self.open_mut() {
            open.set_pace(Pace::Prefetching(token));
            out.0.push(Action::Schedule { token, after: PREFETCH_AFTER });
        }
    }

    fn pick_at(&mut self, x: f32, y: f32, out: &mut Out) {
        let Some(open) = self.open_mut() else { return };
        let (width, height) = (open.stack.width as f32, open.stack.height as f32);
        if !(x >= 0.0 && y >= 0.0 && x < width && y < height) {
            return;
        }
        let found = open.stack.frame_at(x as u32, y as u32).and_then(|i| open.stack.paths.get(i));
        match found.filter(|p| open.frames.contains(p)).cloned() {
            Some(path) => {
                tracing::debug!("paint: picked the sharpest frame at a point");
                self.pick_frame(path, out);
            }
            None => {
                let key = open.key.clone();
                let text = say!(self.words, "paint-no-detail-record");
                out.report(Report::Notice { key, text });
            }
        }
    }

    fn fired(&mut self, token: Token, out: &mut Out) {
        let Some(open) = self.open_mut() else { return };
        match open.pace.clone() {
            Pace::Settling(t) if t == token => match open.selection.clone() {
                kind @ SourceKind::Frame { .. } => {
                    self.send_pick(kind, false, out);
                    self.prefetch_later(out);
                }
                _ => open.set_pace(Pace::Idle),
            },
            Pace::Prefetching(t) if t == token => {
                open.set_pace(Pace::Idle);
                let SourceKind::Frame { path } = &open.selection else { return };
                if let Some(next) = step_frame(&open.frames, path, open.step) {
                    out.send(Order::Prefetch(SourceKind::Frame { path: next.clone() }));
                }
            }
            _ => tracing::debug!("paint: a timer outdated by what happened since"),
        }
    }

    fn output(&mut self, output: Output, out: &mut Out) {
        let painting = self.painting().map(|(key, _)| key.clone());
        let open_on = |key: &RenderId| painting.as_ref() == Some(key);
        match output {
            Output::Canvas { key, display } => {
                let Some(open) = self.open_mut().filter(|o| o.key == key) else {
                    return dropped("a canvas");
                };
                open.set_canvas(CanvasState::Ready);
                out.report(Report::Canvas { key, display });
            }
            Output::Patches { key, patches } => {
                let leaving = matches!(&self.mode, Mode::Leaving(k) if *k == key);
                if !(open_on(&key) || leaving) {
                    return dropped("display tiles");
                }
                out.report(Report::Patches { key, patches });
            }
            Output::Source { key, kind, display } => {
                if !open_on(&key) {
                    return dropped("a source");
                }
                out.report(Report::Source { key, kind, display });
            }
            Output::Presented { key, display } => out.report(Report::Presented { key, display }),
            Output::SourceFailed { message, .. } | Output::Failed(message) => {
                out.report(Report::Failed { key: painting.clone(), message })
            }
            Output::OpenFailed { key, message } => {
                let Some(open) = self.open_mut().filter(|o| o.key == key) else {
                    return dropped("an open's failure");
                };
                open.set_canvas(CanvasState::Failed(message));
            }
            Output::State(None) => {
                if let Mode::Leaving(_) = self.mode {
                    self.set_mode(Mode::Closed);
                }
            }
            Output::State(Some(state)) => {
                let Some(open) = self.open_mut().filter(|o| o.key == state.key) else {
                    return dropped("a state");
                };
                let answered = state.pick;
                let kind = state.source_kind().cloned().unwrap_or(SourceKind::Original);
                open.worker = Some(state);
                if let Pace::Scrubbing { pick, next } = open.pace.clone()
                    && answered >= pick
                {
                    match next {
                        Some(path) => {
                            self.send_scrub(SourceKind::Frame { path }, out);
                            return;
                        }
                        None => {
                            let token = self.token();
                            let open = self.open_mut().expect("open, checked above");
                            open.set_pace(Pace::Settling(token));
                            out.0.push(Action::Schedule { token, after: SCRUB_SETTLE });
                        }
                    }
                }
                let open = self.open_mut().expect("open, checked above");
                // Every pick answered: a pick it couldn't load falls back.
                let caught_up =
                    answered >= open.sent && !matches!(open.pace, Pace::Scrubbing { .. });
                if caught_up && open.selection != kind {
                    tracing::debug!(
                        kept = kind_name(&kind),
                        "paint: the worker kept another source"
                    );
                    open.select(kind, false);
                }
            }
        }
    }
}

fn dropped(what: &str) {
    tracing::debug!("paint: {what} dropped: its render is no longer painted");
}

/// The first frame of `frames` that `stack` stacked.
fn first_stacked(stack: &CompletedStack, frames: &[PathBuf]) -> Option<PathBuf> {
    frames.iter().find(|p| stack.stacked(p)).cloned()
}

/// For the log: the kind, no file names.
fn kind_name(kind: &SourceKind) -> &'static str {
    match kind {
        SourceKind::Original => "original",
        SourceKind::Frame { .. } => "frame",
        SourceKind::Render { .. } => "render",
    }
}

impl Open {
    fn status(&self) -> Status {
        let worker = self.worker.as_ref();
        let ready = worker.is_some_and(|w| {
            w.pick >= self.sent && w.source_kind() == Some(&self.selection) && w.source_ready()
        });
        let lit = match &self.selection {
            SourceKind::Frame { path } if self.stack.frame_map.is_some() => {
                self.stack.paths.iter().position(|p| p == path)
            }
            _ => None,
        };
        Status {
            key: self.key.clone(),
            canvas: self.canvas.clone(),
            selection: self.selection.clone(),
            ready,
            last_frame: self.last_frame.clone(),
            lit,
            undo: worker.map_or(0, |w| w.undo),
            redo: worker.map_or(0, |w| w.redo),
            undo_label: worker.and_then(|w| w.undo_label.clone()),
            redo_label: worker.and_then(|w| w.redo_label.clone()),
            painted: worker.is_some_and(|w| w.painted),
            unsaveable: worker.and_then(|w| w.unsaveable.clone()),
            painted_from: worker.map_or_else(Vec::new, |w| w.painted_from.clone()),
        }
    }

    fn first_stacked(&self) -> Option<PathBuf> {
        first_stacked(&self.stack, &self.frames)
    }

    /// `scrubbing`: a frame of a scrub, which doesn't move the previous
    /// source after the first.
    fn select(&mut self, kind: SourceKind, scrubbing: bool) {
        if kind == self.selection {
            return;
        }
        tracing::debug!(
            from = kind_name(&self.selection),
            to = kind_name(&kind),
            "paint: selection"
        );
        let mid_scrub =
            scrubbing && matches!(self.pace, Pace::Scrubbing { .. } | Pace::Settling(_));
        if let (SourceKind::Frame { path: from }, SourceKind::Frame { path: to }) =
            (&self.selection, &kind)
        {
            let at = |p: &Path| self.frames.iter().position(|f| f == p);
            if let (Some(from), Some(to)) = (at(from), at(to)) {
                self.step = (to as isize - from as isize).signum();
            }
        }
        if let SourceKind::Frame { path } = &kind {
            self.last_frame = Some(path.clone());
        }
        let old = std::mem::replace(&mut self.selection, kind);
        if !mid_scrub {
            self.previous = Some(old);
        }
    }

    fn set_canvas(&mut self, canvas: CanvasState) {
        tracing::debug!(from = self.canvas.name(), to = canvas.name(), "paint: canvas");
        self.canvas = canvas;
    }

    fn set_pace(&mut self, pace: Pace) {
        if self.pace.name() != pace.name() {
            tracing::debug!(from = self.pace.name(), to = pace.name(), "paint: pace");
        }
        self.pace = pace;
    }
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
