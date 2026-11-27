//! Retouch around the photo: title buttons, the Paint from header and footer
//! over the list, the brush panel and footer in the inspector, labels over
//! the photo, ⌥-click for the sharpest frame and \ held for the source. The
//! painting is `crate::paint`'s.

use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::RenderId;
use deptheos_app::paint::SourceKind;
use gpui::prelude::*;
use gpui::{App, Context, FontWeight, SharedString, Window, div, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use super::AppView;
use super::actions::*;
use crate::form::{Segment, Segmented};
use crate::look::{Fill, Pickable as _, overlay_surface};
use crate::paint::SourcesTab;
use crate::params::labeled_slider;
use crate::renders::{render_line, render_title, thumb_box};
use crate::viewer::ViewMode;
use crate::words::{DesktopWords, Wording as _, say};

/// How tall a render's thumbnail is in Retouch's list of renders.
const PAINT_THUMB_HEIGHT: f32 = 36.0;

/// "⌥" on macOS, "Alt" elsewhere: the key ⌥-click is.
pub(crate) const ALT: &str = if cfg!(target_os = "macos") { "⌥" } else { "Alt" };

/// "1 stroke", "12 strokes".
pub(crate) fn strokes(n: usize, w: &DesktopWords) -> String {
    say!(w, "retouch-strokes", n = n)
}

/// How many frames and renders a render was painted from.
pub(crate) fn source_counts(painted_from: &[SourceKind]) -> (usize, usize) {
    let frames = painted_from.iter().filter(|k| matches!(k, SourceKind::Frame { .. })).count();
    let renders = painted_from.iter().filter(|k| matches!(k, SourceKind::Render { .. })).count();
    (frames, renders)
}

/// A key as the key list and the hints show it.
pub(crate) fn key_cap(text: &'static str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .px_1()
        .min_w(px(16.0))
        .flex()
        .justify_center()
        .rounded(px(3.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(text)
}

impl AppView {
    /// The render painted, while Retouch is on.
    fn retouched(&self, cx: &App) -> Option<deptheos_app::library::history::Render> {
        let paint = self.paint.read(cx);
        let id = &paint.state()?.key;
        self.renders.read(cx).find(id).cloned()
    }

    /// "Frame 23" for a listed frame, else its file name; "Original" for the
    /// eraser; a render's label.
    pub(crate) fn paint_source_text(&self, cx: &App) -> SharedString {
        let paint = self.paint.read(cx);
        match paint.source() {
            Some(SourceKind::Frame { path }) => {
                let files = self.files.read(cx);
                let row = files.row_of(path).and_then(|id| files.rows().position(|(r, _)| r == id));
                match row {
                    Some(i) => {
                        let number = i + 1;
                        say!(cx, "retouch-source-frame", number = number).into()
                    }
                    None => file_label(path).into(),
                }
            }
            Some(SourceKind::Original) => say!(cx, "paint-source-original").into(),
            Some(SourceKind::Render { .. }) => self
                .painting()
                .and_then(|painting| paint.source_name(painting, cx))
                .unwrap_or_default(),
            None => say!(cx, "retouch-source-none").into(),
        }
    }

    /// "Retouching #4", and "canon1 · 12 strokes".
    pub(crate) fn retouch_title(&self, cx: &App) -> (SharedString, String) {
        let render = self.retouched(cx);
        let [undo, _] = self.paint.read(cx).steps();
        let w = cx.wording();
        match &render {
            Some(r) => (
                say!(w, "retouch-title-render", number = r.number).into(),
                say!(
                    w,
                    "retouch-title-detail",
                    stack = self.renders.read(cx).stack_name(&r.stack, w),
                    strokes = undo
                ),
            ),
            None => (say!(w, "retouch-title").into(), strokes(undo, w)),
        }
    }

    pub(super) fn render_retouch_title(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let (title, what) = self.retouch_title(cx);
        super::toolbar::title_block()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_xs().text_color(muted).text_ellipsis().child(what))
    }

    /// Undo and Redo, Retouch (selected), Export, Done and the inspector toggle.
    pub(super) fn render_retouch_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let ctx = Some(PAINT_KEY_CONTEXT);
        let paint = self.paint.read(cx);
        let [undo, redo] = paint.steps();
        let labels = paint.state().map(|s| [s.undo_label.clone(), s.redo_label.clone()]);
        let [undo_label, redo_label] = labels.unwrap_or_default();
        let undo_tip = undo_tip(undo_label.as_deref(), undo, cx);
        let redo_tip = redo_tip(redo_label.as_deref(), redo, cx);
        let can_export = self.can_export(cx);
        let no_drag = super::toolbar::no_drag;
        h_flex()
            .gap_1p5()
            .flex_shrink_0()
            .child(no_drag(
                "retouch-undo",
                Button::new("retouch-undo")
                    .icon(IconName::Undo2)
                    .ghost()
                    .disabled(undo == 0)
                    .tooltip_with_action(undo_tip, &Undo, ctx)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.with_paint(cx, |p, cx| p.undo(cx))),
                    ),
            ))
            .child(no_drag(
                "retouch-redo",
                Button::new("retouch-redo")
                    .icon(IconName::Redo2)
                    .ghost()
                    .disabled(redo == 0)
                    .tooltip_with_action(redo_tip, &Redo, ctx)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.with_paint(cx, |p, cx| p.redo(cx))),
                    ),
            ))
            // Export and Done sit at the bar's trailing end; Done is the single
            // clear exit from Retouch.
            .child(self.export_button(say!(cx, "retouch-export-tooltip"), can_export, ctx, cx))
            .child(no_drag(
                "retouch-done",
                Button::new("retouch-done")
                    .icon(IconName::Check)
                    .label(say!(cx, "retouch-done"))
                    .primary()
                    .tooltip_with_action(say!(cx, "retouch-done-tooltip"), &LeavePaint, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.leave_paint(cx))),
            ))
            .child(self.inspector_toggle(ctx, cx))
    }

    /// "Paint from · Click a frame", over the frames/renders switch (`tab`
    /// listed).
    pub(crate) fn render_paint_from_header(
        &self,
        tab: SourcesTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let none = self.other_stack_renders(cx).is_empty();
        let hint = match tab {
            SourcesTab::Frames => say!(cx, "retouch-click-a-frame"),
            SourcesTab::Renders => say!(cx, "retouch-click-a-render"),
        };
        let weak = cx.entity().downgrade();
        let switch = Segmented::new("paint-sources")
            .full_width()
            .child(Segment::new(say!(cx, "retouch-sources-frames")).tooltip(|window, cx| {
                Tooltip::new(say!(cx, "retouch-sources-frames-tooltip")).build(window, cx)
            }))
            .child(
                Segment::new(say!(cx, "retouch-sources-renders"))
                    .disabled(none && tab != SourcesTab::Renders)
                    .tooltip(move |window, cx| {
                        let tip = if none {
                            say!(cx, "retouch-sources-renders-none")
                        } else {
                            say!(cx, "retouch-sources-renders-tooltip")
                        };
                        Tooltip::new(tip).build(window, cx)
                    }),
            )
            .selected_index(match tab {
                SourcesTab::Frames => 0,
                SourcesTab::Renders => 1,
            })
            .on_click(move |ix, _, cx| {
                let tab = if *ix == 0 { SourcesTab::Frames } else { SourcesTab::Renders };
                weak.update(cx, |this, cx| this.set_sources_tab(tab, cx)).ok();
            });
        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(say!(cx, "retouch-paint-from")),
                    )
                    .child(div().text_xs().text_color(muted).child(hint)),
            )
            .child(div().debug_selector(|| "paint-sources".into()).child(switch))
    }

    /// The stack's other renders as their tiles read ("#3 ★ · sharper",
    /// "Depth · R8 S2.5 B3"); a click paints from one, exposure-matched.
    pub(crate) fn render_paint_renders(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let renders = self.other_stack_renders(cx);
        let theme = cx.theme();
        let (muted, blank) = (theme.muted_foreground, theme.muted);
        if renders.is_empty() {
            return div()
                .flex_1()
                .text_xs()
                .text_color(muted)
                .child(say!(cx, "retouch-sources-renders-none"))
                .into_any_element();
        }
        let unread: Vec<RenderId> = {
            let history = self.renders.read(cx);
            renders.iter().filter(|r| history.thumbnail(&r.id).is_none()).map(|r| r.id.clone())
        }
        .collect();
        if !unread.is_empty() {
            self.renders.update(cx, |r, cx| r.request_thumbnails(unread, cx));
        }
        let from = match self.paint.read(cx).source() {
            Some(SourceKind::Render { render }) => Some(render.clone()),
            _ => None,
        };
        let mut list = v_flex()
            .id("paint-renders")
            .debug_selector(|| "paint-renders".into())
            .flex_1()
            .min_h_0()
            .gap_0p5()
            .overflow_y_scroll();
        for render in renders {
            let selected = from.as_deref() == Some(render.id.as_str());
            let aspect =
                if render.height == 0 { 1.5 } else { render.width as f32 / render.height as f32 };
            let width =
                (PAINT_THUMB_HEIGHT * aspect).clamp(PAINT_THUMB_HEIGHT, 2.0 * PAINT_THUMB_HEIGHT);
            let thumbnail = self.renders.read(cx).thumbnail(&render.id);
            let title = render_title(cx.wording(), &render);
            let settings = render.config.summary(crate::words::of(cx));
            let id = render.id.clone();
            list =
                list.child(
                    h_flex()
                        .id(SharedString::from(format!("paint-render-{}", id.as_str())))
                        .debug_selector({
                            let id = id.clone();
                            move || format!("paint-render-{}", id.as_str())
                        })
                        .gap_2()
                        .p_1()
                        .items_center()
                        .rounded_md()
                        .pickable(selected, true, cx)
                        .child(
                            thumb_box(thumbnail, blank)
                                .flex_shrink_0()
                                .w(px(width))
                                .h(px(PAINT_THUMB_HEIGHT))
                                .rounded_sm(),
                        )
                        .child(
                            v_flex().flex_1().min_w_0().gap_0p5().child(render_line(title)).child(
                                div().text_xs().text_color(muted).truncate().child(settings),
                            ),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.compare_with_render(id.clone(), window, cx)
                        })),
                );
        }
        list.into_any_element()
    }

    /// "• Painted from · 3 frames", under the list.
    pub(crate) fn render_paint_from_footer(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border, fg) = (theme.muted_foreground, theme.sidebar_border, theme.foreground);
        let painted_from = self.paint.read(cx).painted_from();
        let text = if painted_from.is_empty() {
            say!(cx, "retouch-nothing-painted")
        } else {
            let (frames, renders) = source_counts(painted_from);
            say!(cx, "retouch-painted-from", frames = frames, renders = renders)
        };
        h_flex()
            .pt_2()
            .gap_1p5()
            .items_center()
            .border_t_1()
            .border_color(border)
            .text_xs()
            .text_color(muted)
            .when(!painted_from.is_empty(), |el| {
                el.child(div().size(px(6.0)).rounded_full().bg(fg))
            })
            .child(text)
    }

    /// Over the photo shown alone: what it is ("Painted · #4", or the source
    /// while \ is held) and how to see the source.
    pub(super) fn render_retouch_labels(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let viewer = self.viewer.read(cx);
        let peeking = viewer.peeking();
        if !self.is_retouching() || (viewer.mode() != ViewMode::Result && !peeking) {
            return None;
        }
        let what: SharedString = if peeking {
            let name = self.paint_source_text(cx);
            say!(cx, "paint-source", name = name.as_ref()).into()
        } else {
            match self.retouched(cx) {
                Some(r) => say!(cx, "retouch-painted-render", number = r.number).into(),
                None => say!(cx, "paint-painted").into(),
            }
        };
        // The hint is one sentence with the key in it, drawn as a key cap.
        let hint = say!(cx, "retouch-hold-to-see-source", key = HOLD_KEY);
        let (before, after) = hint.split_once(HOLD_KEY).unwrap_or((&hint, ""));
        let (before, after) = (before.trim().to_string(), after.trim().to_string());
        let chip = || {
            overlay_surface(Fill::Hud, cx)
                .flex()
                .flex_row()
                .gap_1()
                .items_center()
                .px_1p5()
                .py_0p5()
                .text_xs()
        };
        Some(
            h_flex()
                .absolute()
                .top_2()
                .left_2()
                .right_2()
                .justify_between()
                .child(chip().font_weight(FontWeight::SEMIBOLD).child(what))
                .when(!peeking, |el| {
                    el.child(chip().child(before).child(key_cap(HOLD_KEY, cx)).child(after))
                }),
        )
    }

    /// ⌥-click: paint from the frame the result took that pixel from.
    pub(super) fn pick_sharpest(
        &mut self,
        x: u32,
        y: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_retouching() {
            return;
        }
        // One the render can't tell is said by the session.
        self.paint.update(cx, |p, cx| p.pick_at(x, y, cx));
        let picked = match self.paint.read(cx).source() {
            Some(SourceKind::Frame { path }) => Some(path.clone()),
            _ => None,
        };
        self.highlight_paint_frame(picked.as_deref(), window, cx);
    }

    /// `path`'s row highlighted in the list; none when it is not listed.
    fn highlight_paint_frame(
        &mut self,
        path: Option<&std::path::Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = path.and_then(|p| self.files.read(cx).row_of(p));
        if self.files.read(cx).selected(cx) != row {
            self.files.update(cx, |f, cx| f.highlight(row, window, cx));
        }
    }

    /// Hold \: the source in place of the painted result.
    pub(super) fn peek_source(&mut self, peek: bool, cx: &mut Context<Self>) {
        let peek = peek && self.is_retouching();
        self.viewer.update(cx, |v, cx| v.set_peek(peek, cx));
    }

    /// The brush: Brush or Eraser, sliders, Only Where Colors Match and keys
    /// (Undo, Redo and Done are in the title bar).
    pub(super) fn render_paint_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let paint = self.paint.read(cx);
        let brush = paint.current_brush(cx);
        let b = &paint.brush;
        let (size, hardness, opacity, flow, tolerance) = (
            b.size.clone(),
            b.hardness.clone(),
            b.opacity.clone(),
            b.flow.clone(),
            b.tolerance.clone(),
        );
        let tolerance_on = b.tolerance_on;
        let erasing = paint.source() == Some(&deptheos_app::paint::SourceKind::Original);
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.sidebar_border);
        let section = move || v_flex().gap_2().pb_3().border_b_1().border_color(border);
        let percent = |v: f32| say!(cx, "unit-percent", value = format!("{:.0}", v * 100.0));
        let weak = cx.entity().downgrade();
        let tool = |label: String, key: &'static str, cx: &App| {
            Segment::new(label).suffix(div().ml_1().child(key_cap(key, cx)))
        };
        let tools = Segmented::new("paint-tool")
            .full_width()
            .child(tool(say!(cx, "sidebar-brush"), "B", cx).tooltip(|window, cx| {
                Tooltip::new(say!(cx, "sidebar-brush-tooltip"))
                    .action(&BrushTool, Some(PAINT_KEY_CONTEXT))
                    .build(window, cx)
            }))
            .child(tool(say!(cx, "sidebar-eraser"), "E", cx).tooltip(|window, cx| {
                Tooltip::new(say!(cx, "sidebar-eraser-tooltip"))
                    .action(&Eraser, Some(PAINT_KEY_CONTEXT))
                    .build(window, cx)
            }))
            .selected_index(if erasing { 1 } else { 0 })
            .on_click(move |ix, _, cx| {
                let ix = *ix;
                weak.update(cx, |this, cx| {
                    this.paint.update(cx, |p, cx| {
                        if ix == 0 { p.pick_brush(cx) } else { p.pick_original(cx) }
                    })
                })
                .ok();
            });
        let keys: [(&[&'static str], String); 8] = [
            (&["[", "]"], say!(cx, "sidebar-key-size")),
            (&["{", "}"], say!(cx, "sidebar-key-hardness")),
            (&[ALT], say!(cx, "sidebar-key-pick-frame")),
            (&["←", "→"], say!(cx, "sidebar-key-frame")),
            (&["1", "0"], say!(cx, "sidebar-key-opacity")),
            (&["\\"], say!(cx, "sidebar-key-peek")),
            (&["Space"], say!(cx, "sidebar-key-pan")),
            (&["Esc"], say!(cx, "sidebar-key-done")),
        ];
        // One key per row: caps in their own column (two caps or "Esc"
        // fit), the hint beside, wrapping.
        let key_row = |ix: usize, caps: &[&'static str], what: String, cx: &App| {
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    h_flex()
                        .w(px(44.0))
                        .flex_shrink_0()
                        .gap_1()
                        .children(caps.iter().map(|c| key_cap(c, cx))),
                )
                .child(
                    div()
                        .debug_selector(move || format!("paint-key-{ix}"))
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(muted)
                        .child(what),
                )
        };
        let mut grid = v_flex().gap_1p5();
        for (ix, (caps, what)) in keys.into_iter().enumerate() {
            grid = grid.child(key_row(ix, caps, what, cx));
        }
        v_flex()
            .gap_3()
            .child(section().child(tools).child(
                div().text_xs().text_color(muted).child(say!(cx, "sidebar-paint-tools-note")),
            ))
            .child(
                section()
                    .child(labeled_slider(
                        "brush-size",
                        say!(cx, "sidebar-size"),
                        say!(cx, "unit-px", value = format!("{:.0}", brush.radius)),
                        say!(cx, "sidebar-size-tooltip"),
                        &size,
                    ))
                    .child(labeled_slider(
                        "brush-hardness",
                        say!(cx, "sidebar-hardness"),
                        percent(brush.hardness),
                        say!(cx, "sidebar-hardness-tooltip"),
                        &hardness,
                    ))
                    .child(labeled_slider(
                        "brush-opacity",
                        say!(cx, "sidebar-opacity"),
                        percent(brush.opacity),
                        say!(cx, "sidebar-opacity-tooltip"),
                        &opacity,
                    ))
                    .child(labeled_slider(
                        "brush-flow",
                        say!(cx, "sidebar-flow"),
                        percent(brush.flow),
                        say!(cx, "sidebar-flow-tooltip"),
                        &flow,
                    )),
            )
            .child(
                section()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_sm().child(say!(cx, "sidebar-colors-match")))
                            .child(
                                Switch::new("paint-tolerance")
                                    .checked(tolerance_on)
                                    .small()
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        let on = *checked;
                                        this.paint.update(cx, |p, cx| p.set_tolerance_on(on, cx))
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(say!(cx, "sidebar-colors-match-note")),
                    )
                    .when(tolerance_on, |el| {
                        let t = brush.tolerance.unwrap_or_default();
                        el.child(labeled_slider(
                            "brush-within",
                            say!(cx, "sidebar-within"),
                            percent(t),
                            say!(cx, "sidebar-within-tooltip"),
                            &tolerance,
                        ))
                    }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(say!(cx, "sidebar-keys")),
                    )
                    .child(grid),
            )
    }

    /// "12 strokes from 3 frames" (or the source loading) and Reset….
    pub(super) fn render_paint_footer(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let paint = self.paint.read(cx);
        let loading = paint.state().is_some_and(|s| !s.ready);
        let painted_from = paint.painted_from();
        let [undo, redo] = paint.steps();
        let painted = paint.is_painted();
        let w = cx.wording();
        let text = if paint.is_opening() {
            say!(w, "sidebar-paint-opening")
        } else if loading {
            say!(w, "sidebar-paint-loading")
        } else if !painted {
            say!(w, "sidebar-paint-nothing")
        } else if painted_from.is_empty() {
            strokes(undo, w)
        } else {
            let (frames, renders) = source_counts(painted_from);
            let strokes = undo;
            say!(
                w,
                "sidebar-paint-strokes-from",
                strokes = strokes,
                frames = frames,
                renders = renders
            )
        };
        let can_reset = painted || undo > 0 || redo > 0;
        let theme = cx.theme();
        h_flex()
            .pt_2()
            .gap_2()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(div().text_xs().text_color(theme.muted_foreground).child(text))
            .child(
                Button::new("paint-reset")
                    .label(say!(cx, "sidebar-paint-reset"))
                    .small()
                    .ghost()
                    .text_color(theme.danger)
                    .tooltip(say!(cx, "sidebar-paint-reset-tooltip"))
                    .disabled(!can_reset)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_reset(window, cx))),
            )
    }

    /// Ask, then restore the original and clear the history: Reset can't be
    /// undone.
    fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paint = self.paint.downgrade();
        crate::form::ask_destructive(
            say!(cx, "sidebar-reset-title"),
            say!(cx, "sidebar-reset-text"),
            say!(cx, "sidebar-reset-confirm"),
            move |_, cx| {
                paint.update(cx, |p, cx| p.reset(cx)).ok();
            },
            window,
            cx,
        );
    }
}

