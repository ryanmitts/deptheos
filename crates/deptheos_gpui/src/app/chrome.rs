//! The panels around the photo in Stacking: the frames sidebar (⌃⌘S), the
//! inspector (⌃⌘I) docked or floating, Focus Mode (Tab) and the window's
//! width, as one [`WorkspaceChrome`].

use gpui::{App, Context, Pixels, Window, px};

use super::{AppView, Screen};
use crate::ui_state::update_ui_state;

/// At or below this width (points) the title bar's Retouch and Export are
/// icons, the title keeps name and Edited, and strip tiles drop settings.
pub(crate) const NARROW_WIDTH: f32 = 1280.0;
/// Below this width the inspector floats over the photo (⌃⌘I).
pub(crate) const CRAMPED_WIDTH: f32 = 1100.0;

/// A panel shown or hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    Shown,
    Hidden,
}

impl Panel {
    fn of(shown: bool) -> Self {
        if shown { Panel::Shown } else { Panel::Hidden }
    }

    pub(crate) fn is_shown(self) -> bool {
        self == Panel::Shown
    }

    fn toggled(self) -> Self {
        Self::of(!self.is_shown())
    }
}

/// `Wide` above [`NARROW_WIDTH`], `Cramped` below [`CRAMPED_WIDTH`],
/// `Narrow` between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Width {
    Wide,
    /// The title bar and the strip are pared down.
    Narrow,
    /// Pared down; Stacking's inspector floats while `floating`, hidden on
    /// entering, not remembered, gone on widening.
    Cramped {
        floating: Panel,
    },
}

impl Width {
    /// The class at `width` from `self`: staying cramped keeps the floating
    /// inspector.
    fn at(self, width: Pixels) -> Self {
        if width >= px(CRAMPED_WIDTH) {
            if width <= px(NARROW_WIDTH) { Width::Narrow } else { Width::Wide }
        } else if let Width::Cramped { .. } = self {
            self
        } else {
            Width::Cramped { floating: Panel::Hidden }
        }
    }
}

/// The panels as chosen, and the width they stand in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Panels {
    pub(crate) width: Width,
    /// The frames sidebar: remembered between launches.
    pub(crate) sidebar: Panel,
    /// The docked inspector, remembered between launches. Cramped Stacking
    /// floats its own; the Library docks by this at any width.
    pub(crate) inspector: Panel,
}

/// What stands around the photo.
///
/// Lifecycle: `Panels` (sidebar and inspector as set) or `Focus` (Focus
/// Mode, holding the panels to return to), owned by `AppView` and changed
/// only by `AppView::set_chrome`, from these transitions:
///
/// - Starts `Panels`, wide, as last launch left it ([`Self::restored`]).
/// - `Panels` ↔ `Focus`: Tab ([`Self::toggled_focus`]). Focus hides
///   sidebar, inspector and strip.
/// - The window's width, each frame ([`Self::at_width`]), in either.
/// - ⌃⌘S ([`Self::toggled_sidebar`]); ⌃⌘I ([`Self::toggled_inspector`]):
///   the floating inspector when cramped in Stacking, else the docked one.
///   From `Focus`, either goes to `Panels` with that panel shown.
/// - A stack failing ([`Self::revealing_inspector`]): `Focus` → `Panels`,
///   the inspector shown for its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceChrome {
    Panels(Panels),
    /// Focus Mode: the title bar, the photo and the view bar only.
    Focus {
        back: Panels,
    },
}

impl WorkspaceChrome {
    /// As the saved layout left it, wide until the first frame measures.
    pub(crate) fn restored(sidebar_hidden: bool, inspector_hidden: bool) -> Self {
        WorkspaceChrome::Panels(Panels {
            width: Width::Wide,
            sidebar: Panel::of(!sidebar_hidden),
            inspector: Panel::of(!inspector_hidden),
        })
    }

    /// The panels as chosen: in Focus Mode, what Tab goes back to.
    pub(crate) fn panels(&self) -> &Panels {
        match self {
            WorkspaceChrome::Panels(p) | WorkspaceChrome::Focus { back: p } => p,
        }
    }

    fn map(self, f: impl FnOnce(Panels) -> Panels) -> Self {
        match self {
            WorkspaceChrome::Panels(p) => WorkspaceChrome::Panels(f(p)),
            WorkspaceChrome::Focus { back } => WorkspaceChrome::Focus { back: f(back) },
        }
    }

    pub(crate) fn is_focus(&self) -> bool {
        matches!(self, WorkspaceChrome::Focus { .. })
    }

    /// At most [`NARROW_WIDTH`] wide.
    pub(crate) fn narrow(&self) -> bool {
        self.panels().width != Width::Wide
    }

    /// Narrower than [`CRAMPED_WIDTH`].
    pub(crate) fn cramped(&self) -> bool {
        matches!(self.panels().width, Width::Cramped { .. })
    }

