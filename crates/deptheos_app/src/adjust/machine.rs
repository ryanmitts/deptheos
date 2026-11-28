//! Adjust as one state machine, shared by both apps (ADR-153).
//!
//! The session feeds each [`Event`] to [`Machine::on`] and carries out the
//! [`Action`]s back, in order. No I/O and no clock. Every request it asks
//! for (the reduced copy, a preview tick, the full picture, a write) has a
//! [`RequestId`], and its result counts only while the state still holds
//! that id: a stale result is dropped and counted.
//!
//! ```text
//!          open             its reduced copy in
//!   Closed ──────▶ Opening ─────────────────────▶ Ready (its Pace)
//!     ▲             ▲  ▲ │ the upload failed       │  │ a draw failed
//!     │       Retry │  │ ▼                         │  ▼
//!     │             │  Failed ◀──────────────────────┘
//!     │             │      away (any open state)   │
//!     │             └── back ── Away ◀─────────────┘
//!     │
//!     └── Leaving, its write and last full picture in ◀── leave (any open state)
//! ```
//!
//! What it settles, the same way on both apps:
//!
//! - **A change is a set, then a commit.** [`Event::Set`] moves the value
//!   shown (a drag tick, an arrow key, a typed value, the eyedropper, a
//!   crop, a paste, a reset); [`Event::Commit`] ends the change: one store
//!   write and one undo step if the value moved, never one per tick. So a
//!   typed value or an arrow key writes and undoes as a drag does.
//! - **One preview tick in flight** (`Pace`): a set while one draws waits,
//!   the newest only, and is drawn when it lands. A commit draws the full
//!   picture once the tick in flight lands, so a held arrow key keeps
//!   showing previews.
//! - **Undo and redo** step through whole values, one per committed
//!   change, kept per render for the launch, across leaving the mode, as
//!   Retouch's are (ADR-148). Each writes.
//! - **Writes outlive the session.** Leaving waits for the render's last
//!   write; a delete takes [`AdjustClosed`], given only when none of its
//!   renders is open or has a write pending.

use std::collections::HashMap;
use std::fmt;

use crate::RenderId;
use crate::library::adjustments::RenderAdjustments;

/// The most undo steps a render keeps; the oldest go first.
pub const KEPT_STEPS: usize = 500;

/// A request's name: its result counts only while the state still holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

/// What the session saw: a call from the app, a result of a request.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Adjust on render `key`, whose adjustments read `value`, as the
    /// viewer shows it now. Another render open is left first.
    Open {
        key: RenderId,
        value: RenderAdjustments,
    },
    /// The value shown moves; a change goes on until [`Event::Commit`].
    Set(RenderAdjustments),
    /// The change ends: written and undoable if the value moved.
    Commit,
    Undo,
    Redo,
    /// The open failed: upload the reduced copy again.
    Retry,
    /// Leave Adjust.
    Leave,
    /// The app went to the background, or memory ran low: free the GPU
    /// copies, keep the values.
    Away,
    /// The app came back: make the reduced copy again.
    Back,
    /// The stored picture with its paint changed (its generation number
    /// moved): the reduced copy and the settled picture are stale.
    StoredChanged,
    ReducedReady(RequestId),
    ReducedFailed {
        id: RequestId,
        why: String,
    },
    PreviewDrawn(RequestId),
    FullDrawn(RequestId),
    /// The histogram of `id` counted; [`Event::CountFailed`] if not.
    Counted(RequestId),
    CountFailed(RequestId),
    DrawFailed {
        id: RequestId,
        why: String,
    },
    Written {
        key: RenderId,
        id: RequestId,
    },
    WriteFailed {
        key: RenderId,
        id: RequestId,
        why: String,
    },
}

/// What the session is to do, in the order given.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Make the open render's reduced copy on the GPU.
    UploadReduced(RequestId),
    /// Free the open render's GPU copies.
    FreeGpu,
    /// Draw `value` from the reduced copy, at the view shown.
    DrawPreview {
        id: RequestId,
        value: RenderAdjustments,
    },
    /// Draw `value` in full, from the stored pixels.
    DrawFull {
        id: RequestId,
        value: RenderAdjustments,
    },
    /// Count `value`'s histogram from the reduced copy, the full picture
    /// on screen already showing it.
    CountHistogram {
        id: RequestId,
        value: RenderAdjustments,
    },
    /// Show the picture or histogram drawn for `id`: it is current.
    Show(RequestId),
    /// Write render `key`'s adjustments, through the store thread.
    Write {
        key: RenderId,
        id: RequestId,
        value: RenderAdjustments,
    },
    Report(Report),
}

