//! Quitting: wait for files being written, save the retouching, or confirm.
//! Quit, closing the window and, on macOS, the system's own quit (Dock,
//! log out, shut down; [`guard_system_quit`]) all ask the same question.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use gpui::{App, Context, Task, Window};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariant;

use super::AppView;
use crate::notices::{Kind, Subject};
use crate::paint::SAVE_ON_QUIT;
use crate::words::say;

/// The longest quitting waits for files still being written.
const QUIT_WAIT: Duration = Duration::from_secs(60);
/// How often a quit checks on what it waits for.
const QUIT_POLL: Duration = Duration::from_millis(20);
/// How long the retouching's save goes before the window says so.
const SAY_SAVING_AFTER: Duration = Duration::from_millis(250);

/// Where a quit is.
///
/// Lifecycle: staying, waiting, kept; changed only by
/// [`AppView::set_quitting`], from a quit asked to the app's quit hook.
///
/// ```text
/// Staying ──quit──▶ Waiting ──files written, retouching saved──▶ Kept ──▶ quit
///    ▲                 │
///    └─────────────────┘ retouching not saved: asked (Quit Anyway: Kept)
/// ```
pub(crate) enum Quitting {
    /// No quit under way.
    Staying,
    /// Waits on files being written, then the retouching saved, then quits.
    Waiting { _task: Task<()> },
    /// Retouching saved or let go: the quit hook doesn't save it again.
    Kept,
}

impl Quitting {
    fn name(&self) -> &'static str {
        match self {
            Self::Staying => "staying",
            Self::Waiting { .. } => "waiting",
            Self::Kept => "kept",
        }
    }
}

impl AppView {
    fn set_quitting(&mut self, quitting: Quitting) {
        tracing::debug!(from = self.quitting.name(), to = quitting.name(), "quit");
        WAITING.store(matches!(quitting, Quitting::Waiting { .. }), Ordering::SeqCst);
        self.quitting = quitting;
    }

