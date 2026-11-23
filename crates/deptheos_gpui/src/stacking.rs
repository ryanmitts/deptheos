//! The stacking model: one run at a time, its progress and the finished
//! stack. Draws nothing; the app view reacts to its [`StackingEvent`]s.
//!
//! The engine's blocking `stack` runs on a worker ([`crate::work`]); its
//! callbacks send progress and live maps over a channel a UI task drains.
//! The run ends with `Done`, and an engine panic is caught as a failure, so
//! the window never waits on a run that is gone.

use deptheos_app::Cancel;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use deptheos_app::library::history::RenderId;
use deptheos_app::runs::session::{Progress, Step};
use deptheos_app::{
    CompletedStack, Engine, Explanation, PresentedImage, PresentedStack, SessionEvent,
    SessionOptions, StackerConfig,
};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{Context, EventEmitter, Task};
use image::RgbImage;

use crate::work::{self, panic_message};

#[derive(Clone, Debug, PartialEq)]
pub enum StackingEvent {
    /// A new live map is waiting in [`Stacking::take_live_map`].
    LiveMap,
    /// Finished; images in [`Stacking::take_final_images`], or a batch's
    /// stack in [`Stacking::take_made`].
    Finished {
        seconds: f32,
    },
    Canceled,
    /// The run ended with an error, explained for the desktop.
    Failed(Explanation),
    /// The result on screen changed: another, or none.
    Shown,
}

/// A finished stack's display and map, and its render's thumbnail.
pub struct FinalImages {
    pub display: Option<PresentedImage>,
    pub map: Option<PresentedImage>,
    pub thumbnail: Option<RgbImage>,
}

/// Where a run is.
///
/// Lifecycle: reading, aligning, restarting, blending, finishing or
/// canceling, owned by the run's [`Track`] and changed only by
/// [`Track::apply_event`] and [`Stacking::cancel`]. `Canceling` holds until
/// the run ends.
///
/// - `Reading` → `Aligning` (first frame loaded and aligned)
/// - `Aligning` → `Restarting` (frames in reverse order: pass 1 restarts
///   reversed) → `Aligning`
/// - `Reading`, `Aligning` or `Restarting` → `Blending` (pass 1 done)
/// - `Blending` → `Finishing` (result made, being presented)
/// - any → `Canceling` (the run ends at its next check)
///
/// The run ends with `Done`, in any phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Reading the first frame.
    Reading,
    /// Loading and aligning each frame: `loaded` of the run's `total`.
    Aligning { loaded: usize, total: usize },
    /// The frames turned out to be in reverse order; aligning starts over.
    Restarting,
    /// Blending the aligned frames.
    Blending,
    /// The result is made and being presented.
    Finishing,
    /// Cancel was asked; the run stops at its next check.
    Canceling,
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Self::Reading => "reading",
            Self::Aligning { .. } => "aligning",
            Self::Restarting => "restarting",
            Self::Blending => "blending",
            Self::Finishing => "finishing",
            Self::Canceling => "canceling",
        }
    }
}

/// A run's phase and time estimate over the engine's [`Progress`].
#[derive(Debug)]
struct Track {
    /// The step, the frames taken in and the progress bar.
    progress: Progress,
    /// When the time estimate starts: the run's start, or its restart.
    estimate_from: Instant,
    phase: Phase,
}

/// How far a run of the stack on screen has come, for the rail: `count`
/// of `of` frames taken in, last to first when `reversed`. Gone with the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Taken {
    pub count: usize,
    pub of: usize,
    pub reversed: bool,
}

impl Taken {
    /// Each frame's legend color once taken in (`None` before), in frame
    /// order: the final map's colors (`CompletedStack::frame_color`).
    pub fn legend(&self) -> Vec<Option<[u8; 3]>> {
        deptheos_app::engine::present::running_legend(self.of, self.count, self.reversed)
    }
}

impl Track {
    fn new(total: usize, needs_blend: bool) -> Self {
        Self {
            progress: Progress::new(total as u32, needs_blend),
            estimate_from: Instant::now(),
            phase: Phase::Reading,
        }
    }

    fn set_phase(&mut self, next: Phase) {
        if self.phase == Phase::Canceling {
            return;
        }
        if std::mem::discriminant(&self.phase) != std::mem::discriminant(&next) {
            tracing::debug!(from = self.phase.name(), to = next.name(), "stack run");
        }
        self.phase = next;
    }