/// Where the open render stands, as the app shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// The reduced copy uploading: sliders set, nothing drawn yet.
    Opening,
    Ready,
    /// In the background or under memory pressure, GPU copies freed.
    Away,
    /// Retry opens it again.
    Failed(String),
}

/// Adjust on one render, as the app shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub key: RenderId,
    pub phase: Phase,
    /// The value shown, a change in progress included.
    pub value: RenderAdjustments,
    pub undo: usize,
    pub redo: usize,
}

/// What the app hears, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    Status(Status),
    /// Adjust left render `key`, its last write in. `redraw`: the picture
    /// on screen isn't drawn under its saved value, so the viewer draws it
    /// again.
    Closed {
        key: RenderId,
        redraw: bool,
    },
    /// A write of render `key` failed: what to say.
    WriteFailed {
        key: RenderId,
        why: String,
    },
}

/// Proof that the renders about to be deleted have no Adjust session open
/// and no write pending ([`Machine::closed`]).
#[must_use = "a delete takes it"]
#[derive(Debug)]
pub struct AdjustClosed(());

impl AdjustClosed {
    /// For a Library with no session adjusting or writing the renders
    /// ([`super::session::close`]).
    pub(crate) fn none_open() -> Self {
        Self(())
    }
}

/// One render's undo: whole values, one per committed change.
#[derive(Clone, Debug, Default, PartialEq)]
struct Steps {
    /// Values before each change, the latest last.
    done: Vec<RenderAdjustments>,
    /// Values undone, the latest undone last.
    undone: Vec<RenderAdjustments>,
}

impl Steps {
    fn push(&mut self, before: RenderAdjustments) {
        self.done.push(before);
        if self.done.len() > KEPT_STEPS {
            self.done.remove(0);
        }
        self.undone.clear();
    }
}

/// The render adjusted.
#[derive(Debug)]
struct Open {
    key: RenderId,
    /// What is shown and drawn, a change in progress included.
    value: RenderAdjustments,
    /// What the last commit wrote (or the value as opened).
    committed: RenderAdjustments,
    /// The value of the full picture on screen; `None` when it is stale
    /// whatever the value (the stored picture changed).
    settled: Option<RenderAdjustments>,
    steps: Steps,
}

impl Open {
    fn is_settled(&self) -> bool {
        self.settled.as_ref() == Some(&self.value)
    }
}

/// The open render's draws.
///
/// Lifecycle: idle, counting, scrubbing, scrubbed, settling; changed only
/// by [`Machine::on`].
///
/// ```text
/// (opened, the full picture on screen showing the value) ──▶ Counting
/// Counting ──its histogram lands or fails──▶ Idle
/// Idle, Counting, Scrubbed, Settling ──set──▶ Scrubbing (a tick sent)
/// Scrubbing ──set──▶ Scrubbing (next: Preview)   ──commit──▶ Scrubbing (next: Settle)
/// Scrubbing ──its tick lands──▶ next None: Scrubbed; Preview: Scrubbing; Settle: Settling
/// Idle, Counting, Scrubbed ──commit, undo, redo──▶ Settling (the full picture sent)
/// Settling ──it lands──▶ Idle
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pace {
    /// The full picture on screen shows the value.
    Idle,
    /// The full picture on screen shows the value; its histogram, `id`,
    /// counting.
    Counting(RequestId),
    /// Preview `tick` drawing; `next` is what follows it.
    Scrubbing { tick: RequestId, next: Next },
    /// A preview shows the value, the full picture not drawn: a drag held
    /// still.
    Scrubbed,
    /// The full picture of the value drawing.
    Settling(RequestId),
}

/// What follows the preview tick in flight.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Next {
    Nothing,
    /// A preview of the value as it is then.
    Preview,
    /// The full picture of the value as it is then.
    Settle,
}

impl Pace {
    fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Counting(_) => "counting",
            Self::Scrubbing { .. } => "scrubbing",
            Self::Scrubbed => "scrubbed",
            Self::Settling(_) => "settling",
        }
    }
}

