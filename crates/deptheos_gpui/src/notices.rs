//! The desktop's notices and the errors they say. Every notice goes through
//! [`post`]: its [`Kind`] sets how long it stays, a newer one about the same
//! [`Subject`] replaces it, and its buttons are [`Action`]s.
//!
//! Errors follow docs/terms-and-usage.md (section 4): the caller's lead,
//! then the reason and advice `deptheos_app::words::explain` words. A fault
//! shows the fixed reason and where its details are: Copy Details, or Help ▸
//! Save Diagnostics… offered with Show Log Folder ([`fault_actions`]). The
//! reason and advice are in the app's [`Words`] (`crate::words`), the rest
//! in the desktop's own.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use deptheos_app::library::history::StackId;
use deptheos_app::words::explain::fault as fault_reason;
use deptheos_app::{Explain, Explanation, ExplanationCause, ExplanationKind, Words};
use gpui::{
    App, Global, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window, WindowId, div,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::{Sizable as _, WindowExt as _, h_flex};

use crate::words::{DesktopWords, say};

/// The desktop's [`Explanation`] of `error`, its details logged.
pub(crate) fn explain(error: &dyn Explain, words: &Words) -> Explanation {
    let explained = error.explain(words);
    if explained.kind != ExplanationKind::Canceled {
        tracing::error!("{}", explained.details);
    }
    explained
}

/// Whether the message is shown with a Copy Details button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Details {
    /// It has one: a fault asks for them to be sent.
    Copy,
    /// None: a fault names Save Diagnostics…, which its notice offers.
    Diagnostics,
}

/// A notice's text, and whether it names a fault so the notice offers its
/// details ([`fault_actions`]); rewording keeps that ([`Worded::reworded`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Worded {
    pub(crate) text: String,
    pub(crate) fault: bool,
}

impl Worded {
    /// Text that names no fault.
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), fault: false }
    }

    /// Text that names a fault.
    pub(crate) fn of_fault(text: impl Into<String>) -> Self {
        Self { text: text.into(), fault: true }
    }

    /// `f`'s text around this one's, a fault if this one is.
    pub(crate) fn reworded(self, f: impl FnOnce(&str) -> String) -> Self {
        Self { text: f(&self.text), fault: self.fault }
    }
}

impl From<String> for Worded {
    fn from(text: String) -> Self {
        Self::plain(text)
    }
}

impl From<&str> for Worded {
    fn from(text: &str) -> Self {
        Self::plain(text)
    }
}

/// The paint worker's failures, worded on its thread.
impl From<deptheos_app::paint::Failure> for Worded {
    fn from(failed: deptheos_app::paint::Failure) -> Self {
        Self { text: failed.text, fault: failed.fault }
    }
}

impl std::fmt::Display for Worded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// `lead`, then `explained`'s reason and advice; for a fault, the fixed
/// reason and how to report it.
pub(crate) fn sentences(
    w: &DesktopWords,
    lead: Option<&str>,
    explained: &Explanation,
    details: Details,
) -> Worded {
    let body = match explained.kind {
        ExplanationKind::User if explained.cause == ExplanationCause::NotAllowed => {
            let said = explained.text(w.shared());
            deptheos_app::words::sentences(w.shared(), [said.as_str(), &not_allowed_advice(w)])
        }
        ExplanationKind::User | ExplanationKind::Canceled => explained.text(w.shared()),
        ExplanationKind::Fault => {
            let reason = explained.reason.as_str();
            match details {
                Details::Copy => say!(w, "error-fault-copy", reason = reason),
                Details::Diagnostics => say!(
                    w,
                    "error-fault-diagnostics",
                    reason = reason,
                    address = crate::app::diagnostics::SUPPORT_ADDRESS
                ),
            }
        }
    };
    let text = match lead {
        Some(lead) => say!(w, "error-with-lead", lead = lead, why = body),
        None => body,
    };
    Worded { text, fault: explained.kind == ExplanationKind::Fault }
}