    /// The progress after `event`, and the phase it puts the run in.
    fn apply_event(&mut self, event: SessionEvent) {
        self.progress.on(event);
        let total = self.progress.frames() as usize;
        match self.progress.step() {
            Step::Started => {}
            Step::Aligning { current } => {
                self.set_phase(Phase::Aligning { loaded: current as usize, total })
            }
            Step::Reversed => {
                self.set_phase(Phase::Restarting);
                self.estimate_from = Instant::now();
            }
            Step::Finalized | Step::Blending { .. } => self.set_phase(Phase::Blending),
            Step::Completed => self.set_phase(Phase::Finishing),
        }
    }

    /// The progress bar, 0 to 1.
    fn fraction(&self) -> f32 {
        self.progress.fraction()
    }

    /// The run's phase, bar, and frames taken in with whether reversed.
    #[cfg(test)]
    fn seen(&self) -> (Phase, f32, usize, bool) {
        let p = &self.progress;
        (self.phase, p.fraction(), p.taken() as usize, p.reversed())
    }

    /// Roughly the time left, once enough of the run (since any restart) tells.
    fn time_left(&self) -> Option<std::time::Duration> {
        let done = self.fraction();
        if done < 0.1 || matches!(self.phase, Phase::Restarting | Phase::Canceling) {
            return None;
        }
        let elapsed = self.estimate_from.elapsed().as_secs_f32();
        Some(std::time::Duration::from_secs_f32(elapsed * (1.0 - done) / done))
    }
}

/// Whose stack a run makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whose {
    /// The stack on screen's: it replaces the result, showing live maps.
    Screen,
    /// A batch's: the result stays, the stack handed over by
    /// [`Stacking::take_made`].
    Batch,
}

/// A batch's stack, made and waiting to be taken.
pub struct Made {
    pub stack: Arc<CompletedStack>,
    pub thumbnail: Option<RgbImage>,
}

struct Run {
    whose: Whose,
    cancel: Cancel,
    started: Instant,
    track: Track,
    _drain: Task<()>,
}

enum Message {
    Event(SessionEvent),
    LiveMap(PresentedImage),
    Done(Box<Result<PresentedStack, deptheos_app::Error>>),
}

/// The result on screen and, once saved, its render: the one record of
/// what is shown, read by the strip, title, paint and compare.
pub struct ShownResult {
    pub stack: Arc<CompletedStack>,
    pub render: Option<RenderId>,
}

pub struct Stacking {
    engine: Arc<Engine>,
    run: Option<Run>,
    /// Its stack is shared with any export being written, so a new stack
    /// never pulls it out from under one.
    shown: Option<ShownResult>,
    /// The last stack's files in stack order (a position indexes
    /// `geometry.transforms`), by path so a reopened render finds its frames.
    stacked: Vec<PathBuf>,
    live_map: Option<PresentedImage>,
    final_images: Option<FinalImages>,
    /// A batch's stack, made and not yet taken.
    made: Option<Made>,
}

impl EventEmitter<StackingEvent> for Stacking {}

