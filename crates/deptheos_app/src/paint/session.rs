//! Retouch for both apps behind one object: the paint worker, the
//! [`Machine`] and its timers, as `BatchQueue` wraps the batch machine.
//!
//! Every call and worker output goes through [`Machine::on`] under one
//! lock, and its reports are queued in order. They are delivered after the
//! lock is released, one at a time, so an observer that calls back in
//! can't deadlock. The observer is called on the app's, the worker's, the
//! store's or the timers' thread.
//!
//! Dropping the session saves every render's paint, waiting for the
//! worker.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use super::brush::{Brush, Dab};
use super::frame::{Siblings, Told, frame_loader, render_loader};
use super::machine::{Action, Event, Machine, OpenRender, Order, Report, Status};
use super::source::SourceKind;
use super::worker::{Command, Commands, Failure, Flattened, PaintWorker, SourceLoader};
use crate::library::history::History;
use crate::words::say;
use crate::{CompletedStack, Engine, Moves, RenderId};

/// Hears the session's reports, in order, each once.
pub trait Observer: Send + Sync {
    fn report(&self, report: Report);
}

/// Runs a closure later. Never from inside [`Clock::after`] itself.
pub trait Clock: Send + Sync {
    fn after(&self, delay: Duration, fire: Box<dyn FnOnce() + Send>);
}

type Fire = Box<dyn FnOnce() + Send>;

/// The real [`Clock`]: one thread, ending with the last handle.
pub struct Timers {
    jobs: Mutex<Sender<(Instant, Fire)>>,
}

impl Timers {
    pub fn start() -> Arc<Self> {
        let (tx, rx) = channel::<(Instant, Fire)>();
        let thread = crate::engine::work::thread("paint-timers").spawn(move || {
            let mut due: Vec<(Instant, Fire)> = Vec::new();
            loop {
                let next = due.iter().map(|(at, _)| *at).min();
                let job = match next {
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                };
                match job {
                    Ok(job) => due.push(job),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                let now = Instant::now();
                let (fire, wait) = due.into_iter().partition(|(at, _)| *at <= now);
                due = wait;
                for (_, fire) in fire {
                    fire();
                }
            }
        });
        if let Err(e) = thread {
            // Scrubs never settle; a stroke still fetches the pixels.
            tracing::error!("paint: the timers' thread didn't start: {e}");
        }
        Arc::new(Self { jobs: Mutex::new(tx) })
    }
}

impl Clock for Timers {
    fn after(&self, delay: Duration, fire: Fire) {
        let _ = crate::lock(&self.jobs).send((Instant::now() + delay, fire));
    }
}

/// The queued reports, and whether a thread is delivering.
#[derive(Default)]
struct Outbox {
    queue: VecDeque<Report>,
    delivering: bool,
}

/// The machine, and the open render's siblings for its frame loaders.
struct Core {
    machine: Machine,
    siblings: Option<Siblings>,
}

struct Inner {
    engine: Arc<Engine>,
    history: Arc<History>,
    told: Told,
    commands: Commands,
    core: Mutex<Core>,
    outbox: Mutex<Outbox>,
    observer: Arc<dyn Observer>,
    clock: Arc<dyn Clock>,
    me: Weak<Inner>,
}

/// Retouch, one render at a time: create one per app.
pub struct PaintSession {
    /// Dropped first, so the worker's last outputs still find the session.
    worker: PaintWorker,
    inner: Arc<Inner>,
}

impl PaintSession {
    /// Start the paint worker over `history`. `told` words a source's load
    /// error as the app shows it.
    pub fn new(
        engine: Arc<Engine>,
        history: Arc<History>,
        observer: Arc<dyn Observer>,
        told: Told,
    ) -> Self {
        Self::with_clock(engine, history, observer, told, Timers::start())
    }

    /// [`Self::new`] with the tests' `clock`.
    fn with_clock(
        engine: Arc<Engine>,
        history: Arc<History>,
        observer: Arc<dyn Observer>,
        told: Told,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let mut worker = None;
        let inner = Arc::new_cyclic(|me: &Weak<Inner>| {
            let to = me.clone();
            let output = move |output| {
                if let Some(inner) = to.upgrade() {
                    inner.on(Event::Worker(output));
                }
            };
            let spawned = PaintWorker::spawn(engine.clone(), history.clone(), output);
            let commands = spawned.commands();
            worker = Some(spawned);
            let machine = Machine::new(engine.words().clone());
            Inner {
                engine,
                history,
                told,
                commands,
                core: Mutex::new(Core { machine, siblings: None }),
                outbox: Mutex::default(),
                observer,
                clock,
                me: me.clone(),
            }
        });
        let worker = worker.expect("new_cyclic ran its closure");
        Self { worker, inner }
    }

