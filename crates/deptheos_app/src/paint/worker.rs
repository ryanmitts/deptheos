//! The paint worker: one thread that owns every doc and runs paint commands
//! in order. It drains the queue before one display update for the union
//! of what changed, so a fast stroke never falls behind.
//!
//! The disk is the store thread's (ADR-143): it writes undo spills and
//! commits docs (`store.rs`), so a stroke never waits on it. A commit is
//! sent as each change ends, when its render closes, on [`Command::Save`]
//! and when the handle drops; one overtaken by a newer commit of the same
//! render is skipped ([`Snapshot::is_overtaken`]).
//!
//! A closed doc keeps its history, but its painted tiles go once committed
//! and are read back on the next open ([`PaintDoc::release`]).

use std::collections::{BTreeSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::brush::{Brush, Dab, Spacer};
use super::canvas::Canvas;
use super::display::{CanvasDisplay, DisplayPatch};
use super::doc::{PaintDoc, PaintDocs, SourceCounts};
use super::source::{PaintSource, SourceCache, SourceKind};
use super::store;
use super::store::{Snapshot, Spill, Writers};
use super::stroke::{CoveragePool, Stroke, StrokeSource};
use super::tile::TileId;
use super::undo::{Blob, Change, UndoStack};
use crate::engine::memory::Budget;
use crate::engine::present::PresentedImage;
use crate::library::history::History;
use crate::words::explain::{Explain, Kind};
use crate::words::{Words, say, sentences};
use crate::{CompletedStack, Engine, Error, Moves, RenderId};

/// The undo history an open render may keep on disk.
const HISTORY_DISK_CAP: u64 = 2 << 30;

/// Builds a source in the canvas's space: its pixels when asked for them
/// (`true`), and its display. Runs on the worker.
pub type SourceLoader =
    Arc<dyn Fn(bool) -> Result<(Option<Vec<u16>>, PresentedImage), Failure> + Send + Sync>;

/// A failure in sentences ready to show; `fault` when its details are in
/// the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub text: String,
    pub fault: bool,
}

impl Failure {
    /// A failure that is not a fault.
    pub fn user(text: impl Into<String>) -> Self {
        Self { text: text.into(), fault: false }
    }

    /// A fault.
    pub fn fault(text: impl Into<String>) -> Self {
        Self { text: text.into(), fault: true }
    }
}

impl From<String> for Failure {
    fn from(text: String) -> Self {
        Self::user(text)
    }
}

impl From<&str> for Failure {
    fn from(text: &str) -> Self {
        Self::user(text)
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// What the worker is asked to do.
pub enum Command {
    /// Paint `stack`, the render `key`, its paint kept in the render's
    /// directory.
    Open {
        key: RenderId,
        stack: Arc<CompletedStack>,
    },
    /// Leave paint mode, saving the doc.
    Close,
    /// Renders are being deleted ([`close`]): their docs go unsaved, and
    /// `closed` is answered once the store thread has closed their files.
    Remove {
        keys: Vec<RenderId>,
        closed: Sender<()>,
    },
    /// `key`'s painted result over `stack`, shown outside paint mode:
    /// [`Output::Presented`] follows (no display when nothing is painted or
    /// it can't be shown), unless a later present or open of the render
    /// answers for it ([`superseded`]).
    Present {
        key: RenderId,
        stack: Arc<CompletedStack>,
    },
    /// `key`'s painted pixels over `stack`, for an export; `None` when
    /// nothing is painted, [`Error::PaintUnread`] when its paint can't be
    /// read (the export fails rather than drop the retouching).
    Flatten {
        key: RenderId,
        stack: Arc<CompletedStack>,
        reply: Sender<Flattened>,
    },
    /// Paint from `kind`. `scrub` builds its display only, and is skipped
    /// when a later pick overtakes it ([`superseded`]). The next state
    /// carries `pick`, taken or failed ([`PaintState::pick`]).
    SetSource {
        kind: SourceKind,
        load: SourceLoader,
        scrub: bool,
        pick: u64,
    },
    /// Fetch `kind` ahead, if it fits and nothing else is waiting.
    Prefetch {
        kind: SourceKind,
        load: SourceLoader,
    },
    BeginStroke {
        brush: Brush,
    },
    /// Pointer samples, in image pixels.
    Dabs(Vec<Dab>),
    EndStroke,
    Undo,
    Redo,
    /// Every painted tile back to the original and the history cleared:
    /// not undoable, so the UI asks first.
    Reset,
    /// Full-size display tiles (`[tx, ty]`) of the open canvas:
    /// [`Output::Patches`] follows.
    RenderTiles(Vec<[u32; 2]>),
    /// Folders moved: kept sources follow, and loaders of moved frames go.
    /// Docs name frames by root, so they don't change.
    Relink(Moves),
    /// Memory is short ([`crate::engine::memory::shed`]): drop the previous
    /// and prefetched sources and the spilled undo history in RAM; when
    /// critical, also the oldest unspillable steps, never a doc's newest
    /// three.
    Shed(crate::engine::memory::Shed),
    /// Commit every unsaved doc and close every file (quit, iOS
    /// background), then reply whether every change is on disk: `false`
    /// when a commit failed or paint is in RAM only
    /// ([`PaintDoc::unsaveable`]).
    Save(Sender<bool>),
}

/// [`Command::Flatten`]'s answer.
pub type Flattened = Result<Option<Vec<u16>>, Error>;

/// The open render's doc and source, for the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintState {
    /// The render open.
    pub key: RenderId,
    pub undo: usize,
    pub redo: usize,
    /// What undo and redo would take back next ("Paint", "Erase").
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub painted: bool,
    /// Why paint on the open render can't be saved, for a standing notice
    /// ([`super::Unsaveable`]).
    pub unsaveable: Option<String>,
    /// The source picked; `None` until one is.
    pub source: Option<PickedSource>,
    /// The `pick` of the last [`Command::SetSource`] answered.
    pub pick: u64,
    /// The sources some painted pixel came from, the original aside.
    pub painted_from: Vec<SourceKind>,
}

impl PaintState {
    /// Render `key` before the worker has reported on it.
    pub fn empty(key: RenderId) -> Self {
        Self {
            key,
            undo: 0,
            redo: 0,
            undo_label: None,
            redo_label: None,
            painted: false,
            unsaveable: None,
            source: None,
            pick: 0,
            painted_from: Vec::new(),
        }
    }

    /// The source picked, if any.
    pub fn source_kind(&self) -> Option<&SourceKind> {
        self.source.as_ref().map(|s| &s.kind)
    }

