//! The panels beside the viewer: the frames (list and tools) on the left;
//! the inspector on the right with the settings (`crate::params`) and what
//! Stack will use or a run's progress; in paint mode the paint panel and
//! footer (`retouch`) replace them.

use deptheos_app::frames::folder::Source;
use deptheos_app::frames::list::StackProblem;
use gpui::prelude::*;
use gpui::{App, Context, FontWeight, IntoElement, Pixels, Window, div, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};

use super::actions::ClearFrames;
use super::{AppView, Snapshot};
use crate::form::{Segment, Segmented};
use crate::paint::SourcesTab;
use crate::ui_state::{SIDEBAR_RANGE, SIDEBAR_WIDTH, update_ui_state};
use crate::words::{Wording as _, say};
use deptheos_app::FrameOrder;

impl AppView {
    /// The list: header and tools, RAW or JPEG, missing files, rows.
    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let processing = self.is_processing(cx);
        if let Some(tab) = self.sources_tab() {
            // Retouch: frames or renders; a click picks what to paint from.
            let list = match tab {
                SourcesTab::Frames => self.files.clone().into_any_element(),
                SourcesTab::Renders => self.render_paint_renders(cx),
            };
            return panel(cx)
                .border_r_1()
                .child(self.render_paint_from_header(tab, cx))
                .child(list)
                .child(self.render_paint_from_footer(cx));
        }

