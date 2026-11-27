//! Adjust around the photo: title buttons, the slider panel and its
//! histogram in the inspector, Before held, the eyedropper, and Copy and
//! Paste Adjustments. The session is `crate::adjust`'s.

use deptheos_app::adjust::histogram::{BINS, Histogram};
use deptheos_app::adjust::machine::Phase;
use deptheos_app::adjust::{Group, Slider};
use deptheos_app::library::history::RenderId;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, FontWeight, Hsla, MouseButton, Pixels, SharedString, Window, canvas, div,
    fill, point, px, size,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape as InputEscape, Input};
use gpui_kit::component::slider::Slider as Track;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};

use super::AppView;
use super::actions::*;
use super::retouch::{key_cap, redo_tip, undo_tip};
use crate::adjust::{Adjust, AdjustTool};
use crate::r#type::TypeStyled as _;
use crate::viewer::Tool;
use crate::words::{Wording as _, say};

/// The histogram's height in the panel.
const HISTOGRAM_HEIGHT: f32 = 72.0;

impl AppView {
    /// `f` on Adjust, while it is open.
    pub(super) fn with_adjust(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Adjust, &mut Context<Adjust>),
    ) {
        if self.adjusting().is_some() {
            self.adjust.update(cx, f);
        }
    }

    /// Esc or Done: the eyedropper dropped or the crop tool canceled, else
    /// Adjust left, a change in progress committed.
    pub(super) fn leave_adjust(&mut self, cx: &mut Context<Self>) {
        if self.adjusting().is_none() || self.cancel_crop(cx) {
            return;
        }
        if self.adjust.read(cx).tool() == AdjustTool::Eyedropper {
            self.set_adjust_tool(AdjustTool::Sliders, cx);
        } else {
            self.set_mode(super::mode::Mode::Viewing, cx);
        }
    }

    /// Before held up (`true`) or let go: the render without its
    /// adjustments in the picture's place.
    pub(super) fn hold_before(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.adjusting().is_some() || !held {
            self.viewer.update(cx, |v, cx| v.hold_before(held, cx));
        }
    }

    /// W or the eyedropper button: the tool on, or off again.
    pub(super) fn toggle_eyedropper(&mut self, cx: &mut Context<Self>) {
        if self.adjusting().is_none() {
            return;
        }
        let on = self.adjust.read(cx).tool() == AdjustTool::Eyedropper;
        self.set_adjust_tool(if on { AdjustTool::Sliders } else { AdjustTool::Eyedropper }, cx);
    }

    /// Adjust's tool, and the viewer's pointer with it.
    pub(crate) fn set_adjust_tool(&mut self, tool: AdjustTool, cx: &mut Context<Self>) {
        self.adjust.update(cx, |a, cx| a.set_tool(tool, cx));
        let pointer = match tool {
            AdjustTool::Sliders => Tool::Pan,
            AdjustTool::Eyedropper => Tool::Eyedropper,
            AdjustTool::Crop { aspect, .. } => Tool::Crop(aspect),
        };
        self.viewer.update(cx, |v, cx| v.set_tool(pointer, cx));
    }

    /// Whether Copy Adjustments has a render: the saved render shown, on
    /// Stacking, nothing running.
    pub(crate) fn can_copy_adjustments(&self, cx: &App) -> bool {
        self.adjusting().is_some() || self.can_paint(cx)
    }

    /// Copy Adjustments (⇧⌘C): the sliders of the render adjusted, or of
    /// the saved render shown.
    pub(super) fn copy_adjustments(&mut self, cx: &mut Context<Self>) {
        if self.adjusting().is_some() {
            self.adjust.update(cx, |a, cx| a.copy(cx));
        } else if self.can_copy_adjustments(cx)
            && let Some((_, render)) = self.result_doc(cx)
        {
            self.renders.update(cx, |r, cx| r.copy_adjustments(render, cx));
        }
        cx.notify();
    }

    /// Paste Adjustments (⇧⌘V): onto the render adjusted as one change, or
    /// onto the saved render shown; either keeps its crop.
    pub(super) fn paste_adjustments(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(copied) = crate::adjust::copied(cx) else { return };
        if self.adjusting().is_some() {
            self.adjust.update(cx, |a, cx| a.paste(cx));
        } else if self.can_copy_adjustments(cx)
            && let Some((stack, render)) = self.result_doc(cx)
        {
            self.renders.update(cx, |r, cx| r.paste_adjustments(render, stack, copied, cx));
        }
    }

    /// Adjustments landed on render `id` from outside Adjust: shown again
    /// under them if it is on screen.
    pub(super) fn adjustments_pasted(
        &mut self,
        id: &RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = self.result_doc(cx).is_some_and(|(_, key)| key == *id);
        if shown && self.adjusting().is_none() && !self.is_retouching() {
            self.renders.update(cx, |r, cx| r.open(id, window, cx));
        }
    }

    /// "Adjusting #4", and the stack's name.
    fn adjust_title(&self, cx: &App) -> (SharedString, String) {
        let w = cx.wording();
        let render = self.adjusting().and_then(|a| self.renders.read(cx).find(&a.render).cloned());
        match render {
            Some(r) => (
                say!(w, "adjust-title-render", number = r.number).into(),
                self.renders.read(cx).stack_name(&r.stack, w),
            ),
            None => (say!(w, "adjust-title").into(), String::new()),
        }
    }

    pub(super) fn render_adjust_title(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let (title, what) = self.adjust_title(cx);
        super::toolbar::title_block()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_xs().text_color(muted).text_ellipsis().child(what))
    }

    /// Undo and Redo, Adjust (selected), Export, Done and the inspector toggle.
    pub(super) fn render_adjust_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let ctx = Some(ADJUST_KEY_CONTEXT);
        let [undo, redo] = self.adjust.read(cx).steps();
        let step = say!(cx, "adjust-step");
        let undo_tip = undo_tip((undo > 0).then_some(step.as_str()), undo, cx);
        let redo_tip = redo_tip((redo > 0).then_some(step.as_str()), redo, cx);
        let can_export = self.can_export(cx);
        let no_drag = super::toolbar::no_drag;
        h_flex()
            .gap_1p5()
            .flex_shrink_0()
            .child(no_drag(
                "adjust-undo",
                Button::new("adjust-undo")
                    .icon(IconName::Undo2)
                    .ghost()
                    .disabled(undo == 0)
                    .tooltip_with_action(undo_tip, &Undo, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.with_adjust(cx, |a, _| a.undo()))),
            ))
            .child(no_drag(
                "adjust-redo",
                Button::new("adjust-redo")
                    .icon(IconName::Redo2)
                    .ghost()
                    .disabled(redo == 0)
                    .tooltip_with_action(redo_tip, &Redo, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.with_adjust(cx, |a, _| a.redo()))),
            ))
            .child(self.adjust_button(say!(cx, "adjust-leave-tooltip"), ctx, cx))
            .child(self.export_button(say!(cx, "adjust-export-tooltip"), can_export, ctx, cx))
            .child(no_drag(
                "adjust-done",
                Button::new("adjust-done")
                    .icon(IconName::Check)
                    .label(say!(cx, "adjust-done"))
                    .primary()
                    .tooltip_with_action(say!(cx, "adjust-done-tooltip"), &LeaveAdjust, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.leave_adjust(cx))),
            ))
            .child(self.inspector_toggle(ctx, cx))
    }

    /// Adjust, beside Retouch in Stacking and selected in Adjust, where it
    /// leaves.
    pub(super) fn adjust_button(
        &self,
        tip: String,
        ctx: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let adjusting = self.adjusting().is_some();
        let blocked = self.paint_blocked(cx);
        let button = Button::new("adjust")
            .icon(IconName::SlidersHorizontal)
            .when(!self.narrow(), |b| b.label(say!(cx, "toolbar-adjust")))
            .outline()
            .selected(adjusting)
            .disabled(!adjusting && blocked.is_some())
            .tooltip_with_action(tip, &ToggleAdjust, ctx)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_adjust(cx)));
        super::toolbar::no_drag("adjust", button)
    }

    /// The inspector in Adjust: the histogram, then each group's sliders.
    pub(super) fn render_adjust_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let crop = self.render_crop_section(cx);
        let adjust = self.adjust.read(cx);
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.sidebar_border);
        let section = move || v_flex().gap_2().pb_3().border_b_1().border_color(border);
        let mut panel = v_flex().gap_3().child(section().child(histogram(adjust.histogram(), cx)));
        let phase = adjust.status().map(|s| s.phase.clone());
        if let Some(note) = match &phase {
            None | Some(Phase::Opening) => Some(say!(cx, "adjust-opening")),
            Some(Phase::Away) => Some(say!(cx, "adjust-away")),
            Some(Phase::Failed(why)) => Some(why.clone()),
            Some(Phase::Ready) => None,
        } {
            let failed = matches!(phase, Some(Phase::Failed(_)));
            panel = panel.child(
                section().child(div().text_xs().text_color(muted).child(note)).when(failed, |el| {
                    el.child(
                        Button::new("adjust-retry")
                            .label(say!(cx, "adjust-retry"))
                            .small()
                            .outline()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.adjust.update(cx, |a, _| a.retry())
                            })),
                    )
                }),
            );
        }
        let Some(panel_of) = adjust.panel() else { return panel };
        panel = panel.child(section().child(crop));
        let eyedropping = adjust.tool() == AdjustTool::Eyedropper;
        for (group, sliders) in panel_of.groups() {
            let mut heading = h_flex().justify_between().items_center().child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(group.label(cx.wording().shared())),
            );
            if group == Group::WhiteBalance {
                heading = heading.child(
                    Button::new("adjust-eyedropper")
                        .icon(IconName::Pipette)
                        .xsmall()
                        .ghost()
                        .selected(eyedropping)
                        .tooltip_with_action(
                            say!(cx, "adjust-eyedropper-tooltip"),
                            &Eyedropper,
                            Some(ADJUST_KEY_CONTEXT),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_eyedropper(cx))),
                );
            }
            let mut rows = section().child(heading);
            for slider in sliders {
                rows = rows.child(self.slider_row(slider, window, cx));
            }
            panel = panel.child(rows);
        }
        panel.child(self.render_adjust_keys(cx))
    }

    /// One slider: its name (double-click resets it), its value (click to
    /// type one) and its track.
    fn slider_row(
        &self,
        slider: Slider,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let adjust = self.adjust.read(cx);
        let words = cx.wording().shared().clone();
        let panel = adjust.panel().expect("rows are of a panel");
        let value = adjust.value(slider).unwrap_or_else(|| slider.neutral(&panel));
        let text = slider.value_text(value, &panel, &words);
        let help = slider.help(&words);
        let label = slider.label(&words);
        let selected = adjust.selected() == Some(slider);
        let changed = !slider.is_neutral(
            &adjust.status().map(|s| s.value.adjustments.clone()).unwrap_or_default(),
            &panel,
        );
        let track = adjust.track(slider).cloned();
        let typing = adjust.typing(slider).cloned();
        let theme = cx.theme();
        let (accent, muted, fg) = (theme.primary, theme.muted_foreground, theme.foreground);
        let id = format!("adjust-{slider:?}").to_lowercase();
        let value_el = match typing {
            Some(input) => div()
                .w(px(64.0))
                .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                    this.adjust.update(cx, |a, cx| a.finish_typing(false, window, cx))
                }))
                .child(Input::new(&input).xsmall())
                .into_any_element(),
            None => div()
                .id(SharedString::from(format!("{id}-value")))
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-value")
                })
                .tabular()
                .cursor_text()
                .text_color(if changed { fg } else { muted })
                .child(text)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.adjust.update(cx, |a, cx| a.start_typing(slider, window, cx))
                }))
                .into_any_element(),
        };
        let name = div()
            .id(SharedString::from(format!("{id}-label")))
            .debug_selector({
                let id = id.clone();
                move || format!("{id}-label")
            })
            .child(label)
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.adjust.update(cx, |a, cx| match event.click_count() {
                    2 => a.reset(slider, cx),
                    _ => a.select(slider, cx),
                })
            }));
        v_flex()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .tooltip(move |window, cx| Tooltip::new(help.clone()).build(window, cx))
            .gap_0p5()
            .pl_1p5()
            .border_l_2()
            .border_color(if selected { accent } else { gpui::transparent_black() })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.adjust.update(cx, |a, cx| a.select(slider, cx))
                }),
            )
            .child(h_flex().justify_between().text_xs().child(name).child(value_el))
            .children(track.map(|t| Track::new(&t)))
    }

    /// The keys Adjust takes, under the sliders.
    fn render_adjust_keys(&self, cx: &App) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let cropping = matches!(self.adjust.read(cx).tool(), AdjustTool::Crop { .. });
        let keys: Vec<(&[&'static str], String)> = if cropping {
            vec![
                (&["X"], say!(cx, "adjust-key-aspect-swap")),
                (&["⌘[", "⌘]"], say!(cx, "adjust-key-turn")),
                (&["↩"], say!(cx, "adjust-key-crop-done")),
                (&["Esc"], say!(cx, "adjust-key-crop-cancel")),
            ]
        } else {
            vec![
                (&["↑", "↓"], say!(cx, "adjust-key-slider")),
                (&["←", "→"], say!(cx, "adjust-key-nudge")),
                (&["\\"], say!(cx, "adjust-key-before")),
                (&["W"], say!(cx, "adjust-key-eyedropper")),
                (&["C"], say!(cx, "adjust-key-crop")),
                (&["⌘[", "⌘]"], say!(cx, "adjust-key-turn")),
                (&["Esc"], say!(cx, "adjust-key-done")),
            ]
        };
        let mut grid = v_flex()
            .gap_1p5()
            .child(div().text_xs().text_color(muted).child(say!(cx, "adjust-reset-hint")));
        for (caps, what) in keys {
            grid = grid.child(
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
                    .child(div().flex_1().min_w_0().text_xs().text_color(muted).child(what)),
            );
        }
        grid
    }

    /// Under the panel: Before (held) and Reset All.
    pub(super) fn render_adjust_footer(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let adjust = self.adjust.read(cx);
        let changed = adjust.status().is_some_and(|s| !s.value.adjustments.is_neutral());
        let held = self.viewer.read(cx).holding_before();
        let theme = cx.theme();
        let before = div()
            .id("adjust-before")
            .debug_selector(|| "adjust-before".into())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.hold_before(true, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.hold_before(false, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.hold_before(false, cx)),
            )
            .tooltip(|window, cx| {
                Tooltip::new(say!(cx, "adjust-before-tooltip"))
                    .action(&HoldBefore, Some(ADJUST_KEY_CONTEXT))
                    .build(window, cx)
            })
            .child(
                Button::new("adjust-before-button")
                    .icon(IconName::Eye)
                    .label(say!(cx, "adjust-before"))
                    .small()
                    .outline()
                    .selected(held),
            );
        h_flex()
            .pt_2()
            .gap_2()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(before)
            .child(
                div().debug_selector(|| "adjust-reset-all".into()).child(
                    Button::new("adjust-reset-all")
                        .icon(IconName::RotateCcw)
                        .label(say!(cx, "adjust-reset-all"))
                        .small()
                        .ghost()
                        .disabled(!changed)
                        .tooltip(say!(cx, "adjust-reset-all-tooltip"))
                        .on_click(
                            cx.listener(|this, _, _, cx| {
                                this.with_adjust(cx, |a, _| a.reset_all())
                            }),
                        ),
                ),
            )
    }
}

