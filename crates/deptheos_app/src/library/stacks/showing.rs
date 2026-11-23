//! How the working version stands against the render on screen.

use std::fmt;

use super::{Against, Changes, Compared, Working};
use crate::library::history::Render;
use crate::words::{Words, say};

/// How the working version stands against the render on screen, derived
/// each time ([`Showing::new`]) so it can't disagree with the screen.
/// Picking a render never changes the working version.
///
/// Lifecycle: nothing, its base, another render; derived each time
/// (ADR-142). The render shown is stored: the desktop's
/// `Renders::active`, changed when an `Open` lands; iOS's `result` in
/// `Model/AppModel.swift`, changed only by `setResult`.
///
/// ```text
/// Nothing ──Stack finishes──▶ Base (the new render)
/// Base ──a render picked (tile, title menu, Library row, Swap)──▶ Other
/// Other(r) ──another render picked──▶ Other(s)
/// Other(r) ──the base picked, or r deleted──▶ Base
/// Other(r) ──Revert to #r──▶ Base (r is now the base)
/// Base, edited ──Revert──▶ Base, unedited
/// any ──Stack finishes──▶ Base (the new render)
/// any ──the stack's last render deleted──▶ Nothing
/// ```
///
/// Its [`fmt::Display`] is the debug log line ("#3 (other, base #4)").
#[derive(Clone, Copy, Debug)]
pub enum Showing<'a> {
    /// No render on screen: never stacked, or every render deleted.
    Nothing,
    /// The working version's base. `changes` is empty when unedited.
    Base { base: &'a Render, changes: Changes },
    /// Another render. `differences` is against it; `changes` against
    /// `base`, empty unless the base is known.
    Other { shown: &'a Render, differences: Changes, base: OtherBase<'a>, changes: Changes },
}

/// The working version's base while another render is on screen.
#[derive(Clone, Copy, Debug)]
pub enum OtherBase<'a> {
    /// None: nothing of it has been stacked.
    None,
    /// One the front end doesn't have, so its changes can't be counted.
    Unknown,
    Known(&'a Render),
}

impl<'a> OtherBase<'a> {
    fn render(self) -> Option<&'a Render> {
        match self {
            Self::Known(base) => Some(base),
            Self::None | Self::Unknown => None,
        }
    }
}

/// What Revert would do: make `to` the base, its frames and settings the
/// working version's, losing what `loses` says.
#[derive(Clone, Copy, Debug)]
pub struct Revert<'a> {
    pub to: &'a Render,
    /// What unstacked work would be lost; `None` runs Revert without asking.
    pub loses: Option<Loses>,
}

/// What a Revert would lose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loses {
    /// The working version's changes since its base, render #`since`.
    Changes { changes: Changes, since: u32 },
    /// The working version has no base: none of it was stacked.
    Unstacked,
    /// Its base isn't in the records, so the loss can't be counted.
    Uncounted,
}

/// The confirmation a Revert that would lose something asks first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevertAlert {
    /// "Revert to #3?"
    pub title: String,
    /// "Your 3 changes since #4 haven't been stacked, and will be lost.
    /// Every render is kept."
    pub body: String,
}

impl<'a> Showing<'a> {
    /// How `working` stands against `shown`, with `base` the render
    /// [`Working::base`] names, if the front end has it. `compared` is what
    /// the front end offers.
    pub fn new(
        working: &Working,
        shown: Option<&'a Render>,
        base: Option<&'a Render>,
        compared: Compared,
    ) -> Self {
        let is_base = |render: &Render| working.base.as_ref() == Some(&render.id);
        let Some(shown) = shown else { return Self::Nothing };
        if is_base(shown) {
            return Self::Base { base: shown, changes: working.changes(shown, compared) };
        }
        let base = match base.filter(|b| is_base(b)) {
            Some(base) => OtherBase::Known(base),
            None if working.base.is_none() => OtherBase::None,
            None => OtherBase::Unknown,
        };
        Self::Other {
            shown,
            differences: working.changes(shown, compared),
            base,
            changes: base.render().map_or_else(Changes::default, |b| working.changes(b, compared)),
        }
    }