impl Stacking {
    pub fn engine(&self) -> Arc<Engine> {
        self.engine.clone()
    }

    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            run: None,
            shown: None,
            stacked: Vec::new(),
            live_map: None,
            final_images: None,
            made: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// A stack of the stack on screen runs, locking its list (a batch's
    /// frames are not the list's).
    pub fn runs_on_screen(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.whose == Whose::Screen)
    }

    /// The running stack's progress, 0 to 1.
    pub fn progress(&self) -> Option<f32> {
        self.run.as_ref().map(|r| r.track.fraction())
    }

    /// The running stack's phase.
    pub fn phase(&self) -> Option<Phase> {
        self.run.as_ref().map(|r| r.track.phase)
    }

    /// The rail's progress for a run on screen; `None` otherwise.
    pub fn taken(&self) -> Option<Taken> {
        let run = self.run.as_ref().filter(|r| r.whose == Whose::Screen)?;
        let p = &run.track.progress;
        Some(Taken { count: p.taken() as usize, of: p.frames() as usize, reversed: p.reversed() })
    }

    /// The files the last (or running on-screen) stack used, in stack order.
    pub fn stacked(&self) -> &[PathBuf] {
        &self.stacked
    }

    /// Whether the running stack was asked to cancel and is stopping.
    pub fn is_canceling(&self) -> bool {
        self.phase() == Some(Phase::Canceling)
    }

    /// Roughly the running stack's time left, once it can tell.
    pub fn time_left(&self) -> Option<std::time::Duration> {
        self.run.as_ref()?.track.time_left()
    }

    /// The stack on screen.
    pub fn result(&self) -> Option<&Arc<CompletedStack>> {
        self.shown.as_ref().map(|s| &s.stack)
    }

    /// The render on screen, once the result is saved as one.
    pub fn shown_render(&self) -> Option<&RenderId> {
        self.shown.as_ref().and_then(|s| s.render.as_ref())
    }

    /// The result with its render, which paint is keyed by; `None` until saved.
    pub fn saved_result(&self) -> Option<(Arc<CompletedStack>, RenderId)> {
        let shown = self.shown.as_ref()?;
        Some((shown.stack.clone(), shown.render.clone()?))
    }

    /// `stack` was saved as `render`; it is the render on screen only if
    /// still shown (a save can land after another open). Returns whether.
    pub fn mark_saved(
        &mut self,
        stack: &Arc<CompletedStack>,
        render: RenderId,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.shown.as_mut() {
            Some(shown) if Arc::ptr_eq(&shown.stack, stack) => {
                shown.render = Some(render);
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// The last stack's position of the shot of `files` (a RAW, its JPEG).
    pub fn frame_index(&self, files: &[PathBuf]) -> Option<usize> {
        self.stacked.iter().position(|f| files.contains(f))
    }

    /// The last stack's first frame.
    pub fn first_frame(&self) -> Option<&Path> {
        self.stacked.first().map(PathBuf::as_path)
    }

    /// Forget the last stack; a running save keeps its own handle.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.is_running() {
            return;
        }
        self.shown = None;
        self.stacked.clear();
        self.live_map = None;
        self.final_images = None;
        cx.emit(StackingEvent::Shown);
        cx.notify();
    }

    /// Make reopened `render` the result, stacked from `stacked` in order.
    pub fn restore(
        &mut self,
        stack: Arc<CompletedStack>,
        render: RenderId,
        stacked: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.is_running() {
            return;
        }
        self.shown = Some(ShownResult { stack, render: Some(render) });
        self.stacked = stacked;
        self.live_map = None;
        self.final_images = None;
        cx.emit(StackingEvent::Shown);
        cx.notify();
    }

    pub fn take_live_map(&mut self) -> Option<PresentedImage> {
        self.live_map.take()
    }

    pub fn take_final_images(&mut self) -> Option<FinalImages> {
        self.final_images.take()
    }

    /// The batch's stack just made, once.
    pub fn take_made(&mut self) -> Option<Made> {
        self.made.take()
    }

    /// [`Self::start`] needs at least two `paths`.
    pub fn can_start(paths: &[PathBuf]) -> bool {
        paths.len() >= 2
    }

    /// Start a stack of `paths` for `whose`. On screen, the previous result
    /// is dropped (a running save keeps its handle); a batch's leaves it.
    pub fn start(
        &mut self,
        whose: Whose,
        paths: Vec<PathBuf>,
        config: StackerConfig,
        options: SessionOptions,
        live_side: u32,
        cx: &mut Context<Self>,
    ) {
        if self.is_running() || !Self::can_start(&paths) {
            return;
        }
        let needs_blend = config.needs_blend();
        let total = paths.len();
        let cancel = deptheos_app::engine::work::root().child();
        self.made = None;
        if whose == Whose::Screen {
            self.shown = None;
            self.stacked = paths.clone();
            self.live_map = None;
            self.final_images = None;
            cx.emit(StackingEvent::Shown);
        }

        let (tx, mut rx) = mpsc::unbounded();
        let engine = self.engine.clone();
        let flag = cancel.clone();
        let spawned = work::thread("stack").spawn(move || {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                engine.stack(
                    paths,
                    config,
                    options,
                    live_side,
                    &flag,
                    |event| {
                        let _ = tx.unbounded_send(Message::Event(event));
                    },
                    |map| {
                        let _ = tx.unbounded_send(Message::LiveMap(map));
                    },
                )
            }));
            let result = outcome.unwrap_or_else(|payload| {
                let msg = panic_message(&*payload);
                tracing::error!("stacking panicked: {msg}");
                Err(deptheos_app::Error::InvalidState(format!("stacking panicked: {msg}")))
            });
            let _ = tx.unbounded_send(Message::Done(Box::new(result)));
        });
        if let Err(e) = spawned {
            // The closure and its sender are gone: the drain below ends at once.
            tracing::error!("could not start the stacking thread: {e}");
        }

        let drain = cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = matches!(message, Message::Done(_));
                if this.update(cx, |this, cx| this.apply(message, cx)).is_err() || done {
                    return;
                }
            }
            // The worker dropped its sender without a `Done`: it is gone.
            this.update(cx, |this, cx| {
                this.apply(
                    Message::Done(Box::new(Err(deptheos_app::Error::InvalidState(
                        "stacking stopped unexpectedly".into(),
                    )))),
                    cx,
                )
            })
            .ok();
        });
        tracing::debug!(to = Phase::Reading.name(), ?whose, "stack run");
        self.run = Some(Run {
            whose,
            cancel,
            started: Instant::now(),
            track: Track::new(total, needs_blend),
            _drain: drain,
        });
        cx.notify();
    }

    /// An on-screen run with no worker: the test feeds events
    /// ([`Self::feed`]) and ends it canceled ([`Self::end_held`]).
    #[cfg(test)]
    pub(crate) fn hold_run(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        assert!(!self.is_running());
        let total = paths.len();
        self.shown = None;
        self.stacked = paths;
        self.live_map = None;
        self.final_images = None;
        cx.emit(StackingEvent::Shown);
        self.run = Some(Run {
            whose: Whose::Screen,
            cancel: deptheos_app::engine::work::root().child(),
            started: Instant::now(),
            track: Track::new(total, true),
            _drain: Task::ready(()),
        });
        cx.notify();
    }

    /// A session event for the run held by [`Self::hold_run`].
    #[cfg(test)]
    pub(crate) fn feed(&mut self, event: SessionEvent, cx: &mut Context<Self>) {
        self.apply(Message::Event(event), cx);
    }

    /// The held run ends, canceled.
    #[cfg(test)]
    pub(crate) fn end_held(&mut self, cx: &mut Context<Self>) {
        self.apply(Message::Done(Box::new(Err(deptheos_app::Error::Canceled))), cx);
    }

    /// Ask the run to stop: [`Phase::Canceling`] until it does.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.stop() {
            cx.notify();
        }
    }

    /// Set the run's cancel flag; whether a run was there to stop.
    fn stop(&mut self) -> bool {
        let Some(run) = self.run.as_mut() else { return false };
        run.cancel.cancel();
        run.track.set_phase(Phase::Canceling);
        true
    }

    fn apply(&mut self, message: Message, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_mut() else { return };
        match message {
            Message::Event(event) => run.track.apply_event(event),
            // A batch's stack never takes over the viewer.
            Message::LiveMap(map) => {
                self.live_map = Some(map);
                cx.emit(StackingEvent::LiveMap);
            }
            Message::Done(result) => {
                let seconds = run.started.elapsed().as_secs_f32();
                let whose = run.whose;
                tracing::debug!(from = run.track.phase.name(), "stack run ended");
                self.run = None;
                match *result {
                    Ok(done) if whose == Whose::Batch => {
                        let thumbnail = done.thumbnail();
                        self.made = Some(Made { stack: Arc::new(done.stack), thumbnail });
                        cx.emit(StackingEvent::Finished { seconds });
                    }
                    Ok(done) => {
                        let thumbnail = done.thumbnail();
                        self.shown =
                            Some(ShownResult { stack: Arc::new(done.stack), render: None });
                        self.final_images =
                            Some(FinalImages { display: done.display, map: done.map, thumbnail });
                        cx.emit(StackingEvent::Shown);
                        cx.emit(StackingEvent::Finished { seconds });
                    }
                    Err(deptheos_app::Error::Canceled) => cx.emit(StackingEvent::Canceled),
                    Err(e) => cx.emit(StackingEvent::Failed(crate::notices::explain(
                        &e,
                        crate::words::of(cx),
                    ))),
                }
            }
        }
        cx.notify();
    }
}

