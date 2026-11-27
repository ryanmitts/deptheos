//! The title bar, which is also the toolbar: the app menu (without a menu
//! bar); in Stacking ‹ Library, sidebar toggle, title, Retouch, Export,
//! Stack and inspector toggle; in the Library its counts, search, New Stack,
//! New Batch…, Open and details toggle.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, TitleBar, h_flex,
};

use super::actions::*;
use super::window_menu::HAS_WINDOW_MENU;
use super::working::{TitleNote, TitleParts};
use super::{AppView, Screen};
use crate::look::Pickable as _;
use crate::words::say;

/// Room for the controls and, on macOS, the window buttons beside them.
pub(super) const TITLE_BAR_HEIGHT: f32 = 44.0;

/// Every title bar control's height: the kit's medium controls and the
/// progress pill.
pub(super) const TITLE_BAR_CONTROL_HEIGHT: f32 = 32.0;

/// Wraps a title-bar control so pressing it doesn't move the window. On
/// Windows the bar is the OS drag area, whose move swallows the release;
/// occluding the mouse takes the control out of it (and out of macOS and
/// Linux's drag and double-click). The space between controls still moves
/// the window. `name` finds it in tests.
pub(super) fn no_drag(name: &'static str, control: impl IntoElement) -> Div {
    // Flex and free to shrink, so a control that shrinks in a narrow window still can.
    div().flex().min_w_0().occlude().debug_selector(|| name.into()).child(control)
}

/// The title: name, then what is shown, on one line that gives way first.
pub(super) fn title_block() -> Div {
    h_flex()
        // No width of its own: it grows into the room left and asks for its
        // least, so it gives way before the controls.
        .w(px(0.0))
        .flex_1()
        .min_w(px(60.0))
        .mx_2()
        .gap_2()
        // Centered: GPUI has no text baseline, and asking for one staggers
        // the parts.
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
}

