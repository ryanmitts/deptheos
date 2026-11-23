//! The engine's threads (ADR-143): the builder every thread is made by
//! (8 MiB stack, a name), the root cancel token, which only quit cancels,
//! and the bounded pool.
//!
//! The pool has `clamp(cores / 2, 2, 8)` workers and two lanes. Interactive
//! work goes first; background work never takes the last free worker. An
//! interactive request is a latest-wins slot by kind and owner
//! ([`Key::owned_by`]): a new one cancels the one before, and one per slot
//! runs at a time. A request of a particular thing ([`Key::of`]) takes no
//! slot. Dropping a request or canceling its token cancels its work. Each
//! job runs with its token and codec budget as the thread's current work
//! ([`deptheos_core::cancel::within`]).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Context, Poll};

use futures::channel::oneshot;

pub use deptheos_core::cancel::{Cancel, CancelOnDrop, Hook, codec_threads, current, within};
pub use deptheos_core::threads::{STACK_SIZE, cores, thread};

use crate::lock;

/// The engine's root token, canceled only by quit; runs' tokens and
/// run-less requests hang from it.
pub fn root() -> &'static Cancel {
    static ROOT: OnceLock<Cancel> = OnceLock::new();
    ROOT.get_or_init(Cancel::root)
}

/// A token for work quit must let finish (an export): no ancestor, so only
/// dropping its request cancels it, and quit waits while one lives
/// ([`writing`]).
pub fn must_finish() -> Cancel {
    Cancel::counted(&WRITERS)
}

static WRITERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Work quit must let finish is under way: a [`must_finish`] token lives.
pub fn writing() -> bool {
    WRITERS.load(Ordering::SeqCst) > 0
}

/// Stacks running now ([`stacking`]).
static STACKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A stack run, counted in [`stacks_running`] until dropped. Quit doesn't
/// wait for its thread, which may still be in a GPU pass.
#[must_use = "dropping it ends the count"]
pub struct Stacking(&'static std::sync::atomic::AtomicUsize);

/// Count a stack run until the guard is dropped.
pub fn stacking() -> Stacking {
    Stacking::on(&STACKS)
}

impl Stacking {
    fn on(count: &'static std::sync::atomic::AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for Stacking {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How many stack runs are under way ([`stacking`]).
pub fn stacks_running() -> usize {
    STACKS.load(Ordering::SeqCst)
}

/// Whether quit must skip static destructors: a codec call or a stack run
/// is under way.
fn ends_at_once(in_codec: usize, stacks: usize) -> bool {
    in_codec > 0 || stacks > 0
}

/// The process's last step after quit's waits: flush the store thread (a
/// few seconds at most), flush the log, and end the process at once if a
/// codec call or stack run is under way, so no static destructor frees
/// what it uses. Returns otherwise.
pub fn exit() {
    if !crate::engine::store::store().flush(std::time::Duration::from_secs(5)) {
        tracing::error!("quitting with Library writes not done");
    }
    let (in_codec, stacks) = (deptheos_core::cancel::in_codec(), stacks_running());
    let at_once = ends_at_once(in_codec, stacks);
    if at_once {
        tracing::warn!(
            in_codec,
            stacks,
            "quitting while a codec call or a stack runs: ending the process now"
        );
    }
    if let Some(logs) = crate::engine::logs::installed() {
        logs.flush();
    }
    if at_once {
        // SAFETY: skips atexit handlers and static destructors on purpose;
        // nothing is left to write.
        unsafe { libc::_exit(0) };
    }
}

/// The process's pool, started on first use.
pub fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(workers()))
}

/// The pool's size for this machine: `clamp(cores / 2, 2, 8)`.
pub fn workers() -> usize {
    (cores() / 2).clamp(2, 8)
}

/// Which lane a request waits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lane {
    /// What the user waits for: first, latest of a kind wins.
    Interactive,
    /// Work nobody is watching: never on the last free worker.
    Background,
}

impl Lane {
    fn index(self) -> usize {
        match self {
            Self::Interactive => 0,
            Self::Background => 1,
        }
    }
}

/// A request's kind (its slot and log name) and what it is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub kind: &'static str,
    of: u64,
    owner: u64,
}

impl Key {
    /// A request of `kind`, of nothing in particular.
    pub const fn kind(kind: &'static str) -> Self {
        Self { kind, of: 0, owner: 0 }
    }