/// The key held to see the source.
const HOLD_KEY: &str = "\\";

/// Undo's tooltip: "Undo Paint · 3 steps".
pub(crate) fn undo_tip(label: Option<&str>, steps: usize, cx: &App) -> String {
    match label {
        Some(label) => say!(cx, "retouch-undo-tooltip", step = label, n = steps),
        None => say!(cx, "retouch-nothing-to-undo"),
    }
}

/// Redo's tooltip: "Redo Paint · 3 steps".
pub(crate) fn redo_tip(label: Option<&str>, steps: usize, cx: &App) -> String {
    match label {
        Some(label) => say!(cx, "retouch-redo-tooltip", step = label, n = steps),
        None => say!(cx, "retouch-nothing-to-redo"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_summed_up_by_kind() {
        let w = &DesktopWords::english();
        let frame = |p: &str| SourceKind::Frame { path: p.into() };
        let render = SourceKind::Render { render: "r".into() };
        let painted_from = |kinds: &[SourceKind]| {
            let (frames, renders) = source_counts(kinds);
            say!(w, "retouch-painted-from", frames = frames, renders = renders)
        };
        assert_eq!(painted_from(&[frame("a")]), "Painted from · 1 frame");
        assert_eq!(painted_from(&[frame("a"), frame("b"), frame("c")]), "Painted from · 3 frames");
        assert_eq!(painted_from(std::slice::from_ref(&render)), "Painted from · 1 render");
        assert_eq!(
            painted_from(&[frame("a"), frame("b"), render]),
            "Painted from · 2 frames and 1 render"
        );
        let from = say!(w, "sidebar-paint-strokes-from", strokes = 1, frames = 2, renders = 0);
        assert_eq!(from, "1 stroke from 2 frames");
        assert_eq!(strokes(1, w), "1 stroke");
        assert_eq!(strokes(12, w), "12 strokes");
    }
}