impl AppView {
    /// The title bar (see the module doc); its empty space drags the window.
    pub(super) fn render_toolbar(
        &mut self,
        parts: TitleParts,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let ctx = Some(KEY_CONTEXT);

        // macOS has the menu bar; elsewhere the ≡ menu holds it.
        let window_menu = HAS_WINDOW_MENU.then(|| self.render_window_menu(cx));
        let library = self.screen == Screen::Library;
        let painting = self.is_retouching();
        let (title, actions) = if library {
            (
                self.render_library_title(cx).into_any_element(),
                self.render_library_actions(cx).into_any_element(),
            )
        } else if painting {
            (
                self.render_retouch_title(cx).into_any_element(),
                self.render_retouch_actions(cx).into_any_element(),
            )
        } else if self.adjusting().is_some() {
            (
                self.render_adjust_title(cx).into_any_element(),
                self.render_adjust_actions(cx).into_any_element(),
            )
        } else {
            (
                self.render_title(parts, cx).into_any_element(),
                self.render_title_actions(cx).into_any_element(),
            )
        };
        let sidebar_toggle = Button::new("toggle-sidebar")
            .icon(IconName::PanelLeft)
            .ghost()
            .selected(self.chrome.shows_sidebar())
            .tooltip_with_action(say!(cx, "toolbar-toggle-sidebar"), &ToggleSidebar, ctx)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)));
        // The Library is home; Done (or Esc) leaves Retouch first.
        let back = Button::new("library-back")
            .icon(IconName::ChevronLeft)
            .label(say!(cx, "toolbar-library"))
            .ghost()
            .disabled(painting)
            .tooltip_with_action(say!(cx, "toolbar-library-tooltip"), &ShowLibrary, None)
            .on_click(
                cx.listener(|this, _, window, cx| this.set_screen(Screen::Library, window, cx)),
            );
        // The title takes what the actions leave, down to its least; only a
        // search field among the actions gives way.
        let bar = h_flex()
            .h_full()
            // No width of its own, so the kit's bar (which doesn't shrink)
            // takes the window's.
            .w(px(0.0))
            .flex_1()
            .pr_3()
            .gap_1()
            .child(
                h_flex()
                    .flex_1()
                    .items_center()
                    .gap_1()
                    .when_some(window_menu, |el, menu| el.child(no_drag("window-menu", menu)))
                    .when(!library, |el| {
                        el.child(no_drag("library-back", back))
                            .child(no_drag("toggle-sidebar", sidebar_toggle))
                    })
                    .child(title),
            )
            .child(h_flex().min_w_0().items_center().justify_end().gap_1().child(actions));
        TitleBar::new()
            .h(px(TITLE_BAR_HEIGHT))
            .bg(cx.theme().secondary)
            .border_color(cx.theme().border)
            .child(bar)
    }

    /// Name, render shown ("#3 · Depth · 44 of 46 frames", or the working
    /// version), and "Edited" with a dot or "Not stacked yet". Once stacked,
    /// clicking the name renames the stack.
    fn render_title(&self, parts: TitleParts, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let edited = crate::params::edited_color(cx);
        let can_rename = self.renders.read(cx).has_renders(&self.stack);
        let name: SharedString =
            parts.name.unwrap_or_else(|| say!(cx, "toolbar-new-stack-title")).into();
        let name = if can_rename {
            let tip = say!(cx, "library-details-rename-tip");
            let label = div()
                .id("stack-title")
                .px_1p5()
                .py_0p5()
                .rounded(cx.theme().radius)
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_ellipsis()
                .pickable(false, true, cx)
                .tooltip(move |window, cx| {
                    Tooltip::new(tip.clone())
                        .action(&RenameStack, Some(KEY_CONTEXT))
                        .build(window, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| this.rename_stack(window, cx)))
                .child(name);
            no_drag("stack-title", label).into_any_element()
        } else {
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_ellipsis()
                .child(name)
                .into_any_element()
        };
        // A narrow window keeps the name and the note.
        let what = parts.what.filter(|_| !self.narrow());
        title_block()
            .child(name)
            .when_some(what, |el, what| {
                el.child(div().text_xs().text_color(muted).text_ellipsis().child(what))
            })
            .when_some(parts.note, |el, note| match note {
                TitleNote::NotStacked(text) => {
                    el.child(div().text_xs().text_color(muted).text_ellipsis().child(text))
                }
                TitleNote::Edited(text) => el.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_xs()
                        .child(div().size(px(6.0)).rounded_full().bg(edited))
                        .child(text),
                ),
            })
    }

    /// Retouch, Export, Stack (the window's one primary button) and the
    /// inspector toggle.
    fn render_title_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let ctx = Some(KEY_CONTEXT);
        let blocked = self.paint_blocked(cx);
        let can_export = self.can_export(cx);
        let can_stack = self.can_stack(cx);
        let retouch_tip = blocked.unwrap_or_else(|| say!(cx, "toolbar-retouch-tooltip"));
        let retouch = self.retouch_button(retouch_tip, ctx, cx);
        let adjust_tip =
            self.paint_blocked(cx).unwrap_or_else(|| say!(cx, "toolbar-adjust-tooltip"));
        let adjust = self.adjust_button(adjust_tip, ctx, cx);
        let export_tip = match self.busy_reason(cx) {
            Some(busy) if !can_export && self.has_result(cx) => busy,
            _ => say!(cx, "toolbar-export-tooltip"),
        };
        let export = self.export_button(export_tip, can_export, ctx, cx);
        let stack = Button::new("stack")
            .icon(IconName::Layers)
            .label(say!(cx, "toolbar-stack"))
            .primary()
            .disabled(!can_stack)
            .tooltip_with_action(say!(cx, "toolbar-stack-tooltip"), &RunStack, ctx)
            .on_click(cx.listener(|this, _, window, cx| this.run_stacking(window, cx)));
        h_flex()
            .gap_1p5()
            .flex_shrink_0()
            .child(retouch)
            .child(adjust)
            .child(export)
            .child(match self.render_run_status("cancel-stack", ctx, cx) {
                Some(pill) => no_drag("stack-progress", pill),
                None => no_drag("stack", stack),
            })
            .child(self.inspector_toggle(ctx, cx))
    }

    /// The inspector toggle, at the bar's right end in Stacking and Retouch.
    pub(super) fn inspector_toggle(
        &self,
        ctx: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let button = Button::new("toggle-inspector")
            .icon(IconName::PanelRight)
            .ghost()
            .selected(self.chrome.shows_inspector(true))
            .tooltip_with_action(say!(cx, "toolbar-toggle-inspector"), &ToggleInspector, ctx)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_inspector(window, cx)));
        no_drag("toggle-inspector", button)
    }

    /// Retouch, in the same place in Stacking and Retouch, where it is
    /// selected and leaves.
    pub(super) fn retouch_button(
        &self,
        tip: String,
        ctx: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let painting = self.is_retouching();
        let blocked = self.paint_blocked(cx);
        let button = Button::new("retouch")
            .icon(IconName::Brush)
            .when(!self.narrow(), |b| b.label(say!(cx, "toolbar-retouch")))
            .outline()
            .selected(painting)
            .disabled(!painting && blocked.is_some())
            .tooltip_with_action(tip, &TogglePaint, ctx)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_paint(cx)));
        no_drag("retouch", button)
    }

    /// Export, the same in Stacking and Retouch; while writing, a spinner
    /// and "Exporting bee-4.tif…" (spinner alone when narrow).
    pub(super) fn export_button(
        &self,
        tip: String,
        can_export: bool,
        ctx: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(text) = self.export_progress_text(cx) {
            return self.export_progress("export-progress", text, cx);
        }
        let button = Button::new("export")
            .icon(IconName::Share)
            .when(!self.narrow(), |b| b.label(say!(cx, "toolbar-export")))
            .outline()
            .disabled(!can_export)
            .tooltip_with_action(tip, &SaveResult, ctx)
            .on_click(cx.listener(|this, _, window, cx| this.save_result(window, cx)));
        no_drag("export", button).into_any_element()
    }

    /// A spinner and the export's words (spinner alone when narrow, words as
    /// tooltip).
    fn export_progress(&self, id: &'static str, text: String, cx: &Context<Self>) -> AnyElement {
        let narrow = self.narrow();
        let progress = h_flex()
            .id(id)
            .h(px(TITLE_BAR_CONTROL_HEIGHT))
            .px_2()
            .gap_1p5()
            .items_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(Spinner::new().small())
            .when(!narrow, |el| el.child(div().max_w(px(220.0)).truncate().child(text.clone())))
            .when(narrow, |el| {
                el.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
            });
        no_drag(id, progress).into_any_element()
    }

    /// "Library · 12 stacks · 68 renders · 4.2 GB".
    fn render_library_title(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let count = self.library.read(cx).count_text(cx);
        title_block()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(say!(cx, "toolbar-library-title")),
            )
            .child(div().text_xs().text_color(muted).child(count))
    }

    /// ⌘F in the Library: focus search, unless nothing to search; Esc
    /// returns to the grid.
    pub(super) fn find_stacks(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let library = self.library.read(cx);
        if self.screen != Screen::Library || library.is_blank(cx) {
            return;
        }
        let field = gpui::Focusable::focus_handle(library.search.read(cx), cx);
        window.focus(&field, cx);
    }

    /// The Library's search, New Stack, New Batch…, Open and inspector
    /// toggle, none primary (the welcome's Add Frames… is). An empty Library
    /// keeps them in place, dimmed, so the bar holds still.
    fn render_library_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let ctx = Some(LIBRARY_KEY_CONTEXT);
        let library = self.library.read(cx);
        let muted = cx.theme().muted_foreground;
        let can_open = library.can_open(cx) && !self.is_processing(cx);
        // Why the buttons a run dims are dimmed.
        let busy = self.busy_reason(cx);
        let blank = library.is_blank(cx);
        let search = div().flex_1().child(
            Input::new(&library.search)
                .disabled(blank)
                .cleanable(true)
                .prefix(Icon::new(IconName::Search).xsmall().text_color(muted)),
        );
        let new_batch = Button::new("new-batch")
            .icon(IconName::Plus)
            .label(say!(cx, "toolbar-new-batch"))
            .outline()
            .disabled(self.is_processing(cx))
            .tooltip_with_action(
                busy.clone().unwrap_or_else(|| say!(cx, "toolbar-new-batch-tooltip")),
                &OpenBatch,
                ctx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_batch(window, cx)));
        let new_stack = Button::new("new-stack")
            .icon(IconName::Plus)
            .label(say!(cx, "toolbar-new-stack"))
            .outline()
            .disabled(self.is_processing(cx))
            .tooltip_with_action(
                busy.clone().unwrap_or_else(|| say!(cx, "toolbar-new-stack-tooltip")),
                &NewStack,
                ctx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.new_stack(window, cx)));
        let open = Button::new("library-open")
            .icon(IconName::ArrowRight)
            .label(say!(cx, "toolbar-open"))
            .outline()
            .disabled(!can_open)
            .tooltip_with_action(
                busy.unwrap_or_else(|| say!(cx, "toolbar-open-tooltip")),
                &OpenSelected,
                ctx,
            )
            .on_click(
                cx.listener(|this, _, _, cx| this.library.update(cx, |l, cx| l.open_selected(cx))),
            );
        // Stacking's inspector and saved choice; can't hide on a blank Library.
        let inspector = Button::new("toggle-details")
            .icon(IconName::PanelRight)
            .ghost()
            .selected(self.chrome.inspector_chosen() && !blank)
            .disabled(blank)
            .tooltip_with_action(say!(cx, "toolbar-toggle-inspector"), &ToggleInspector, ctx)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_inspector(window, cx)));
        // A run's progress and Cancel, or an export's words.
        let progress = self
            .render_run_status("library-cancel", Some(LIBRARY_RUNNING_KEY_CONTEXT), cx)
            .map(|pill| no_drag("library-progress", pill));
        let exporting = self
            .export_progress_text(cx)
            .map(|text| self.export_progress("library-export-progress", text, cx));
        // Search narrows to 120 points with the window; buttons keep width.
        let fixed = |name, control| no_drag(name, control).flex_shrink_0();
        h_flex()
            .gap_1p5()
            .min_w_0()
            .children(progress.map(|p| p.flex_shrink_0()))
            .children(exporting)
            .child(no_drag("library-search", search).w(px(240.0)).min_w(px(120.0)).flex_shrink(1.0))
            .child(fixed("new-stack", new_stack.into_any_element()))
            .child(fixed("new-batch", new_batch.into_any_element()))
            .child(fixed("library-open", open.into_any_element()))
            .child(fixed("toggle-details", inspector.into_any_element()))
    }
}