    /// Whether a source is picked and its pixels are in.
    pub fn source_ready(&self) -> bool {
        self.source.as_ref().is_some_and(|s| s.ready)
    }
}

/// The source picked, in [`PaintState`].
#[derive(Clone, Debug, PartialEq)]
pub struct PickedSource {
    pub kind: SourceKind,
    /// Its pixels are in; a source scrubbed past has its display only.
    pub ready: bool,
}

/// What the worker reports.
pub enum Output {
    /// The canvas's whole display, when a render is opened.
    Canvas { key: RenderId, display: PresentedImage },
    /// Display tiles of `key`'s canvas the latest commands changed; a
    /// canvas no longer shown drops them.
    Patches { key: RenderId, patches: Vec<DisplayPatch> },
    /// [`Command::Present`]'s answer: `None` when it shows as stacked.
    Presented { key: RenderId, display: Option<PresentedImage> },
    /// The active source's display, for `key`'s canvas.
    Source { key: RenderId, kind: SourceKind, display: Arc<PresentedImage> },
    /// `kind` couldn't be loaded: it wasn't picked, or the stroke that
    /// needed its pixels was refused.
    SourceFailed { kind: SourceKind, message: Failure },
    /// The open render's state, or `None` once none is open.
    State(Option<PaintState>),
    /// [`Command::Open`] of `key` failed and nothing is open. Apart from
    /// [`Output::Failed`] so a late stroke or save failure isn't taken
    /// for it.
    OpenFailed { key: RenderId, message: Failure },
    /// Something else went wrong.
    Failed(Failure),
}

/// The leads of the worker's failures.
#[derive(Clone, Copy)]
enum Lead {
    Shown,
    ShownWithout,
    Saved,
    Opened,
    UndoHistory,
}

impl Lead {
    fn text(self, words: &Words) -> String {
        match self {
            Self::Shown => say!(words, "paint-not-shown"),
            Self::ShownWithout => say!(words, "paint-shown-without"),
            Self::Saved => say!(words, "paint-not-saved"),
            Self::Opened => say!(words, "paint-not-opened"),
            Self::UndoHistory => say!(words, "paint-undo-unread"),
        }
    }
}

/// [`Output::SourceFailed`]'s message: the lead, then `reason`.
fn source_failed(words: &Words, reason: &Failure) -> Failure {
    let text = sentences(words, [say!(words, "paint-source-failed"), reason.text.clone()]);
    Failure { text, fault: reason.fault }
}

/// `lead`, then `error`'s reason (and for a fault, that details are in the
/// log, where they go).
fn failed(words: &Words, lead: Lead, error: &dyn Explain) -> Failure {
    // No advice: it differs by platform.
    let lead = lead.text(words);
    let explained = error.explain(words);
    tracing::error!("{lead} {}", explained.details);
    let text = match explained.kind {
        Kind::User => sentences(words, [lead, explained.reason]),
        Kind::Fault | Kind::Canceled => {
            sentences(words, [lead, explained.reason, say!(words, "details-in-log")])
        }
    };
    Failure { text, fault: explained.kind == Kind::Fault }
}

/// Whether a later command in `queue` makes `command` pointless:
///
/// - A [`Command::Present`], by a present or open of the same render:
///   nothing can paint it between the two, and each present flattens at
///   full size.
/// - A scrub [`Command::SetSource`], by a later pick with only prefetches,
///   display tiles or presents between; the later pick answers for it. A
///   pick in full isn't skipped: the cache keeps it as the previous source.
fn superseded(command: &Command, queue: &VecDeque<Command>) -> bool {
    match command {
        Command::Present { key, .. } => queue.iter().any(|later| match later {
            Command::Present { key: k, .. } | Command::Open { key: k, .. } => k == key,
            _ => false,
        }),
        Command::SetSource { scrub: true, .. } => queue
            .iter()
            .find(|later| {
                !matches!(
                    later,
                    Command::Prefetch { .. } | Command::RenderTiles(_) | Command::Present { .. }
                )
            })
            .is_some_and(|later| matches!(later, Command::SetSource { .. })),
        _ => false,
    }
}

/// Called from the worker's thread and the store thread.
type Report = Arc<dyn Fn(Output) + Send + Sync>;

/// The paint's work on the store thread, done in the order sent.
enum StoreJob {
    Spill(Arc<Spill>, Vec<Arc<Blob>>),
    Commit(Snapshot),
    /// Retouch left this render: its file is tidied and closed.
    Leave(PathBuf),
    /// This render is being deleted: its file is closed as it is.
    Close(PathBuf),
    /// Roll back the file's journal, which only a writer can.
    RollBack(PathBuf),
}

/// The paint's store-thread jobs (ADR-143) and the writers they keep open,
/// used only there.
#[derive(Clone)]
struct PaintStore {
    writers: Arc<Mutex<Writers>>,
    engine: Arc<Engine>,
    report: Report,
}

impl PaintStore {
    fn new(engine: Arc<Engine>, report: Report) -> Self {
        Self { writers: Arc::default(), engine, report }
    }

    /// Do `job` on the store thread after what was sent before.
    fn send(&self, job: StoreJob) {
        let this = self.clone();
        let _done = crate::engine::store::store().write(move || this.run(job));
    }

    fn run(&self, job: StoreJob) {
        let mut writers = crate::lock(&self.writers);
        match job {
            StoreJob::Spill(spill, blobs) => {
                if let Err(e) = Blob::write_all(&spill, &blobs) {
                    tracing::error!("undo snapshots weren't written to disk: {e}");
                }
            }
            // A newer commit queued behind writes these changes too.
            StoreJob::Commit(snapshot) if snapshot.is_overtaken() => {}
            StoreJob::Commit(snapshot) => {
                if let Err(e) = writers.commit(&snapshot) {
                    (self.report)(Output::Failed(failed(self.engine.words(), Lead::Saved, &e)));
                }
            }
            StoreJob::Leave(dir) => writers.leave(&dir),
            StoreJob::Close(dir) => writers.close(&dir),
            StoreJob::RollBack(dir) => writers.roll_back(&dir),
        }
    }

    /// Wait for the jobs sent so far.
    fn drain(&self) {
        crate::engine::store::store().write(|| ()).wait();
    }

