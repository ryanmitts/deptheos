//! The menu bar's side of the window: item words and states ([`MenuState`]),
//! reset whenever they change, and the menu-only items (Add Folder…, Show
//! Frames in Finder, Close Window, Rename Render…, Keyboard Shortcuts,
//! Choosing a Method).

use gpui::prelude::*;
use gpui::{App, Context, FontWeight, SharedString, Window, div, px};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};

use super::actions::{CancelItem, MenuState, RevertItem};
use super::retouch::ALT;
use super::working::Snapshot;
use super::{AppView, Running, Screen};
use crate::params::Algorithm;
use crate::viewer::Look;
use crate::words::{DesktopWords, Wording as _, say};

/// The keys the Keyboard Shortcuts window lists, by what they act on:
/// each group's title, and its keys (as written on macOS) with what they do.
fn shortcuts(w: &DesktopWords) -> Vec<(String, Vec<(String, String)>)> {
    let keys = |k: &str| k.to_string();
    vec![
        (
            say!(w, "shortcuts-stacking"),
            vec![
                (keys("⌘↩"), say!(w, "shortcut-stack")),
                (keys("⌘O / ⇧⌘O"), say!(w, "shortcut-add")),
                (keys("⌘E / ⇧⌘E"), say!(w, "shortcut-export")),
                (keys("⌘I"), say!(w, "shortcut-render-info")),
                (keys("⇧⌘B"), say!(w, "shortcut-new-batch")),
                (keys("X"), say!(w, "shortcut-exclude")),
                (keys("⌫ / ⌦"), say!(w, "shortcut-exclude-delete")),
                (keys("K"), say!(w, "shortcut-star")),
                (keys("⌘."), say!(w, "shortcut-cancel")),
            ],
        ),
        (
            say!(w, "shortcuts-viewing"),
            vec![
                (keys("1 2 3 4"), say!(w, "shortcut-layouts")),
                (keys("\\"), say!(w, "shortcut-flicker")),
                (keys("S"), say!(w, "shortcut-swap-sides")),
                (keys("M / C"), say!(w, "shortcut-map")),
                (keys("← / →, PgUp / PgDn"), say!(w, "shortcut-frames")),
                (say!(w, "shortcut-key-drag", key = ALT), say!(w, "shortcut-fine-scrub")),
                (keys("⌘0 / 0 / F"), say!(w, "shortcut-zoom-to-fit")),
                (keys("⌘1"), say!(w, "shortcut-actual-size")),
                (keys("Space"), say!(w, "shortcut-fit-or-actual-size")),
                (keys("⌘= / ⌘−"), say!(w, "shortcut-zoom-in-out")),
                (say!(w, "shortcut-double-click"), say!(w, "shortcut-double-click-zoom")),
                (say!(w, "shortcut-key-scroll", key = "⌘"), say!(w, "shortcut-scroll-zoom")),
                (keys("Esc"), say!(w, "shortcut-stop-compare")),
                (keys("Tab"), say!(w, "shortcut-focus-mode")),
                (keys("⌥⌘1 / ⌥⌘2"), say!(w, "shortcut-screens")),
                (keys("⌃⌘S / ⌃⌘I"), say!(w, "shortcut-panels")),
            ],
        ),
        (
            say!(w, "shortcuts-retouch"),
            vec![
                (keys("P"), say!(w, "shortcut-retouch")),
                (keys("B / E"), say!(w, "shortcut-tools")),
                (keys("[ ]"), say!(w, "shortcut-brush-size")),
                (keys("⇧[ ⇧]"), say!(w, "shortcut-hardness")),
                (keys("1 … 0"), say!(w, "shortcut-opacity")),
                (say!(w, "shortcut-key-click", key = "⌥"), say!(w, "shortcut-sharpest")),
                (say!(w, "shortcut-key-held", key = "\\"), say!(w, "shortcut-see-source")),
                (say!(w, "shortcut-key-drag", key = "Space"), say!(w, "shortcut-pan")),
                (keys("⌘Z / ⇧⌘Z"), say!(w, "shortcut-undo")),
            ],
        ),
        (
            say!(w, "shortcuts-library"),
            vec![
                (keys("↩"), say!(w, "shortcut-library-open")),
                (keys("← → ↑ ↓"), say!(w, "shortcut-library-move")),
                (keys("⇧← ⇧→ ⇧↑ ⇧↓"), say!(w, "shortcut-library-extend")),
                (keys("Space"), say!(w, "shortcut-library-preview")),
                (keys("⌘A"), say!(w, "shortcut-library-select-all")),
                (keys("⌘F"), say!(w, "shortcut-library-search")),
                (keys("⌘⌫ / ⌦"), say!(w, "shortcut-library-delete")),
                (keys("Esc"), say!(w, "shortcut-library-deselect")),
            ],
        ),
    ]
}

