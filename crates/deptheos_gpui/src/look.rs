//! Hover, selection and focus looks for rows and tiles everywhere
//! (`design/README.md`: the accent marks selection and focus, hover is the
//! neutral surface-hover), the recipe for chrome over the photo
//! ([`overlay_surface`]), and light or dark ([`apply_appearance`]).

use gpui::{
    App, BoxShadow, Div, Hsla, InteractiveElement, Styled, Window, WindowAppearance, div, point,
    px, rgba,
};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};

use crate::brand::{HUD, HUD_BORDER, HUD_TEXT};
use crate::prefs::{Appearance, Prefs, prefs};

/// The theme now: the chosen one, or with System the `system` appearance.
fn theme_mode(chosen: Appearance, system: WindowAppearance) -> ThemeMode {
    match chosen {
        Appearance::System => system.into(),
        Appearance::Light => ThemeMode::Light,
        Appearance::Dark => ThemeMode::Dark,
    }
}

/// Put every window in the theme it should have ([`theme_mode`]); nothing if
/// already. `window` is the one whose system appearance changed, if any: its
/// reading is the reliable one on Linux.
///
/// The one place deciding: called once the themes load (`load` forces the
/// load in the current mode), when a window sees the system change, and when
/// the choice changes ([`follow_appearance_choice`]).
pub(crate) fn apply_appearance(window: Option<&Window>, load: bool, cx: &mut App) {
    let chosen = prefs(cx).appearance;
    let system = window.map_or_else(|| cx.window_appearance(), Window::appearance);
    let mode = theme_mode(chosen, system);
    if load || cx.theme().mode != mode {
        tracing::debug!("appearance {chosen:?} with the system {system:?}: {mode:?}");
        Theme::change(mode, None, cx);
    }
}

/// Follow Settings ▸ General ▸ Appearance for the app's life.
pub(crate) fn follow_appearance_choice(cx: &mut App) {
    let mut chosen = prefs(cx).appearance;
    cx.observe_global::<Prefs>(move |cx| {
        let now = prefs(cx).appearance;
        if now != chosen {
            chosen = now;
            apply_appearance(None, false, cx);
        }
    })
    .detach();
}

/// What a piece of floating chrome is filled with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fill {
    /// The `hud` tokens: translucent dark fill, edge and light text, the same
    /// in both modes, for what the app draws over the photo (pills, tags,
    /// chips, drop targets, the Library's preview).
    Hud,
    /// The theme's popover surface, for bars and cards of kit controls (view
    /// bar, progress card), whose theme colors a dark fill would hide in
    /// light mode.
    Controls,
}

/// Chrome over the photo: `fill`, a 1 px edge, the theme's corner and a
/// shadow; the caller lays it out.
pub(crate) fn overlay_surface(fill: Fill, cx: &App) -> Div {
    let theme = cx.theme();
    let el = div().border_1().rounded(theme.radius).shadow_md();
    match fill {
        Fill::Hud => el.bg(rgba(HUD)).border_color(rgba(HUD_BORDER)).text_color(rgba(HUD_TEXT)),
        Fill::Controls => el.bg(theme.popover).border_color(theme.border),
    }
}

/// Text and icons on [`Fill::Hud`].
pub(crate) fn hud_text() -> Hsla {
    rgba(HUD_TEXT).into()
}

/// What is selected: the accent's tint.
pub(crate) fn selected_bg(cx: &App) -> Hsla {
    cx.theme().list_active
}

/// Under the pointer: the neutral hover surface.
pub(crate) fn hover_bg(cx: &App) -> Hsla {
    cx.theme().list_hover
}

/// The focus ring: the accent outside the edge, taking no room.
fn focus_ring(cx: &App) -> Vec<BoxShadow> {
    let ring = cx.theme().ring;
    vec![BoxShadow {
        color: ring,
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(2.0),
        inset: false,
    }]
}