    /// Close every file, after the jobs sent so far, and wait for it.
    fn leave_all(&self) {
        let writers = self.writers.clone();
        crate::engine::store::store().write(move || crate::lock(&writers).leave_all()).wait();
    }
}

/// Every paint worker started, held weakly, for [`shed`] and [`close`],
/// which forget those gone.
static WORKERS: Mutex<Vec<Started>> = Mutex::new(Vec::new());

struct Started {
    /// Its history's root.
    library: PathBuf,
    commands: Weak<Sender<Command>>,
}

/// Sends [`Command::Shed`] to every paint worker running, without waiting.
pub(crate) fn shed(level: crate::engine::memory::Shed) {
    crate::lock(&WORKERS).retain(|worker| {
        worker
            .commands
            .upgrade()
            .is_some_and(|commands| commands.send(Command::Shed(level)).is_ok())
    });
}

/// Renders are about to be deleted from `library`: sends each of its
/// workers [`Command::Remove`] and waits for their answers. Never on the
/// store thread, which the worker waits for: there it is logged and
/// skipped ([`History::close_paint`]).
pub(crate) fn close(library: &Path, ids: &[RenderId]) {
    let workers: Vec<Weak<Sender<Command>>> = {
        let mut started = crate::lock(&WORKERS);
        started.retain(|w| w.commands.strong_count() > 0);
        started.iter().filter(|w| w.library == library).map(|w| w.commands.clone()).collect()
    };
    if workers.is_empty() {
        return;
    }
    if crate::engine::store::store().is_current() {
        tracing::error!(
            "paint asked to close {} renders from the store thread: skipped, it would never return",
            ids.len()
        );
        debug_assert!(false, "close_paint called on the store thread");
        return;
    }
    for worker in workers {
        let Some(commands) = worker.upgrade() else { continue };
        let (closed, done) = channel();
        let sent = commands.send(Command::Remove { keys: ids.to_vec(), closed });
        // Not held while waiting: a worker let go of meanwhile stops.
        drop(commands);
        if sent.is_ok() {
            let _ = done.recv();
        }
    }
}

/// The UI's handle. Dropping it stops the worker after the commands sent,
/// saving every doc first.
pub struct PaintWorker {
    /// The only strong reference, so dropping it stops the worker.
    commands: Option<Arc<Sender<Command>>>,
    thread: Option<JoinHandle<()>>,
}

impl PaintWorker {
    /// Start the worker; `output` is called on its thread and the store
    /// thread. Each render's paint is kept in its directory in `history`.
    pub fn spawn(
        engine: Arc<Engine>,
        history: Arc<History>,
        output: impl Fn(Output) + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        let report: Report = Arc::new(output);
        let store = PaintStore::new(engine.clone(), report.clone());
        let library = history.clone();
        let thread = crate::engine::work::thread("paint")
            .spawn(move || {
                Worker::new(engine, library, report, store.clone()).run(rx);
                // Every file closed before the worker is gone.
                store.leave_all();
            })
            .expect("the paint worker's thread could not start");
        let commands = Arc::new(tx);
        let library = history.root().to_path_buf();
        crate::lock(&WORKERS).push(Started { library, commands: Arc::downgrade(&commands) });
        Self { commands: Some(commands), thread: Some(thread) }
    }

    pub fn send(&self, command: Command) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(command);
        }
    }

    /// A sender that doesn't keep the worker running.
    pub fn commands(&self) -> Commands {
        Commands(self.commands.as_ref().map_or_else(Weak::new, Arc::downgrade))
    }

    /// Commit every unsaved doc, waiting up to `timeout`; whether every
    /// change is on disk in time.
    pub fn save(&self, timeout: Duration) -> bool {
        self.start_save().recv_timeout(timeout).unwrap_or(false)
    }

    /// [`Self::save`] without waiting; the receiver disconnects if the
    /// worker is gone.
    pub fn start_save(&self) -> Receiver<bool> {
        let (reply, done) = channel();
        self.send(Command::Save(reply));
        done
    }
}

/// [`PaintWorker::commands`].
#[derive(Clone)]
pub struct Commands(Weak<Sender<Command>>);

impl Commands {
    /// Send `command` while the worker runs.
    pub fn send(&self, command: Command) {
        if let Some(tx) = self.0.upgrade() {
            let _ = tx.send(command);
        }
    }
}

impl Drop for PaintWorker {
    fn drop(&mut self) {
        self.commands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Active {
    key: RenderId,
    display: CanvasDisplay,
}

struct Painting {
    stroke: Stroke,
    spacer: Spacer,
    label: String,
}

/// What the worker is doing with the render it has open.
///
/// Lifecycle: idle, open, stroking; changed only by its own methods, on
/// the worker's thread.
///
/// ```text
/// Idle ──open──▶ Open ──begin_stroke──▶ Stroking
///   ▲             │ ▲                      │
///   └────close────┘ └──────end_stroke──────┘
/// ```
///
/// `close` ends a stroke first, so a stroke's canvas is the open doc's.
enum Mode {
    Idle,
    Open(Active),
    Stroking(Active, Painting),
}

impl Mode {
    fn name(&self) -> &'static str {
        match self {
            Mode::Idle => "idle",
            Mode::Open(_) => "open",
            Mode::Stroking(..) => "stroking",
        }
    }

    fn set(&mut self, next: Mode) {
        self.set_from(self.name(), next);
    }

    /// `set` after a take: `from` is the name read before it.
    fn set_from(&mut self, from: &'static str, next: Mode) {
        tracing::debug!(from, to = next.name(), "paint mode");
        *self = next;
    }

    fn active(&self) -> Option<&Active> {
        match self {
            Mode::Idle => None,
            Mode::Open(active) | Mode::Stroking(active, _) => Some(active),
        }
    }

    fn active_mut(&mut self) -> Option<&mut Active> {
        match self {
            Mode::Idle => None,
            Mode::Open(active) | Mode::Stroking(active, _) => Some(active),
        }
    }

    fn is_stroking(&self) -> bool {
        matches!(self, Mode::Stroking(..))
    }

    /// Idle → Open. `close` comes first: anything open is dropped.
    fn open(&mut self, active: Active) {
        self.set(Mode::Open(active));
    }

    /// Open → Stroking. Without a render open the stroke is handed back.
    fn begin_stroke(&mut self, painting: Painting) -> Option<Painting> {
        let from = self.name();
        match std::mem::replace(self, Mode::Idle) {
            Mode::Open(active) => {
                self.set_from(from, Mode::Stroking(active, painting));
                None
            }
            other => {
                *self = other;
                Some(painting)
            }
        }
    }

    /// Stroking → Open, with the stroke; `None` when none was in progress.
    fn end_stroke(&mut self) -> Option<Painting> {
        let from = self.name();
        match std::mem::replace(self, Mode::Idle) {
            Mode::Stroking(active, painting) => {
                self.set_from(from, Mode::Open(active));
                Some(painting)
            }
            other => {
                *self = other;
                None
            }
        }
    }

    /// Open → Idle. A stroke in progress is dropped: [`Worker::close`]
    /// ends it first.
    fn close(&mut self) -> Option<Active> {
        let from = self.name();
        match std::mem::replace(self, Mode::Idle) {
            Mode::Idle => None,
            Mode::Open(active) | Mode::Stroking(active, _) => {
                tracing::debug!(from, to = "idle", "paint mode");
                Some(active)
            }
        }
    }
}

struct Worker {
    engine: Arc<Engine>,
    history: Arc<History>,
    output: Report,
    docs: PaintDocs,
    mode: Mode,
    sources: SourceCache,
    /// Loaders of the last four sources picked, newest first, to fetch a
    /// scrubbed one's pixels when painted from.
    loaders: VecDeque<(SourceKind, SourceLoader)>,
    pool: CoveragePool,
    store: PaintStore,
    /// Canvas tiles changed since the last display update.
    dirty: BTreeSet<TileId>,
    /// The state last sent.
    state: Option<PaintState>,
    /// Send the state even if unchanged: a pick is answered.
    resend_state: bool,
    picked: u64,
    budget: Budget,
    /// Kept counts for [`PaintState::painted_from`], recounted only after
    /// a change a stroke's end didn't follow (undo, redo, reset, another
    /// render).
    painted_from: Option<PaintedFrom>,
}

/// [`SourceCounts`] of a doc at one [`PaintDoc::revision`], between
/// strokes.
struct PaintedFrom {
    key: RenderId,
    revision: u64,
    counts: SourceCounts,
}

impl Worker {
    fn new(engine: Arc<Engine>, history: Arc<History>, output: Report, store: PaintStore) -> Self {
        Self {
            engine,
            history,
            output,
            docs: PaintDocs::default(),
            mode: Mode::Idle,
            sources: SourceCache::new(Budget::PaintSources),
            loaders: VecDeque::new(),
            pool: CoveragePool::default(),
            store,
            dirty: BTreeSet::new(),
            state: None,
            resend_state: false,
            picked: 0,
            budget: Budget::PaintHistory,
            painted_from: Default::default(),
        }
    }