impl Drop for Stacking {
    /// A closed window stops its stack instead of letting it run unseen.
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use deptheos_app::SessionEvent;

    use super::{Phase, Track};

    fn run(track: &mut Track, events: impl IntoIterator<Item = SessionEvent>) {
        for event in events {
            track.apply_event(event);
        }
    }

    /// Each event with what the run shows after: phase, bar, frames taken
    /// in, reversed or not.
    fn table(frames: usize, two_pass: bool, rows: &[(SessionEvent, Phase, f32, usize, bool)]) {
        let mut track = Track::new(frames, two_pass);
        for &(event, phase, progress, taken, reversed) in rows {
            track.apply_event(event);
            let (p, bar, t, r) = track.seen();
            assert_eq!((p, t, r), (phase, taken, reversed), "after {event:?}");
            assert!((bar - progress).abs() < 1e-5, "after {event:?}: {bar}, not {progress}");
        }
    }

    /// A two-pass run fills the bar to half in pass 1, to 0.9 by its blend,
    /// never going back.
    #[test]
    fn a_two_pass_run_fills_the_bar_once() {
        use SessionEvent::*;
        let aligning = |loaded| Phase::Aligning { loaded, total: 3 };
        table(
            3,
            true,
            &[
                (Started { total: 3 }, Phase::Reading, 0.0, 0, false),
                (Accumulated { current: 1, total: 3 }, aligning(1), 0.5 / 3.0, 1, false),
                (Accumulated { current: 3, total: 3 }, aligning(3), 0.5, 3, false),
                (Finalized, Phase::Blending, 0.5, 3, false),
                (Blended { current: 1, total: 3 }, Phase::Blending, 0.5 + 0.4 / 3.0, 3, false),
                (Blended { current: 3, total: 3 }, Phase::Blending, 0.9, 3, false),
                (Completed, Phase::Finishing, 1.0, 3, false),
            ],
        );
    }

