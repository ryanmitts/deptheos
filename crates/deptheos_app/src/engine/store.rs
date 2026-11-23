//! The store thread (ADR-143): one serial thread for every Library write
//! and the desktop's preferences, in the order sent. It never encodes
//! (that is the pool's), so a rename never waits behind an encode.
//!
//! A write is never canceled; dropping its [`Written`] only stops the
//! wait. A kept value ([`Store::keep`]) is written, newest only, once it
//! has stood for [`KEEP_AFTER`]. Quit waits ([`Store::busy`],
//! [`Store::flush`]).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::channel::oneshot;

use crate::lock;

/// How long a kept value stands unchanged before it is written.
pub const KEEP_AFTER: Duration = Duration::from_millis(250);

type Job = Box<dyn FnOnce() + Send>;

/// The process's store thread, started on first use.
pub fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(Store::start)
}

/// The handle to the store thread; see the module.
pub struct Store {
    inner: Arc<Inner>,
    thread: std::thread::ThreadId,
}

struct Inner {
    queue: Mutex<Queue>,
    /// Wakes the thread for a job, and waiters once it is idle.
    changed: Condvar,
}

/// The jobs in the order sent, and the kept values by key until due.
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Kept values not written yet, by key: the newest, and when it is due.
    kept: HashMap<String, (Instant, Job)>,
    mode: Mode,
}

/// Lifecycle: waiting, writing, flushing, writing while flushing. Jobs
/// change it ([`Mode::start_job`], [`Mode::end_job`]); a [`Store::flush`]
/// makes kept values due at once ([`Mode::start_flush`],
/// [`Mode::end_flush`]).
///
/// ```text
/// Waiting ──a job──▶ Writing ──done──▶ Waiting
/// Flushing ──a job──▶ FlushingWriting ──done──▶ Flushing
/// Waiting, Writing ──a flush──▶ Flushing, FlushingWriting ──it ends──▶ Waiting, Writing
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Waiting,
    Writing,
    Flushing,
    FlushingWriting,
}

impl Mode {
    fn writing(self) -> bool {
        matches!(self, Self::Writing | Self::FlushingWriting)
    }

    fn flushing(self) -> bool {
        matches!(self, Self::Flushing | Self::FlushingWriting)
    }

    fn start_job(&mut self) {
        self.to(match *self {
            Self::Waiting | Self::Writing => Self::Writing,
            Self::Flushing | Self::FlushingWriting => Self::FlushingWriting,
        });
    }

    fn end_job(&mut self) {
        self.to(match *self {
            Self::Waiting | Self::Writing => Self::Waiting,
            Self::Flushing | Self::FlushingWriting => Self::Flushing,
        });
    }

    fn start_flush(&mut self) {
        self.to(match *self {
            Self::Waiting | Self::Flushing => Self::Flushing,
            Self::Writing | Self::FlushingWriting => Self::FlushingWriting,
        });
    }

    fn end_flush(&mut self) {
        self.to(match *self {
            Self::Waiting | Self::Flushing => Self::Waiting,
            Self::Writing | Self::FlushingWriting => Self::Writing,
        });
    }

    fn to(&mut self, next: Self) {
        if *self != next {
            tracing::debug!("store: {self:?} -> {next:?}");
            *self = next;
        }
    }
}

impl Queue {
    fn is_idle(&self) -> bool {
        self.jobs.is_empty() && self.kept.is_empty() && !self.mode.writing()
    }

    /// The next job to run now, or how long until a kept one is due.
    fn next(&mut self) -> Result<Job, Option<Duration>> {
        if let Some(job) = self.jobs.pop_front() {
            return Ok(job);
        }
        let now = Instant::now();
        let flushing = self.mode.flushing();
        let due = self.kept.iter().filter(|(_, (at, _))| flushing || *at <= now);
        if let Some(key) = due.min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone()) {
            return Ok(self.kept.remove(&key).expect("a key just found").1);
        }
        Err(self.kept.values().map(|(at, _)| at.saturating_duration_since(now)).min())
    }
}

impl Store {
    fn start() -> Self {
        let inner = Arc::new(Inner { queue: Mutex::default(), changed: Condvar::new() });
        let worker = inner.clone();
        let thread = crate::engine::work::thread("store")
            .spawn(move || worker.run())
            .expect("the store thread could not start");
        Self { inner, thread: thread.thread().id() }
    }

    /// Whether the caller runs on the store thread, where waiting on a
    /// write would deadlock.
    pub(crate) fn is_current(&self) -> bool {
        std::thread::current().id() == self.thread
    }