    /// Quit (or close the window) now? During a run, ask first and answer
    /// `false` (quitting keeps what is finished, as Cancel does). Otherwise
    /// answer `false` and quit once writes and retouching are done
    /// ([`Self::quit_when_kept`]).
    pub(crate) fn confirm_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Asked again while waiting: now.
        if !matches!(self.quitting, Quitting::Staying) {
            return true;
        }
        let title = if self.batch_run().is_some_and(|run| !run.machine.is_ending()) {
            say!(cx, "quit-batch-running")
        } else if self.sequences.read(cx).is_running() {
            say!(cx, "quit-sequence-running")
        } else if self.stacking.read(cx).is_running() {
            say!(cx, "quit-stack-running")
        } else {
            self.quit_when_kept(window, cx);
            return false;
        };
        let description = say!(cx, "quit-description");
        let quit = say!(cx, "quit-button");
        let this = cx.weak_entity();
        let cancel = say!(cx, "button-cancel");
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text(quit.clone())
                .cancel_text(cancel.clone())
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.cancel_run(window, cx);
                        this.quit_when_kept(window, cx);
                    });
                    true
                })
        });
        false
    }

    /// Ask Quit Anyway or Cancel: the retouching couldn't be written (full
    /// disk, Library disk gone).
    fn ask_about_unsaved_paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = say!(cx, "quit-paint-unsaved");
        let description = say!(cx, "quit-paint-unsaved-description");
        let quit = say!(cx, "quit-anyway-button");
        let cancel = say!(cx, "button-cancel");
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text(quit.clone())
                .cancel_text(cancel.clone())
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    tracing::warn!("quit with retouching not saved");
                    let _ = this.update(cx, |this, _| this.set_quitting(Quitting::Kept));
                    quit_now(cx);
                    true
                })
        });
    }

    /// This window writes files: a render being kept, an export, a batch's
    /// export, or a sequence's frames (canceled too, until its worker stops).
    pub(super) fn files_being_written(&self, cx: &gpui::App) -> bool {
        self.renders.read(cx).is_saving()
            || self.is_writing_export()
            || self.batch_run().is_some_and(|run| run.machine.is_exporting())
            || self.sequences.read(cx).is_running()
    }

    /// What quit must let finish (ADR-143): this window's writes, a Library
    /// write, or work holding a must-finish token. Quit handlers get only
    /// 200 ms, so quit waits for these instead.
    pub(super) fn must_finish(&self, cx: &gpui::App) -> bool {
        self.files_being_written(cx)
            || deptheos_app::engine::store::store().busy()
            || crate::work::writing()
    }

    /// Quit once writes finish (or after [`QUIT_WAIT`]) and all paint is
    /// saved; if some couldn't be, ask first. The window keeps drawing and
    /// says what it waits for; Quit again quits at once.
    fn quit_when_kept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.quitting, Quitting::Staying) {
            return;
        }
        // Nothing is abandoned until the quit is decided (`quit_now`): a
        // canceled root can't be undone, and the user may still stay.
        if self.must_finish(cx) {
            let text = say!(cx, "quit-waiting");
            self.post(Kind::Pinned, Subject::Quit, text, Vec::new(), window, cx);
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            let started = Instant::now();
            while this.update(cx, |this, cx| this.must_finish(cx)).unwrap_or(false) {
                if started.elapsed() > QUIT_WAIT {
                    tracing::error!("quitting with files still being written");
                    break;
                }
                cx.background_executor().timer(QUIT_POLL).await;
            }
            let saving = this.update(cx, |this, cx| this.paint.read(cx).start_save());
            let saved = match saving {
                Ok(saving) => {
                    let started = Instant::now();
                    let mut said = false;
                    loop {
                        match saving.try_recv() {
                            Ok(saved) => break saved,
                            Err(TryRecvError::Disconnected) => break false,
                            Err(TryRecvError::Empty) => {}
                        }
                        if started.elapsed() > SAVE_ON_QUIT {
                            break false;
                        }
                        if !said && started.elapsed() > SAY_SAVING_AFTER {
                            said = true;
                            let _ = this.update_in(cx, |this, window, cx| {
                                let text = say!(cx, "quit-saving-retouching");
                                this.post(
                                    Kind::Pinned,
                                    Subject::Quit,
                                    text,
                                    Vec::new(),
                                    window,
                                    cx,
                                );
                            });
                        }
                        cx.background_executor().timer(QUIT_POLL).await;
                    }
                }
                Err(_) => false,
            };
            let quit = this.update_in(cx, |this, window, cx| {
                if saved {
                    this.set_quitting(Quitting::Kept);
                    quit_now(cx);
                } else {
                    tracing::error!("the retouching isn't all saved");
                    this.set_quitting(Quitting::Staying);
                    crate::notices::dismiss(&Subject::Quit, window, cx);
                    stay();
                    this.ask_about_unsaved_paint(window, cx);
                }
            });
            if quit.is_err() {
                let _ = cx.update(|_, cx| quit_now(cx));
            }
        });
        self.set_quitting(Quitting::Waiting { _task: task });
    }
}

impl AppView {
    /// What the window keeps on quit, in order: all paint saved (unless
    /// [`Quitting::Kept`]), then the list sent to the store.
    fn keep_on_quit(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.quitting, Quitting::Kept) {
            self.paint.read(cx).save();
        }
        self.write_list_now(cx);
    }
}

/// The main window's view, for the quit's order ([`on_quit`]).
struct MainView(gpui::WeakEntity<AppView>);

impl gpui::Global for MainView {}

/// `view` is the main window's: quitting keeps what it holds.
pub(crate) fn set_main_view(view: &gpui::Entity<AppView>, cx: &mut App) {
    cx.set_global(MainView(view.downgrade()));
}

/// The app's one quit hook, so its steps run in order whatever else
/// registers one: the main window's paint and list, then `exit`, which
/// waits for the store and ends the process
/// (`deptheos_app::engine::work::exit`). Register once.
pub(crate) fn on_quit(cx: &mut App, exit: impl FnOnce() + 'static) {
    let mut exit = Some(exit);
    cx.on_app_quit(move |cx| {
        tracing::debug!("quit: keeping the paint and the list");
        let main = cx.try_global::<MainView>().and_then(|main| main.0.upgrade());
        if let Some(view) = main {
            view.update(cx, |view, cx| view.keep_on_quit(cx));
        }
        if let Some(exit) = exit.take() {
            exit();
        }
        async {}
    })
    .detach();
}

/// Set once quitting is decided, so the system's question that follows
/// (`cx.quit()` is a `terminate:` on macOS) isn't asked again.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// The main window's [`Quitting`] is `Waiting`, for AppKit's question outside
/// GPUI. Set only by [`AppView::set_quitting`].
static WAITING: AtomicBool = AtomicBool::new(false);