/// The histogram of the picture shown: red, green and blue over the luma,
/// a corner lit for each end clipped.
fn histogram(histogram: Option<&Histogram>, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (bg, border, muted) = (theme.secondary, theme.border, theme.muted_foreground);
    let clipping =
        histogram.map(|h| (h.clipping.shadows_clipped(), h.clipping.highlights_clipped()));
    let (shadows, highlights) = clipping.unwrap_or((false, false));
    let drawn = histogram.map(|h| (h.channels, h.luminance));
    let corner = |id: &'static str, on: bool, tip: String| {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .size(px(8.0))
            .rounded(px(2.0))
            .border_1()
            .border_color(muted)
            .when(on, |el| el.bg(gpui::white()))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(corner(
                    "adjust-shadows-clipped",
                    shadows,
                    say!(cx, "adjust-shadows-clipped"),
                ))
                .child(corner(
                    "adjust-highlights-clipped",
                    highlights,
                    say!(cx, "adjust-highlights-clipped"),
                )),
        )
        .child(
            div()
                .id("adjust-histogram")
                .debug_selector(|| "adjust-histogram".into())
                .h(px(HISTOGRAM_HEIGHT))
                .w_full()
                .rounded(px(4.0))
                .border_1()
                .border_color(border)
                .bg(bg)
                .child(
                    canvas(
                        |_, _, _| (),
                        move |b, _, window, _| {
                            if let Some((channels, luminance)) = &drawn {
                                paint_histogram(b, channels, luminance, window);
                            }
                        },
                    )
                    .size_full(),
                ),
        )
}

/// The bins as columns over `b`, each count's square root against the
/// tallest's, the ends' excepted so a clipped end doesn't flatten the rest.
fn paint_histogram(
    b: Bounds<Pixels>,
    channels: &[[u32; BINS]; 3],
    luminance: &[u32; BINS],
    window: &mut Window,
) {
    let inner = &channels.iter().chain([luminance]).flat_map(|c| c[1..BINS - 1].iter()).max();
    let top = inner.map_or(1, |m| (*m).max(1)) as f32;
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let column = w / BINS as f32;
    let colors: [Hsla; 4] = [
        gpui::opaque_grey(0.6, 0.55),
        gpui::rgba(0xff505099).into(),
        gpui::rgba(0x50e05099).into(),
        gpui::rgba(0x5080ff99).into(),
    ];
    for (counts, color) in [luminance].into_iter().chain(channels.iter()).zip(colors) {
        for (i, &n) in counts.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let tall = ((n as f32 / top).sqrt() * h).min(h);
            let bar = Bounds {
                origin: point(b.origin.x + px(i as f32 * column), b.origin.y + px(h - tall)),
                size: size(px(column.max(1.0)), px(tall)),
            };
            window.paint_quad(fill(bar, color));
        }
    }
}
