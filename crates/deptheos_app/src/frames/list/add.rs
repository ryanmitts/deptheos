//! What adding listed images to a stack comes to (ADR-144): which are new,
//! whether to offer New Stack Instead, unreadable folders, and whether
//! they can be added at all. A pure function run on the UI thread when a
//! listing lands ([`List::expand`] runs off it), by both apps.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::frames::list::{Expanded, List};
use crate::frames::names::file_label;
use crate::library::history::StackId;
use crate::words::explain::{Cause, Explain};
use crate::words::{Words, say};

/// Where added images came from: the window, or the system (opened with
/// the app, dropped on its Dock icon; desktop only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddedFrom {
    Window,
    Outside,
}

/// The stack on screen when a listing lands.
#[derive(Clone, Copy, Debug)]
pub struct OnScreen<'a> {
    pub stack: &'a StackId,
    /// A stack, batch or sequence runs: its list doesn't change.
    pub running: bool,
    pub list: &'a List,
    pub has_renders: bool,
}

/// Why listed images aren't added: they go only to the stack they were
/// listed for, and never into a running one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Another stack went on screen while they were listed.
    OtherStack,
    /// A stack started while they were listed.
    Stacking,
}

impl Refused {
    /// Its notice.
    pub fn text(self, words: &Words) -> String {
        match self {
            Self::OtherStack => say!(words, "add-refused-other-stack"),
            Self::Stacking => say!(words, "add-refused-stacking"),
        }
    }
}

/// What the listing comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing is added, and the notice says why.
    Refused(Refused),
    /// Every image is in the list already, or none is an image.
    Nothing,
    /// `files` are new, in listed order; `offer` shows New Stack Instead.
    Add { files: Vec<PathBuf>, offer: bool },
}

/// The outcome, plus unreadable folders (each reported regardless).
#[derive(Debug)]
pub struct Plan {
    pub outcome: Outcome,
    pub unreadable: Vec<(PathBuf, std::io::Error)>,
}

/// What adding `read`, listed for `into`, comes to with `now` on screen.
///
/// Lifecycle: none of its own; derived once when a listing lands
/// (ADR-144), by the desktop's `land` (`deptheos_gpui/src/app/list.rs`)
/// and iOS's `add_plan` in `AppModel.addPaths`.
///
/// - No image: nothing, whatever is on screen.
/// - Another stack on screen, or a running one: refused, with a notice.
/// - New Stack Instead is offered when the list had frames, or the system
///   handed them into a stack with renders; never into an empty stack.
pub fn plan(read: Expanded, into: &StackId, now: OnScreen, from: AddedFrom) -> Plan {
    let Expanded { files, unreadable } = read;
    let outcome = if files.is_empty() {
        Outcome::Nothing
    } else if now.stack != into {
        Outcome::Refused(Refused::OtherStack)
    } else if now.running {
        Outcome::Refused(Refused::Stacking)
    } else {
        let mut listed: BTreeSet<&PathBuf> = now.list.files.iter().collect();
        let new: Vec<PathBuf> = files.iter().filter(|f| listed.insert(f)).cloned().collect();
        let had_frames = !now.list.is_empty();
        match new.is_empty() {
            true => Outcome::Nothing,
            false => Outcome::Add {
                files: new,
                offer: had_frames || (from == AddedFrom::Outside && now.has_renders),
            },
        }
    };
    tracing::debug!("add plan: {outcome:?}, {} folders unreadable", unreadable.len());
    Plan { outcome, unreadable }
}

/// "No frames were added…": [`Outcome::Nothing`]'s notice.
pub fn nothing_text(words: &Words) -> String {
    say!(words, "add-nothing")
}

/// "Added 12 frames to canon1.": the New Stack Instead notice; `frames`
/// counts shots.
pub fn added_text(frames: usize, stack: &str, words: &Words) -> String {
    say!(words, "add-added", n = frames, name = stack)
}

/// The first clause for an unreadable `folder`: permission denied, or
/// unreadable. The front end words the reason after it.
pub fn unreadable_lead(folder: &std::path::Path, error: &std::io::Error, words: &Words) -> String {
    let folder = file_label(folder);
    match error.explain(words).cause {
        Cause::NotAllowed => say!(words, "add-folder-not-allowed", folder = folder),
        Cause::Other | Cause::NotFound => say!(words, "add-folder-unreadable", folder = folder),
    }
}

#[cfg(test)]
mod tests;