/// The one look for a row or tile that can be picked.
pub(crate) trait Pickable: InteractiveElement + Styled + Sized {
    /// Selected: the accent tint; else, when `enabled`, hover under the
    /// pointer. Enabled ones get a hand pointer and a focus ring; disabled
    /// ones neither hover nor hand.
    fn pickable(self, selected: bool, enabled: bool, cx: &App) -> Self {
        let (active, hover, ring) = (selected_bg(cx), hover_bg(cx), focus_ring(cx));
        let el = match (selected, enabled) {
            (true, _) => self.bg(active),
            (false, true) => self.hover(move |s| s.bg(hover)),
            (false, false) => self,
        };
        match enabled {
            true => el.cursor_pointer().focus_visible(move |s| s.shadow(ring)),
            false => el,
        }
    }
}

impl<E: InteractiveElement + Styled + Sized> Pickable for E {}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, div};

    use super::*;

    #[gpui::test]
    fn selected_is_tinted_and_dimmed_rows_point_at_nothing(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let active = selected_bg(cx);
            let mut selected = div().id("a").pickable(true, true, cx);
            assert_eq!(selected.style().background, Some(active.into()));
            assert_eq!(selected.style().mouse_cursor, Some(gpui::CursorStyle::PointingHand));
            let mut plain = div().id("b").pickable(false, true, cx);
            assert_eq!(plain.style().background, None, "tinted only under the pointer");
            let mut dimmed = div().id("c").pickable(false, false, cx);
            assert_eq!(dimmed.style().mouse_cursor, None);
        });
    }

    /// Chrome over the photo is the same in both modes; a kit bar takes the
    /// theme's surface.
    #[gpui::test]
    fn hud_chrome_is_one_look_in_both_modes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let mut looks = Vec::new();
            for mode in
                [gpui_kit::component::ThemeMode::Light, gpui_kit::component::ThemeMode::Dark]
            {
                gpui_kit::component::Theme::change(mode, None, cx);
                let mut hud = overlay_surface(Fill::Hud, cx);
                let mut controls = overlay_surface(Fill::Controls, cx);
                let style = hud.style().clone();
                assert_eq!(style.background, Some(gpui::rgba(HUD).into()));
                assert_eq!(style.text.color, Some(hud_text()));
                assert_eq!(controls.style().background, Some(cx.theme().popover.into()));
                looks.push(style);
            }
            assert_eq!(looks[0], looks[1]);
        });
    }

    /// System follows the system; Light and Dark hold.
    #[test]
    fn the_theme_is_the_choice_or_the_systems() {
        use WindowAppearance::{Dark, Light, VibrantDark};
        assert_eq!(theme_mode(Appearance::System, Dark), ThemeMode::Dark);
        assert_eq!(theme_mode(Appearance::System, VibrantDark), ThemeMode::Dark);
        assert_eq!(theme_mode(Appearance::System, Light), ThemeMode::Light);
        for system in [Light, Dark] {
            assert_eq!(theme_mode(Appearance::Light, system), ThemeMode::Light);
            assert_eq!(theme_mode(Appearance::Dark, system), ThemeMode::Dark);
        }
    }

    /// Choosing Dark or Light applies at once; System returns to the system's.
    #[gpui::test]
    fn choosing_an_appearance_changes_the_theme(cx: &mut TestAppContext) {
        let system: ThemeMode = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::prefs::Prefs::install(crate::prefs::Preferences::default(), None, cx);
            apply_appearance(None, true, cx);
            follow_appearance_choice(cx);
            cx.window_appearance().into()
        });
        assert_eq!(cx.update(|cx| cx.theme().mode), system);
        let other = if system.is_dark() { ThemeMode::Light } else { ThemeMode::Dark };
        let choice = |mode: ThemeMode| match mode {
            ThemeMode::Light => Appearance::Light,
            ThemeMode::Dark => Appearance::Dark,
        };
        for (chosen, mode) in [
            (choice(other), other),
            (Appearance::System, system),
            (choice(system), system),
            (choice(other), other),
        ] {
            cx.update(|cx| crate::prefs::update_prefs(cx, |p| p.appearance = chosen));
            assert_eq!(cx.update(|cx| cx.theme().mode), mode, "{chosen:?}");
        }
    }
}