/// Lifecycle: closed, opening, ready (with its `Pace`), away, failed,
/// leaving; changed only by [`Machine::on`], as the module's diagram shows.
///
/// - Closed → Opening: [`Event::Open`].
/// - Opening → Ready: its reduced copy in; → Failed: its upload failed.
/// - Ready → Failed: a draw failed; Failed → Opening: [`Event::Retry`].
/// - Opening, Ready, Failed → Away: [`Event::Away`]; Away → Opening:
///   [`Event::Back`].
/// - Ready, Opening → Opening: [`Event::StoredChanged`].
/// - any open state → Leaving: [`Event::Leave`] or another render opened;
///   Leaving → Closed: its write and last full draw in.
#[derive(Debug)]
enum State {
    Closed,
    /// The reduced copy uploading, request `upload`.
    Opening {
        open: Box<Open>,
        upload: RequestId,
    },
    Ready {
        open: Box<Open>,
        pace: Pace,
    },
    /// GPU copies freed; the values are kept and still written.
    Away(Box<Open>),
    Failed {
        open: Box<Open>,
        why: String,
    },
    /// Left render `key`, its full picture `full` still drawing.
    Leaving {
        key: RenderId,
        full: Option<RequestId>,
        redraw: bool,
    },
}

impl State {
    fn name(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Opening { .. } => "opening",
            Self::Ready { .. } => "ready",
            Self::Away(_) => "away",
            Self::Failed { .. } => "failed",
            Self::Leaving { .. } => "leaving",
        }
    }

    fn open(&self) -> Option<&Open> {
        match self {
            Self::Opening { open, .. }
            | Self::Ready { open, .. }
            | Self::Away(open)
            | Self::Failed { open, .. } => Some(open),
            Self::Closed | Self::Leaving { .. } => None,
        }
    }

    fn open_mut(&mut self) -> Option<&mut Open> {
        match self {
            Self::Opening { open, .. }
            | Self::Ready { open, .. }
            | Self::Away(open)
            | Self::Failed { open, .. } => Some(open),
            Self::Closed | Self::Leaving { .. } => None,
        }
    }

    /// The render this state is about, leaving included.
    fn key(&self) -> Option<&RenderId> {
        match self {
            Self::Leaving { key, .. } => Some(key),
            _ => self.open().map(|open| &open.key),
        }
    }
}

/// Adjust's state, one per app.
///
/// Lifecycle: the render adjusted (`State`: closed, opening, ready, away,
/// failed, leaving) and, while ready, its draws (`Pace`: idle, scrubbing,
/// scrubbed, settling); changed only by [`Machine::on`]. The module's
/// diagram shows the transitions.
pub struct Machine {
    state: State,
    /// Each render's undo for the launch, while its session is closed.
    steps: HashMap<RenderId, Steps>,
    /// Each render's latest write not yet in.
    writes: HashMap<RenderId, RequestId>,
    requests: u64,
    /// Results dropped because their request was no longer current.
    stale: u64,
    reported: Option<Status>,
}

impl fmt::Debug for Machine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Machine({}, {} writes pending)", self.state.name(), self.writes.len())
    }
}

/// What one event does, gathered.
#[derive(Default)]
struct Out(Vec<Action>);

impl Out {
    fn push(&mut self, action: Action) {
        self.0.push(action);
    }
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    pub fn new() -> Self {
        Self {
            state: State::Closed,
            steps: HashMap::new(),
            writes: HashMap::new(),
            requests: 0,
            stale: 0,
            reported: None,
        }
    }

    /// `None` while Adjust is off.
    pub fn status(&self) -> Option<Status> {
        let open = self.state.open()?;
        let phase = match &self.state {
            State::Opening { .. } => Phase::Opening,
            State::Ready { .. } => Phase::Ready,
            State::Away(_) => Phase::Away,
            State::Failed { why, .. } => Phase::Failed(why.clone()),
            State::Closed | State::Leaving { .. } => return None,
        };
        Some(Status {
            key: open.key.clone(),
            phase,
            value: open.value.clone(),
            undo: open.steps.done.len() + usize::from(open.value != open.committed),
            redo: open.steps.undone.len(),
        })
    }

    /// The render adjusted, while Adjust is on.
    pub fn adjusting(&self) -> Option<&RenderId> {
        self.state.open().map(|open| &open.key)
    }

    /// Whether render `key` has a write not yet in: an export of it waits.
    pub fn is_writing(&self, key: &RenderId) -> bool {
        self.writes.contains_key(key)
    }