    /// A request of `kind` for `what` (a render's id, a frame at a size).
    pub fn of(kind: &'static str, what: impl Hash) -> Self {
        Self { kind, of: hash(what), owner: 0 }
    }

    /// Scopes the slot to `owner` (a window, a UI thread).
    pub fn owned_by(self, owner: impl Hash) -> Self {
        Self { owner: hash(owner), ..self }
    }

    /// The latest-wins slot an interactive request takes; none for a
    /// request of a particular thing ([`Key::of`]), which its asker drops.
    fn slot(&self) -> Option<Slot> {
        (self.of == 0).then_some((self.kind, self.owner))
    }
}

/// An interactive slot: a kind, for an owner.
type Slot = (&'static str, u64);

fn hash(what: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    what.hash(&mut hasher);
    hasher.finish()
}

/// Why a request gave no result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// Its token was canceled, or a newer request of its kind replaced it.
    Canceled,
    /// The work panicked, with the panic's message.
    Panicked(String),
}

impl std::fmt::Display for Ended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Canceled => f.write_str("canceled"),
            Self::Panicked(message) => write!(f, "panicked: {message}"),
        }
    }
}

/// A bounded pool of engine threads in two lanes; see the module.
pub struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    wake: Condvar,
    /// Threads a decode on the pool may start of its own (`cores / W`).
    codec_threads: usize,
}

#[derive(Default)]
struct State {
    queued: [VecDeque<Arc<Job>>; 2],
    /// Workers not running a job.
    idle: usize,
    /// Interactive slots running, each at most once.
    running: HashMap<Slot, Arc<Job>>,
    /// The latest interactive request of each slot, queued or running.
    slots: HashMap<Slot, Arc<Job>>,
    next: u64,
}

struct Job {
    id: u64,
    lane: Lane,
    key: Key,
    cancel: Cancel,
    /// Runs the work and sends the result; `None` once taken. Dropped
    /// unrun, its request sees it canceled.
    work: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Pool {
    /// A pool of `workers` threads, at least 2 so one lane can't hold all.
    pub fn new(workers: usize) -> Self {
        let workers = workers.max(2);
        let inner = Arc::new(Inner {
            state: Mutex::new(State { idle: workers, ..State::default() }),
            wake: Condvar::new(),
            codec_threads: (cores() / workers).max(1),
        });
        // A launch that can't start its threads ends here, as the store's.
        for i in 0..workers {
            let inner = inner.clone();
            thread(format!("work-{i}"))
                .spawn(move || inner.work())
                .expect("a work pool thread could not start");
        }
        Self { inner }
    }

    /// The threads a decode on the pool may start of its own.
    pub fn codec_threads(&self) -> usize {
        self.inner.codec_threads
    }

    /// Runs `f` on the pool, canceled with `cancel` or by dropping the
    /// request.
    pub fn run<T, F>(&self, lane: Lane, key: Key, cancel: &Cancel, f: F) -> Request<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        #[cfg(any(test, feature = "test-support"))]
        let held = gate::of(key.kind);
        let (tx, rx) = oneshot::channel();
        let job = self.inner.job(lane, key, holding(cancel, f), tx);
        #[cfg(any(test, feature = "test-support"))]
        if let Some(opened) = held {
            let (inner, held) = (self.inner.clone(), job.clone());
            std::thread::spawn(move || {
                let _ = futures::executor::block_on(opened);
                inner.submit(&held);
            });
            return Request::new(rx, job, &self.inner, cancel);
        }
        self.inner.submit(&job);
        Request::new(rx, job, &self.inner, cancel)
    }
}

/// Lets a test hold a kind of work back: after [`hold`], that thread's
/// requests of the kind ([`Key::kind`], or [`STORE`] for
/// [`crate::engine::store::Store::write`]) wait unsent until the [`Held`]
/// opens or drops, leaving the shared pool free. Also for the desktop's
/// tests, through `test-support`.
#[cfg(any(test, feature = "test-support"))]
pub mod gate {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;

    use futures::FutureExt as _;
    use futures::channel::oneshot;
    use futures::future::Shared;

    /// The kind a Library write is held by.
    pub const STORE: &str = "store";