    /// The render on screen.
    pub fn shown(&self) -> Option<&'a Render> {
        match *self {
            Self::Nothing => None,
            Self::Base { base, .. } => Some(base),
            Self::Other { shown, .. } => Some(shown),
        }
    }

    /// The working version's base, when it is known.
    pub fn base(&self) -> Option<&'a Render> {
        match *self {
            Self::Nothing => None,
            Self::Base { base, .. } => Some(base),
            Self::Other { base, .. } => base.render(),
        }
    }

    /// The working version against its base, what Edited means; empty with
    /// no render or base.
    pub fn changes(&self) -> Changes {
        match *self {
            Self::Nothing => Changes::default(),
            Self::Base { changes, .. } | Self::Other { changes, .. } => changes,
        }
    }

    /// Whether the working version is edited: "• Edited" on its base.
    pub fn is_edited(&self) -> bool {
        self.changes().any()
    }

    /// The line by the working version's controls: "2 changes to the frames
    /// since #4", "1 difference from #3 in the settings", or "Same frames
    /// and settings as #3". `None` for no render or the unedited base.
    pub fn change_line(&self, words: &Words) -> Option<String> {
        match *self {
            Self::Nothing => None,
            Self::Base { base, changes } => changes.in_full(Against::Base, base.number, words),
            Self::Other { shown, differences, .. } => {
                let number = shown.number;
                differences
                    .in_full(Against::Other, number, words)
                    .or_else(|| Some(say!(words, "same-as-render", number = number.to_string())))
            }
        }
    }

    /// The base tile's second line while edited, "3 changes". `None` when
    /// unedited or with no base.
    pub fn base_tile_line(&self, words: &Words) -> Option<String> {
        self.base().and(self.changes().short(words))
    }

    /// What Revert would do. `None` when not offered: no render, the
    /// unedited base, or a render already equal.
    pub fn revert(&self) -> Option<Revert<'a>> {
        match *self {
            Self::Nothing => None,
            Self::Base { base, changes } => changes.any().then_some(Revert {
                to: base,
                loses: Some(Loses::Changes { changes, since: base.number }),
            }),
            Self::Other { shown, differences, base, changes } => {
                let loses = match base {
                    OtherBase::None => Some(Loses::Unstacked),
                    OtherBase::Unknown => Some(Loses::Uncounted),
                    OtherBase::Known(base) => {
                        changes.any().then_some(Loses::Changes { changes, since: base.number })
                    }
                };
                differences.any().then_some(Revert { to: shown, loses })
            }
        }
    }
}

impl Revert<'_> {
    /// The number reverted to.
    pub fn number(&self) -> u32 {
        self.to.number
    }

    /// Its confirmation; `None` when nothing would be lost.
    pub fn alert(&self, words: &Words) -> Option<RevertAlert> {
        let body = match self.loses? {
            Loses::Changes { changes, since } => say!(
                words,
                "revert-loses-changes",
                count = changes.count(),
                base = since.to_string()
            ),
            Loses::Unstacked => say!(words, "revert-loses-unstacked"),
            Loses::Uncounted => say!(words, "revert-loses-uncounted"),
        };
        let title = say!(words, "revert-title", number = self.to.number.to_string());
        Some(RevertAlert { title, body })
    }
}