    /// A single-pass run (Pyramid) fills the bar in its one pass.
    #[test]
    fn a_single_pass_run_fills_the_bar_in_its_pass() {
        use SessionEvent::*;
        let aligning = |loaded| Phase::Aligning { loaded, total: 4 };
        table(
            4,
            false,
            &[
                (Started { total: 4 }, Phase::Reading, 0.0, 0, false),
                (Accumulated { current: 2, total: 4 }, aligning(2), 0.475, 2, false),
                (Accumulated { current: 4, total: 4 }, aligning(4), 0.95, 4, false),
                (Finalized, Phase::Blending, 0.98, 4, false),
                (Completed, Phase::Finishing, 1.0, 4, false),
            ],
        );
    }

    /// A reversed run restarts bar and count from the last frame, and has
    /// every frame from its blend on.
    #[test]
    fn a_reversed_run_starts_over_from_the_last_frame() {
        use SessionEvent::*;
        let aligning = |loaded| Phase::Aligning { loaded, total: 4 };
        table(
            4,
            true,
            &[
                (Started { total: 4 }, Phase::Reading, 0.0, 0, false),
                (Accumulated { current: 2, total: 4 }, aligning(2), 0.25, 2, false),
                (Reversed, Phase::Restarting, 0.0, 0, true),
                (Accumulated { current: 1, total: 4 }, aligning(1), 0.125, 1, true),
                (Accumulated { current: 4, total: 4 }, aligning(4), 0.5, 4, true),
                (Finalized, Phase::Blending, 0.5, 4, true),
                (Blended { current: 4, total: 4 }, Phase::Blending, 0.9, 4, true),
                (Completed, Phase::Finishing, 1.0, 4, true),
            ],
        );
    }

    /// The count is the run's own, whatever else was listed.
    #[test]
    fn the_count_is_the_runs_own() {
        use SessionEvent::*;
        table(
            5,
            false,
            &[
                (Started { total: 3 }, Phase::Reading, 0.0, 0, false),
                (
                    Accumulated { current: 3, total: 3 },
                    Phase::Aligning { loaded: 3, total: 3 },
                    0.95,
                    3,
                    false,
                ),
                (Finalized, Phase::Blending, 0.98, 3, false),
            ],
        );
    }

    /// Reverse-ordered frames restart aligning and the time estimate.
    #[test]
    fn a_reversed_run_restarts_its_estimate() {
        let mut track = Track::new(4, false);
        run(&mut track, [SessionEvent::Accumulated { current: 2, total: 4 }]);
        let before = track.estimate_from;
        assert!(track.time_left().is_some());
        run(&mut track, [SessionEvent::Reversed]);
        assert_eq!(track.seen().0, Phase::Restarting);
        assert!(track.estimate_from >= before);
        assert_eq!(track.time_left(), None);
        run(&mut track, [SessionEvent::Accumulated { current: 1, total: 4 }]);
        assert_eq!(track.seen().0, Phase::Aligning { loaded: 1, total: 4 });
    }

    /// Once canceling, later events move the bar but not the phase.
    #[test]
    fn canceling_holds_until_the_run_ends() {
        let mut track = Track::new(3, true);
        run(&mut track, [SessionEvent::Accumulated { current: 1, total: 3 }]);
        track.set_phase(Phase::Canceling);
        run(&mut track, [SessionEvent::Accumulated { current: 2, total: 3 }]);
        let (phase, bar, ..) = track.seen();
        assert_eq!(phase, Phase::Canceling);
        assert!((bar - 1.0 / 3.0).abs() < 1e-5, "the bar moves: {bar}");
        run(&mut track, [SessionEvent::Finalized]);
        assert_eq!(track.seen().0, Phase::Canceling);
        assert_eq!(track.time_left(), None);
    }
}