    type At = (ThreadId, &'static str);
    /// Resolves once its gate is opened.
    pub type Opened = Shared<oneshot::Receiver<()>>;

    /// Each kind held, with how many requests have come to it.
    static GATES: Mutex<Vec<(At, Opened, Arc<AtomicUsize>)>> = Mutex::new(Vec::new());

    /// One kind held back on one thread, until opened or dropped.
    #[must_use = "the kind is held only while this is kept"]
    pub struct Held {
        at: At,
        open: Option<oneshot::Sender<()>>,
        came: Arc<AtomicUsize>,
    }

    /// Holds `kind` on this thread.
    pub fn hold(kind: &'static str) -> Held {
        let at = (std::thread::current().id(), kind);
        let (open, opened) = oneshot::channel();
        let came = Arc::new(AtomicUsize::new(0));
        let mut gates = crate::lock(&GATES);
        gates.retain(|(k, ..)| *k != at);
        gates.push((at, opened.shared(), came.clone()));
        Held { at, open: Some(open), came }
    }

    impl Held {
        /// How many requests have come to it so far.
        pub fn waiting(&self) -> usize {
            self.came.load(Ordering::SeqCst)
        }

        /// Releases the requests held and later ones.
        pub fn open(self) {}
    }

    impl Drop for Held {
        fn drop(&mut self) {
            crate::lock(&GATES).retain(|(k, ..)| *k != self.at);
            if let Some(open) = self.open.take() {
                let _ = open.send(());
            }
        }
    }

    /// The gate a request of `kind` asked on this thread waits at, if held.
    pub fn of(kind: &'static str) -> Option<Opened> {
        let at = (std::thread::current().id(), kind);
        let gates = crate::lock(&GATES);
        let (_, opened, came) = gates.iter().find(|(k, ..)| *k == at)?;
        came.fetch_add(1, Ordering::SeqCst);
        Some(opened.clone())
    }
}

/// `f`, holding the asker's token until run or dropped, so a
/// [`must_finish`] token counts while its work does.
fn holding<T, F: FnOnce() -> T>(cancel: &Cancel, f: F) -> impl FnOnce() -> T + use<T, F> {
    let asked = cancel.clone();
    move || {
        let _asked = asked;
        f()
    }
}

impl Inner {
    fn job<T, F>(
        &self,
        lane: Lane,
        key: Key,
        f: F,
        reply: oneshot::Sender<Result<T, Ended>>,
    ) -> Arc<Job>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let cancel = Cancel::root();
        let codec_threads = self.codec_threads;
        let work = {
            let cancel = cancel.clone();
            Box::new(move || {
                let result = if cancel.is_canceled() {
                    Err(Ended::Canceled)
                } else {
                    std::panic::catch_unwind(AssertUnwindSafe(|| within(&cancel, codec_threads, f)))
                        .map_err(|payload| {
                            let message = panic_message(&*payload);
                            tracing::error!(kind = key.kind, "work panicked: {message}");
                            Ended::Panicked(message)
                        })
                };
                let _ = reply.send(result);
            }) as Box<dyn FnOnce() + Send>
        };
        let id = {
            let mut state = lock(&self.state);
            state.next += 1;
            state.next
        };
        Arc::new(Job { id, lane, key, cancel, work: Mutex::new(Some(work)) })
    }

    fn submit(&self, job: &Arc<Job>) {
        let replaced = {
            let mut state = lock(&self.state);
            state.queued[job.lane.index()].push_back(job.clone());
            match (job.lane, job.key.slot()) {
                (Lane::Interactive, Some(slot)) => state.slots.insert(slot, job.clone()),
                _ => None,
            }
        };
        tracing::debug!(kind = job.key.kind, lane = ?job.lane, "work queued");
        if let Some(old) = replaced {
            tracing::debug!(kind = old.key.kind, "work replaced by a newer request");
            self.drop_job(&old);
        }
        self.wake.notify_all();
    }

    /// Cancels `job` and removes it from the queue and maps.
    fn drop_job(&self, job: &Arc<Job>) {
        job.cancel.cancel();
        let unrun = {
            let mut state = lock(&self.state);
            let queue = &mut state.queued[job.lane.index()];
            let at = queue.iter().position(|j| j.id == job.id);
            let unrun = at.and_then(|at| queue.remove(at)).is_some();
            forget(&mut state, job);
            unrun
        };
        if unrun {
            // Its reply goes with it, outside the locks.
            let work = lock(&job.work).take();
            drop(work);
        }
        self.wake.notify_all();
    }

    /// `job`'s request is gone: its work is canceled.
    fn leave(&self, job: &Arc<Job>) {
        tracing::debug!(kind = job.key.kind, "work dropped by its request");
        self.drop_job(job);
    }