/// Quit, everything asked: cancel what can be abandoned
/// ([`crate::work::root`]); `deptheos_app::engine::work::exit` is the last step.
pub(crate) fn quit_now(cx: &mut App) {
    QUITTING.store(true, Ordering::SeqCst);
    abandon();
    #[cfg(target_os = "macos")]
    if macos::answer(true) {
        return;
    }
    cx.quit();
}

/// Stop abandonable work (previews, thumbnails, listings); not in tests,
/// which share the root token.
fn abandon() {
    #[cfg(not(test))]
    crate::work::root().cancel();
}

/// The quit waited on is off: a system quit waiting on it is told no.
fn stay() {
    #[cfg(target_os = "macos")]
    macos::answer(false);
}

/// Ask `may_quit` (true: quit now; false: it asked or waits, and quits
/// itself) before the system quits. GPUI's delegate answers only
/// `applicationWillTerminate:`, too late to ask, so
/// `applicationShouldTerminate:` is added to its class. Call once, after
/// GPUI sets its delegate.
#[cfg(target_os = "macos")]
pub(crate) fn guard_system_quit(may_quit: impl Fn() -> bool + 'static) {
    macos::install(Box::new(may_quit));
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::Ordering;

    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{class, msg_send, sel};

    use super::{QUITTING, WAITING};

    /// `NSApplicationTerminateReply`.
    const TERMINATE_CANCEL: usize = 0;
    const TERMINATE_NOW: usize = 1;
    const TERMINATE_LATER: usize = 2;

    thread_local! {
        /// The question, on the main thread, where AppKit asks it.
        static MAY_QUIT: RefCell<Option<Box<dyn Fn() -> bool>>> = RefCell::new(None);
        /// The system's quit was answered "later" and waits on [`answer`].
        static OWED: Cell<bool> = const { Cell::new(false) };
    }

    /// Answer a system quit answered "later", if one waits; whether one did.
    pub(super) fn answer(quit: bool) -> bool {
        if !OWED.with(|owed| owed.replace(false)) {
            return false;
        }
        tracing::debug!(quit, "answered the system's quit");
        // SAFETY: on the main thread, as every quit step is; the method
        // takes one BOOL.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, replyToApplicationShouldTerminate: quit];
        }
        true
    }

    pub(super) fn install(may_quit: Box<dyn Fn() -> bool>) {
        MAY_QUIT.with(|q| *q.borrow_mut() = Some(may_quit));
        let Some(class) = AnyClass::get(c"GPUIApplicationDelegate") else {
            tracing::warn!("no GPUI app delegate: the system's quit is not confirmed");
            return;
        };
        let reply: extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> usize =
            should_terminate;
        // SAFETY: the method's types are "NSUInteger (self, _cmd, id)", the
        // encoding given; the class is GPUI's, registered at its start, and
        // AppKit calls the method on the main thread.
        unsafe {
            let imp: Imp = std::mem::transmute(reply);
            let class = class as *const AnyClass as *mut AnyClass;
            let added = objc2::ffi::class_addMethod(
                class,
                sel!(applicationShouldTerminate:),
                imp,
                c"Q@:@".as_ptr(),
            );
            if !added.as_bool() {
                tracing::warn!("the app delegate already answers applicationShouldTerminate:");
            }
            // AppKit may cache the delegate's answers when set: set it again.
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let delegate: *mut AnyObject = msg_send![app, delegate];
            let _: () = msg_send![app, setDelegate: delegate];
        }
    }

    /// `-applicationShouldTerminate:`: now when decided or allowed; later
    /// while waiting on files and retouching ([`answer`]); else canceled,
    /// the app quitting itself on a yes.
    extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
        if QUITTING.load(Ordering::SeqCst) {
            return TERMINATE_NOW;
        }
        let ask = || MAY_QUIT.with(|q| q.borrow().as_ref().is_none_or(|may_quit| may_quit()));
        // A panic must not unwind into AppKit; quitting is then the way out.
        let quit = std::panic::catch_unwind(ask).unwrap_or(true);
        tracing::debug!(quit, "the system asked to quit");
        if quit {
            QUITTING.store(true, Ordering::SeqCst);
            TERMINATE_NOW
        } else if WAITING.load(Ordering::SeqCst) {
            OWED.with(|owed| owed.set(true));
            TERMINATE_LATER
        } else {
            TERMINATE_CANCEL
        }
    }
}