    /// Whether full picture `id` is the one the render settles on now:
    /// the session shows its view on screen first while it is.
    pub fn is_settling(&self, id: RequestId) -> bool {
        matches!(&self.state, State::Ready { pace: Pace::Settling(full), .. } if *full == id)
    }

    /// Results dropped as stale so far.
    pub fn stale(&self) -> u64 {
        self.stale
    }

    /// Proof for deleting `ids`: none of them is adjusted, being left, or
    /// has a write pending.
    pub fn closed(&self, ids: &[RenderId]) -> Option<AdjustClosed> {
        let busy = |id: &RenderId| self.state.key() == Some(id) || self.writes.contains_key(id);
        (!ids.iter().any(busy)).then_some(AdjustClosed(()))
    }

    pub fn on(&mut self, event: Event) -> Vec<Action> {
        let mut out = Out::default();
        self.handle(event, &mut out);
        if let Some(status) = self.status()
            && self.reported.as_ref() != Some(&status)
        {
            self.reported = Some(status.clone());
            out.push(Action::Report(Report::Status(status)));
        }
        out.0
    }

    fn set_state(&mut self, state: State) {
        let pace = match &state {
            State::Ready { pace, .. } => pace.name(),
            _ => "",
        };
        tracing::debug!(from = self.state.name(), to = state.name(), pace, "adjust session");
        self.state = state;
    }

    fn request(&mut self) -> RequestId {
        self.requests += 1;
        RequestId(self.requests)
    }

    fn drop_stale(&mut self, what: &str) {
        self.stale += 1;
        tracing::debug!(stale = self.stale, "adjust: a stale {what} dropped");
    }

    /// The state taken out, closed in its place.
    fn take(&mut self) -> State {
        std::mem::replace(&mut self.state, State::Closed)
    }

    fn handle(&mut self, event: Event, out: &mut Out) {
        match event {
            Event::Open { key, value } => self.open(key, value, out),
            Event::Set(value) => self.set(value, out),
            Event::Commit => {
                if self.commit(out) {
                    self.settle(out);
                }
            }
            Event::Undo => self.step(true, out),
            Event::Redo => self.step(false, out),
            Event::Retry => {
                if let State::Failed { .. } = self.state
                    && let State::Failed { open, .. } = self.take()
                {
                    self.upload(open, out);
                }
            }
            Event::Leave => self.leave(out),
            Event::Away => match self.take() {
                State::Opening { open, .. } => {
                    out.push(Action::FreeGpu);
                    self.set_state(State::Away(open));
                }
                State::Ready { open, pace } => {
                    out.push(Action::FreeGpu);
                    self.set_state(State::Away(on_screen(open, pace)));
                }
                State::Failed { open, .. } => self.set_state(State::Away(open)),
                other => self.state = other,
            },
            Event::Back => match self.take() {
                State::Away(open) => self.upload(open, out),
                other => self.state = other,
            },
            Event::StoredChanged => match self.take() {
                State::Opening { mut open, .. } | State::Ready { mut open, .. } => {
                    open.settled = None;
                    self.upload(open, out);
                }
                mut other => {
                    if let Some(open) = other.open_mut() {
                        open.settled = None;
                    }
                    self.state = other;
                }
            },
            Event::ReducedReady(id) => match self.take() {
                State::Opening { open, upload } if upload == id => {
                    self.set_state(State::Ready { open, pace: Pace::Idle });
                    if self.state.open().is_some_and(Open::is_settled) {
                        self.count(out);
                    } else {
                        self.settle(out);
                    }
                }
                other => {
                    self.state = other;
                    self.drop_stale("reduced copy");
                }
            },
            Event::ReducedFailed { id, why } => match self.take() {
                State::Opening { open, upload } if upload == id => {
                    tracing::debug!("adjust: the reduced copy failed: {why}");
                    self.set_state(State::Failed { open, why });
                }
                other => {
                    self.state = other;
                    self.drop_stale("reduced copy failure");
                }
            },
            Event::PreviewDrawn(id) => self.preview_drawn(id, out),
            Event::FullDrawn(id) => self.full_drawn(id, out),
            Event::Counted(id) => self.counted(id, true, out),
            Event::CountFailed(id) => self.counted(id, false, out),
            Event::DrawFailed { id, why } => self.draw_failed(id, why, out),
            Event::Written { key, id } => self.written(&key, id, out),
            Event::WriteFailed { key, id, why } => {
                tracing::debug!(key = key.as_str(), "adjust: a write failed: {why}");
                out.push(Action::Report(Report::WriteFailed { key: key.clone(), why }));
                self.written(&key, id, out);
            }
        }
    }