/// How many rows the Keyboard Shortcuts window lists.
#[cfg(test)]
pub(super) fn shortcut_rows(w: &DesktopWords) -> usize {
    shortcuts(w).iter().map(|(_, keys)| keys.len()).sum()
}

impl AppView {
    /// What the menu bar should say now. Full screen is read as the menu is
    /// set, so it is what [`Self::sync_menus`] last saw.
    pub(super) fn menu_state(&self, cx: &App) -> MenuState {
        self.menu_state_in(&self.snapshot(cx), self.menu_state.fullscreen, cx)
    }

    /// [`Self::menu_state`], from this frame's `now` and whether the
    /// window fills the screen.
    fn menu_state_in(&self, now: &Snapshot, fullscreen: bool, cx: &App) -> MenuState {
        let paint = self.paint.read(cx);
        let painting = paint.is_open();
        let viewer = self.viewer.read(cx);
        let has_result = self.has_result(cx) && self.screen == Screen::Stacking;
        let renders = self.renders.read(cx);
        let shown = renders.active(cx).and_then(|id| renders.find(id));
        let files = self.files.read(cx);
        let processing = self.is_processing(cx);
        // Everything below that acts on the working version or its render
        // is for the Stacking screen: in the Library that stack is hidden.
        let stacking = self.screen == Screen::Stacking;
        let can_edit = stacking && !processing;
        let showing = now.showing();
        let revert = match showing.shown() {
            Some(shown) => RevertItem::Revert {
                number: shown.number,
                enabled: can_edit && !painting && showing.revert().is_some(),
            },
            None => RevertItem::None,
        };
        let layout = viewer.mode().look().map(|look| match look {
            Look::Result => 0,
            Look::Split => 1,
            Look::SideBySide => 2,
            Look::TopAndBottom => 3,
            Look::Flicker => 4,
        });
        let comparing =
            has_result && !painting && viewer.has_compare() && viewer.mode().comparing().is_some();
        let contribution =
            (has_result && self.has_contribution(cx)).then(|| self.contribution_on(cx));
        MenuState {
            undo: paint.state().and_then(|s| s.undo_label.clone()),
            redo: paint.state().and_then(|s| s.redo_label.clone()),
            can_undo: paint.steps()[0] > 0,
            can_redo: paint.steps()[1] > 0,
            has_result,
            has_image: stacking && viewer.has_content(),
            can_stack: stacking && self.can_stack(cx),
            running: processing,
            cancel: self.can_cancel(cx).then_some(match self.running {
                Running::Batch(_) => CancelItem::Batch,
                Running::Sequence { .. } => CancelItem::Sequence,
                Running::Idle | Running::Stack(_) => CancelItem::Stack,
            }),
            can_export: has_result && self.can_export(cx),
            can_retouch: has_result && (painting || self.can_paint(cx)),
            comparing,
            layout: layout.filter(|_| has_result),
            map: self.can_show_map(cx).then(|| viewer.mode().is_map()),
            contribution,
            library: self.screen == Screen::Library,
            sidebar: self.chrome.sidebar_chosen(),
            inspector: self.can_toggle_inspector(cx).then(|| self.inspector_visible()),
            focus: self.chrome.is_focus(),
            revert,
            info: stacking && shown.is_some(),
            picked: shown.filter(|_| stacking).map(|r| r.picked),
            excludes: files.selection_excludes(),
            all_included: files.all_included(),
            frames: files.rows().next().is_some() && can_edit,
            stacked: stacking && self.renders.read(cx).has_renders(&self.stack),
            painting,
            log_folder: super::diagnostics::log_folder().is_some(),
            can_try_sample: self.can_try_sample(cx),
            sample_stacks: self.sample_stacks(),
            fullscreen,
        }
    }