    /// The sidebar is chosen shown (drawn out of Focus Mode).
    pub(crate) fn sidebar_chosen(&self) -> bool {
        self.panels().sidebar.is_shown()
    }

    /// The docked inspector is chosen shown.
    pub(crate) fn inspector_chosen(&self) -> bool {
        self.panels().inspector.is_shown()
    }

    /// The sidebar is drawn.
    pub(crate) fn shows_sidebar(&self) -> bool {
        !self.is_focus() && self.sidebar_chosen()
    }

    /// The inspector is shown, ignoring Focus Mode: docked, or floating in
    /// cramped Stacking.
    pub(crate) fn inspector_visible(&self, stacking: bool) -> bool {
        match self.panels().width {
            Width::Cramped { floating } if stacking => floating.is_shown(),
            _ => self.inspector_chosen(),
        }
    }

    /// The inspector is drawn.
    pub(crate) fn shows_inspector(&self, stacking: bool) -> bool {
        !self.is_focus() && self.inspector_visible(stacking)
    }

    /// In a window `width` wide.
    pub(crate) fn at_width(self, width: Pixels) -> Self {
        self.map(|p| Panels { width: p.width.at(width), ..p })
    }

    /// Tab: into Focus Mode, or back to the panels it left.
    pub(crate) fn toggled_focus(self) -> Self {
        match self {
            WorkspaceChrome::Panels(p) => WorkspaceChrome::Focus { back: p },
            WorkspaceChrome::Focus { back } => WorkspaceChrome::Panels(back),
        }
    }

    /// ⌃⌘S; in Focus Mode, out of it with the sidebar shown.
    pub(crate) fn toggled_sidebar(self) -> Self {
        match self {
            WorkspaceChrome::Panels(p) => {
                WorkspaceChrome::Panels(Panels { sidebar: p.sidebar.toggled(), ..p })
            }
            WorkspaceChrome::Focus { back } => {
                WorkspaceChrome::Panels(Panels { sidebar: Panel::Shown, ..back })
            }
        }
    }

    /// ⌃⌘I: the floating inspector in cramped Stacking, else the docked one;
    /// in Focus Mode, out of it with that inspector shown.
    pub(crate) fn toggled_inspector(self, stacking: bool) -> Self {
        let (p, set) = match self {
            WorkspaceChrome::Panels(p) => (p, None),
            WorkspaceChrome::Focus { back } => (back, Some(Panel::Shown)),
        };
        WorkspaceChrome::Panels(match p.width {
            Width::Cramped { floating } if stacking => Panels {
                width: Width::Cramped { floating: set.unwrap_or(floating.toggled()) },
                ..p
            },
            _ => Panels { inspector: set.unwrap_or(p.inspector.toggled()), ..p },
        })
    }

    /// Out of Focus Mode with the inspector shown (floating when cramped),
    /// for a failed stack's card.
    pub(crate) fn revealing_inspector(self) -> Self {
        let p = *self.panels();
        let width = match p.width {
            Width::Cramped { .. } => Width::Cramped { floating: Panel::Shown },
            other => other,
        };
        WorkspaceChrome::Panels(Panels { width, inspector: Panel::Shown, ..p })
    }
}

impl AppView {
    /// The only way [`Self::chrome`] changes.
    pub(super) fn set_chrome(&mut self, chrome: WorkspaceChrome, cx: &mut Context<Self>) {
        if chrome != self.chrome {
            tracing::debug!("chrome: {:?} -> {chrome:?}", self.chrome);
            self.chrome = chrome;
            self.sync_library(cx);
            cx.notify();
        }
    }