    fn open(&mut self, key: RenderId, value: RenderAdjustments, out: &mut Out) {
        if self.state.open().is_some_and(|open| open.key == key) {
            return;
        }
        self.leave(out);
        let steps = self.steps.remove(&key).unwrap_or_default();
        let open = Open {
            key,
            value: value.clone(),
            committed: value.clone(),
            settled: Some(value),
            steps,
        };
        self.upload(Box::new(open), out);
    }

    /// Ask for `open`'s reduced copy.
    fn upload(&mut self, open: Box<Open>, out: &mut Out) {
        let upload = self.request();
        out.push(Action::UploadReduced(upload));
        self.set_state(State::Opening { open, upload });
    }

    fn set(&mut self, value: RenderAdjustments, out: &mut Out) {
        let Some(open) = self.state.open_mut() else { return };
        if open.value == value {
            return;
        }
        open.value = value;
        let State::Ready { pace, .. } = &self.state else { return };
        match *pace {
            Pace::Scrubbing { tick, .. } => {
                self.set_pace(Pace::Scrubbing { tick, next: Next::Preview })
            }
            Pace::Idle | Pace::Counting(_) | Pace::Scrubbed | Pace::Settling(_) => self.tick(out),
        }
    }

    /// Count the histogram of the value, the full picture on screen.
    fn count(&mut self, out: &mut Out) {
        let Some(open) = self.state.open() else { return };
        let value = open.value.clone();
        let id = self.request();
        out.push(Action::CountHistogram { id, value });
        self.set_pace(Pace::Counting(id));
    }

    fn counted(&mut self, id: RequestId, ok: bool, out: &mut Out) {
        if !matches!(self.state, State::Ready { pace: Pace::Counting(c), .. } if c == id) {
            return self.drop_stale("histogram");
        }
        if ok {
            out.push(Action::Show(id));
        }
        self.set_pace(Pace::Idle);
    }

    /// Send a preview tick of the value.
    fn tick(&mut self, out: &mut Out) {
        let Some(open) = self.state.open() else { return };
        let value = open.value.clone();
        let tick = self.request();
        out.push(Action::DrawPreview { id: tick, value });
        self.set_pace(Pace::Scrubbing { tick, next: Next::Nothing });
    }

    fn set_pace(&mut self, to: Pace) {
        if let State::Ready { pace, .. } = &mut self.state {
            if pace.name() != to.name() {
                tracing::debug!(from = pace.name(), to = to.name(), "adjust pace");
            }
            *pace = to;
        }
    }

    /// End the change: write the value and keep an undo step if it moved.
    /// Whether a session is open.
    fn commit(&mut self, out: &mut Out) -> bool {
        let Some(open) = self.state.open_mut() else { return false };
        if open.value != open.committed {
            let before = std::mem::replace(&mut open.committed, open.value.clone());
            open.steps.push(before);
            self.write(out);
        }
        true
    }

    /// Write the open render's committed value.
    fn write(&mut self, out: &mut Out) {
        let id = self.request();
        let Some(open) = self.state.open() else { return };
        let (key, value) = (open.key.clone(), open.committed.clone());
        self.writes.insert(key.clone(), id);
        out.push(Action::Write { key, id, value });
    }

    /// Draw the full picture of the value, now or once the tick in flight
    /// lands, unless it is on screen or drawing.
    fn settle(&mut self, out: &mut Out) {
        let State::Ready { open, pace } = &self.state else { return };
        match *pace {
            Pace::Idle | Pace::Counting(_) if open.is_settled() => {}
            Pace::Settling(_) => {}
            Pace::Scrubbing { tick, .. } => {
                self.set_pace(Pace::Scrubbing { tick, next: Next::Settle });
            }
            Pace::Idle | Pace::Counting(_) | Pace::Scrubbed => self.full(out),
        }
    }

    /// Send the full picture of the value.
    fn full(&mut self, out: &mut Out) {
        let Some(open) = self.state.open() else { return };
        let value = open.value.clone();
        let id = self.request();
        out.push(Action::DrawFull { id, value });
        self.set_pace(Pace::Settling(id));
    }

