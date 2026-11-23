//! The bridge's doorway to the engine's threads (ADR-143). Every call that
//! would block is `async`: it hands the work at once to the pool in its
//! lane, the store thread or its own thread (the stack), and awaits a
//! one-shot channel. UniFFI polls on whichever thread resumes Swift, so
//! nothing blocks it and no async runtime is needed.
//!
//! UniFFI doesn't carry Swift's cancellation into Rust, so each stoppable
//! call takes a [`Request`], canceled with its task (`Request.run` in the
//! app). A Library write takes none: it is never canceled once sent
//! (`deptheos_app::engine::store`).
//!
//! iOS stops GPU work in the background, so every request but the stack's
//! is under one foreground token: [`leave_foreground`] cancels it, and new
//! requests are refused until [`enter_foreground`]. The stack's and an
//! export's ([`Request::lasting`]) go on in the time iOS gives them, or the
//! app stops them: an export is the person's own write, and finishes.

use std::sync::{Arc, Mutex};

use deptheos_app::engine::work::{self, Cancel, Ended, Key, Lane};
use deptheos_app::{Engine as AppEngine, Words as AppWords};

use crate::{EngineError, Result};

/// One call's cancel: a token under the engine's root, canceled only by the
/// app and quit. Canceled, queued work never runs and running work ends at
/// its next check; the call throws `Canceled` unless it finished first.
/// Cancel saves work; the app's own check keeps results correct.
#[derive(uniffi::Object)]
pub struct Request {
    pub(crate) cancel: Cancel,
}

#[uniffi::export]
impl Request {
    /// Canceled as the app leaves the foreground, refused while away.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { cancel: FOREGROUND.child() })
    }

    /// Goes on in the background: the stack's and an export's (within
    /// iOS's time, or until the app stops them), and the engine's own
    /// starts and writes.
    #[uniffi::constructor]
    pub fn lasting() -> Arc<Self> {
        Arc::new(Self { cancel: work::root().child() })
    }

    /// Stop the work this request asked for.
    pub fn cancel(&self) {
        if !self.cancel.is_canceled() {
            tracing::debug!("request canceled");
        }
        self.cancel.cancel();
    }
}

/// The app left the foreground: every request but the stack's is canceled,
/// and new ones refused until [`enter_foreground`]. The app then asks again
/// for what it still wants.
#[uniffi::export]
pub fn leave_foreground() {
    FOREGROUND.leave();
}

/// The app is back in front: requests are taken again.
#[uniffi::export]
pub fn enter_foreground() {
    FOREGROUND.enter();
}

static FOREGROUND: Foreground = Foreground::new();

/// The token the app's foreground requests are made under:
///
/// ```text
/// in front (a live token) ──leave──▶ away (that token canceled)
/// away ──enter──▶ in front (a new token)
/// ```
///
/// Leaving or entering twice does nothing more.
pub(crate) struct Foreground(Mutex<Option<Cancel>>);

impl Foreground {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// A token under the current one (the engine's root's child at first),
    /// canceled from the start while away.
    pub(crate) fn child(&self) -> Cancel {
        crate::lock(&self.0).get_or_insert_with(|| work::root().child()).child()
    }

    pub(crate) fn leave(&self) {
        // Canceled outside the lock: its hooks may ask for a request.
        let token = crate::lock(&self.0).get_or_insert_with(|| work::root().child()).clone();
        if !token.is_canceled() {
            tracing::debug!("foreground: in front → away, its requests canceled");
            token.cancel();
        }
    }

    pub(crate) fn enter(&self) {
        let mut token = crate::lock(&self.0);
        if token.as_ref().is_some_and(Cancel::is_canceled) {
            tracing::debug!("foreground: away → in front");
            *token = Some(work::root().child());
        }
    }
}

/// What a call asks of the pool: the latest of its kind for one engine,
/// replacing the one before (a render opening), or one of its own.
pub(crate) enum Ask<'a> {
    Latest(&'static str, &'a Arc<AppEngine>),
    Own(&'static str),
}

