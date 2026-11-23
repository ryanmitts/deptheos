//! The desktop's doorway to the engine's threads (ADR-143): blocking work
//! runs on the engine's pool, never GPUI's background executor (on macOS a
//! GCD queue with 512 KB stacks). The GPUI task awaiting it owns the
//! request, so dropping the task cancels the work, not only the wait.

use std::future::Future;

pub use deptheos_app::engine::work::{
    Cancel, Ended, Key, Lane, current, must_finish, panic_message, root, thread, writing,
};

/// Run `f` on the pool in `lane` as request `key`, canceled by `cancel` or
/// dropping the future, which resolves to `f`'s result or a panic message.
/// A canceled request never resolves: a newer one replaced it or quit is
/// under way, so nothing applies; the pending future simply goes away.
pub fn run<T, F>(
    lane: Lane,
    key: Key,
    cancel: &Cancel,
    f: F,
) -> impl Future<Output = Result<T, String>> + use<T, F>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    // Slots belong to the asking UI thread, so tests' apps don't replace
    // each other's opens.
    let key = key.owned_by(std::thread::current().id());
    let request = deptheos_app::engine::work::pool().run(lane, key, cancel, f);
    async move {
        match request.await {
            Ok(done) => Ok(done),
            Err(Ended::Canceled) => std::future::pending().await,
            Err(Ended::Panicked(message)) => Err(format!("panicked: {message}")),
        }
    }
}

/// [`run`] for what the user waits on (open, preview, sort): the latest of
/// `kind` wins; quit abandons it.
pub fn interactive<T, F>(
    kind: &'static str,
    f: F,
) -> impl Future<Output = Result<T, String>> + use<T, F>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    run(Lane::Interactive, Key::kind(kind), root(), f)
}

/// [`run`] for unwatched work (thumbnails, listing, measuring); quit abandons it.
pub fn background<T, F>(
    kind: &'static str,
    f: F,
) -> impl Future<Output = Result<T, String>> + use<T, F>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    run(Lane::Background, Key::kind(kind), root(), f)
}

/// [`run`] for a write quit waits on (export, save, copy); only dropping the
/// future cancels it.
pub fn write<T, F>(kind: &'static str, f: F) -> impl Future<Output = Result<T, String>> + use<T, F>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    run(Lane::Background, Key::kind(kind), &must_finish(), f)
}

/// A Library write on the store thread, after all earlier ones (ADR-143).
/// Never canceled: dropping stops only the wait, and quit waits for it.
pub fn store<T, F>(f: F) -> impl Future<Output = Result<T, String>> + use<T, F>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let written = deptheos_app::engine::store::store().write(f);
    async move { written.await.ok_or_else(|| "the write panicked".to_string()) }
}

/// Tests hold a kind of work back while acting
/// ([`deptheos_app::engine::work::gate`]).
#[cfg(test)]
pub use deptheos_app::engine::work::gate;
#[cfg(test)]
pub use deptheos_app::engine::work::gate::STORE;

/// [`run`] for fallible work: its value, its error as text, or [`run`]'s
/// message, for callers treating failures alike.
pub fn try_run<T, E, F>(
    lane: Lane,
    key: Key,
    cancel: &Cancel,
    f: F,
) -> impl Future<Output = Result<T, String>> + use<T, E, F>
where
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    let ran = run(lane, key, cancel, f);
    async move { ran.await.and_then(|result| result.map_err(|e| e.to_string())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Work gets more stack than a GCD worker's 512 KB: 2 MB of locals fit.
    #[test]
    fn work_has_a_main_thread_stack() {
        #[inline(never)]
        fn deep() -> u8 {
            let buffer = std::hint::black_box([7u8; 2 << 20]);
            buffer[buffer.len() - 1]
        }
        let ran = run(Lane::Background, Key::kind("stack-test"), root(), deep);
        assert_eq!(futures::executor::block_on(ran), Ok(7));
    }

    /// A held kind waits until released; other kinds and threads go on.
    #[test]
    fn a_kind_held_waits_until_let_go() {
        use futures::FutureExt as _;
        let held = gate::hold("gate-test");
        let mut waits = run(Lane::Interactive, Key::kind("gate-test"), root(), || 1).boxed();
        assert_eq!(held.waiting(), 1);
        let other = run(Lane::Interactive, Key::kind("gate-other"), root(), || 2);
        assert_eq!(futures::executor::block_on(other), Ok(2));
        let elsewhere = std::thread::spawn(|| {
            futures::executor::block_on(run(
                Lane::Interactive,
                Key::kind("gate-test"),
                root(),
                || 3,
            ))
        });
        assert_eq!(elsewhere.join().unwrap(), Ok(3));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!((&mut waits).now_or_never().is_none(), "held");
        held.open();
        assert_eq!(futures::executor::block_on(waits), Ok(1));
        let write = gate::hold(STORE);
        let mut written = store(|| 4).boxed();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!((&mut written).now_or_never().is_none(), "a Library write held");
        drop(write);
        assert_eq!(futures::executor::block_on(written), Ok(4));
    }

    /// Dropping the future cancels the work.
    #[test]
    fn dropping_the_future_cancels_the_work() {
        let (tx, rx) = std::sync::mpsc::channel();
        let ran = run(Lane::Interactive, Key::kind("drop-test"), root(), move || {
            let token = deptheos_app::engine::work::current();
            tx.send(token.clone()).unwrap();
            while !token.is_canceled() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let token = rx.recv().unwrap();
        drop(ran);
        assert!(token.is_canceled());
    }
}