    /// Undo (`back`) or redo one committed change, a change in progress
    /// committed first.
    fn step(&mut self, back: bool, out: &mut Out) {
        if !self.commit(out) {
            return;
        }
        let Some(open) = self.state.open_mut() else { return };
        let (from, to) = if back {
            (&mut open.steps.done, &mut open.steps.undone)
        } else {
            (&mut open.steps.undone, &mut open.steps.done)
        };
        let Some(value) = from.pop() else { return };
        to.push(std::mem::replace(&mut open.committed, value.clone()));
        open.value = value;
        self.write(out);
        self.settle(out);
    }

    fn preview_drawn(&mut self, id: RequestId, out: &mut Out) {
        let State::Ready { pace: Pace::Scrubbing { tick, next }, .. } = self.state else {
            return self.drop_stale("preview");
        };
        if tick != id {
            return self.drop_stale("preview");
        }
        out.push(Action::Show(id));
        match next {
            Next::Nothing => self.set_pace(Pace::Scrubbed),
            Next::Preview => self.tick(out),
            Next::Settle => self.full(out),
        }
    }

    fn full_drawn(&mut self, id: RequestId, out: &mut Out) {
        match &mut self.state {
            State::Ready { open, pace: pace @ Pace::Settling(_) }
                if *pace == Pace::Settling(id) =>
            {
                open.settled = Some(open.value.clone());
                out.push(Action::Show(id));
                self.set_pace(Pace::Idle);
            }
            State::Leaving { full, redraw, .. } if *full == Some(id) => {
                out.push(Action::Show(id));
                *full = None;
                *redraw = false;
                self.close_if_done(out);
            }
            _ => self.drop_stale("full picture"),
        }
    }

    fn draw_failed(&mut self, id: RequestId, why: String, out: &mut Out) {
        let current = match &self.state {
            State::Ready { pace: Pace::Scrubbing { tick, .. }, .. } => *tick == id,
            State::Ready { pace: Pace::Settling(full), .. } => *full == id,
            State::Leaving { full, .. } => *full == Some(id),
            _ => false,
        };
        if !current {
            return self.drop_stale("draw failure");
        }
        tracing::debug!("adjust: a draw failed: {why}");
        match self.take() {
            State::Ready { open, pace } => {
                out.push(Action::FreeGpu);
                self.set_state(State::Failed { open: on_screen(open, pace), why });
            }
            State::Leaving { key, .. } => {
                self.state = State::Leaving { key, full: None, redraw: true };
                self.close_if_done(out);
            }
            other => self.state = other,
        }
    }

    fn written(&mut self, key: &RenderId, id: RequestId, out: &mut Out) {
        if self.writes.get(key) != Some(&id) {
            // A later write of the render is still to come.
            return;
        }
        self.writes.remove(key);
        self.close_if_done(out);
    }

    /// Leave the open render, a change in progress committed: Leaving
    /// until its write and last full draw are in.
    fn leave(&mut self, out: &mut Out) {
        if !self.commit(out) {
            return;
        }
        let (open, ready) = match self.take() {
            State::Ready { open, pace } => (on_screen(open, pace), true),
            State::Opening { open, .. } | State::Away(open) | State::Failed { open, .. } => {
                (open, false)
            }
            other @ (State::Closed | State::Leaving { .. }) => {
                self.state = other;
                return;
            }
        };
        let Open { key, committed, settled, steps, .. } = *open;
        self.steps.insert(key.clone(), steps);
        let shown = settled.as_ref() == Some(&committed);
        // Drawn only where the GPU copies still are; else the viewer draws it.
        let full = (!shown && ready).then(|| {
            let id = self.request();
            out.push(Action::DrawFull { id, value: committed });
            id
        });
        self.reported = None;
        self.set_state(State::Leaving { key, full, redraw: !shown && full.is_none() });
        self.close_if_done(out);
    }

    /// Closed, once leaving's write and full draw are in.
    fn close_if_done(&mut self, out: &mut Out) {
        let State::Leaving { key, full: None, redraw } = &self.state else { return };
        if self.writes.contains_key(key) {
            return;
        }
        let report = Report::Closed { key: key.clone(), redraw: *redraw };
        out.push(Action::FreeGpu);
        out.push(Action::Report(report));
        self.set_state(State::Closed);
    }
}

/// `open` as the screen has it under `pace`: with a preview over the full
/// picture, what the full picture shows no longer counts.
fn on_screen(mut open: Box<Open>, pace: Pace) -> Box<Open> {
    if !matches!(pace, Pace::Idle | Pace::Counting(_)) {
        open.settled = None;
    }
    open
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
