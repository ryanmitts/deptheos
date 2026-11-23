//! Stacks: the working version, what it shows against the render shown,
//! and a render's setup.

use std::path::PathBuf;

use deptheos_app::Words as AppWords;
use deptheos_app::frames::list::List;
use deptheos_app::library::history::StackId;
use deptheos_app::library::stacks::{self, SetupPart, Working};

use crate::{BadId, ListInfo, StackSettings, render_id, stack_id};

/// A new stack id: Clear, Add… to an empty list, a burst, and each batch
/// stack start one.
#[uniffi::export]
pub fn new_stack_id() -> String {
    StackId::new().as_str().to_string()
}

/// A stack's working version (`deptheos_app::library::stacks::Working`):
/// the list and settings being edited, and their base. The app keeps the
/// one on screen, written with [`Engine::keep_working`].
#[derive(Clone, Debug, uniffi::Record)]
pub struct WorkingInfo {
    pub stack: String,
    /// None before the stack's first render.
    pub base: Option<String>,
    pub list: ListInfo,
    /// None for a list kept before settings were.
    pub settings: Option<StackSettings>,
}

impl From<&Working> for WorkingInfo {
    fn from(working: &Working) -> Self {
        Self {
            stack: working.stack.as_str().to_string(),
            base: working.base.as_ref().map(|b| b.as_str().to_string()),
            list: ListInfo::from(&working.list),
            settings: working.settings.as_ref().map(|s| StackSettings::of(&s.config, &s.options)),
        }
    }
}

impl TryFrom<WorkingInfo> for Working {
    type Error = BadId;

    fn try_from(working: WorkingInfo) -> std::result::Result<Self, BadId> {
        Ok(Self {
            stack: stack_id(&working.stack)?,
            base: working.base.as_deref().map(render_id).transpose()?,
            list: List::from(working.list),
            settings: working
                .settings
                .map(|s| stacks::Settings { config: s.algorithm.config(), options: s.options() }),
        })
    }
}

/// Frames kept and no render yet: "Not stacked yet" in the Library.
/// [`Engine::switch_stack`] to it brings them back.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UnstackedInfo {
    pub stack: String,
    /// Its given name, else its frames' folder; `None` with neither, which
    /// the app words ("Untitled stack").
    pub name: Option<String>,
    /// A RAW and JPEG pair is one.
    pub frames: u32,
}

/// Which render is on screen, against the working version
/// (`deptheos_app::library::stacks::Showing`): none, the base, or another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ShowingKind {
    Nothing,
    Base,
    Other,
}

/// How the working version stands against the render on screen, worded by
/// the engine ([`Engine::showing`]). Derived each ask; the app keeps none.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ShowingInfo {
    pub kind: ShowingKind,
    /// The number of the render on screen.
    pub shown: Option<u32>,
    /// When it is in the history.
    pub base: Option<u32>,
    /// Changed since its base: "• Edited" on the base's tile.
    pub edited: bool,
    /// The line by Revert: "2 changes to the frames since #4", "1
    /// difference from #3 in the settings", "Same frames and settings as
    /// #3". `None` for no render and the unedited base.
    pub change_line: Option<String>,
    /// The base tile's second line while edited: "3 changes".
    pub base_tile_line: Option<String>,
    /// `None` when it isn't offered.
    pub revert: Option<RevertInfo>,
    /// For the log when the render shown changes: "#3 (other, base #4)".
    pub log: String,
}

/// The working version against the render on screen and each of its
/// stack's renders, in one ask ([`Engine::showings`]), instead of a call
/// per tile.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ShowingsInfo {
    pub shown: ShowingInfo,
    /// Against each render asked, as if on screen, by id: what Revert would
    /// do and, for the base, whether edited. Renders gone from the history
    /// are left out.
    pub renders: std::collections::HashMap<String, ShowingInfo>,
}

/// Revert to a render: its frames and settings become the working version,
/// based on it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RevertInfo {
    /// By id.
    pub render: String,
    /// "Revert to #3".
    pub number: u32,
    /// Asked first when unstacked changes would be lost; `None` runs at once.
    pub alert: Option<RevertAlertInfo>,
}

/// "Revert to #3?", with what would be lost.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RevertAlertInfo {
    pub title: String,
    pub body: String,
}

impl ShowingInfo {
    pub(crate) fn of(showing: &stacks::Showing<'_>, words: &AppWords) -> Self {
        Self {
            kind: match showing {
                stacks::Showing::Nothing => ShowingKind::Nothing,
                stacks::Showing::Base { .. } => ShowingKind::Base,
                stacks::Showing::Other { .. } => ShowingKind::Other,
            },
            shown: showing.shown().map(|r| r.number),
            base: showing.base().map(|r| r.number),
            edited: showing.is_edited(),
            change_line: showing.change_line(words),
            base_tile_line: showing.base_tile_line(words),
            revert: showing.revert().map(|revert| RevertInfo {
                render: revert.to.id.as_str().to_string(),
                number: revert.number(),
                alert: revert
                    .alert(words)
                    .map(|alert| RevertAlertInfo { title: alert.title, body: alert.body }),
            }),
            log: showing.to_string(),
        }
    }

    /// No render on screen.
    pub(crate) fn nothing(words: &AppWords) -> Self {
        Self::of(&stacks::Showing::Nothing, words)
    }
}

/// A Render Info section's part of the record
/// (`deptheos_app::library::stacks::SetupPart`).
#[uniffi::remote(Enum)]
pub enum SetupPart {
    /// The method, then each of its sliders.
    Method,
    /// Align Frames, the aligner and the crop.
    Alignment,
    /// The camera RAW decoder: only for a render of RAW frames.
    Raw,
    /// How many were included, their order, then each frame.
    Frames,
    /// The frames the stack couldn't stack, and why.
    LeftOut,
}

/// A section of Render Info: "Method" and its rows.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SetupSectionInfo {
    pub part: SetupPart,
    pub title: String,
    pub rows: Vec<SetupRowInfo>,
}

/// "Smoothness" and "4", with "Working version: 6" under it when it
/// differs.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SetupRowInfo {
    pub label: String,
    /// Empty for an included frame with nothing more to say.
    pub value: String,
    /// The working version differs: the row carries the edited dot.
    pub differs: bool,
    /// The working version's value, worded whole, when it differs.
    pub working: Option<String>,
    /// A Frames or Left out row's frame, for its thumbnail.
    pub file: Option<PathBuf>,
}

impl From<stacks::SetupSection> for SetupSectionInfo {
    fn from(section: stacks::SetupSection) -> Self {
        Self {
            part: section.part,
            title: section.title,
            rows: section
                .rows
                .into_iter()
                .map(|row| SetupRowInfo {
                    differs: row.differs(),
                    label: row.label,
                    value: row.value,
                    working: row.working,
                    file: row.file,
                })
                .collect(),
        }
    }
}