    /// `None` while paint mode is off; the last [`Report::Status`] or
    /// newer.
    pub fn status(&self) -> Option<Status> {
        crate::lock(&self.inner.core).machine.status()
    }

    /// The render painted and its stack, while paint mode is on.
    pub fn painting(&self) -> Option<(RenderId, Arc<CompletedStack>)> {
        let core = crate::lock(&self.inner.core);
        core.machine.painting().map(|(key, stack)| (key.clone(), stack.clone()))
    }

    /// Paint `render`: [`Report::Canvas`] follows, or a failed status.
    pub fn open(&self, render: OpenRender) {
        self.inner.on(Event::Open(render));
    }

    /// Open the render again after its open failed.
    pub fn retry(&self) {
        self.inner.on(Event::Retry);
    }

    /// Leave paint mode, saving the render's paint.
    pub fn close(&self) {
        self.inner.on(Event::Close);
    }

    pub fn pick_frame(&self, path: PathBuf) {
        self.inner.on(Event::PickFrame(path));
    }

    pub fn scrub_to(&self, path: PathBuf) {
        self.inner.on(Event::ScrubTo(path));
    }

    pub fn step(&self, by: isize) {
        self.inner.on(Event::Step(by));
    }

    pub fn pick_original(&self) {
        self.inner.on(Event::PickOriginal);
    }

    /// Paint from render `id`, lined up through a frame both stacked.
    pub fn pick_render(&self, id: &RenderId) {
        self.inner.on(Event::PickRender(id.as_str().to_string()));
    }

    pub fn swap(&self) {
        self.inner.on(Event::Swap);
    }

    pub fn use_brush(&self) {
        self.inner.on(Event::UseBrush);
    }

    pub fn use_frames(&self) {
        self.inner.on(Event::UseFrames);
    }

    /// Paint from the frame sharpest at `x`, `y` in image pixels;
    /// [`Report::Notice`] when the render can't tell.
    pub fn pick_at(&self, x: f32, y: f32) {
        self.inner.on(Event::PickAt { x, y });
    }

    /// Start a stroke, at `at` if given; only on a canvas that is in.
    pub fn begin_stroke(&self, brush: Brush, at: Option<Dab>) {
        self.inner.on(Event::BeginStroke { brush, at });
    }

    pub fn dabs(&self, dabs: Vec<Dab>) {
        if !dabs.is_empty() {
            self.inner.on(Event::Dabs(dabs));
        }
    }

    pub fn end_stroke(&self) {
        self.inner.on(Event::EndStroke);
    }

    pub fn undo(&self) {
        self.inner.on(Event::Undo);
    }

    pub fn redo(&self) {
        self.inner.on(Event::Redo);
    }

    /// Clear all paint and history: not undoable, so the app asks first.
    pub fn reset(&self) {
        self.inner.on(Event::Reset);
    }

    /// Show `key`'s paint outside paint mode; [`Report::Presented`]
    /// answers.
    pub fn present(&self, key: RenderId, stack: Arc<CompletedStack>) {
        self.worker.send(Command::Present { key, stack });
    }

    /// `stack` with `key`'s paint, for an export: `None` when nothing is
    /// painted, an error when it can't be read.
    pub fn flatten(&self, key: RenderId, stack: Arc<CompletedStack>) -> Receiver<Flattened> {
        let (reply, rx) = channel();
        self.worker.send(Command::Flatten { key, stack, reply });
        rx
    }

    /// Commit all unsaved paint and close the files, waiting up to
    /// `timeout`; whether every change is on disk. Only this says a commit
    /// landed.
    pub fn save(&self, timeout: Duration) -> bool {
        self.worker.save(timeout)
    }

    /// [`Self::save`] without waiting; the receiver disconnects if the
    /// worker is gone.
    pub fn start_save(&self) -> Receiver<bool> {
        self.worker.start_save()
    }

    /// Folders were relinked ([`Command::Relink`]); the app opens the
    /// render again.
    pub fn relink(&self, moves: &Moves) {
        self.worker.send(Command::Relink(moves.clone()));
    }

