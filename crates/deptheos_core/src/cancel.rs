//! Cancel tokens, in one tree (ADR-143): the engine's root, canceled only
//! by quit; under it one token per run (a stack, a batch, an export); under
//! a run, its work (a decode, a conversion, a preview).
//!
//! Canceling a token cancels its descendants, each once, by pushing down,
//! so [`Cancel::is_canceled`] is one relaxed load, cheap in an inner loop;
//! the codecs take [`Cancel::flag`]. Parents hold children weakly and prune
//! the gone. What can't poll is woken by a hook ([`Cancel::on_cancel`]),
//! run once on the canceling thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// A cancel token. Clones share it.
#[derive(Clone)]
pub struct Cancel(Arc<Node>);

struct Node {
    canceled: AtomicBool,
    links: Mutex<Links>,
    /// Held while the hooks run, so dropping a running hook waits for it.
    running: Mutex<()>,
    /// Counts this token while it lives ([`Cancel::counted`]).
    counted: Option<&'static std::sync::atomic::AtomicUsize>,
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Some(count) = self.counted {
            count.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[derive(Default)]
struct Links {
    children: Vec<Weak<Node>>,
    hooks: Vec<(u64, Box<dyn FnOnce() + Send>)>,
    next_hook: u64,
}

impl Cancel {
    /// A token with no parent: the engine's root, or one for a test.
    pub fn root() -> Self {
        Self::with_count(None)
    }

    /// A root token counted in `count` while it or a descendant lives, for
    /// what quit waits for (`deptheos_app::engine::work::must_finish`).
    pub fn counted(count: &'static std::sync::atomic::AtomicUsize) -> Self {
        Self::with_count(Some(count))
    }

    fn with_count(counted: Option<&'static std::sync::atomic::AtomicUsize>) -> Self {
        if let Some(count) = counted {
            count.fetch_add(1, Ordering::SeqCst);
        }
        Self(Arc::new(Node {
            canceled: AtomicBool::new(false),
            links: Mutex::default(),
            running: Mutex::new(()),
            counted,
        }))
    }

    /// A token canceled with this one, or canceled already if this one is.
    pub fn child(&self) -> Self {
        let child = Self::with_count(self.0.counted);
        let mut links = self.0.links();
        if self.is_canceled() {
            child.0.canceled.store(true, Ordering::Relaxed);
        } else {
            links.children.retain(|c| c.strong_count() > 0);
            links.children.push(Arc::downgrade(&child.0));
        }
        child
    }

    /// Cancel this token and every token under it, running their hooks.
    /// Canceling again does nothing.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn is_canceled(&self) -> bool {
        self.0.canceled.load(Ordering::Relaxed)
    }

    /// `Err(Canceled)` once canceled, for `?` between steps.
    pub fn check(&self) -> crate::Result<()> {
        if self.is_canceled() { Err(crate::CoreError::Canceled) } else { Ok(()) }
    }

    /// The flag itself, for the codecs, which take a plain `&AtomicBool`.
    pub fn flag(&self) -> &AtomicBool {
        &self.0.canceled
    }

    /// Run `f` once when this token is canceled, on the canceling thread
    /// (at once if it is already). Dropping the [`Hook`] first removes it.
    pub fn on_cancel(&self, f: impl FnOnce() + Send + 'static) -> Hook {
        let mut links = self.0.links();
        if self.is_canceled() {
            drop(links);
            f();
            return Hook { node: Weak::new(), id: 0 };
        }
        links.next_hook += 1;
        let id = links.next_hook;
        links.hooks.push((id, Box::new(f)));
        Hook { node: Arc::downgrade(&self.0), id }
    }

    /// A guard that cancels this token when dropped, held by the request.
    pub fn guard(&self) -> CancelOnDrop {
        CancelOnDrop(self.clone())
    }
}

impl Default for Cancel {
    fn default() -> Self {
        Self::root()
    }
}

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Cancel").field(&self.is_canceled()).finish()
    }
}