    /// Writes with `f` after everything sent before it.
    pub fn write<T, F>(&self, f: F) -> Written<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move || {
            let _ = tx.send(f());
        });
        #[cfg(any(test, feature = "test-support"))]
        if let Some(opened) = crate::engine::work::gate::of(crate::engine::work::gate::STORE) {
            let inner = self.inner.clone();
            std::thread::spawn(move || {
                let _ = futures::executor::block_on(opened);
                lock(&inner.queue).jobs.push_back(job);
                inner.changed.notify_all();
            });
            return Written(rx);
        }
        self.send(job);
        Written(rx)
    }

    /// Writes `key`'s value with `f` once it has stood [`KEEP_AFTER`]; a
    /// newer one replaces it and restarts the wait.
    pub fn keep(&self, key: impl Into<String>, f: impl FnOnce() + Send + 'static) {
        let mut queue = lock(&self.inner.queue);
        queue.kept.insert(key.into(), (Instant::now() + KEEP_AFTER, Box::new(f)));
        drop(queue);
        self.inner.changed.notify_all();
    }

    /// Drops `key`'s unwritten kept value, for a write that supersedes it.
    pub fn forget(&self, key: &str) {
        if lock(&self.inner.queue).kept.remove(key).is_some() {
            tracing::debug!("store: {key}'s kept value dropped for a write over it");
        }
        self.inner.changed.notify_all();
    }

    fn send(&self, job: Job) {
        lock(&self.inner.queue).jobs.push_back(job);
        self.inner.changed.notify_all();
    }

    /// Something is queued, kept or being written.
    pub fn busy(&self) -> bool {
        !lock(&self.inner.queue).is_idle()
    }

    /// Writes everything now, waiting up to `timeout`; `true` once done.
    pub fn flush(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        let mut queue = lock(&self.inner.queue);
        queue.mode.start_flush();
        self.inner.changed.notify_all();
        while !queue.is_idle() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            queue =
                self.inner.changed.wait_timeout(queue, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        queue.mode.end_flush();
        queue.is_idle()
    }
}

impl Inner {
    fn run(&self) {
        let mut queue = lock(&self.queue);
        loop {
            let job = match queue.next() {
                Ok(job) => job,
                Err(due) => {
                    queue = match due {
                        Some(wait) => {
                            self.changed
                                .wait_timeout(queue, wait)
                                .unwrap_or_else(|e| e.into_inner())
                                .0
                        }
                        None => self.changed.wait(queue).unwrap_or_else(|e| e.into_inner()),
                    };
                    continue;
                }
            };
            queue.mode.start_job();
            drop(queue);
            if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(job)) {
                tracing::error!("a write panicked: {}", crate::engine::work::panic_message(&panic));
            }
            queue = lock(&self.queue);
            queue.mode.end_job();
            self.changed.notify_all();
        }
    }
}

/// A write's result, to await or wait for. Dropping it doesn't stop the
/// write.
pub struct Written<T>(oneshot::Receiver<T>);

impl<T> Written<T> {
    /// Block until the write is done; `None` if it panicked.
    pub fn wait(self) -> Option<T> {
        futures::executor::block_on(self.0).ok()
    }
}

impl<T> Future for Written<T> {
    /// `None` if the write panicked.
    type Output = Option<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(Result::ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes land in order: two quick renames end with the second name.
    #[test]
    fn writes_land_in_the_order_sent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("name");
        let slow = {
            let path = path.clone();
            store().write(move || {
                std::thread::sleep(Duration::from_millis(30));
                std::fs::write(&path, "first").unwrap();
            })
        };
        let quick = {
            let path = path.clone();
            store().write(move || std::fs::write(&path, "second").unwrap())
        };
        assert_eq!(quick.wait(), Some(()));
        assert_eq!(slow.wait(), Some(()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }

    /// A kept value is written once, the newest, after it stands; a flush
    /// writes it at once.
    #[test]
    fn a_kept_value_is_written_once_it_stands() {
        let written = Arc::new(Mutex::new(Vec::new()));
        for value in 1..=3 {
            let written = written.clone();
            store().keep("store-test", move || written.lock().unwrap().push(value));
        }
        assert!(store().busy());
        assert!(store().flush(Duration::from_secs(5)));
        assert_eq!(*written.lock().unwrap(), [3]);
        let later = written.clone();
        store().keep("store-test", move || later.lock().unwrap().push(4));
        let start = Instant::now();
        while written.lock().unwrap().len() < 2 {
            assert!(start.elapsed() < Duration::from_secs(5), "never written");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(start.elapsed() >= KEEP_AFTER - Duration::from_millis(5));
        assert_eq!(*written.lock().unwrap(), [3, 4]);
    }

    /// A value forgotten before it stands is never written.
    #[test]
    fn a_forgotten_value_is_not_written() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let kept = written.clone();
        store().keep("store-forget-test", move || kept.lock().unwrap().push(1));
        store().forget("store-forget-test");
        assert!(store().flush(Duration::from_secs(5)));
        assert!(written.lock().unwrap().is_empty());
    }

    /// A write that panics is reported, and the next one still runs.
    #[test]
    fn a_panicking_write_does_not_stop_the_thread() {
        assert_eq!(store().write(|| panic!("died")).wait(), None::<()>);
        assert_eq!(store().write(|| 7).wait(), Some(7));
    }

    /// A flush and a job overlap either way round: each ends only itself.
    #[test]
    fn a_flush_and_a_job_end_apart() {
        let mut mode = Mode::Waiting;
        mode.start_job();
        mode.start_flush();
        assert_eq!(mode, Mode::FlushingWriting);
        mode.end_job();
        assert_eq!(mode, Mode::Flushing);
        assert!(mode.flushing() && !mode.writing());
        mode.start_job();
        mode.end_flush();
        assert_eq!(mode, Mode::Writing);
        mode.end_job();
        assert_eq!(mode, Mode::Waiting);
    }

    /// A kept value is due at once while a flush waits, and not otherwise.
    #[test]
    fn a_kept_value_is_due_at_once_only_while_flushing() {
        let mut queue = Queue::default();
        queue.kept.insert("k".into(), (Instant::now() + KEEP_AFTER, Box::new(|| {})));
        assert!(queue.next().is_err());
        queue.mode.start_flush();
        assert!(queue.next().is_ok());
        queue.mode.end_flush();
        assert!(queue.is_idle());
    }
}