/// Advice when the system wouldn't let Deptheos open something: on macOS,
/// where privacy settings allow it.
pub(crate) fn not_allowed_advice(w: &DesktopWords) -> String {
    if cfg!(target_os = "macos") {
        say!(w, "error-not-allowed-advice-macos")
    } else {
        say!(w, "error-not-allowed-advice")
    }
}

/// `lead`, then why `error` happened, for a notice with no Copy Details.
pub(crate) fn notice(w: &DesktopWords, lead: &str, error: &dyn Explain) -> Worded {
    sentences(w, Some(lead), &explain(error, w.shared()), Details::Diagnostics)
}

/// `lead` and the fixed fault sentence, for a worker that panicked or never
/// ran (`details` logged), with no Copy Details.
pub(crate) fn fault(w: &DesktopWords, lead: &str, details: &str) -> Worded {
    tracing::error!("{lead} {details}");
    fault_sentence(w).reworded(|why| say!(w, "error-with-lead", lead = lead, why = why))
}

/// The fixed fault sentence and where the details are, without Copy Details.
pub(crate) fn fault_sentence(w: &DesktopWords) -> Worded {
    Worded::of_fault(say!(
        w,
        "error-fault-diagnostics",
        reason = fault_reason(w.shared()),
        address = crate::app::diagnostics::SUPPORT_ADDRESS
    ))
}

/// A [`crate::work::run`] result as its value, or `lead` and why it failed,
/// without Copy Details.
pub(crate) fn worked<T, E: Explain>(
    w: &DesktopWords,
    lead: &str,
    ran: Result<Result<T, E>, String>,
) -> Result<T, Worded> {
    match ran {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(notice(w, lead, &e)),
        Err(details) => Err(fault(w, lead, &details)),
    }
}

// Notices.

/// A fault notice's buttons: Save Diagnostics… (`save`, run by the app view)
/// and Show Log Folder.
pub(crate) fn fault_actions(
    w: &DesktopWords,
    save: impl Fn(&mut Window, &mut App) + 'static,
) -> Vec<Action> {
    vec![
        Action::new(say!(w, "error-save-diagnostics"), save),
        Action::new(say!(w, "error-show-log-folder"), |_, cx| {
            crate::app::diagnostics::show_log_folder(cx)
        }),
    ]
}

/// How long an [`Kind::Info`] notice stays.
const INFO_STAYS: Duration = Duration::from_secs(8);

/// What a notice says about how it went, and so how long it stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Something finished as asked: hides by itself (the kit's 5 s).
    Done,
    /// Worth knowing, or an offer: hides after [`INFO_STAYS`] on its own
    /// timer (the kit's auto-hide is a fixed 5 s), which hover doesn't pause.
    Info,
    /// Worth knowing and staying until closed: an offer whose button must be
    /// reachable (an update), or a lasting state (quitting after writes).
    Pinned,
    /// It happened, but not quite as asked: stays until closed.
    Warning,
    /// It didn't happen: stays until closed.
    Error,
}

/// What a notice is about: a newer one on the same subject replaces it;
/// [`Subject::Alone`] is never replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Subject {
    /// Nothing another notice would update.
    Alone,
    /// One stack: its run, its frames, its renders.
    Stack(StackId),
    /// An export.
    Export,
    /// A batch.
    Batch,
    /// A turntable or grow sequence.
    Sequence,
    /// The update check.
    Update,
    /// A delete into Recently Deleted; a newer one's Undo notice replaces it.
    Deleted,
    /// A quit waiting on what is being saved.
    Quit,
}

impl Subject {
    /// The kit's id for the notice; `serial` keeps [`Subject::Alone`] unique.
    fn key(&self, serial: u64) -> SharedString {
        match self {
            Self::Alone => format!("alone-{serial}").into(),
            Self::Stack(id) => format!("stack-{}", id.as_str()).into(),
            Self::Export => "export".into(),
            Self::Batch => "batch".into(),
            Self::Sequence => "sequence".into(),
            Self::Update => "update".into(),
            Self::Deleted => "deleted".into(),
            Self::Quit => "quit".into(),
        }
    }
}