    /// A worker: the next job it may take, run, until the process ends.
    fn work(&self) {
        loop {
            let job = {
                let mut state = lock(&self.state);
                loop {
                    if let Some(job) = take(&mut state) {
                        state.idle -= 1;
                        if let (Lane::Interactive, Some(slot)) = (job.lane, job.key.slot()) {
                            state.running.insert(slot, job.clone());
                        }
                        break job;
                    }
                    state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner());
                }
            };
            set_quality_of_service(job.lane);
            // A GPU error the job before left unchecked is not this one's.
            deptheos_gpu::clear_thread_faults();
            let work = lock(&job.work).take();
            if let Some(work) = work {
                work();
            }
            let mut state = lock(&self.state);
            state.idle += 1;
            if let Some(slot) = job.key.slot()
                && state.running.get(&slot).is_some_and(|j| j.id == job.id)
            {
                state.running.remove(&slot);
            }
            forget(&mut state, &job);
            drop(state);
            self.wake.notify_all();
        }
    }
}

/// The next job a free worker may take: interactive first, of a slot not
/// running; background only if another worker is free.
fn take(state: &mut State) -> Option<Arc<Job>> {
    let interactive = &mut state.queued[0];
    let free = |j: &Arc<Job>| j.key.slot().is_none_or(|slot| !state.running.contains_key(&slot));
    if let Some(at) = interactive.iter().position(free) {
        return interactive.remove(at);
    }
    if state.idle >= 2 {
        return state.queued[1].pop_front();
    }
    None
}

/// Take `job` out of the maps that name it.
fn forget(state: &mut State, job: &Job) {
    if let Some(slot) = job.key.slot()
        && state.slots.get(&slot).is_some_and(|j| j.id == job.id)
    {
        state.slots.remove(&slot);
    }
}

/// The thread's quality of service on Apple platforms follows the lane.
fn set_quality_of_service(lane: Lane) {
    #[cfg(target_vendor = "apple")]
    {
        let class = match lane {
            Lane::Interactive => libc::qos_class_t::QOS_CLASS_USER_INITIATED,
            Lane::Background => libc::qos_class_t::QOS_CLASS_UTILITY,
        };
        // SAFETY: sets this thread's own class; no pointers.
        unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
    }
    #[cfg(not(target_vendor = "apple"))]
    let _ = lane;
}

/// A request on the pool: a future of its result. Dropping it or canceling
/// its token cancels the work.
pub struct Request<T> {
    reply: oneshot::Receiver<Result<T, Ended>>,
    member: Member,
}

struct Member {
    job: Arc<Job>,
    inner: Arc<Inner>,
    left: Arc<AtomicBool>,
    _hook: Hook,
}

impl<T> Request<T> {
    fn new(
        reply: oneshot::Receiver<Result<T, Ended>>,
        job: Arc<Job>,
        inner: &Arc<Inner>,
        cancel: &Cancel,
    ) -> Self {
        let left = Arc::new(AtomicBool::new(false));
        let hook = cancel.on_cancel({
            let (job, inner, left) = (job.clone(), Arc::downgrade(inner), left.clone());
            move || {
                if !left.swap(true, Ordering::AcqRel)
                    && let Some(inner) = inner.upgrade()
                {
                    inner.leave(&job);
                }
            }
        });
        Self { reply, member: Member { job, inner: inner.clone(), left, _hook: hook } }
    }

    /// Blocks until the result is in, for callers that are not async.
    pub fn wait(self) -> Result<T, Ended> {
        futures::executor::block_on(self)
    }

    /// Lets the work finish with nobody waiting.
    pub fn detach(self) {
        self.member.left.store(true, Ordering::Release);
    }

    /// The work's token, canceled once its request has left.
    pub fn token(&self) -> &Cancel {
        &self.member.job.cancel
    }
}

impl Drop for Member {
    fn drop(&mut self) {
        if !self.left.swap(true, Ordering::AcqRel) {
            self.inner.leave(&self.job);
        }
    }
}

impl<T> Future for Request<T> {
    type Output = Result<T, Ended>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.reply).poll(cx).map(|r| r.unwrap_or(Err(Ended::Canceled)))
    }
}

