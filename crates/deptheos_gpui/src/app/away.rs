//! A run ending while the window is behind: a system notification says so
//! and the window asks for attention (Dock bounce, taskbar flash); clicking
//! the notification brings the window forward ([`answer_clicks`]).
//!
//! macOS shows these only for a trusted app bundle, asking once; a build
//! from `target/` posts nothing. Windows needs the app's identity (`main.rs`).

use gpui::{App, SystemNotification, Window};

use crate::words::{DesktopWords, Wording as _, say};

/// The tag every run's notification carries: a newer one replaces it.
const TAG: &str = "run-end";

/// How a run ended, for its system notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunEnd {
    /// A stack finished.
    Stacked,
    /// A stack didn't finish; the number its render would have had.
    StackFailed(Option<u32>),
    /// A batch closed, however it went, unless canceled.
    Batch,
    /// A grow sequence was written.
    Sequence,
    /// A grow sequence didn't finish.
    SequenceFailed,
    /// The user canceled it: they are there.
    Canceled,
}

/// The notification's title for `end`, when the user should hear of it:
/// only while the window is not active, and never for a cancel.
pub(crate) fn away_title(w: &DesktopWords, window_active: bool, end: RunEnd) -> Option<String> {
    if window_active {
        return None;
    }
    Some(match end {
        RunEnd::Stacked => say!(w, "away-stacked"),
        RunEnd::StackFailed(Some(number)) => {
            say!(w, "run-failure-title-numbered", number = number.to_string())
        }
        RunEnd::StackFailed(None) => say!(w, "run-failure-title"),
        RunEnd::Batch => say!(w, "away-batch"),
        RunEnd::Sequence => say!(w, "away-sequence"),
        RunEnd::SequenceFailed => say!(w, "away-sequence-failed"),
        RunEnd::Canceled => return None,
    })
}

/// If the window is behind, tell the user the run ended as `end` says,
/// with `body`, and ask for attention.
pub(crate) fn tell(end: RunEnd, body: &str, window: &mut Window, cx: &mut App) {
    let Some(title) = away_title(cx.wording(), window.is_window_active(), end) else { return };
    tracing::debug!(?end, "system notification");
    cx.show_system_notification(SystemNotification {
        tag: TAG.into(),
        title: title.into(),
        body: body.to_owned().into(),
        actions: Vec::new(),
    });
    window.request_attention();
}

/// A click on a run's notification brings the app and its window forward;
/// it does nothing else.
pub(crate) fn answer_clicks(cx: &mut App) {
    cx.on_system_notification_response(|response, cx| {
        if response.tag.as_ref() != TAG {
            return;
        }
        cx.activate(true);
        if let Some(window) = cx.windows().into_iter().next() {
            window.update(cx, |_, window, _| window.activate_window()).ok();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{RunEnd, away_title};
    use crate::words::DesktopWords;

    /// Only an end the user didn't see is told: the window in front hears
    /// nothing, and neither does a cancel.
    #[test]
    fn only_an_end_out_of_sight_is_told() {
        let w = DesktopWords::english();
        for end in [RunEnd::Stacked, RunEnd::Batch, RunEnd::Sequence, RunEnd::Canceled] {
            assert_eq!(away_title(&w, true, end), None, "{end:?} with the window in front");
        }
        assert_eq!(away_title(&w, false, RunEnd::Canceled), None);
        assert_eq!(away_title(&w, false, RunEnd::Stacked).as_deref(), Some("Stack finished"));
        assert_eq!(
            away_title(&w, false, RunEnd::StackFailed(Some(5))).as_deref(),
            Some("Stack #5 didn't finish")
        );
        assert_eq!(away_title(&w, false, RunEnd::Batch).as_deref(), Some("Batch finished"));
        assert_eq!(away_title(&w, false, RunEnd::Sequence).as_deref(), Some("Sequence written"));
        assert_eq!(
            away_title(&w, false, RunEnd::SequenceFailed).as_deref(),
            Some("The sequence didn't finish")
        );
    }
}
