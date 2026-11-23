//! What a failing call tells the app ([`EngineError`], worded through
//! [`Told`]), and the ids the app passes back.

use deptheos_app::library::history::{RenderId, StackId};
use deptheos_app::words::ios::Line;
use deptheos_app::{Explain, Explanation, ExplanationKind, Platform, Words as AppWords};

/// What a call tells the app went wrong. `Failed`'s message is what the
/// user reads: full sentences in the app's language
/// (`deptheos_app::words::explain` for engine errors); technical text goes
/// to the log.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EngineError {
    #[error("{message}")]
    Failed { message: String },
    #[error("canceled")]
    Canceled,
}

/// What iOS shows for a fault: the fixed sentence and how to report it.
fn fault_message(words: &AppWords) -> String {
    let report = Line::ReportFault.text(words);
    deptheos_app::words::sentences(words, [deptheos_app::words::explain::fault(words), report])
}

/// What iOS shows for `explanation`: its reason and advice, or a fault's
/// fixed message. Details go to the log at error level.
pub(crate) fn shown(explanation: &Explanation, words: &AppWords) -> String {
    match explanation.kind {
        ExplanationKind::Canceled => {
            tracing::debug!("canceled: {}", explanation.details);
            explanation.text(words)
        }
        ExplanationKind::User => {
            tracing::error!("{}", explanation.details);
            explanation.text(words)
        }
        ExplanationKind::Fault => {
            tracing::error!("fault: {}", explanation.details);
            fault_message(words)
        }
    }
}

impl EngineError {
    /// `error` in the user's terms and `words`' language; technical text to
    /// the log.
    pub(crate) fn explained(error: &(impl Explain + ?Sized), words: &AppWords) -> Self {
        let explanation = error.explain(words);
        let message = shown(&explanation, words);
        match explanation.kind {
            ExplanationKind::Canceled => Self::Canceled,
            ExplanationKind::User | ExplanationKind::Fault => Self::Failed { message },
        }
    }

    /// An unexpected state the user can't fix: the fixed message, after
    /// `lead` (what didn't happen) if any. `details` go to the log.
    pub(crate) fn fault(
        words: &AppWords,
        lead: Option<Line>,
        details: impl std::fmt::Display,
    ) -> Self {
        tracing::error!("fault: {details}");
        let message = match lead {
            Some(lead) => {
                deptheos_app::words::sentences(words, [lead.text(words), fault_message(words)])
            }
            None => fault_message(words),
        };
        Self::Failed { message }
    }

    /// `error` explained after `lead`, a sentence saying what didn't happen.
    pub(crate) fn led(lead: Line, error: &(impl Explain + ?Sized), words: &AppWords) -> Self {
        match Self::explained(error, words) {
            Self::Failed { message } => Self::Failed {
                message: deptheos_app::words::sentences(words, [lead.text(words), message]),
            },
            Self::Canceled => Self::Canceled,
        }
    }

    /// Something the user can cause: one of the bridge's own sentences.
    pub(crate) fn user(line: Line, words: &AppWords) -> Self {
        Self::Failed { message: line.text(words) }
    }
}

/// A result whose error the engine explains in the user's terms and
/// language, with `.told(words)?`.
pub(crate) trait Told<T> {
    fn told(self, words: &AppWords) -> Result<T>;
}

impl<T, E: Explain> Told<T> for std::result::Result<T, E> {
    fn told(self, words: &AppWords) -> Result<T> {
        self.map_err(|e| EngineError::explained(&e, words))
    }
}

/// A foreign callback threw something unexpected. Converted with no
/// language to hand, the fault is said in English.
impl From<uniffi::UnexpectedUniFFICallbackError> for EngineError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        let words = AppWords::english(Platform::Ios);
        Self::fault(&words, None, format!("callback failed: {}", e.reason))
    }
}

pub(crate) type Result<T> = std::result::Result<T, EngineError>;

/// An id from the app that doesn't parse: a fault, since the app passes
/// back only ids the engine gave it.
#[derive(Debug, thiserror::Error)]
#[error("not a {what} id: {id}")]
pub struct BadId {
    what: &'static str,
    id: String,
}

impl Explain for BadId {
    fn explain(&self, words: &AppWords) -> Explanation {
        Explanation {
            kind: ExplanationKind::Fault,
            reason: deptheos_app::words::explain::fault(words),
            advice: String::new(),
            details: self.to_string(),
            cause: deptheos_app::ExplanationCause::Other,
        }
    }
}

pub(crate) fn render_id(id: &str) -> std::result::Result<RenderId, BadId> {
    RenderId::parse(id).ok_or_else(|| BadId { what: "render", id: id.into() })
}

/// Every one of `ids` as a render id, or why one isn't.
pub(crate) fn render_ids(ids: &[String]) -> std::result::Result<Vec<RenderId>, BadId> {
    ids.iter().map(|id| render_id(id)).collect()
}

pub(crate) fn deleted_ids(
    ids: &[String],
) -> std::result::Result<Vec<deptheos_app::library::history::DeletedId>, BadId> {
    let id = |id: &String| {
        deptheos_app::library::history::DeletedId::parse(id)
            .ok_or_else(|| BadId { what: "deleted", id: id.clone() })
    };
    ids.iter().map(id).collect()
}

pub(crate) fn stack_id(id: &str) -> std::result::Result<StackId, BadId> {
    StackId::parse(id).ok_or_else(|| BadId { what: "stack", id: id.into() })
}