/// Text of a panic payload, for messages and logs.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const SOON: Duration = Duration::from_secs(5);

    /// Quit ends at once during a codec call or stack run, else as usual.
    #[test]
    fn quit_ends_at_once_past_a_codec_or_a_stack() {
        assert!(!ends_at_once(0, 0));
        assert!(ends_at_once(1, 0), "inside a codec");
        assert!(ends_at_once(0, 1), "a stack winding down");
        assert!(ends_at_once(2, 1));
    }

    /// A stack run counts until its guard drops, a panic included.
    #[test]
    fn a_stack_run_counts_until_it_ends() {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let running = || COUNT.load(Ordering::SeqCst);
        let first = Stacking::on(&COUNT);
        let second = Stacking::on(&COUNT);
        assert_eq!(running(), 2);
        drop(first);
        assert_eq!(running(), 1);
        drop(second);
        assert_eq!(running(), 0);
        let panicked = thread("stack-test")
            .spawn(move || {
                let _stacking = Stacking::on(&COUNT);
                assert_eq!(running(), 1);
                panic!("the run failed");
            })
            .unwrap()
            .join();
        assert!(panicked.is_err());
        assert_eq!(running(), 0, "a panicked run is no longer counted");
    }

    /// A job that signals its start, then waits for `gate` or a cancel.
    fn held(
        started: mpsc::Sender<&'static str>,
        name: &'static str,
        gate: Cancel,
    ) -> impl FnOnce() -> &'static str {
        move || {
            started.send(name).unwrap();
            let token = current();
            while !gate.is_canceled() && !token.is_canceled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            name
        }
    }

    /// A GPU error a job leaves unchecked doesn't fail the next job.
    #[test]
    fn a_gpu_error_left_by_one_job_does_not_fail_the_next() {
        let Some(ctx) = deptheos_gpu::GpuContext::new_or_skip() else { return };
        let pool = Pool::new(2);
        let (started, starts) = mpsc::channel();
        let (gate, root) = (Cancel::root(), Cancel::root());
        // One worker held, so both jobs run on the other.
        let holding = pool.run(
            Lane::Interactive,
            Key::kind("hold"),
            &root,
            held(started, "hold", gate.clone()),
        );
        assert_eq!(starts.recv_timeout(SOON), Ok("hold"));
        let failing = ctx.clone();
        let first = pool.run(Lane::Interactive, Key::kind("gpu-a"), &root, move || {
            let format = wgpu::TextureFormat::R32Float;
            drop(failing.create_texture(0, 16, format, wgpu::TextureUsages::COPY_SRC, "bad"));
            std::thread::current().id()
        });
        let first = first.wait().unwrap();
        let next = pool.run(Lane::Interactive, Key::kind("gpu-b"), &root, move || {
            (std::thread::current().id(), ctx.check())
        });
        let (on, checked) = next.wait().unwrap();
        assert_eq!(on, first, "the same worker");
        assert!(checked.is_ok(), "{checked:?}");
        gate.cancel();
        assert_eq!(holding.wait(), Ok("hold"));
    }

    /// Background work never takes the last free worker; interactive does.
    #[test]
    fn background_never_takes_the_last_free_worker() {
        let pool = Pool::new(2);
        let (started, starts) = mpsc::channel();
        let (gate_a, gate_b) = (Cancel::root(), Cancel::root());
        let root = Cancel::root();
        let a = pool.run(
            Lane::Interactive,
            Key::kind("a"),
            &root,
            held(started.clone(), "a", gate_a.clone()),
        );
        assert_eq!(starts.recv_timeout(SOON), Ok("a"));
        let background = pool.run(
            Lane::Background,
            Key::kind("bg"),
            &root,
            held(started.clone(), "bg", Cancel::root()),
        );
        assert!(
            starts.recv_timeout(Duration::from_millis(200)).is_err(),
            "background took the last worker"
        );
        let b =
            pool.run(Lane::Interactive, Key::kind("b"), &root, held(started, "b", gate_b.clone()));
        assert_eq!(
            starts.recv_timeout(SOON),
            Ok("b"),
            "interactive work starts on the last worker"
        );
        gate_a.cancel();
        gate_b.cancel();
        assert_eq!(a.wait(), Ok("a"));
        assert_eq!(b.wait(), Ok("b"));
        assert_eq!(starts.recv_timeout(SOON), Ok("bg"), "background starts once two are free");
        drop(background);
    }

    /// A newer interactive request cancels and waits for the one before;
    /// requests of particular things aren't replaced.
    #[test]
    fn interactive_requests_are_latest_wins() {
        let pool = Pool::new(4);
        let (started, starts) = mpsc::channel();
        let root = Cancel::root();
        let first = pool.run(
            Lane::Interactive,
            Key::kind("open"),
            &root,
            held(started.clone(), "first", Cancel::root()),
        );
        assert_eq!(starts.recv_timeout(SOON), Ok("first"));
        let first_token = first.token().clone();
        let queued = pool.run(
            Lane::Interactive,
            Key::kind("open"),
            &root,
            held(started.clone(), "queued", Cancel::root()),
        );
        let gate = Cancel::root();
        let last = pool.run(
            Lane::Interactive,
            Key::kind("open"),
            &root,
            held(started, "last", gate.clone()),
        );
        assert!(first_token.is_canceled(), "the running one is canceled");
        assert_eq!(queued.wait(), Err(Ended::Canceled), "the queued one never runs");
        assert_eq!(first.wait(), Ok("first"));
        assert_eq!(starts.recv_timeout(SOON), Ok("last"));
        gate.cancel();
        assert_eq!(last.wait(), Ok("last"));
        // Of particular things, two run side by side.
        let (started, starts) = mpsc::channel();
        let gate = Cancel::root();
        let one = pool.run(
            Lane::Interactive,
            Key::of("open", 1),
            &root,
            held(started.clone(), "one", gate.clone()),
        );
        let two = pool.run(
            Lane::Interactive,
            Key::of("open", 2),
            &root,
            held(started, "two", gate.clone()),
        );
        let mut both = [starts.recv_timeout(SOON).unwrap(), starts.recv_timeout(SOON).unwrap()];
        both.sort();
        assert_eq!(both, ["one", "two"]);
        gate.cancel();
        assert_eq!((one.wait(), two.wait()), (Ok("one"), Ok("two")));
    }

    /// Quit waits while any must-finish token lives, and not after.
    #[test]
    fn must_finish_work_is_counted_while_it_lives() {
        let pool = Pool::new(2);
        let token = must_finish();
        let (started, starts) = mpsc::channel();
        let gate = Cancel::root();
        let writing_job = pool.run(
            Lane::Background,
            Key::kind("write"),
            &token,
            held(started, "write", gate.clone()),
        );
        drop(token);
        assert_eq!(starts.recv_timeout(SOON), Ok("write"));
        assert!(writing(), "the job holds its token");
        gate.cancel();
        assert_eq!(writing_job.wait(), Ok("write"));
        let start = std::time::Instant::now();
        while writing() {
            assert!(start.elapsed() < SOON, "the token outlived its job");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Canceling a request's token cancels its work, as dropping it does.
    #[test]
    fn a_request_s_token_canceled_cancels_its_work() {
        let pool = Pool::new(2);
        let (started, starts) = mpsc::channel();
        let mine = Cancel::root().child();
        let request = pool.run(
            Lane::Background,
            Key::of("preview", 1),
            &mine,
            held(started, "x", Cancel::root()),
        );
        assert_eq!(starts.recv_timeout(SOON), Ok("x"));
        let token = request.token().clone();
        mine.cancel();
        assert!(token.is_canceled());
        assert_eq!(request.wait(), Ok("x"), "the work ran, and stopped at its token");
    }

    /// Dropping a request cancels its work; a panic is reported, not lost.
    #[test]
    fn a_dropped_request_cancels_and_a_panic_is_reported() {
        let pool = Pool::new(2);
        let (started, starts) = mpsc::channel();
        let root = Cancel::root();
        let request = pool.run(
            Lane::Interactive,
            Key::kind("drop"),
            &root,
            held(started, "d", Cancel::root()),
        );
        assert_eq!(starts.recv_timeout(SOON), Ok("d"));
        let token = request.token().clone();
        drop(request);
        assert!(token.is_canceled());
        let panicked =
            pool.run(Lane::Background, Key::kind("panic"), &root, || -> u8 { panic!("boom") });
        assert_eq!(panicked.wait(), Err(Ended::Panicked("boom".into())));
    }

    /// A job runs as its thread's current work, with the codec budget.
    #[test]
    fn a_job_runs_as_the_threads_current_work() {
        let pool = Pool::new(2);
        let (threads, canceled) = pool
            .run(Lane::Interactive, Key::kind("budget"), &Cancel::root(), || {
                (deptheos_core::cancel::codec_threads(), current().is_canceled())
            })
            .wait()
            .unwrap();
        assert_eq!((threads, canceled), (pool.codec_threads(), false));
    }
}