impl fmt::Display for Showing<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let edited = if self.is_edited() { ", edited" } else { "" };
        match self {
            Self::Nothing => write!(f, "nothing"),
            Self::Base { base, .. } => write!(f, "#{} (base{edited})", base.number),
            Self::Other { shown, base: OtherBase::Known(base), .. } => {
                write!(f, "#{} (other, base #{}{edited})", shown.number, base.number)
            }
            Self::Other { shown, base: OtherBase::None, .. } => {
                write!(f, "#{} (other, no base)", shown.number)
            }
            Self::Other { shown, base: OtherBase::Unknown, .. } => {
                write!(f, "#{} (other, base not in the records)", shown.number)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::stacks::setup::tests::{edit, render};
    use crate::words::explain::Platform;

    fn words() -> Words {
        Words::english(Platform::Desktop)
    }

    fn line(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn nothing_on_screen_says_nothing_and_offers_no_revert() {
        let base = render(4);
        let working = Working::of_render(&base);
        let showing = Showing::new(&working, None, Some(&base), Compared::ALL);
        assert!(matches!(showing, Showing::Nothing));
        assert_eq!(showing.change_line(&words()), None);
        assert!(
            showing.revert().is_none()
                && showing.revert().and_then(|r| r.alert(&words())).is_none()
        );
        assert_eq!(showing.to_string(), "nothing");
    }

    #[test]
    fn the_unedited_base_says_nothing_and_offers_no_revert() {
        let base = render(4);
        let working = Working::of_render(&base);
        let showing = Showing::new(&working, Some(&base), None, Compared::ALL);
        assert!(matches!(showing, Showing::Base { changes, .. } if !changes.any()));
        assert_eq!(showing.change_line(&words()), None);
        assert_eq!(showing.base_tile_line(&words()), None);
        assert!(showing.revert().is_none());
        assert_eq!(showing.to_string(), "#4 (base)");
    }

    /// The edited base: the change line, its tile's count, and a Revert
    /// that asks.
    #[test]
    fn the_edited_base_counts_its_changes_and_asks_before_reverting() {
        let w = words();
        let base = render(4);
        let cases = [
            (1, 0, "1 change to the frames since #4", "1 change", "Your 1 change since #4 hasn't"),
            (
                0,
                2,
                "2 changes to the settings since #4",
                "2 changes",
                "Your 2 changes since #4 haven't",
            ),
            (
                2,
                1,
                "1 change to the settings and 2 to the frames since #4",
                "3 changes",
                "Your 3 changes since #4 haven't",
            ),
        ];
        for (frames, settings, change_line, tile, body) in cases {
            let working = edit(&base, frames, settings);
            let showing = Showing::new(&working, Some(&base), Some(&base), Compared::ALL);
            assert_eq!(showing.changes(), Changes { frames, settings });
            assert!(showing.is_edited());
            assert_eq!(showing.change_line(&w), line(change_line));
            assert_eq!(showing.base_tile_line(&w), line(tile));
            let revert = showing.revert().unwrap();
            assert_eq!(revert.number(), 4);
            assert_eq!(
                revert.loses,
                Some(Loses::Changes { changes: Changes { frames, settings }, since: 4 })
            );
            let alert = showing.revert().and_then(|r| r.alert(&w)).unwrap();
            assert_eq!(alert.title, "Revert to #4?");
            assert_eq!(
                alert.body,
                format!("{body} been stacked, and will be lost. Every render is kept.")
            );
        }
    }

    #[test]
    fn another_render_says_how_the_working_version_differs_from_it() {
        let w = words();
        let (base, other) = (render(4), render(3));
        let cases = [
            (1, 0, "1 difference from #3 in the frames"),
            (2, 0, "2 differences from #3 in the frames"),
            (0, 1, "1 difference from #3 in the settings"),
            (0, 3, "3 differences from #3 in the settings"),
            (2, 1, "1 difference from #3 in the settings and 2 in the frames"),
            (1, 2, "2 differences from #3 in the settings and 1 in the frames"),
        ];
        for (frames, settings, want) in cases {
            // Based on #4, and that far from #3.
            let working = Working { base: Some(base.id.clone()), ..edit(&other, frames, settings) };
            let showing = Showing::new(&working, Some(&other), Some(&base), Compared::ALL);
            let Showing::Other { differences, .. } = showing else { panic!("{showing:?}") };
            assert_eq!(differences, Changes { frames, settings }, "{want}");
            assert_eq!(showing.change_line(&w), line(want));
            assert_eq!(showing.revert().map(|r| r.number()), Some(3));
        }
    }

    #[test]
    fn another_render_equal_to_the_working_version_says_so_and_offers_no_revert() {
        let (base, other) = (render(4), render(3));
        let working = Working { base: Some(base.id.clone()), ..Working::of_render(&other) };
        let showing = Showing::new(&working, Some(&other), Some(&base), Compared::ALL);
        assert!(matches!(showing, Showing::Other { differences, .. } if !differences.any()));
        assert_eq!(showing.change_line(&words()), line("Same frames and settings as #3"));
        assert!(showing.revert().is_none());
    }

    /// Reverting to another render asks only when edited or with no base.
    #[test]
    fn reverting_to_another_render_asks_only_when_something_would_be_lost() {
        let w = words();
        let (base, other) = (render(4), render(3));

        // Unedited against #4, different from #3: runs at once.
        let working = Working::of_render(&base);
        let other = Render { config: crate::StackerKind::Depth.default_config(), ..other };
        let showing = Showing::new(&working, Some(&other), Some(&base), Compared::ALL);
        let revert = showing.revert().unwrap();
        assert_eq!((revert.number(), revert.loses), (3, None));
        assert!(showing.revert().and_then(|r| r.alert(&w)).is_none());
        assert!(!showing.is_edited());
        assert_eq!(showing.base_tile_line(&w), None);
        assert_eq!(showing.to_string(), "#3 (other, base #4)");

        // Edited against #4: asks, naming #4's changes.
        let working = edit(&base, 1, 0);
        let showing = Showing::new(&working, Some(&other), Some(&base), Compared::ALL);
        assert_eq!(showing.base_tile_line(&w), line("1 change"));
        let alert = showing.revert().and_then(|r| r.alert(&w)).unwrap();
        assert_eq!(alert.title, "Revert to #3?");
        assert_eq!(
            alert.body,
            "Your 1 change since #4 hasn't been stacked, and will be lost. Every render is kept."
        );
        assert_eq!(showing.to_string(), "#3 (other, base #4, edited)");

        // No base: none of it has been stacked.
        let working = Working { base: None, ..Working::of_render(&base) };
        let showing = Showing::new(&working, Some(&other), Some(&base), Compared::ALL);
        assert!(matches!(showing, Showing::Other { base: OtherBase::None, .. }), "{showing:?}");
        assert_eq!(showing.revert().unwrap().loses, Some(Loses::Unstacked));
        assert_eq!(
            showing.revert().and_then(|r| r.alert(&w)).unwrap().body,
            "Your frames and settings haven't been stacked, and will be lost. Every render is kept."
        );
        assert_eq!(showing.to_string(), "#3 (other, no base)");
    }

    /// A base not in the records is still a base: Revert asks without a
    /// count.
    #[test]
    fn a_base_not_in_the_records_is_not_unstacked() {
        let w = words();
        let (base, other) = (render(4), render(3));
        let other = Render { config: crate::StackerKind::Depth.default_config(), ..other };
        let working = Working::of_render(&base);
        for passed in [None, Some(&other)] {
            let showing = Showing::new(&working, Some(&other), passed, Compared::ALL);
            assert!(matches!(showing, Showing::Other { base: OtherBase::Unknown, .. }));
            assert_eq!(showing.base().map(|b| b.number), None);
            assert!(!showing.is_edited());
            let revert = showing.revert().unwrap();
            assert_eq!((revert.number(), revert.loses), (3, Some(Loses::Uncounted)));
            assert_eq!(
                revert.alert(&w).unwrap().body,
                "Any changes you haven't stacked will be lost. Every render is kept."
            );
            assert_eq!(showing.to_string(), "#3 (other, base not in the records)");
        }
    }
}