    /// Show or hide the frames sidebar (⌃⌘S); remembered.
    pub(crate) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_chrome(self.chrome.toggled_sidebar(), cx);
        // A list row may have had the keyboard: the window keeps it.
        window.focus(&self.focus_handle, cx);
        let hidden = !self.chrome.sidebar_chosen();
        update_ui_state(cx, |s| s.sidebar_hidden = hidden);
    }

    /// At most [`chrome::NARROW_WIDTH`] wide: title bar and strip pared down.
    pub(crate) fn narrow(&self) -> bool {
        self.chrome.narrow()
    }

    /// Narrower than [`chrome::CRAMPED_WIDTH`]: the inspector floats.
    pub(crate) fn cramped(&self) -> bool {
        self.chrome.cramped()
    }

    /// The inspector is shown, docked or floating.
    pub(crate) fn inspector_visible(&self) -> bool {
        self.chrome.inspector_visible(self.screen == Screen::Stacking)
    }

    /// The inspector's footer is on screen: Stacking, inspector shown, out
    /// of Focus Mode and Retouch.
    pub(super) fn footer_in_sight(&self, cx: &App) -> bool {
        self.screen == Screen::Stacking
            && self.inspector_visible()
            && !self.chrome.is_focus()
            && !self.paint.read(cx).is_open()
    }

    /// The inspector can toggle: not on a blank Library under the welcome.
    pub(crate) fn can_toggle_inspector(&self, cx: &App) -> bool {
        self.screen != Screen::Library || !self.library.read(cx).is_blank(cx)
    }

    /// ⌃⌘I: docked and remembered, or floating when cramped until closed;
    /// nothing on a blank Library.
    pub(crate) fn toggle_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_toggle_inspector(cx) {
            return;
        }
        let docked = self.chrome.inspector_chosen();
        self.set_chrome(self.chrome.toggled_inspector(self.screen == Screen::Stacking), cx);
        window.focus(&self.focus_handle, cx);
        // The docked choice is remembered, not the floating one.
        let shown = self.chrome.inspector_chosen();
        if shown != docked {
            update_ui_state(cx, |s| s.inspector_hidden = !shown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: f32 = 1440.0;
    const NARROW: f32 = 1280.0;
    const CRAMPED: f32 = 1000.0;

    fn start() -> WorkspaceChrome {
        WorkspaceChrome::restored(false, false).at_width(px(WIDE))
    }

    #[test]
    fn old_saved_layouts_restore() {
        let c = WorkspaceChrome::restored(true, false);
        assert!(!c.sidebar_chosen() && c.inspector_chosen() && !c.is_focus());
        let c = WorkspaceChrome::restored(false, true);
        assert!(c.sidebar_chosen() && !c.inspector_chosen());
        assert!(!c.narrow(), "wide until measured");
    }

    #[test]
    fn the_window_narrows_and_widens() {
        let c = start();
        assert!(!c.narrow() && !c.cramped());
        let c = c.at_width(px(NARROW));
        assert!(c.narrow() && !c.cramped(), "1280 is narrow");
        assert!(c.inspector_visible(true), "still docked");
        let c = c.at_width(px(1100.0));
        assert!(!c.cramped(), "1100 isn't cramped");

        // Cramped: Stacking's inspector floats, hidden at first; the
        // Library's stays docked as chosen.
        let c = c.at_width(px(CRAMPED));
        assert!(c.cramped() && !c.inspector_visible(true) && c.inspector_visible(false));
        let c = c.toggled_inspector(true);
        assert!(c.inspector_visible(true) && c.inspector_chosen(), "the docked choice is kept");
        assert!(c.at_width(px(900.0)).inspector_visible(true), "still cramped, still floating");

        // Wider: docked as chosen; cramped again, the float is gone.
        let c = c.at_width(px(WIDE));
        assert!(c.inspector_visible(true));
        assert!(!c.at_width(px(CRAMPED)).inspector_visible(true));
    }

    #[test]
    fn each_panel_toggles() {
        let c = start().toggled_sidebar();
        assert!(!c.sidebar_chosen() && !c.shows_sidebar());
        let c = c.toggled_sidebar().toggled_inspector(true);
        assert!(c.shows_sidebar() && !c.shows_inspector(true));
        // The Library toggles the docked inspector even when cramped.
        let c = c.at_width(px(CRAMPED)).toggled_inspector(false);
        assert!(c.inspector_chosen() && !c.inspector_visible(true));
        let c = c.toggled_inspector(true);
        assert!(c.inspector_visible(true));
        let c = c.toggled_inspector(true);
        assert!(!c.inspector_visible(true) && c.inspector_chosen(), "only the float closes");
    }

    #[test]
    fn focus_mode_hides_the_panels_and_returns_to_them() {
        let c = start().toggled_inspector(true);
        let focus = c.toggled_focus();
        assert!(focus.is_focus() && !focus.shows_sidebar() && !focus.shows_inspector(true));
        assert_eq!(focus.toggled_focus(), c, "Tab brings back what was there");

        // ⌃⌘S or ⌃⌘I in Focus Mode leaves it with that panel shown.
        let hidden = start().toggled_sidebar().toggled_inspector(true).toggled_focus();
        let c = hidden.toggled_sidebar();
        assert!(!c.is_focus() && c.shows_sidebar() && !c.shows_inspector(true));
        let c = hidden.toggled_inspector(true);
        assert!(!c.is_focus() && c.shows_inspector(true) && !c.shows_sidebar());
        let c = hidden.at_width(px(CRAMPED)).toggled_inspector(true);
        assert!(!c.is_focus() && c.shows_inspector(true), "floating");
        assert!(!c.inspector_chosen(), "the docked choice is kept");

        // The window resizes under Focus Mode.
        let resized = focus.at_width(px(CRAMPED));
        assert!(resized.is_focus() && resized.cramped());
        assert!(!resized.toggled_focus().inspector_visible(true), "the float starts hidden");
    }

    #[test]
    fn a_failed_stack_reveals_the_inspector() {
        let hidden = start().toggled_inspector(true).toggled_focus();
        let c = hidden.revealing_inspector();
        assert!(!c.is_focus() && c.shows_inspector(true) && c.inspector_chosen());
        let c = start().at_width(px(CRAMPED)).revealing_inspector();
        assert!(c.shows_inspector(true), "floating");
    }
}