/// What a notice's button does.
type Run = Rc<dyn Fn(&mut Window, &mut App)>;

/// A notice button: label and action. Never primary; a click closes the notice.
#[derive(Clone)]
pub(crate) struct Action {
    label: SharedString,
    run: Run,
}

impl Action {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self { label: label.into(), run: Rc::new(run) }
    }
}

/// Show in Finder (or the file manager) on a notice, showing `folder`.
pub(crate) fn show_folder(w: &DesktopWords, folder: std::path::PathBuf) -> Action {
    Action::new(crate::renders::reveal_label(w), move |_, _| crate::renders::reveal(&folder))
}

/// The kit's id type for every notice [`post`] shows.
struct Notice;

/// The latest notice per window and subject, so an [`Kind::Info`] timer
/// closes it only while still shown.
#[derive(Default)]
struct Posted {
    serial: u64,
    latest: HashMap<(WindowId, SharedString), u64>,
    /// What each window's last notice said, for the tests.
    #[cfg(test)]
    said: HashMap<WindowId, String>,
}

impl Global for Posted {}

/// Show `text` in `window` as a `kind` notice about `subject` with `actions`,
/// replacing the window's notice on that subject.
pub(crate) fn post(
    kind: Kind,
    subject: Subject,
    text: impl Into<SharedString>,
    actions: Vec<Action>,
    window: &mut Window,
    cx: &mut App,
) {
    post_titled(kind, subject, None, text, actions, window, cx);
}

/// [`post`], with a `title` over the text.
pub(crate) fn post_titled(
    kind: Kind,
    subject: Subject,
    title: Option<SharedString>,
    text: impl Into<SharedString>,
    actions: Vec<Action>,
    window: &mut Window,
    cx: &mut App,
) {
    let posted = cx.default_global::<Posted>();
    posted.serial += 1;
    let serial = posted.serial;
    let key = subject.key(serial);
    let window_id = window.window_handle().window_id();
    posted.latest.insert((window_id, key.clone()), serial);
    let text: SharedString = text.into();
    #[cfg(test)]
    posted.said.insert(window_id, text.to_string());
    tracing::debug!("notice {kind:?} about {subject:?}");

    let kind_of = match kind {
        Kind::Done => NotificationType::Success,
        Kind::Info | Kind::Pinned => NotificationType::Info,
        Kind::Warning => NotificationType::Warning,
        Kind::Error => NotificationType::Error,
    };
    // The kit's `action` has one slot and disables auto-hide, so buttons are
    // drawn as content and auto-hide is set here.
    let mut notice = Notification::new()
        .message(text)
        .with_type(kind_of)
        .id1::<Notice>(key.clone())
        .autohide(kind == Kind::Done);
    if let Some(title) = title {
        notice = notice.title(title);
    }
    if !actions.is_empty() {
        notice = notice.content(move |_, _, cx| {
            h_flex()
                .mt_2()
                .gap_2()
                .flex_wrap()
                .children(actions.iter().enumerate().map(|(ix, action)| {
                    let run = action.run.clone();
                    let button = Button::new(("notice-action", ix))
                        .label(action.label.clone())
                        .small()
                        .on_click(cx.listener(move |notice, _, window, cx| {
                            notice.dismiss(window, cx);
                            // After this click's update, so the action may
                            // close or post notices itself.
                            let run = run.clone();
                            window.defer(cx, move |window, cx| run(window, cx));
                        }));
                    div().debug_selector(move || format!("notice-action-{ix}")).child(button)
                }))
                .into_any_element()
        });
    }
    window.push_notification(notice, cx);

    if kind == Kind::Info {
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(INFO_STAYS).await;
                cx.update(|window, cx| {
                    let current = cx
                        .try_global::<Posted>()
                        .and_then(|p| p.latest.get(&(window_id, key.clone())))
                        == Some(&serial);
                    if current {
                        window.remove_notification1::<Notice>(key.clone(), cx);
                    }
                })
                .ok();
            })
            .detach();
    }
}