    fn run(mut self, rx: Receiver<Command>) {
        loop {
            let Ok(first) = rx.recv() else { break };
            // One drain and flush is one job: an earlier job's GPU error
            // isn't its own.
            deptheos_gpu::clear_thread_faults();
            let mut queue = VecDeque::from([first]);
            while let Some(command) = queue.pop_front() {
                queue.extend(rx.try_iter());
                // A prefetch gives way to anything asked after it.
                if matches!(command, Command::Prefetch { .. }) && !queue.is_empty() {
                    continue;
                }
                if superseded(&command, &queue) {
                    let what = match command {
                        Command::Present { .. } => "a present",
                        _ => "a frame scrubbed past",
                    };
                    tracing::debug!(
                        "paint: {what} skipped, overtaken by a command queued behind it"
                    );
                    continue;
                }
                let opening = match &command {
                    Command::Open { key, .. } => Some(key.clone()),
                    _ => None,
                };
                self.guarded_for(opening, |w| w.handle(command));
            }
            self.guarded(Self::flush);
        }
        // The UI let go: save what is left.
        self.guarded(|w| {
            w.end_stroke();
            w.save_all();
            w.drain_store();
        });
    }

    /// Run `f`, surviving a panic in it: a stroke cut short stays as an
    /// undoable entry, and the UI is told.
    fn guarded(&mut self, f: impl FnOnce(&mut Self)) {
        self.guarded_for(None, f);
    }

    /// [`Self::guarded`] for a command opening `opening`: a panic that
    /// leaves it unopened is [`Output::OpenFailed`].
    fn guarded_for(&mut self, opening: Option<RenderId>, f: impl FnOnce(&mut Self)) {
        if std::panic::catch_unwind(AssertUnwindSafe(|| f(self))).is_err() {
            if std::panic::catch_unwind(AssertUnwindSafe(|| self.recover_stroke())).is_err() {
                self.mode.end_stroke();
            }
            // Only the active doc stays open, not one `open` panicked on.
            let active = self.mode.active().map(|a| a.key.clone());
            for (key, doc) in self.docs.iter_mut() {
                if Some(key) != active.as_ref() && doc.canvas().is_some() {
                    doc.close();
                }
            }
            let message = Failure::fault(say!(self.engine.words(), "paint-fault"));
            let opened = self.mode.active().map(|a| &a.key);
            match opening {
                Some(key) if opened != Some(&key) => {
                    (self.output)(Output::OpenFailed { key, message })
                }
                _ => (self.output)(Output::Failed(message)),
            }
        }
    }

    /// Keep a stroke cut short by a panic as an undo entry.
    fn recover_stroke(&mut self) {
        let Some(mut painting) = self.mode.end_stroke() else { return };
        let dirty = painting.stroke.take_dirty();
        let finished = painting.stroke.abandon();
        let Some(key) = self.mode.active().map(|a| a.key.clone()) else { return };
        let Some(doc) = self.docs.get_mut(&key) else { return };
        let blobs = doc.history.push(painting.label, finished.before);
        let spill = doc.history.spill().cloned();
        doc.mark_changed();
        Self::commit(&self.store, doc);
        self.engine.generations().raise(&key);
        self.store_blobs(spill, blobs);
        self.dirty.extend(dirty);
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Open { key, stack } => self.open(key, stack),
            Command::Close => self.close(),
            Command::Remove { keys, closed } => {
                self.remove(&keys);
                let _ = closed.send(());
            }
            Command::Present { key, stack } => {
                let display = self.present(&key, &stack);
                (self.output)(Output::Presented { key, display });
            }
            Command::Flatten { key, stack, reply } => {
                let flattened = self.painted(&key, stack).map_err(|unread| {
                    tracing::error!(
                        "an export of {} can't have its retouching: {unread}",
                        key.as_str()
                    );
                    Error::PaintUnread(unread)
                });
                let _ = reply.send(flattened);
            }
            Command::SetSource { kind, load, scrub, pick } => {
                self.set_source(kind, load, scrub, pick)
            }
            Command::Prefetch { kind, load } => self.prefetch(kind, load),
            Command::BeginStroke { brush } => self.begin_stroke(brush),
            Command::Dabs(points) => self.dabs(points),
            Command::EndStroke => self.end_stroke(),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Reset => self.reset(),
            Command::RenderTiles(tiles) => self.render_tiles(&tiles),
            Command::Relink(moves) => self.relink(&moves),
            Command::Shed(level) => self.shed(level),
            Command::Save(reply) => {
                self.end_stroke();
                self.flush();
                self.save_all();
                // Files closed: the app may be suspended next (ADR-148).
                self.store.leave_all();
                let unsaved: Vec<&RenderId> =
                    self.docs.iter().filter(|(_, doc)| !doc.is_saved()).map(|(k, _)| k).collect();
                for key in &unsaved {
                    tracing::error!("the retouching of {} isn't saved", key.as_str());
                }
                let _ = reply.send(unsaved.is_empty());
            }
        }
    }

