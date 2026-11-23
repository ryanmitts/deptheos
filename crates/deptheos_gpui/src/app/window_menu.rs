//! Windows and Linux have no menu bar: the title bar's ≡ holds File, Edit,
//! View, Stack, Settings and Help, built from the macOS menus ([`menus`]).
//! F10, or Alt tapped alone, opens it.

use gpui::prelude::*;
use gpui::{
    Context, DismissEvent, DispatchPhase, Focusable as _, Menu, MenuItem, ModifiersChangedEvent,
    MouseDownEvent, OwnedMenuItem, Window, anchored, canvas, deferred, div, px,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::PopupMenu;

use super::AppView;
use super::actions::*;
use crate::words::{DesktopWords, Wording as _, say};

/// Whether the title bar has the ≡ menu (everywhere but macOS).
pub(crate) const HAS_WINDOW_MENU: bool = !cfg!(target_os = "macos");

/// The ≡ menu: the bar's menus less Deptheos and Window, whose items move
/// to File (Exit), Settings and Help (Check for Updates, About).
pub(crate) fn window_menu_items(state: &MenuState, words: &DesktopWords) -> Vec<MenuItem> {
    let w = words;
    let mut items = Vec::new();
    for (kind, menu) in bar_menus(state, w) {
        let Menu { name, items: mut inner, .. } = menu;
        match kind {
            BarMenu::App | BarMenu::Window => continue,
            BarMenu::File => {
                // Found by what it does, not by its words: off macOS the
                // file manager goes by another name.
                for item in &mut inner {
                    if let MenuItem::Action { name, action, .. } = item
                        && action.partial_eq(&ShowFramesInFinder)
                    {
                        *name = if cfg!(target_os = "windows") {
                            say!(w, "menu-show-frames-in-explorer").into()
                        } else {
                            say!(w, "menu-show-frames-in-file-manager").into()
                        };
                    }
                }
                inner.extend([MenuItem::separator(), MenuItem::action(say!(w, "menu-exit"), Quit)]);
            }
            BarMenu::Help => inner.extend([
                MenuItem::separator(),
                MenuItem::action(say!(w, "menu-check-for-updates"), CheckForUpdates),
                MenuItem::action(say!(w, "menu-about"), About),
            ]),
            BarMenu::Edit | BarMenu::View | BarMenu::Stack => {}
        }
        items.push(MenuItem::submenu(Menu::new(name).items(inner)));
        if kind == BarMenu::Stack {
            items.push(MenuItem::separator());
            items.push(MenuItem::action(say!(w, "menu-settings"), OpenPreferences));
            items.push(MenuItem::separator());
        }
    }
    items
}

/// Fill `menu` with `items`, submenus and all.
fn fill(
    mut menu: PopupMenu,
    items: Vec<OwnedMenuItem>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    for item in items {
        menu = match item {
            OwnedMenuItem::Separator => menu.separator(),
            OwnedMenuItem::Submenu(sub) => {
                let inner = sub.items;
                menu.submenu(sub.name, window, cx, move |m, window, cx| {
                    fill(m, inner.clone(), window, cx)
                })
            }
            OwnedMenuItem::Action { name, action, checked, disabled, .. } => {
                menu.menu_with_check_and_disabled(name, checked, action, disabled)
            }
            OwnedMenuItem::SystemMenu(_) => menu,
        };
    }
    menu
}

impl AppView {
    /// Open the ≡ menu, or close it when open.
    pub(super) fn toggle_window_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.window_menu.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }
        let items: Vec<OwnedMenuItem> = window_menu_items(&self.menu_state(cx), cx.wording())
            .into_iter()
            .map(MenuItem::owned)
            .collect();
        let context = self.focus_handle.clone();
        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
            fill(menu.action_context(context), items, window, cx)
        });
        menu.read(cx).focus_handle(cx).focus(window, cx);
        let closed = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
            this.window_menu = None;
            window.focus(&this.focus_handle, cx);
            cx.notify();
        });
        self.window_menu = Some((menu, closed));
        cx.notify();
    }

    /// Alt tapped alone opens ≡, as on Windows. A key or press meanwhile
    /// voids the tap ([`Self::render_tap_guard`]): Alt-click in Retouch picks
    /// the sharpest frame, and Alt during a rail drag steps it.
    pub(super) fn alt_tap(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = event.modifiers;
        let alt_alone = m.alt && !m.control && !m.shift && !m.platform && !m.function;
        let tapped = self.alt_down && !alt_alone && !m.modified();
        self.alt_down = alt_alone && !self.rail_dragging();
        if tapped && HAS_WINDOW_MENU {
            self.toggle_window_menu(window, cx);
        }
    }

    /// Another key or a mouse press came with Alt down: it is no tap.
    pub(super) fn alt_spoiled(&mut self) {
        self.alt_down = false;
    }

    /// Any mouse press in the window, wherever it lands (over a control
    /// that occludes the photo too), ends an Alt tap.
    pub(super) fn render_tap_guard(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let this = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        this.update(cx, |this, _| this.alt_spoiled()).ok();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    /// The ≡ button, with its menu under it when open.
    pub(super) fn render_window_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let open = self.window_menu.as_ref().map(|(menu, _)| menu.clone());
        div()
            .relative()
            .child(
                Button::new("window-menu")
                    .icon(IconName::Menu)
                    .ghost()
                    .selected(open.is_some())
                    .tooltip(say!(cx, "window-menu-tooltip"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle_window_menu(window, cx)),
                    ),
            )
            .when_some(open, |el, menu| {
                el.child(
                    div().absolute().top_full().left_0().child(
                        deferred(
                            anchored()
                                .snap_to_window_with_margin(px(8.0))
                                .child(div().mt_1().occlude().child(menu)),
                        )
                        .with_priority(1),
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[MenuItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                MenuItem::Submenu(m) => m.name.to_string(),
                MenuItem::Action { name, .. } => name.to_string(),
                MenuItem::Separator => "-".into(),
                MenuItem::SystemMenu(_) => "system".into(),
            })
            .collect()
    }

    #[test]
    fn holds_the_bars_menus_and_settings() {
        let items = window_menu_items(&MenuState::default(), &DesktopWords::english());
        assert_eq!(names(&items), ["File", "Edit", "View", "Stack", "-", "Settings…", "-", "Help"]);
        let sub =
            |name: &str| match items.iter().find(|i| names(std::slice::from_ref(i))[0] == name) {
                Some(MenuItem::Submenu(m)) => names(&m.items),
                _ => panic!("{name}"),
            };
        assert_eq!(sub("File").last().map(String::as_str), Some("Exit"));
        assert!(sub("File").iter().all(|n| !n.contains("Finder")));
        assert!(sub("Help").contains(&"About Deptheos".to_string()));
        assert!(sub("Help").contains(&"Save Diagnostics…".to_string()));
        assert!(sub("Help").contains(&"Show Log Folder".to_string()));
    }
}