/// What the last notice posted in `window` said.
#[cfg(test)]
pub(crate) fn last_said(window: &Window, cx: &App) -> Option<String> {
    let said = &cx.try_global::<Posted>()?.said;
    said.get(&window.window_handle().window_id()).cloned()
}

/// Close `window`'s notice about `subject`, with nothing in its place; a
/// [`Subject::Alone`] notice can't be named, so none closes.
pub(crate) fn dismiss(subject: &Subject, window: &mut Window, cx: &mut App) {
    if *subject == Subject::Alone {
        return;
    }
    let key = subject.key(0);
    let window_id = window.window_handle().window_id();
    if cx.default_global::<Posted>().latest.remove(&(window_id, key.clone())).is_some() {
        tracing::debug!("notice about {subject:?} closed");
        window.remove_notification1::<Notice>(key, cx);
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn words() -> DesktopWords {
        DesktopWords::english()
    }

    #[test]
    fn a_reason_follows_the_lead_as_its_own_sentence() {
        let e = explain(&deptheos_app::Error::NoFrames, words().shared());
        assert_eq!(
            sentences(&words(), Some("The stack didn't finish."), &e, Details::Diagnostics).text,
            "The stack didn't finish. There are no frames to stack. Add frames, or include the \
             excluded ones."
        );
    }

    #[test]
    fn a_fault_says_where_its_details_are() {
        let e = explain(
            &deptheos_app::Error::InvalidState("no result texture".into()),
            words().shared(),
        );
        assert_eq!(
            sentences(&words(), None, &e, Details::Copy).text,
            "Deptheos ran into an unexpected problem and stopped. Click Copy Details, then choose \
             Help ▸ Report a Problem… and paste them into the report."
        );
        assert_eq!(
            notice(
                &words(),
                "The render couldn't be exported.",
                &deptheos_app::Error::InvalidState("x".into())
            )
            .text,
            "The render couldn't be exported. Deptheos ran into an unexpected problem and \
             stopped. Help ▸ Save Diagnostics… gathers the details to email to support@deptheos.com."
        );
        assert_eq!(e.details, "no result texture");
        let w = words();
        assert!(fault(&w, "The render couldn't be exported.", "x").fault);
        assert!(sentences(&w, None, &e, Details::Copy).fault);
        assert!(!notice(&w, "The stack didn't finish.", &deptheos_app::Error::NoFrames).fault);
    }

    mod notices {
        use std::time::Duration;

        use gpui::{AnyWindowHandle, AppContext as _, Render, TestAppContext, Window, div};
        use gpui_kit::component::WindowExt as _;

        use super::super::{Action, Kind, Subject, post};
        use crate::test_support::open;

        struct Blank;

        impl Render for Blank {
            fn render(
                &mut self,
                _: &mut Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                div()
            }
        }

        fn window(cx: &mut TestAppContext) -> AnyWindowHandle {
            open(cx, |_, cx| cx.new(|_| Blank)).0
        }

        fn post_in(
            cx: &mut TestAppContext,
            window: AnyWindowHandle,
            kind: Kind,
            subject: Subject,
            actions: Vec<Action>,
        ) {
            cx.update_window(window, |_, window, cx| {
                post(kind, subject, "Note", actions, window, cx)
            })
            .unwrap();
            cx.run_until_parked();
        }

        fn shown(cx: &mut TestAppContext, window: AnyWindowHandle) -> usize {
            cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
        }

        /// Let `seconds` pass, and a closing notice's exit with them.
        fn wait(cx: &mut TestAppContext, seconds: f32) {
            cx.background_executor.advance_clock(Duration::from_secs_f32(seconds));
            cx.run_until_parked();
            cx.background_executor.advance_clock(Duration::from_millis(250));
            cx.run_until_parked();
        }

        fn stack() -> Subject {
            Subject::Stack(deptheos_app::library::history::StackId::new())
        }

        /// Done hides by itself, even with buttons; Info stays about 8 s;
        /// pinned notices, warnings and errors stay until closed.
        #[gpui::test]
        fn a_notice_stays_as_long_as_its_kind_says(cx: &mut TestAppContext) {
            let window = window(cx);
            post_in(cx, window, Kind::Done, Subject::Alone, Vec::new());
            post_in(cx, window, Kind::Done, Subject::Export, vec![Action::new("Show", |_, _| {})]);
            post_in(cx, window, Kind::Info, Subject::Alone, Vec::new());
            post_in(cx, window, Kind::Pinned, Subject::Update, Vec::new());
            post_in(cx, window, Kind::Warning, Subject::Alone, Vec::new());
            post_in(cx, window, Kind::Error, Subject::Alone, Vec::new());
            assert_eq!(shown(cx, window), 6);
            wait(cx, 6.0);
            assert_eq!(shown(cx, window), 4, "both Done notices hid");
            wait(cx, 2.5);
            assert_eq!(shown(cx, window), 3, "the Info notice hid");
            wait(cx, 60.0);
            assert_eq!(shown(cx, window), 3, "a pinned notice, a warning and an error stay");
        }

        /// A button runs its action and closes the notice; the action may
        /// post its own on the same subject.
        #[gpui::test]
        fn a_button_runs_its_action_and_closes_the_notice(cx: &mut TestAppContext) {
            let window = window(cx);
            let ran = std::rc::Rc::new(std::cell::Cell::new(0));
            let (first, second) = (ran.clone(), ran.clone());
            let actions = vec![
                Action::new("Show", move |_, _| first.set(first.get() + 1)),
                Action::new("Again", move |window, cx| {
                    second.set(second.get() + 10);
                    post(Kind::Error, Subject::Export, "Again", Vec::new(), window, cx);
                }),
            ];
            post_in(cx, window, Kind::Error, Subject::Export, actions);
            let mut vcx = gpui::VisualTestContext::from_window(window, cx);
            let again = vcx.debug_bounds("notice-action-1").expect("the second button is drawn");
            assert!(vcx.debug_bounds("notice-action-0").is_some());
            vcx.simulate_click(again.center(), gpui::Modifiers::none());
            wait(cx, 1.0);
            assert_eq!(ran.get(), 10, "only the clicked button's action ran");
            assert_eq!(shown(cx, window), 1, "the action's notice replaced the clicked one");
        }

        /// A newer notice on a subject replaces the older; an Alone one never is.
        #[gpui::test]
        fn a_newer_notice_replaces_one_on_the_same_subject(cx: &mut TestAppContext) {
            let window = window(cx);
            let bee = stack();
            post_in(cx, window, Kind::Error, bee.clone(), Vec::new());
            post_in(cx, window, Kind::Warning, bee, Vec::new());
            assert_eq!(shown(cx, window), 1);
            post_in(cx, window, Kind::Error, stack(), Vec::new());
            post_in(cx, window, Kind::Error, Subject::Batch, Vec::new());
            assert_eq!(shown(cx, window), 3);
            post_in(cx, window, Kind::Error, Subject::Alone, Vec::new());
            post_in(cx, window, Kind::Error, Subject::Alone, Vec::new());
            assert_eq!(shown(cx, window), 5);
        }

        /// An Info timer closes only its own notice: a replacement keeps its
        /// time, an error replacing it stays.
        #[gpui::test]
        fn an_info_timer_spares_the_notice_that_replaced_it(cx: &mut TestAppContext) {
            let window = window(cx);
            post_in(cx, window, Kind::Info, Subject::Batch, Vec::new());
            wait(cx, 5.0);
            post_in(cx, window, Kind::Info, Subject::Batch, Vec::new());
            wait(cx, 4.0);
            assert_eq!(shown(cx, window), 1, "the first's timer left the second");
            wait(cx, 4.5);
            assert_eq!(shown(cx, window), 0, "the second hid on its own time");

            post_in(cx, window, Kind::Info, Subject::Export, Vec::new());
            post_in(cx, window, Kind::Error, Subject::Export, Vec::new());
            wait(cx, 20.0);
            assert_eq!(shown(cx, window), 1, "the error stays");
        }
    }
}