    fn doc(&mut self) -> Option<&mut PaintDoc> {
        let key = &self.mode.active()?.key;
        self.docs.get_mut(key)
    }

    /// Puts `key`'s doc in `self.docs`, ready to open: the one in memory
    /// (tiles restored if released), or read afresh from `render_dir`.
    fn doc_to_open(&mut self, key: &RenderId, render_dir: &Path, [width, height]: [u32; 2]) {
        let afresh = match self.docs.get_mut(key) {
            None => true,
            // An unsaveable doc with nothing painted is read afresh, so a
            // later open can save it.
            Some(doc) if doc.is_untouched_ram_only() => true,
            Some(doc) => match doc.restore() {
                Ok(()) => false,
                Err(e) => {
                    // Read afresh instead; the session's history goes.
                    let dir = render_dir.display();
                    tracing::warn!("the retouching of {dir} couldn't be read back: {e}");
                    self.docs.remove(key);
                    true
                }
            },
        };
        if afresh {
            // Read with no connection open, so damaged paint can be set
            // aside.
            self.store.send(StoreJob::Close(render_dir.to_path_buf()));
            self.drain_store();
            let roll_back = || {
                self.store.send(StoreJob::RollBack(render_dir.to_path_buf()));
                self.drain_store();
            };
            let doc = store::open_doc_rolling_back(render_dir, width, height, roll_back);
            self.docs.insert(key.clone(), doc);
        }
    }

    fn open(&mut self, key: RenderId, stack: Arc<CompletedStack>) {
        self.close();
        let render_dir = self.history.render_path(&key);
        let size = [stack.width, stack.height];
        self.doc_to_open(&key, &render_dir, size);
        let doc = self.docs.get_mut(&key).expect("just opened");
        doc.open(stack.clone());
        // Changes a commit failed to write are tried again now.
        Self::commit(&self.store, doc);
        // An unpainted canvas is the stack itself: no frame-sized copy.
        let flat = doc.is_painted().then(|| doc.canvas().expect("just opened").flatten());
        let pixels = flat.as_deref().unwrap_or(&stack.pixels);
        let display = match self.engine.present_pixels(crate::Look::stored(&stack), pixels) {
            Ok(display) => display,
            Err(e) => {
                doc.put_away();
                let message = failed(self.engine.words(), Lead::Opened, &e);
                (self.output)(Output::OpenFailed { key, message });
                return;
            }
        };
        let canvas_display = CanvasDisplay::new(self.engine.gpu(), &display);
        (self.output)(Output::Canvas { key: key.clone(), display });
        self.mode.open(Active { key, display: canvas_display });
    }

    /// [`Command::Present`]: `None` when nothing is painted or it can't be
    /// shown, which is reported.
    fn present(&mut self, key: &RenderId, stack: &Arc<CompletedStack>) -> Option<PresentedImage> {
        let painted = self.painted(key, stack.clone());
        let words = self.engine.words();
        let error = match painted {
            Ok(flat) => match self.engine.present_pixels(crate::Look::stored(stack), &flat?) {
                Ok(display) => return Some(display),
                Err(e) => failed(words, Lead::Shown, &e),
            },
            Err(unread) => failed(words, Lead::ShownWithout, &Error::PaintUnread(unread)),
        };
        (self.output)(Output::Failed(error));
        None
    }

    /// `key`'s doc flattened over `stack`, if painted; `Err` when its paint
    /// can't be read. A doc not in memory is read from disk and dropped
    /// after, so browsing renders doesn't keep their paint in RAM.
    fn painted(
        &mut self,
        key: &RenderId,
        stack: Arc<CompletedStack>,
    ) -> Result<Option<Vec<u16>>, store::Unread> {
        let render_dir = self.history.render_path(key);
        let mut loaded;
        let restored = self.docs.get_mut(key).map(|doc| {
            doc.restore()
                .map_err(|e| {
                    let dir = render_dir.display();
                    tracing::warn!("the retouching of {dir} couldn't be read back: {e}");
                })
                .is_ok()
        });
        let doc = match self.docs.get_mut(key) {
            Some(doc) if restored == Some(true) => doc,
            _ => match store::load_doc(&render_dir, stack.width, stack.height)? {
                Some(doc) => {
                    loaded = doc;
                    &mut loaded
                }
                None => return Ok(None),
            },
        };
        if !doc.is_painted() {
            return Ok(None);
        }
        if doc.canvas().is_some() {
            return Ok(doc.canvas().map(|c| c.flatten()));
        }
        doc.open(stack);
        let flat = doc.canvas().map(|c| c.flatten()).filter(|_| doc.is_painted());
        doc.put_away();
        Ok(flat)
    }

    /// Leave the active doc, committing it, then close its file.
    fn close(&mut self) {
        let left = self.leave();
        if let Some(key) = &left {
            self.save_doc(key);
            let dir = self.history.render_path(key);
            self.store.send(StoreJob::Leave(dir));
        }
        self.close_doc(left);
    }

    /// Drop deleted renders' docs unsaved, and wait for their files to
    /// close.
    fn remove(&mut self, keys: &[RenderId]) {
        if self.mode.active().is_some_and(|a| keys.contains(&a.key)) {
            let left = self.leave();
            self.close_doc(left);
        }
        for key in keys {
            self.docs.remove(key);
            self.store.send(StoreJob::Close(self.history.render_path(key)));
        }
        self.drain_store();
    }

    /// End the stroke, show it, and close the mode, returning its render.
    fn leave(&mut self) -> Option<RenderId> {
        self.end_stroke();
        self.flush();
        self.mode.close().map(|active| active.key)
    }

    /// Drop `left`'s canvas and the sources; its painted tiles go once
    /// committed ([`Self::flush`]).
    fn close_doc(&mut self, left: Option<RenderId>) {
        if let Some(doc) = left.and_then(|key| self.docs.get_mut(&key)) {
            doc.close();
        }
        self.sources.clear();
        self.loaders.clear();
        self.dirty.clear();
    }

    // ------------------------------------------------------------------
    // Saving
    // ------------------------------------------------------------------

    fn drain_store(&self) {
        self.store.drain();
    }

    /// Send `doc`'s commit if it has unsaved changes; whether one was
    /// sent. A failed commit leaves it unsaved for the next to retry.
    fn commit(store: &PaintStore, doc: &PaintDoc) -> bool {
        let snapshot = doc.snapshot().filter(|_| doc.is_unsaved());
        snapshot.map(|snapshot| store.send(StoreJob::Commit(snapshot.sent()))).is_some()
    }

    fn save_doc(&mut self, key: &RenderId) {
        if let Some(doc) = self.docs.get(key) {
            Self::commit(&self.store, doc);
        }
    }