    /// Full-size display tiles of the canvas: [`Report::Patches`] follows.
    pub fn render_tiles(&self, tiles: Vec<[u32; 2]>) {
        self.worker.send(Command::RenderTiles(tiles));
    }
}

impl Inner {
    fn on(&self, event: Event) {
        {
            let mut core = crate::lock(&self.core);
            let actions = core.machine.on(event);
            let mut reports = Vec::new();
            for action in actions {
                match action {
                    Action::Send(order) => self.send(&mut core, order),
                    Action::Schedule { token, after } => {
                        let me = self.me.clone();
                        self.clock.after(
                            after,
                            Box::new(move || {
                                if let Some(inner) = me.upgrade() {
                                    inner.on(Event::Fired(token));
                                }
                            }),
                        );
                    }
                    Action::Report(report) => reports.push(report),
                }
            }
            // Under the machine's lock, so in its order.
            crate::lock(&self.outbox).queue.extend(reports);
        }
        self.deliver();
    }

    /// Deliver queued reports outside every lock, unless another thread
    /// already is.
    fn deliver(&self) {
        {
            let mut outbox = crate::lock(&self.outbox);
            if outbox.delivering {
                return;
            }
            outbox.delivering = true;
        }
        // A panicking observer leaves the rest to the next call.
        let _unwinding = Unwinding(&self.outbox);
        loop {
            let next = {
                let mut outbox = crate::lock(&self.outbox);
                match outbox.queue.pop_front() {
                    Some(report) => report,
                    None => {
                        outbox.delivering = false;
                        return;
                    }
                }
            };
            self.observer.report(next);
        }
    }

    fn send(&self, core: &mut Core, order: Order) {
        let command = match order {
            Order::Open(key) => {
                let Some((_, stack)) = core.machine.painting() else { return };
                let stack = stack.clone();
                // Listed once, lazily: no render is saved while Retouch is
                // open.
                core.siblings = Some(Siblings::listed_once(self.history.clone(), key.clone()));
                Command::Open { key, stack }
            }
            Order::Close => {
                core.siblings = None;
                Command::Close
            }
            Order::Pick { kind, scrub, pick } => {
                // Sent even unloadable: the machine waits for every pick.
                let load = self.loader(core, &kind).unwrap_or_else(|| self.unloadable(&kind));
                Command::SetSource { kind, load, scrub, pick }
            }
            Order::Prefetch(kind) => {
                let Some(load) = self.loader(core, &kind) else { return };
                Command::Prefetch { kind, load }
            }
            Order::BeginStroke(brush) => Command::BeginStroke { brush },
            Order::Dabs(dabs) => Command::Dabs(dabs),
            Order::EndStroke => Command::EndStroke,
            Order::Undo => Command::Undo,
            Order::Redo => Command::Redo,
            Order::Reset => Command::Reset,
        };
        self.commands.send(command);
    }

    /// A loader that fails, for when none can be made.
    fn unloadable(&self, kind: &SourceKind) -> SourceLoader {
        tracing::error!("paint: no loader for {kind:?}");
        let text = say!(self.engine.words(), "paint-pick-again-or-another");
        Arc::new(move |_| Err(Failure::user(text.clone())))
    }

    fn loader(&self, core: &Core, kind: &SourceKind) -> Option<SourceLoader> {
        let (_, stack) = core.machine.painting()?;
        let (engine, stack, told) = (self.engine.clone(), stack.clone(), self.told.clone());
        Some(match kind {
            SourceKind::Original => Arc::new(move |_| {
                let display = engine.present_pixels(&stack, &stack.pixels).map_err(|e| told(&e))?;
                Ok((None, display))
            }),
            SourceKind::Frame { path } => {
                let paths = stack.paths.clone();
                frame_loader(engine, stack, paths, path.clone(), core.siblings.clone()?, told)
            }
            SourceKind::Render { render } => {
                let history = self.history.clone();
                let id = RenderId::parse(render)?;
                let words = told.clone();
                let other = move || history.load_render(&id).map(Arc::new).map_err(|e| words(&e));
                render_loader(engine, stack, other, told)
            }
        })
    }
}

/// Frees the outbox if its deliverer unwinds.
struct Unwinding<'a>(&'a Mutex<Outbox>);

impl Drop for Unwinding<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            crate::lock(self.0).delivering = false;
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