        panel(cx)
            .border_r_1()
            .child(self.render_files_header(processing, cx))
            .when(self.files.read(cx).missing_count() > 0, |el| {
                el.child(self.render_missing_banner(processing, cx))
            })
            .when_some(self.render_order_banner(processing, cx), |el, banner| el.child(banner))
            .when(self.files.read(cx).has_pairs(), |el| {
                el.child(self.render_source_switch(processing, cx))
            })
            .child(self.files.clone())
            .when(self.files.read(cx).selected_rows().len() > 1, |el| {
                el.child(self.render_selection_footer(cx))
            })
    }

    /// The settings under a failed stack's card, with the last stack's line
    /// and the Stack row pinned below; in paint mode the paint panel.
    pub(crate) fn render_inspector(
        &mut self,
        now: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let painting = self.is_retouching();
        let adjusting = self.adjusting().is_some();
        let settings = if painting {
            self.render_paint_panel(cx).into_any_element()
        } else if adjusting {
            self.render_adjust_panel(window, cx).into_any_element()
        } else {
            self.params.clone().into_any_element()
        };
        let failure = if painting { self.render_unsaveable(cx) } else { self.render_failure(cx) };
        let left_out = if painting || adjusting { None } else { self.render_left_out(cx) };
        panel(cx)
            .border_l_1()
            .child(
                div()
                    .id("inspector-settings")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(failure.map(|f| div().pb_3().child(f)))
                    .children(left_out.map(|l| div().pb_3().child(l)))
                    .child(settings),
            )
            .when(!painting && !adjusting, |el| {
                let stacked = self.stacked_line(now, cx);
                el.child(self.render_stack_row(stacked, cx))
            })
            .when(painting, |el| el.child(self.render_paint_footer(cx)))
            .when(adjusting, |el| el.child(self.render_adjust_footer(cx)))
    }

    /// In Retouch: why the render couldn't open (Try Again, Done), or why
    /// its paint can't be saved, while the worker says so.
    fn render_unsaveable(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let paint = self.paint.read(cx);
        let failed = paint.open_failure().map(|f| f.text.clone());
        let text = failed.clone().or_else(|| paint.state()?.unsaveable.clone())?;
        let theme = cx.theme();
        let color = if failed.is_some() { theme.danger } else { theme.warning };
        let card = h_flex()
            .id(if failed.is_some() { "paint-open-failed" } else { "paint-unsaveable" })
            .when(failed.is_some(), |el| el.debug_selector(|| "paint-open-failed".into()))
            .p_2p5()
            .gap_1p5()
            .items_start()
            .rounded(theme.radius)
            .border_1()
            .border_color(color)
            .bg(color.opacity(0.08))
            .text_xs()
            .child(Icon::new(IconName::TriangleAlert).small().text_color(color));
        if failed.is_none() {
            return Some(card.child(div().flex_1().min_w_0().child(text)).into_any_element());
        }
        let buttons = h_flex()
            .gap_2()
            .child(
                Button::new("paint-open-retry")
                    .label(say!(cx, "sidebar-paint-try-again"))
                    .small()
                    .on_click(
                        cx.listener(|this, _, _, cx| this.paint.update(cx, |p, cx| p.retry(cx))),
                    ),
            )
            .child(
                Button::new("paint-open-done")
                    .label(say!(cx, "sidebar-paint-done"))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.leave_paint(cx))),
            );
        let body = v_flex().flex_1().min_w_0().gap_2().child(text).child(buttons);
        Some(card.child(body).into_any_element())
    }

    /// "2 frames left out · Show" (ADR-121); Show selects them.
    fn render_left_out(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let count = self.files.read(cx).left_out_rows().len();
        if count == 0 {
            return None;
        }
        let line = deptheos_app::left_out_line(crate::words::of(cx), count);
        let warning = cx.theme().warning;
        Some(
            h_flex()
                .gap_1p5()
                .items_center()
                .text_xs()
                .child(Icon::new(IconName::TriangleAlert).xsmall().text_color(warning))
                .child(div().flex_1().min_w_0().truncate().child(line))
                .child(
                    Button::new("show-left-out")
                        .label(say!(cx, "sidebar-left-out-show"))
                        .xsmall()
                        .ghost()
                        .tooltip(say!(cx, "sidebar-left-out-show-tooltip"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_left_out(window, cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Select the frames the result on screen left out, the first shown.
    pub(crate) fn show_left_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.files.read(cx).left_out_rows();
        let Some(&first) = rows.first() else { return };
        self.files.update(cx, |f, cx| f.select_rows(rows, window, cx));
        self.show_file(first, window, cx);
    }

    /// "1 frame can't be found", with Locate… and Remove.
    fn render_missing_banner(
        &mut self,
        processing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let files = self.files.read(cx);
        let (missing, unplugged) = (files.missing_count(), files.not_connected_count());
        // All on an unconnected drive or share: connecting it finds them.
        let (title, text) = if unplugged == missing {
            let title = say!(cx, "sidebar-not-connected-title", n = missing);
            (title, say!(cx, "sidebar-not-connected-text", n = missing))
        } else {
            let title = say!(cx, "sidebar-missing-title", n = missing);
            (title, say!(cx, "sidebar-missing-text", n = missing))
        };
        let locate = say!(cx, "sidebar-locate-tooltip", n = missing);
        let theme = cx.theme();
        let warning = theme.warning;
        v_flex()
            .p_2p5()
            .gap_1p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(warning)
            .bg(warning.opacity(0.08))
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(warning)
                    .child(Icon::new(IconName::TriangleAlert).small())
                    .child(title),
            )
            .child(div().text_xs().child(text))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("locate-missing")
                            .label(say!(cx, "sidebar-locate"))
                            .small()
                            .outline()
                            .disabled(processing)
                            .tooltip(locate)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.locate_missing(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("remove-missing")
                            .label(say!(cx, "sidebar-remove-missing"))
                            .small()
                            .ghost()
                            .disabled(processing)
                            .tooltip(say!(cx, "sidebar-remove-missing-tooltip"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.files.update(cx, |f, cx| f.remove_missing(window, cx))
                            })),
                    ),
            )
    }

    /// "3 selected" under a multi-selection; actions are in the menus.
    fn render_selection_footer(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let count = self.files.read(cx).selected_rows().len();
        let theme = cx.theme();
        h_flex()
            .pt_2()
            .items_center()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .whitespace_nowrap()
            .child(say!(cx, "sidebar-selected", count = count))
    }

    /// RAW or JPEG for paired shots, shown when the list has pairs.
    fn render_source_switch(
        &mut self,
        processing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let source = self.files.read(cx).source();
        let weak = cx.entity().downgrade();
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(muted).child(say!(cx, "sidebar-pairs")))
            .child(
                Segmented::new("pair-source")
                    .child(Segment::new("RAW").disabled(processing).tooltip(|window, cx| {
                        Tooltip::new(say!(cx, "sidebar-pair-raw-tooltip")).build(window, cx)
                    }))
                    .child(Segment::new("JPEG").disabled(processing).tooltip(|window, cx| {
                        Tooltip::new(say!(cx, "sidebar-pair-jpeg-tooltip")).build(window, cx)
                    }))
                    .selected_index(match source {
                        Source::Raw => 0,
                        Source::Standard => 1,
                    })
                    .on_click(move |ix, window, cx| {
                        let source = if *ix == 0 { Source::Raw } else { Source::Standard };
                        weak.update(cx, |this, cx| this.set_source(source, window, cx)).ok();
                    }),
            )
    }

    /// "Frames 44 of 46" and Add ▾ (with Remove All Frames); more, sorting
    /// included, in the right-click menu.
    fn render_files_header(
        &mut self,
        processing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let files = self.files.read(cx);
        let muted = cx.theme().muted_foreground;
        let (included, total) = files.counts();
        let stacking = self.stacking.read(cx).runs_on_screen();
        let count = if included == total {
            format!("{total}")
        } else {
            say!(cx, "sidebar-frames-included", included = included, total = total)
        };
        let can_clear = files.can_clear();
        let this = cx.weak_entity();
        let w = cx.wording().clone();
        let add = Button::new("add-frames")
            .icon(IconName::Plus)
            .label(say!(cx, "sidebar-add"))
            .ghost()
            .small()
            .dropdown_caret(true)
            .disabled(processing)
            .dropdown_menu(move |menu, _, _| {
                let (a, b) = (this.clone(), this.clone());
                menu.item(PopupMenuItem::new(say!(w, "sidebar-add-frames")).on_click(
                    move |_, window, cx| {
                        a.update(cx, |this, cx| this.pick_images(window, cx)).ok();
                    },
                ))
                .item(PopupMenuItem::new(say!(w, "sidebar-add-folder")).on_click(
                    move |_, window, cx| {
                        b.update(cx, |this, cx| this.pick_folders(window, cx)).ok();
                    },
                ))
                .separator()
                .item(
                    PopupMenuItem::new(say!(w, "files-remove-all-frames"))
                        .disabled(!can_clear)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ClearFrames), cx)
                        }),
                )
            });
        h_flex()
            .gap_1()
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(say!(cx, "sidebar-frames")),
                    )
                    .when(stacking, |el| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(say!(cx, "sidebar-frames-locked")),
                        )
                    })
                    .when(total > 0, |el| {
                        el.child(
                            div().min_w_0().truncate().text_xs().text_color(muted).child(count),
                        )
                    }),
            )
            .when(!stacking, |el| el.child(add))
    }

    /// A frame order warning: in name order, folders whose capture order
    /// differs (with Sort by Capture Time); in capture order, unsortable ones.
    fn render_order_banner(
        &mut self,
        processing: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let files = self.files.read(cx);
        let notes = files.order_notes();
        let first = notes.first()?.clone();
        let more = notes.len() - 1;
        let by_name = files.order().is_name();
        let theme = cx.theme();
        let text = match (notes.len() > 1, by_name) {
            (true, true) => say!(cx, "sidebar-order-more-wrong", note = first, more = more),
            (true, false) => say!(cx, "sidebar-order-more", note = first, more = more),
            (false, true) => say!(cx, "sidebar-order-wrong", note = first),
            (false, false) => first,
        };
        Some(
            v_flex()
                .gap_1()
                .p_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.warning)
                .text_xs()
                .child(div().child(text))
                .when(by_name, |el| {
                    el.child(
                        Button::new("sort-by-time")
                            .label(say!(cx, "files-sort-by-capture-time"))
                            .xsmall()
                            .outline()
                            .disabled(processing)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.files.update(cx, |f, cx| {
                                    f.set_order(FrameOrder::CaptureTime, window, cx)
                                })
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// What Stack will use, under `stacked` (the last stack's line until a
    /// change). During a sequence or between batch phases, what it is on;
    /// during a stack, its batch and when settings unlock.
    fn render_stack_row(
        &mut self,
        stacked: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let stacking = self.stacking.read(cx);
        if !stacking.is_running()
            && let Some(status) = self.run_status(cx)
        {
            let line =
                div().text_xs().font_weight(FontWeight::MEDIUM).truncate().child(status.text);
            return v_flex().gap_1p5().child(line).into_any_element();
        }
        if !stacking.is_running() {
            let files = self.files.read(cx);
            let (muted, danger) = (cx.theme().muted_foreground, cx.theme().danger);
            let target = match files.stack_problem() {
                Some(problem) => {
                    let wrong =
                        matches!(problem, StackProblem::Missing(_) | StackProblem::MixedKinds(_));
                    div()
                        .text_color(if wrong { danger } else { muted })
                        .child(problem.text(crate::words::of(cx)))
                }
                None => {
                    let text = match files.counts() {
                        (i, t) if i == t => say!(cx, "sidebar-stack-uses", total = t),
                        (i, t) => say!(cx, "sidebar-stack-uses-some", included = i, total = t),
                    };
                    div().text_color(muted).child(text)
                }
            };
            let success = cx.theme().success;
            return v_flex()
                .gap_1()
                .when_some(stacked, |el, stacked| {
                    el.child(
                        h_flex()
                            .debug_selector(|| "stacked-line".into())
                            .gap_1p5()
                            .items_center()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .child(Icon::new(IconName::Check).xsmall().text_color(success))
                            .child(div().flex_1().min_w_0().truncate().child(stacked)),
                    )
                })
                .child(target.text_xs().truncate())
                .into_any_element();
        }
        let batch = self.batch_progress();
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_1p5()
            .when_some(batch, |el, batch| {
                el.child(div().text_xs().font_weight(FontWeight::MEDIUM).truncate().child(batch))
            })
            .child(div().text_xs().text_color(muted).child(say!(cx, "sidebar-settings-unlock")))
            .into_any_element()
    }

    /// Remove All Frames, keeping the stack. A stacked one gets them back
    /// with Revert, so only a never-stacked one asks.
    pub(crate) fn confirm_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_processing(cx) || !self.on_stacking() || !self.files.read(cx).can_clear() {
            return;
        }
        if self.base_render(cx).is_some() {
            self.files.update(cx, |f, cx| f.clear(window, cx));
            return;
        }
        let files = self.files.downgrade();
        crate::form::ask_destructive(
            say!(cx, "sidebar-clear-title"),
            say!(cx, "sidebar-clear-text"),
            say!(cx, "sidebar-clear-confirm"),
            move |window, cx| {
                files.update(cx, |f, cx| f.clear(window, cx)).ok();
            },
            window,
            cx,
        );
    }

    pub(super) fn resize_sidebar(
        &mut self,
        x: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The viewer keeps room to work in on a narrow window.
        let max = (window.viewport_size().width - px(480.0)).min(px(SIDEBAR_RANGE.1));
        let width = x.min(max).max(px(SIDEBAR_RANGE.0));
        if width != self.sidebar_width {
            self.set_sidebar_width(width, cx);
        }
    }

    fn set_sidebar_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        self.sidebar_width = width;
        update_ui_state(cx, |s| s.sidebar_width = Some(width.into()));
        cx.notify();
    }

    /// The sidebar's resize handle; double-click for the default width.
    pub(super) fn render_sidebar_handle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let hover = cx.theme().drag_border;
        div()
            .id("sidebar-resize")
            .absolute()
            .top_0()
            .right_0()
            .w(px(5.0))
            .h_full()
            .cursor_col_resize()
            .hover(move |s| s.bg(hover))
            .on_drag(SidebarResize, |_, _, _, cx| cx.new(|_| SidebarResize))
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.set_sidebar_width(px(SIDEBAR_WIDTH), cx);
                }
            }))
    }
}

/// The sidebar edge being dragged.
pub(super) struct SidebarResize;

impl Render for SidebarResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// A side panel; the caller borders the side facing the viewer.
fn panel(cx: &App) -> gpui::Div {
    let theme = cx.theme();
    v_flex().size_full().p_3().gap_2p5().bg(theme.sidebar).border_color(theme.sidebar_border)
}