    /// Commit every unsaved doc, closing the files of those not open.
    fn save_all(&mut self) {
        let active = self.mode.active().map(|a| a.key.clone());
        for (key, doc) in self.docs.iter_mut() {
            if Self::commit(&self.store, doc) && Some(key) != active.as_ref() {
                self.store.send(StoreJob::Leave(self.history.render_path(key)));
            }
        }
    }

    /// Mark the active doc changed and commit it.
    fn changed(&mut self) {
        let Some(key) = self.mode.active().map(|a| a.key.clone()) else { return };
        if let Some(doc) = self.docs.get_mut(&key) {
            doc.mark_changed();
            Self::commit(&self.store, doc);
            self.engine.generations().raise(&key);
        }
    }

    fn relink(&mut self, moves: &Moves) {
        self.sources.follow(moves);
        // A moved frame's loader would read the old place, so it goes.
        self.loaders.retain(|(kind, _)| !kind.clone().follow(moves));
        self.resend_state = true;
    }

    // ------------------------------------------------------------------
    // Sources
    // ------------------------------------------------------------------

    fn remember(&mut self, kind: &SourceKind, load: &SourceLoader) {
        self.loaders.retain(|(k, _)| k != kind);
        self.loaders.push_front((kind.clone(), load.clone()));
        self.loaders.truncate(4);
    }

    fn build(kind: &SourceKind, load: &SourceLoader, pixels: bool) -> Result<PaintSource, Failure> {
        let (pixels, display) = load(pixels)?;
        Ok(PaintSource {
            kind: kind.clone(),
            pixels: pixels.map(Arc::new),
            display: Arc::new(display),
        })
    }

    /// `None` with the failure reported.
    fn load(
        &self,
        kind: &SourceKind,
        load: &SourceLoader,
        pixels: bool,
    ) -> Option<Arc<PaintSource>> {
        match Self::build(kind, load, pixels) {
            Ok(source) => Some(Arc::new(source)),
            Err(reason) => {
                let message = source_failed(self.engine.words(), &reason);
                (self.output)(Output::SourceFailed { kind: kind.clone(), message });
                None
            }
        }
    }

    fn set_source(&mut self, kind: SourceKind, load: SourceLoader, scrub: bool, pick: u64) {
        // Answered whatever happens.
        self.picked = pick;
        self.resend_state = true;
        if self.mode.active().is_none() {
            return;
        }
        let found = self.sources.find(&kind, !scrub);
        let loaded = found.is_none();
        let source = match found {
            Some(source) => source,
            None => {
                // The original's pixels are the canvas's own base.
                let pixels = !scrub && kind != SourceKind::Original;
                let estimate = self.source_estimate(pixels);
                self.sources.make_room(estimate);
                // A failure leaves the old source, which the state says.
                let Some(source) = self.load(&kind, &load, pixels) else { return };
                source
            }
        };
        self.remember(&kind, &load);
        // Already shown: a scrub settling only fetches the pixels.
        let shown = self.sources.active().is_some_and(|a| a.kind == kind);
        self.sources.activate(source.clone());
        if !shown || loaded {
            self.show_source(&source);
        }
    }

    /// A new source's bytes: a display like the active one's, plus
    /// canvas-sized pixels when `pixels`.
    fn source_estimate(&mut self, pixels: bool) -> usize {
        let display = self.sources.active().map_or(0, |a| a.display.bytes());
        let base = self.doc().and_then(|d| d.canvas()).map_or(0, |c| 2 * c.base().len());
        display + if pixels { base } else { 0 }
    }

    fn prefetch(&mut self, kind: SourceKind, load: SourceLoader) {
        if self.mode.active().is_none() || self.sources.find(&kind, true).is_some() {
            return;
        }
        let estimate = self.sources.active().map_or(0, |a| a.bytes());
        if !self.sources.fits(estimate) {
            return;
        }
        // Not picked yet, so a failure isn't reported.
        match Self::build(&kind, &load, true) {
            Ok(source) => {
                self.remember(&kind, &load);
                self.sources.prefetch(Arc::new(source));
            }
            Err(e) => tracing::debug!("prefetch of {kind:?} failed: {e}"),
        }
    }

    fn show_source(&self, source: &PaintSource) {
        let Some(active) = self.mode.active() else { return };
        (self.output)(Output::Source {
            key: active.key.clone(),
            kind: source.kind.clone(),
            display: source.display.clone(),
        });
    }

    /// The active source, its pixels fetched if only scrubbed past;
    /// `Ok(None)` is the original. `Err` is reported.
    fn paintable_source(&mut self) -> Result<Option<Arc<PaintSource>>, ()> {
        let Some(active) = self.sources.active().cloned() else { return Ok(None) };
        if active.is_paintable() {
            return Ok(Some(active));
        }
        let Some((_, load)) = self.loaders.iter().find(|(k, _)| *k == active.kind).cloned() else {
            let again = Failure::user(say!(self.engine.words(), "paint-pick-again"));
            let message = source_failed(self.engine.words(), &again);
            (self.output)(Output::SourceFailed { kind: active.kind.clone(), message });
            return Err(());
        };
        let source = self.load(&active.kind, &load, true).ok_or(())?;
        if !source.is_paintable() {
            let again = Failure::user(say!(self.engine.words(), "paint-pick-again-or-another"));
            let message = source_failed(self.engine.words(), &again);
            (self.output)(Output::SourceFailed { kind: active.kind.clone(), message });
            return Err(());
        }
        self.sources.activate(source.clone());
        // Shown with the full-size display its pixels came with.
        self.show_source(&source);
        Ok(Some(source))
    }

    // ------------------------------------------------------------------
    // Strokes and history
    // ------------------------------------------------------------------