impl Node {
    fn links(&self) -> MutexGuard<'_, Links> {
        self.links.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cancel(&self) {
        // Set before the lock, read by `child` and `on_cancel` under it:
        // what they add later sees it, what they added before is taken.
        if self.canceled.swap(true, Ordering::Relaxed) {
            return;
        }
        let mut links = self.links();
        let Links { children, hooks, .. } = std::mem::take(&mut *links);
        // Before the links go: a hook dropped from now on waits here.
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        drop(links);
        // A panicking hook doesn't stop the rest of the cancel.
        for (_, hook) in hooks {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(hook)).is_err() {
                tracing::error!("a cancel hook panicked; the rest still run");
            }
        }
        drop(running);
        for child in children.iter().filter_map(Weak::upgrade) {
            child.cancel();
        }
    }
}

thread_local! {
    /// The work this thread runs now ([`within`]).
    static CURRENT: std::cell::RefCell<Option<(Cancel, usize)>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` as this thread's current work, with token `cancel` and
/// `codec_threads` for any codec it calls, so a decode deep in the engine
/// gets both without every call between passing them ([`codec`]).
pub fn within<R>(cancel: &Cancel, codec_threads: usize, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<(Cancel, usize)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let before = self.0.take();
            CURRENT.with(|c| *c.borrow_mut() = before);
        }
    }
    let before = CURRENT.with(|c| c.borrow_mut().replace((cancel.clone(), codec_threads.max(1))));
    let _restore = Restore(before);
    f()
}

/// The token of this thread's current work ([`within`]); outside any, a
/// token never canceled.
pub fn current() -> Cancel {
    CURRENT.with(|c| c.borrow().as_ref().map(|(cancel, _)| cancel.clone())).unwrap_or_else(|| {
        static NEVER: std::sync::OnceLock<Cancel> = std::sync::OnceLock::new();
        NEVER.get_or_init(Cancel::root).clone()
    })
}

/// The threads a codec in this thread's current work may start
/// ([`within`]); every core outside any.
pub fn codec_threads() -> usize {
    CURRENT
        .with(|c| c.borrow().as_ref().map(|&(_, threads)| threads))
        .unwrap_or_else(crate::threads::cores)
}

/// Threads inside a codec call now ([`codec`]).
static IN_CODEC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many threads are inside a codec call now. Quit exits without
/// running static destructors under them, which could crash a codec.
pub fn in_codec() -> usize {
    IN_CODEC.load(Ordering::SeqCst)
}

/// Call a codec with the current work's [`deptheos_raw::Budget`]: the
/// token's flag, its cores, and LibRaw's interrupt hooked to the token.
/// Counted in [`in_codec`].
pub fn codec<R>(f: impl FnOnce(deptheos_raw::Budget) -> R) -> R {
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            IN_CODEC.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let cancel = current();
    let watch = |interrupt: deptheos_raw::Interrupt| -> Box<dyn std::any::Any> {
        Box::new(cancel.on_cancel(move || interrupt.interrupt()))
    };
    IN_CODEC.fetch_add(1, Ordering::SeqCst);
    let _counted = Counted;
    f(deptheos_raw::Budget { cancel: cancel.flag(), threads: codec_threads(), watch: Some(&watch) })
}

/// A hook added by [`Cancel::on_cancel`]; dropping it removes the hook if
/// it hasn't run, and waits for it if it is running now.
#[must_use = "dropping the hook removes it"]
pub struct Hook {
    node: Weak<Node>,
    id: u64,
}

impl Drop for Hook {
    fn drop(&mut self) {
        if let Some(node) = self.node.upgrade() {
            let mut links = node.links();
            let before = links.hooks.len();
            links.hooks.retain(|(id, _)| *id != self.id);
            let taken = links.hooks.len() == before;
            drop(links);
            if taken {
                drop(node.running.lock().unwrap_or_else(|e| e.into_inner()));
            }
        }
    }
}

/// Cancels its token when dropped ([`Cancel::guard`]).
#[must_use = "dropping the guard cancels its token"]
pub struct CancelOnDrop(Cancel);

impl CancelOnDrop {
    pub fn token(&self) -> &Cancel {
        &self.0
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn counter() -> (Arc<AtomicUsize>, impl FnOnce() + Send + 'static) {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        (count, move || {
            c.fetch_add(1, Ordering::Relaxed);
        })
    }

    /// Canceling reaches every descendant, and only them.
    #[test]
    fn cancel_reaches_every_descendant() {
        let root = Cancel::root();
        let run = root.child();
        let work = run.child();
        let other_run = root.child();
        // (token canceled, which tokens are canceled after: root, run, work, other)
        let cases: [(&Cancel, [bool; 4]); 3] = [
            (&work, [false, false, true, false]),
            (&run, [false, true, true, false]),
            (&root, [true, true, true, true]),
        ];
        for (token, expected) in cases {
            token.cancel();
            let got = [&root, &run, &work, &other_run].map(Cancel::is_canceled);
            assert_eq!(got, expected);
        }
    }

    /// A panicking hook doesn't stop the other hooks or the children.
    #[test]
    fn a_panicking_hook_leaves_the_rest_to_run() {
        let root = Cancel::root();
        let child = root.child();
        let _bad = root.on_cancel(|| panic!("a hook's bug"));
        let (count, hook) = counter();
        let _good = root.on_cancel(hook);
        root.cancel();
        assert_eq!(count.load(Ordering::Relaxed), 1, "the next hook ran");
        assert!(child.is_canceled(), "the child was canceled");
        // Its lock isn't poisoned: a hook added now runs at once.
        let (count, hook) = counter();
        let _late = root.on_cancel(hook);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    /// A child of a canceled token starts canceled; a hook added to one
    /// runs at once.
    #[test]
    fn late_children_and_hooks_are_canceled_at_once() {
        let root = Cancel::root();
        root.cancel();
        assert!(root.child().is_canceled());
        assert!(root.check().is_err());
        let (count, hook) = counter();
        let _hook = root.on_cancel(hook);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    /// A hook runs once, however often its token or an ancestor is
    /// canceled; a dropped hook never runs.
    #[test]
    fn a_hook_runs_once() {
        let root = Cancel::root();
        let run = root.child();
        let (count, hook) = counter();
        let _kept = run.on_cancel(hook);
        let (dropped, hook) = counter();
        drop(run.on_cancel(hook));
        run.cancel();
        root.cancel();
        run.cancel();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    /// Dropping a guard cancels its token, and its children with it.
    #[test]
    fn a_dropped_guard_cancels() {
        let root = Cancel::root();
        let guard = root.child().guard();
        let work = guard.token().child();
        assert!(!work.is_canceled());
        drop(guard);
        assert!(work.is_canceled());
        assert!(!root.is_canceled());
    }

    /// Children that have gone are pruned, not kept for ever.
    #[test]
    fn gone_children_are_pruned() {
        let root = Cancel::root();
        for _ in 0..1000 {
            drop(root.child());
        }
        let _kept = root.child();
        assert!(root.0.links().children.len() <= 1);
    }

    /// Dropping a hook while it runs waits for it to finish.
    #[test]
    fn dropping_a_running_hook_waits_for_it() {
        let token = Cancel::root();
        let (started, running) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let hook = token.on_cancel({
            let finished = finished.clone();
            move || {
                started.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(100));
                finished.store(true, Ordering::Relaxed);
            }
        });
        let canceler = std::thread::spawn({
            let token = token.clone();
            move || token.cancel()
        });
        running.recv().unwrap();
        drop(hook);
        assert!(finished.load(Ordering::Relaxed), "the drop returned while its hook ran");
        canceler.join().unwrap();
    }

    /// The current work is the innermost `within`, and none outside.
    #[test]
    fn the_current_work_is_the_innermost() {
        let outer = Cancel::root();
        let inner = outer.child();
        assert!(!current().is_canceled());
        within(&outer, 3, || {
            assert_eq!(codec_threads(), 3);
            within(&inner, 1, || {
                inner.cancel();
                assert!(current().is_canceled());
                assert_eq!(codec_threads(), 1);
            });
            assert!(!current().is_canceled());
            assert_eq!(codec_threads(), 3);
        });
        assert_eq!(codec_threads(), crate::threads::cores());
    }

    /// Tokens canceled from many threads at once each cancel once.
    #[test]
    fn cancel_from_many_threads() {
        let root = Cancel::root();
        let (count, hook) = counter();
        let child = root.child();
        let _hook = child.on_cancel(hook);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| root.cancel());
                s.spawn(|| child.cancel());
            }
        });
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