    /// Set the menu bar again when what it says has changed (macOS; a
    /// no-op elsewhere).
    pub(super) fn sync_menus(&mut self, now: &Snapshot, window: &Window, cx: &mut Context<Self>) {
        let state = self.menu_state_in(now, window.is_fullscreen(), cx);
        if state != self.menu_state {
            cx.set_menus(super::actions::menus(&state, cx.wording()));
            self.menu_state = state;
        }
    }

    /// Close Window (⌘W): as the close button does, so a run asks first.
    pub(super) fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_quit(window, cx) {
            window.remove_window();
        }
    }

    /// Stacking is up. Actions on the working version or its render (Revert,
    /// Remove All Frames, Include/Exclude, Star Render) are dim and inert in
    /// the Library, where that stack is hidden.
    pub(crate) fn on_stacking(&self) -> bool {
        self.screen == Screen::Stacking
    }

    /// Show Frames in Finder: the frame shown, else the first.
    pub(super) fn show_frames_in_finder(&self, cx: &mut Context<Self>) {
        if !self.on_stacking() {
            return;
        }
        let files = self.files.read(cx);
        let path = files
            .selected(cx)
            .and_then(|id| files.path(id))
            .map(std::path::Path::to_path_buf)
            .or_else(|| files.rows().next().map(|(_, p)| p));
        if let Some(path) = path {
            cx.reveal_path(&path);
        }
    }

    /// Rename Stack…: the name every render of the stack on screen shows.
    pub(super) fn rename_stack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Stacking {
            return;
        }
        let stack = self.stack.clone();
        if self.renders.read(cx).has_renders(&stack) {
            self.library.update(cx, |l, cx| l.rename_stack(stack, window, cx));
        }
    }

    /// Rename Render…: the shown render's own name, after its number on
    /// its tile ("#4 · sharper"). The stack's name is Rename Stack….
    pub(super) fn rename_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Stacking {
            return;
        }
        if let Some(id) = self.renders.read(cx).active(cx).cloned() {
            self.renders.update(cx, |r, cx| r.start_rename(&id, window, cx));
        }
    }

    /// Keyboard Shortcuts (⌘/): groups scroll under the title when tall, long
    /// descriptions wrap.
    pub(super) fn show_shortcuts(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            // Not shrunk to the dialog: its height is what the dialog's
            // body scrolls through.
            let mut body = v_flex().flex_shrink_0().gap_4().pb_2();
            let mut row = 0;
            for (title, keys) in shortcuts(cx.wording()) {
                let mut group = v_flex()
                    .gap_1()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title));
                for (key, what) in keys {
                    group = group.child(
                        h_flex()
                            .items_start()
                            .gap_3()
                            .text_sm()
                            .child(
                                div()
                                    .w(px(150.0))
                                    .flex_shrink_0()
                                    .text_color(muted)
                                    .child(super::actions::key_text(&key)),
                            )
                            .child(
                                div()
                                    .debug_selector(move || format!("shortcut-what-{row}"))
                                    .flex_1()
                                    .min_w_0()
                                    .child(what),
                            ),
                    );
                    row += 1;
                }
                body = body.child(group);
            }
            let title = div()
                .debug_selector(|| "shortcuts-title".into())
                .child(say!(cx, "shortcuts-title"));
            dialog.title(title).w(px(560.0)).child(body)
        });
    }

    /// Choosing a Method: what each one is for.
    pub(super) fn choosing_a_method(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let mut body = v_flex().gap_3();
            for algorithm in Algorithm::ALL {
                let name: SharedString = algorithm.label(crate::words::of(cx)).into();
                body = body.child(
                    v_flex()
                        .gap_0p5()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(algorithm.description(crate::words::of(cx))),
                        ),
                );
            }
            body = body
                .child(div().text_xs().text_color(muted).child(say!(cx, "choosing-a-method-hint")));
            dialog.title(say!(cx, "choosing-a-method-title")).w(px(480.0)).child(body)
        });
    }
}