    fn begin_stroke(&mut self, brush: Brush) {
        self.end_stroke();
        if self.mode.active().is_none() {
            return;
        }
        // An unpaintable source refuses the stroke.
        let Ok(source) = self.paintable_source() else { return };
        let words = self.engine.words().clone();
        let frames = self.history.frames().clone();
        let kind = source.as_ref().map_or(SourceKind::Original, |s| s.kind.clone());
        let pixels = source.and_then(|s| s.pixels.clone());
        let Some(doc) = self.doc() else { return };
        let base_len = doc.canvas().expect("the active doc is open").base().len();
        let chosen = match (&kind, pixels) {
            (SourceKind::Original, _) => Ok(StrokeSource::Original),
            (_, None) => Err(say!(words, "paint-cannot-paint-from")),
            (_, Some(pixels)) if pixels.len() != base_len => {
                Err(say!(words, "paint-does-not-line-up"))
            }
            (kind, Some(pixels)) => match doc.sources.id(kind, &frames) {
                Ok(Some(id)) => Ok(StrokeSource::Image { id, pixels }),
                // The original holds one of the 256 ids.
                Ok(None) => Err(say!(words, "paint-too-many-sources")),
                Err(e) => {
                    tracing::error!("a source to paint from couldn't be named: {e}");
                    Err(say!(words, "paint-cannot-paint-from"))
                }
            },
        };
        let stroke_source = match chosen {
            Ok(source) => source,
            Err(message) => {
                (self.output)(Output::Failed(Failure::user(message)));
                return;
            }
        };
        let label = match stroke_source {
            StrokeSource::Original => say!(words, "paint-undo-erase"),
            StrokeSource::Image { .. } => say!(words, "paint-undo-paint"),
        };
        let pool = std::mem::take(&mut self.pool);
        let Some(doc) = self.doc() else { return };
        let canvas = doc.canvas().expect("the active doc is open");
        let stroke = Stroke::new(canvas, brush, stroke_source, pool);
        let painting = Painting { stroke, spacer: Spacer::new(&brush), label };
        if let Some(painting) = self.mode.begin_stroke(painting) {
            // No render open after all: nothing was painted.
            self.pool = painting.stroke.abandon().pool;
        }
    }

    /// The stroke stays in `self.mode`, so a panic leaves it for
    /// [`Self::recover_stroke`].
    fn dabs(&mut self, points: Vec<Dab>) {
        let Mode::Stroking(active, painting) = &mut self.mode else { return };
        let Some(doc) = self.docs.get_mut(&active.key) else { return };
        let canvas = doc.canvas_mut().expect("the active doc is open");
        let mut dabs = Vec::new();
        for point in points {
            painting.spacer.to(point, &mut dabs);
        }
        for dab in dabs {
            painting.stroke.dab(canvas, dab);
        }
        self.dirty.extend(painting.stroke.take_dirty());
    }

    fn end_stroke(&mut self) {
        let Some(painting) = self.mode.end_stroke() else { return };
        let Some(key) = self.mode.active().map(|a| a.key.clone()) else { return };
        let Some(doc) = self.docs.get_mut(&key) else { return };
        let revision = doc.revision;
        let canvas = doc.canvas_mut().expect("the active doc is open");
        let mut stroke = painting.stroke;
        let dirty = stroke.take_dirty();
        let finished = stroke.finish(canvas);
        // A stroke that changed nothing leaves redo as it was.
        let changed = !finished.before.is_empty();
        // Counts current before the stroke follow its tiles alone.
        let followed = match &mut self.painted_from {
            Some(p) if changed && p.key == key && p.revision == revision => {
                p.counts.restroke(&finished.before, canvas);
                true
            }
            _ => false,
        };
        let blobs = doc.history.push(painting.label, finished.before);
        doc.history.trim(HISTORY_DISK_CAP);
        let spill = doc.history.spill().cloned();
        self.pool = finished.pool;
        if changed {
            self.changed();
            if followed && let (Some(p), Some(doc)) = (&mut self.painted_from, self.docs.get(&key))
            {
                p.revision = doc.revision;
            }
        }
        self.dirty.extend(dirty);
        self.store_blobs(spill, blobs);
        self.keep_history_in_budget();
    }

    fn undo(&mut self) {
        self.step_history(UndoStack::undo);
    }

    fn redo(&mut self) {
        self.step_history(UndoStack::redo);
    }

    /// Undo or redo; ignored mid-stroke.
    fn step_history(
        &mut self,
        step: fn(&mut UndoStack, &mut Canvas) -> std::io::Result<Option<Change>>,
    ) {
        if self.mode.is_stroking() {
            return;
        }
        let Some(doc) = self.doc() else { return };
        let Some((canvas, history)) = doc.parts_mut() else { return };
        let changed = step(history, canvas);
        let spill = history.spill().cloned();
        match changed {
            Ok(Some(change)) => {
                self.dirty.extend(&change.tiles);
                self.changed();
                self.store_blobs(spill, change.new_blobs);
                self.keep_history_in_budget();
            }
            Ok(None) => {}
            Err(e) => {
                (self.output)(Output::Failed(failed(self.engine.words(), Lead::UndoHistory, &e)))
            }
        }
    }

    fn reset(&mut self) {
        self.end_stroke();
        let Some(doc) = self.doc() else { return };
        let [undo, redo] = doc.history.counts();
        let canvas = doc.canvas_mut().expect("the active doc is open");
        let painted: Vec<TileId> = canvas.painted().map(|(id, _)| id).collect();
        if painted.is_empty() && undo == 0 && redo == 0 {
            return;
        }
        for id in &painted {
            canvas.replace(*id, None);
        }
        doc.history.clear();
        self.dirty.extend(painted);
        // Committed at once to free the tiles' disk space.
        self.changed();
    }

    /// [`Command::Shed`].
    fn shed(&mut self, level: crate::engine::memory::Shed) {
        let sources = self.sources.shed();
        let before = self.docs.charged();
        let mut left = self.docs.enforce_budget(0);
        if level == crate::engine::memory::Shed::Critical && left > 0 {
            left -= self.docs.shed(left).min(left);
        }
        tracing::info!(
            "paint: {} MB of sources and {} MB of undo history let go of",
            sources >> 20,
            before.saturating_sub(left) >> 20
        );
    }

    /// Hold resident history to its budget: evict what is spilled, then
    /// drop the oldest entries that can't be (no spill, or a full disk).
    fn keep_history_in_budget(&mut self) {
        let budget = self.budget.bytes(self.docs.charged());
        let charged = self.docs.enforce_budget(budget);
        if charged > budget {
            let freed = self.docs.shed(charged - budget);
            if freed > 0 {
                tracing::warn!(
                    "undo history that can't be written to disk was over its budget; \
                     the oldest steps were dropped ({freed} bytes)"
                );
            }
        }
    }

    fn store_blobs(&self, spill: Option<Arc<Spill>>, blobs: Vec<Arc<Blob>>) {
        if let Some(spill) = spill
            && !blobs.is_empty()
        {
            self.store.send(StoreJob::Spill(spill, blobs));
        }
    }

    fn render_tiles(&mut self, tiles: &[[u32; 2]]) {
        let Some(active) = self.mode.active_mut() else { return };
        let Some(canvas) = self.docs.get(&active.key).and_then(|d| d.canvas()) else { return };
        match active.display.render_tiles(self.engine.gpu(), self.engine.color(), canvas, tiles) {
            Ok(patches) if !patches.is_empty() => {
                (self.output)(Output::Patches { key: active.key.clone(), patches })
            }
            Ok(_) => {}
            Err(e) => (self.output)(Output::Failed(failed(self.engine.words(), Lead::Shown, &e))),
        }
    }