impl Ask<'_> {
    fn key(&self, request: &Arc<Request>) -> Key {
        match self {
            // The slot is the engine's: tests run engines side by side.
            Self::Latest(kind, engine) => Key::kind(kind).owned_by(Arc::as_ptr(engine) as usize),
            Self::Own(kind) => Key::of(kind, Arc::as_ptr(request) as usize),
        }
    }
}

/// Run `f` on the pool in `lane` for `request`: its result, `Canceled` if
/// canceled (or replaced) first, a fault if it panicked. A request
/// canceled already asks for nothing.
pub(crate) async fn on_pool<T: Send + 'static>(
    words: &AppWords,
    request: &Arc<Request>,
    lane: Lane,
    ask: Ask<'_>,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    if request.cancel.is_canceled() {
        return Err(EngineError::Canceled);
    }
    let key = ask.key(request);
    ended(words, work::pool().run(lane, key, &request.cancel, f).await)
}

/// Write with `f` on the store thread, after every write sent before it.
/// Never canceled: an app that stops waiting only stops the wait.
pub(crate) async fn on_store<T: Send + 'static>(
    words: &AppWords,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match deptheos_app::engine::store::store().write(f).await {
        Some(written) => written,
        None => Err(EngineError::fault(words, None, "a Library write panicked")),
    }
}

/// Run `f` on its own thread `name` (the stack's), under `request`'s token,
/// as [`on_pool`].
pub(crate) async fn on_thread<T: Send + 'static>(
    words: &AppWords,
    name: &str,
    request: &Arc<Request>,
    f: impl FnOnce(&Cancel) -> Result<T> + Send + 'static,
) -> Result<T> {
    if request.cancel.is_canceled() {
        return Err(EngineError::Canceled);
    }
    let (tx, rx) = futures::channel::oneshot::channel();
    let cancel = request.cancel.clone();
    let spawned = work::thread(name.to_string()).spawn(move || {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&cancel)));
        let _ = tx.send(ran.map_err(|payload| Ended::Panicked(work::panic_message(&*payload))));
    });
    if let Err(e) = spawned {
        return Err(EngineError::fault(words, None, format!("no {name} thread: {e}")));
    }
    ended(words, rx.await.unwrap_or(Err(Ended::Canceled)))
}

fn ended<T>(words: &AppWords, ran: std::result::Result<Result<T>, Ended>) -> Result<T> {
    match ran {
        Ok(done) => done,
        Err(Ended::Canceled) => Err(EngineError::Canceled),
        Err(Ended::Panicked(message)) => {
            Err(EngineError::fault(words, None, format!("panicked: {message}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// Leaving the foreground cancels requests made in front, refuses those
    /// made away, and spares a lasting one; back in front, new ones are
    /// taken.
    #[test]
    fn leaving_the_foreground_stops_its_requests_until_back() {
        let foreground = Foreground::new();
        let lasting = work::root().child();
        let before = foreground.child();
        foreground.leave();
        assert!(before.is_canceled(), "a request in front is canceled");
        assert!(foreground.child().is_canceled(), "one made while away is refused");
        assert!(!lasting.is_canceled(), "the stack's goes on");
        foreground.leave();
        foreground.enter();
        let after = foreground.child();
        assert!(!after.is_canceled(), "taken again in front");
        foreground.enter();
        assert!(!after.is_canceled(), "entering twice changes nothing");
        assert!(before.is_canceled());
    }

    /// A request made away asks the pool for nothing: refused as canceled
    /// at once.
    #[test]
    fn a_request_made_while_away_is_refused() {
        let foreground = Foreground::new();
        foreground.leave();
        let request = Arc::new(Request { cancel: foreground.child() });
        let words = AppWords::english(deptheos_app::Platform::Ios);
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let call = on_pool(&words, &request, Lane::Interactive, Ask::Own("test-away"), move || {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        });
        let answer = futures::executor::block_on(call);
        assert!(matches!(answer, Err(EngineError::Canceled)), "{answer:?}");
        assert!(!ran.load(Ordering::SeqCst));
    }
}