    /// One display update for everything changed, then the state; closed
    /// docs release their committed tiles.
    fn flush(&mut self) {
        let active = self.mode.active().map(|a| a.key.clone());
        for (key, doc) in self.docs.iter_mut() {
            if Some(key) != active.as_ref() {
                doc.release();
            }
        }
        let dirty = std::mem::take(&mut self.dirty);
        if !dirty.is_empty()
            && let Some(active) = self.mode.active_mut()
            && let Some(doc) = self.docs.get(&active.key)
            && let Some(canvas) = doc.canvas()
        {
            match active.display.update(self.engine.gpu(), self.engine.color(), canvas, &dirty) {
                Ok(patches) => (self.output)(Output::Patches { key: active.key.clone(), patches }),
                Err(e) => {
                    (self.output)(Output::Failed(failed(self.engine.words(), Lead::Shown, &e)))
                }
            }
        }
        let state = self.state();
        if state != self.state || std::mem::take(&mut self.resend_state) {
            self.state = state.clone();
            (self.output)(Output::State(state));
        }
    }

    fn state(&mut self) -> Option<PaintState> {
        let painted_from = self.painted_from();
        let key = self.mode.active()?.key.clone();
        let doc = self.docs.get(&key);
        let [undo, redo] = doc.map_or([0, 0], |d| d.history.counts());
        let [undo_label, redo_label] =
            doc.map_or([None, None], |d| d.history.next_labels().map(|l| l.map(str::to_string)));
        let source = self
            .sources
            .active()
            .map(|s| PickedSource { kind: s.kind.clone(), ready: s.is_paintable() });
        Some(PaintState {
            key,
            undo,
            redo,
            undo_label,
            redo_label,
            painted: doc.is_some_and(|d| d.is_painted()),
            unsaveable: doc.and_then(PaintDoc::unsaveable).map(|why| why.text(self.engine.words())),
            source,
            pick: self.picked,
            painted_from,
        })
    }

    /// [`PaintState::painted_from`], from the kept counts while current.
    fn painted_from(&mut self) -> Vec<SourceKind> {
        let Some(active) = self.mode.active() else { return Vec::new() };
        let Some(doc) = self.docs.get(&active.key) else { return Vec::new() };
        let frames = self.history.frames();
        let current = |p: &PaintedFrom| p.key == active.key && p.revision == doc.revision;
        if let Some(p) = self.painted_from.as_ref().filter(|p| current(p)) {
            return p.counts.kinds(&doc.sources, frames);
        }
        let counts = SourceCounts::of(doc);
        let found = counts.kinds(&doc.sources, frames);
        // Mid-stroke counts hold part of it, which its end would count
        // again.
        if !self.mode.is_stroking() {
            let (key, revision) = (active.key.clone(), doc.revision);
            self.painted_from = Some(PaintedFrom { key, revision, counts });
        }
        found
    }
}

#[cfg(test)]
#[path = "worker_model.rs"]
mod model;

#[cfg(test)]
mod tests {
    use super::*;

    /// A shed reaches each worker running, and forgets one let go of.
    #[test]
    fn a_shed_reaches_each_worker_and_forgets_those_gone() {
        use crate::engine::memory::Shed;
        let (tx, heard) = channel();
        let running = Arc::new(tx);
        let gone = Arc::new(channel().0);
        let gone_weak = Arc::downgrade(&gone);
        let library = PathBuf::from("/nowhere");
        let started = |commands| Started { library: library.clone(), commands };
        crate::lock(&WORKERS).push(started(Arc::downgrade(&running)));
        crate::lock(&WORKERS).push(started(gone_weak.clone()));
        drop(gone);
        shed(Shed::Critical);
        assert!(matches!(heard.try_recv(), Ok(Command::Shed(Shed::Critical))));
        assert!(!crate::lock(&WORKERS).iter().any(|w| w.commands.ptr_eq(&gone_weak)), "forgotten");
        drop(running);
    }

    fn key(n: u64) -> RenderId {
        RenderId::parse(&format!("{n:032x}")).unwrap()
    }

    fn stack() -> Arc<CompletedStack> {
        Arc::new(CompletedStack::srgb(2, 2, vec![0; 12], Vec::new()))
    }

    fn present(n: u64) -> Command {
        Command::Present { key: key(n), stack: stack() }
    }

    fn open(n: u64) -> Command {
        Command::Open { key: key(n), stack: stack() }
    }

    fn scrub(scrub: bool) -> Command {
        let load: SourceLoader = Arc::new(|_| unreachable!("not loaded"));
        Command::SetSource { kind: SourceKind::Original, load, scrub, pick: 0 }
    }

    /// A frame scrubbed past gives way to a later pick, unless something
    /// between paints from it; a pick in full never does.
    #[test]
    fn a_frame_scrubbed_past_gives_way_to_a_later_pick() {
        let queue = |commands: Vec<Command>| VecDeque::from(commands);
        assert!(superseded(&scrub(true), &queue(vec![scrub(true)])));
        assert!(superseded(&scrub(true), &queue(vec![scrub(false)])));
        let prefetch = Command::Prefetch {
            kind: SourceKind::Original,
            load: match scrub(true) {
                Command::SetSource { load, .. } => load,
                _ => unreachable!(),
            },
        };
        assert!(superseded(
            &scrub(true),
            &queue(vec![prefetch, Command::RenderTiles(vec![[0, 0]]), present(1), scrub(true)])
        ));
        let brush = Brush::default();
        for between in [Command::BeginStroke { brush }, Command::Close, open(1)] {
            assert!(!superseded(&scrub(true), &queue(vec![between, scrub(true)])));
        }
        assert!(!superseded(&scrub(true), &queue(vec![Command::Undo])));
        assert!(!superseded(&scrub(true), &VecDeque::new()));
        assert!(!superseded(&scrub(false), &queue(vec![scrub(true)])));
    }

    /// A present gives way to a later present or open of its render, and
    /// to nothing else.
    #[test]
    fn a_present_gives_way_to_a_later_one_or_an_open_of_its_render() {
        let queue = |commands: Vec<Command>| VecDeque::from(commands);
        assert!(superseded(&present(1), &queue(vec![present(1)])));
        assert!(superseded(&present(1), &queue(vec![Command::Close, open(1)])));
        assert!(!superseded(&present(1), &queue(vec![present(2), open(2)])));
        assert!(!superseded(&present(1), &queue(vec![Command::Undo])));
        assert!(!superseded(&present(1), &VecDeque::new()));
        // Only a present gives way.
        assert!(!superseded(&open(1), &queue(vec![open(1)])));
        assert!(!superseded(&Command::Close, &queue(vec![present(1)])));
    }
}
