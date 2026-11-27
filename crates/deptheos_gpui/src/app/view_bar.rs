//! The view bar along the bottom of the photo: the home control (the render
//! shown and its stack's renders), the focus rail (`scrubber.rs`), Compare
//! with its layouts, Swap while a render is compared, Contribution, the
//! depth map, zoom, and focus mode. Every tooltip gives its key.
//!
//! In Retouch, Paint From (titled with the source kind, joined to the rail)
//! leads instead; Compare lays out the source (Painted first, no Flicker,
//! no keys since they set opacity there); Contribution is off; the map is
//! lit for the frame painted from.
//!
//! While the stack on screen runs, the bar is the read-only rail with zoom
//! and focus mode. As the window narrows Compare loses its word, then the
//! readout goes, Paint From shows glyphs, and the rail shrinks to its least.

use deptheos_app::paint::SourceKind;
use deptheos_app::view::{Compare, Target};
use gpui::prelude::*;
use gpui::{App, Context, Div, SharedString, div, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape as InputEscape, Input};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Side, Sizable as _, h_flex,
};

use super::AppView;
use super::actions::*;
use super::scrubber::RailOf;
use crate::AppIcon;
use crate::look::{Fill, overlay_surface};
use crate::paint::SourcesTab;
use crate::r#type::TypeStyled as _;
use crate::viewer::Look;
use crate::words::{Wording as _, say};

/// Paint From's minimum width, so the rail holds still as its title changes.
const PAINT_FROM_MIN: f32 = 112.0;
/// The same in a cramped window, where Frames and Original are glyphs:
/// room for glyph and caret.
const PAINT_FROM_COMPACT_MIN: f32 = 48.0;

/// What leads the bar, derived each time from the stacking model and Retouch.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Lead {
    /// Nothing to look through yet: zoom and focus mode alone.
    Bare,
    /// The stack on screen runs: its read-only rail, the live key.
    Run,
    /// The result, with the home control.
    Home,
    /// Retouch, with Paint From.
    PaintFrom(PaintFrom),
}

/// Paint From's title: the source kind, derived from
/// [`crate::paint::Paint::source`] each time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PaintFrom {
    /// A frame, which the rail marks (or no source yet).
    Frames,
    /// Another render of the stack, as the bar names them ("#5 ★").
    Render(SharedString),
    /// The original, painted back.
    Original,
}

impl AppView {
    /// The bar. Before a result only zoom and focus mode are there.
    pub(super) fn render_view_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let zoom = self.viewer.read(cx).zoom_percent();
        let lead = self.lead(cx);
        let ctx = Some(KEY_CONTEXT);
        let theme = cx.theme();
        let border = theme.border;

        let button = |id: &'static str, icon: IconName, tip: String, action: &dyn gpui::Action| {
            Button::new(id).icon(icon).small().ghost().tooltip_with_action(tip, action, ctx)
        };
        let divider = move || div().flex_shrink_0().w(px(1.0)).h(px(18.0)).mx_1().bg(border);

        let this = cx.entity().downgrade();
        let focus = self.focus_handle.clone();
        // Type a percentage and Enter; Escape or a click away reverts. The
        // caret holds presets.
        let zoom_field = div()
            .id("zoom-field")
            .tabular()
            .w(px(56.0))
            .on_action(
                cx.listener(|this, _: &InputEscape, window, cx| this.cancel_zoom_input(window, cx)),
            )
            .tooltip(|window, cx| Tooltip::new(say!(cx, "view-bar-zoom-field")).build(window, cx))
            .child(Input::new(&self.zoom_input).xsmall().disabled(zoom <= 0));
        let presets_focus = focus.clone();
        let zoom_menu = Button::new("zoom-presets")
            .icon(IconName::ChevronDown)
            .xsmall()
            .ghost()
            .tooltip(say!(cx, "view-bar-zoom-presets"))
            .dropdown_menu(move |menu, _, cx| {
                let at = |percent: f32| {
                    let this = this.clone();
                    PopupMenuItem::new(say!(cx, "unit-percent", value = percent.to_string()))
                        .on_click(move |_, _, cx| {
                            this.update(cx, |this, cx| {
                                this.zoom(|v, at| v.zoom_to_percent(percent, at), cx)
                            })
                            .ok();
                        })
                };
                menu.action_context(presets_focus.clone())
                    .menu(say!(cx, "view-bar-zoom-to-fit"), Box::new(ZoomToFit))
                    .menu(say!(cx, "view-bar-actual-size"), Box::new(ZoomTo100))
                    .separator()
                    .item(at(25.0))
                    .item(at(50.0))
                    .item(at(200.0))
                    .item(at(400.0))
            });
        let zoom_group = h_flex()
            .flex_shrink_0()
            .gap_0p5()
            .items_center()
            .child(
                button("zoom-out", IconName::Minus, say!(cx, "view-bar-zoom-out"), &ZoomOut)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(|v, at| v.zoom_out(at), cx))),
            )
            .child(zoom_field)
            .child(zoom_menu)
            .child(
                button("zoom-in", IconName::Plus, say!(cx, "view-bar-zoom-in"), &ZoomIn)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(|v, at| v.zoom_in(at), cx))),
            );
        let focus_mode = button(
            "focus-mode",
            IconName::PanelBottom,
            say!(cx, "view-bar-focus-mode"),
            &ToggleFocusMode,
        )
        .selected(self.chrome.is_focus())
        .on_click(cx.listener(|this, _, window, cx| this.toggle_focus_mode(window, cx)));

        let bar = overlay_surface(Fill::Controls, cx)
            .flex()
            .flex_row()
            .id("view-bar")
            .debug_selector(|| "view-bar".into())
            .occlude()
            .min_w_0()
            .p_1()
            .gap_0p5()
            .items_center();
        // With a rail the bar spans the photo; the rail takes the room left.
        let rail = match lead {
            Lead::Bare => None,
            Lead::Run | Lead::Home | Lead::PaintFrom(_) => self.render_rail_and_readout(cx),
        };
        let bar = bar.when(rail.is_some(), |bar| bar.flex_1());
        // In Retouch Paint From takes the home control's place, its layouts
        // Compare's.
        let lenses = matches!(lead, Lead::Home | Lead::PaintFrom(_));
        let bar = match lead {
            Lead::Bare => bar,
            Lead::Run => bar.children(rail).child(divider()),
            Lead::Home => bar.child(self.render_home(cx)).children(rail),
            Lead::PaintFrom(from) => bar.child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .items_center()
                    .debug_selector(|| "paint-from-rail".into())
                    .child(self.render_paint_from(from, cx))
                    .children(rail),
            ),
        };
        let bar = match lenses {
            false => bar,
            true => bar.child(divider()).child(self.render_lenses(cx)).child(divider()),
        };
        bar.child(zoom_group).child(divider()).child(focus_mode)
    }

    /// What leads now: a run's rail while the stack on screen runs, else the
    /// result's with home control or Paint From.
    fn lead(&self, cx: &App) -> Lead {
        if self.stacking.read(cx).taken().is_some() {
            return Lead::Run;
        }
        if !self.has_result(cx) {
            return Lead::Bare;
        }
        match self.paint_from(cx) {
            None => Lead::Home,
            Some(from) => Lead::PaintFrom(from),
        }
    }

    /// The bar is up: not under a run's card, except for a run of the stack
    /// on screen, whose rail is the key under the card.
    pub(super) fn view_bar_shown(&self, cx: &App) -> bool {
        let stacking = self.stacking.read(cx);
        !stacking.is_running() || stacking.taken().is_some()
    }

    /// The home control: the render shown ("#3 ★"), a click back to it
    /// alone, and a menu of the stack's renders with two-line descriptive rows.
    /// Never lit: the render alone is the resting view (ADR-149).
    fn render_home(&self, cx: &mut Context<Self>) -> Div {
        let history = self.renders.read(cx);
        let shown = history.active(cx).and_then(|id| history.find(id)).cloned();
        let compared = history.compare().cloned();
        let w = crate::words::of(cx);
        let (label, shown_subtitle): (SharedString, Option<SharedString>) = match &shown {
            Some(r) => {
                let name = deptheos_app::library::history::render_name(
                    w,
                    r.number,
                    r.name.as_deref(),
                    r.picked,
                );
                (name.into(), Some(r.config.summary(w).into()))
            }
            None => (say!(cx, "view-bar-home-result").into(), None),
        };
        let renders = self.stack_render_rows(cx);

        let body = Button::new("view-home")
            .icon(AppIcon::Render)
            .label(label.clone())
            .small()
            .ghost()
            .tooltip_with_action(say!(cx, "view-bar-home-tooltip"), &ViewResult, Some(KEY_CONTEXT))
            .on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx)));
        let weak = cx.entity().downgrade();
        let menu = Button::new("view-home-menu")
            .icon(IconName::ChevronDown)
            .xsmall()
            .ghost()
            .tooltip(say!(cx, "view-bar-home-menu"))
            .dropdown_menu(move |menu, _, _| {
                // The checkmark on the right, so the render glyph can lead.
                let mut menu = menu.min_w(px(260.0)).check_side(Side::Right);
                let home = weak.clone();
                let title = label.clone();
                let subtitle = shown_subtitle.clone();
                menu = menu.item(
                    render_menu_item(title, subtitle).checked(compared.is_none()).on_click(
                        move |_, window, cx| {
                            home.update(cx, |this, cx| this.go_home(window, cx)).ok();
                        },
                    ),
                );
                if !renders.is_empty() {
                    menu = menu.separator();
                }
                for (render, title, subtitle) in renders.iter().cloned() {
                    let weak = weak.clone();
                    let checked = compared.as_ref() == Some(&render);
                    menu = menu.item(
                        render_menu_item(title, Some(subtitle)).checked(checked).on_click(
                            move |_, window, cx| {
                                let render = render.clone();
                                weak.update(cx, |this, cx| {
                                    this.pick_home_render(render, window, cx)
                                })
                                .ok();
                            },
                        ),
                    );
                }
                menu
            });
        h_flex()
            .flex_shrink_0()
            .debug_selector(|| "view-home".into())
            .items_center()
            .child(div().debug_selector(|| "view-home-body".into()).child(body))
            .child(menu)
    }

    /// A render picked in the home menu: compared with while a render is
    /// compared, else shown (a frame compared is let go).
    pub(super) fn pick_home_render(
        &mut self,
        render: deptheos_app::library::history::RenderId,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let comparing_render = matches!(
            self.viewer.read(cx).mode().comparing(),
            Some(Compare { target: Target::Render(_), .. })
        );
        if comparing_render {
            self.compare_with_render(render, window, cx)
        } else {
            self.renders.update(cx, |r, cx| r.click_tile(&render, window, cx))
        }
    }

    /// Back home: the result alone, a render compared let go.
    pub(super) fn go_home(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.renders.read(cx).compare().is_some() {
            self.stop_render_compare(cx);
        } else {
            self.set_view(Look::Result, window, cx);
        }
    }

    /// Paint From's title (`None` outside Retouch), derived each time.
    pub(super) fn paint_from(&self, cx: &App) -> Option<PaintFrom> {
        let painting = self.painting()?;
        let paint = self.paint.read(cx);
        Some(match paint.source() {
            None | Some(SourceKind::Frame { .. }) => PaintFrom::Frames,
            Some(SourceKind::Original) => PaintFrom::Original,
            Some(SourceKind::Render { render }) => {
                let w = crate::words::of(cx);
                let name =
                    self.renders.read(cx).renders().iter().find(|r| r.id.as_str() == render).map(
                        |r| {
                            deptheos_app::library::history::render_name(
                                w,
                                r.number,
                                r.name.as_deref(),
                                r.picked,
                            )
                        },
                    );
                PaintFrom::Render(name.map_or_else(
                    || paint.source_name(painting, cx).unwrap_or_default(),
                    Into::into,
                ))
            }
        })
    }

    /// Retouch's Paint From, joined to the rail: it picks the source kind,
    /// the rail the frame. Titled with the source (never the retouched
    /// render), its menu lists Frames, other renders and the original (E).
    /// Cramped, Frames and Original are glyphs; a render keeps its name.
    fn render_paint_from(&self, from: PaintFrom, cx: &mut Context<Self>) -> Div {
        let renders = self.stack_render_rows(cx);
        let cramped = self.cramped();
        let compact = cramped && !matches!(from, PaintFrom::Render(_));
        let (icon, label, lit): (Icon, SharedString, bool) = match &from {
            PaintFrom::Frames => {
                (Icon::new(IconName::Images), say!(cx, "view-bar-paint-from-frames").into(), false)
            }
            PaintFrom::Render(name) => (Icon::new(AppIcon::Render), name.clone(), true),
            PaintFrom::Original => {
                (Icon::new(IconName::Eraser), say!(cx, "view-bar-paint-from-original").into(), true)
            }
        };
        let painted_render = match self.paint.read(cx).source() {
            Some(SourceKind::Render { render }) => Some(render.clone()),
            _ => None,
        };
        let weak = cx.entity().downgrade();
        let focus = self.focus_handle.clone();
        let button = Button::new("view-paint-from")
            .child(h_flex().gap_1().min_w_0().items_center().child(icon.small()).when(
                !compact,
                |el| {
                    el.child(
                        div()
                            .debug_selector(|| "view-paint-from-label".into())
                            .min_w_0()
                            .truncate()
                            .child(label),
                    )
                },
            ))
            .dropdown_caret(true)
            .min_w(px(if cramped { PAINT_FROM_COMPACT_MIN } else { PAINT_FROM_MIN }))
            .small()
            .ghost()
            .selected(lit)
            .tooltip(say!(cx, "view-bar-paint-from"))
            .dropdown_menu(move |menu, _, cx| {
                let w = cx.wording().clone();
                // The checkmark on the right, so the glyphs can lead.
                let frames = weak.clone();
                let mut menu = menu
                    .action_context(focus.clone())
                    .min_w(px(240.0))
                    .check_side(Side::Right)
                    .item(
                        PopupMenuItem::new(say!(w, "view-bar-paint-from-frames"))
                            .icon(IconName::Images)
                            .checked(from == PaintFrom::Frames)
                            .on_click(move |_, window, cx| {
                                frames
                                    .update(cx, |this, cx| this.paint_from_frames(window, cx))
                                    .ok();
                            }),
                    );
                if !renders.is_empty() {
                    menu = menu.separator();
                }
                for (render, title, subtitle) in renders.iter().cloned() {
                    let weak = weak.clone();
                    let checked = painted_render.as_deref() == Some(render.as_str());
                    menu = menu.item(
                        render_menu_item(title, Some(subtitle)).checked(checked).on_click(
                            move |_, window, cx| {
                                let render = render.clone();
                                weak.update(cx, |this, cx| {
                                    this.compare_with_render(render, window, cx)
                                })
                                .ok();
                            },
                        ),
                    );
                }
                let original = weak.clone();
                // Its action names E; the click does what E does.
                menu.separator().item(
                    PopupMenuItem::new(say!(w, "view-bar-original"))
                        .icon(IconName::Eraser)
                        .checked(from == PaintFrom::Original)
                        .action(Box::new(Eraser))
                        .on_click(move |_, _, cx| {
                            original
                                .update(cx, |this, cx| {
                                    this.with_paint(cx, |p, cx| p.pick_original(cx))
                                })
                                .ok();
                        }),
                )
            });
        div().flex_shrink_0().debug_selector(|| "view-paint-from".into()).child(button)
    }

    /// Paint From's Frames: the frame last painted from, else the stack's
    /// first; the left panel lists frames and the rail marks it.
    pub(super) fn paint_from_frames(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if !self.is_retouching() || self.is_processing(cx) {
            return;
        }
        let on_frame = matches!(self.paint.read(cx).source(), Some(SourceKind::Frame { .. }));
        self.set_sources_tab(SourcesTab::Frames, cx);
        self.paint.update(cx, |p, cx| p.use_frames(cx));
        let Some(SourceKind::Frame { path }) = self.paint.read(cx).source().cloned() else {
            return;
        };
        if !on_frame {
            let file = self.files.read(cx).row_of(&path);
            self.highlight_row(file, window, cx);
        }
    }

    /// The rail and, with room, the readout ("18/46", or frames taken in
    /// during a run).
    fn render_rail_and_readout(&self, cx: &mut Context<Self>) -> Option<Div> {
        let model = self.rail_model(cx).filter(|m| !m.paths.is_empty())?;
        let count = model.paths.len().to_string();
        let (number, tip): (String, SharedString) = match model.of {
            RailOf::Run { taken } => {
                (taken.to_string(), say!(cx, "view-bar-rail-running-tooltip").into())
            }
            RailOf::Result => (
                model.current.map_or_else(|| "–".to_string(), |i| (i + 1).to_string()),
                say!(cx, "view-bar-rail-position-tooltip", key = super::retouch::ALT).into(),
            ),
        };
        let readout = (!self.cramped()).then(|| {
            let text = say!(cx, "view-bar-rail-position", number = number, count = count.clone());
            // As wide as its widest ("46/46"), so the rail keeps its width
            // through a drag.
            let widest =
                say!(cx, "view-bar-rail-position", number = count.clone(), count = count.clone());
            div()
                .id("rail-readout")
                .flex_shrink_0()
                .debug_selector(|| "rail-readout".into())
                .relative()
                .px_1()
                .text_xs()
                .tabular()
                .text_color(cx.theme().muted_foreground)
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(div().text_color(gpui::transparent_black()).child(widest))
                .child(div().absolute().inset_0().px_1().flex().justify_end().child(text))
        });
        Some(
            h_flex()
                .flex_1()
                .min_w(px(super::scrubber::RAIL_MIN))
                .gap_1()
                .items_center()
                .px_1()
                .child(self.render_rail(model, cx))
                .children(readout),
        )
    }

    /// After the rail: Compare and layouts, Swap, Contribution and the map,
    /// with Retouch's variants (see the module doc).
    fn render_lenses(&self, cx: &mut Context<Self>) -> Div {
        let viewer = self.viewer.read(cx);
        let mode = viewer.mode();
        let comparing = mode.comparing().is_some();
        let look = mode.look();
        let map_on = mode.is_map();
        let contribution_on = self.contribution_on(cx);
        let has_contribution = self.has_contribution(cx);
        let painting = self.is_retouching();
        let history = self.renders.read(cx);
        let compared = history.compare().is_some() && !painting;
        let can_swap = history.active(cx).is_some() && !self.is_processing(cx);
        let ctx = Some(KEY_CONTEXT);
        let focus = self.focus_handle.clone();

        let compare_icon = match look {
            Some(Look::SideBySide) => IconName::Columns2,
            Some(Look::TopAndBottom) => IconName::Rows2,
            Some(Look::Flicker) => IconName::Blend,
            _ => IconName::SquareSplitHorizontal,
        };
        // Its word when the bar has room.
        let compare = Button::new("view-compare")
            .icon(compare_icon)
            .when(!self.narrow(), |b| b.label(say!(cx, "view-bar-compare")))
            .small()
            .ghost()
            .selected(comparing)
            .tooltip(if painting {
                say!(cx, "view-bar-compare-toggle-retouch")
            } else {
                say!(cx, "view-bar-compare-toggle")
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_compare(window, cx)));
        // The same rows in both; in Retouch the first is the canvas alone,
        // no Flicker, and no keys (1–0 set opacity, \ shows the source).
        let layouts = Button::new("view-layouts")
            .icon(IconName::ChevronDown)
            .xsmall()
            .ghost()
            .tooltip(say!(cx, "view-bar-layouts"))
            .dropdown_menu(move |menu, _, cx| {
                let w = cx.wording().clone();
                let item =
                    |label: String, icon: IconName, target: Look, action: Box<dyn gpui::Action>| {
                        let action: Box<dyn gpui::Action> =
                            if painting { Box::new(RetouchLayout(target)) } else { action };
                        PopupMenuItem::new(label)
                            .icon(icon)
                            .checked(look == Some(target))
                            .action(action)
                    };
                let first = if painting {
                    say!(w, "view-bar-layout-painted")
                } else {
                    say!(w, "view-bar-layout-result")
                };
                menu.action_context(focus.clone())
                    .min_w(px(200.0))
                    .check_side(Side::Right)
                    .item(item(first, IconName::Square, Look::Result, Box::new(ViewResult)))
                    .item(item(
                        say!(w, "view-bar-layout-split"),
                        IconName::SquareSplitHorizontal,
                        Look::Split,
                        Box::new(ViewCompare),
                    ))
                    .item(item(
                        say!(w, "view-bar-layout-side-by-side"),
                        IconName::Columns2,
                        Look::SideBySide,
                        Box::new(ViewSideBySide),
                    ))
                    .item(item(
                        say!(w, "view-bar-layout-top-and-bottom"),
                        IconName::Rows2,
                        Look::TopAndBottom,
                        Box::new(ViewStacked),
                    ))
                    .when(!painting, |menu| {
                        menu.item(item(
                            say!(w, "view-bar-layout-flicker"),
                            IconName::Blend,
                            Look::Flicker,
                            Box::new(Flicker),
                        ))
                    })
            });
        let swap = compared.then(|| {
            Button::new("compare-swap")
                .icon(IconName::ArrowLeftRight)
                .small()
                .ghost()
                .tooltip(say!(cx, "view-bar-swap"))
                .disabled(!can_swap)
                .on_click(cx.listener(|this, _, window, cx| this.swap_render_compare(window, cx)))
        });
        // Off in Retouch, in its place so Compare keeps its own.
        let contribution = div().debug_selector(|| "contribution".into()).child(
            Button::new("contribution")
                .icon(IconName::Highlighter)
                .small()
                .ghost()
                .selected(contribution_on && has_contribution)
                .disabled(!has_contribution)
                .tooltip_with_action(
                    if painting {
                        say!(cx, "view-bar-contribution-retouch")
                    } else if has_contribution {
                        say!(cx, "view-bar-contribution")
                    } else {
                        say!(cx, "view-bar-contribution-unavailable")
                    },
                    &ToggleContribution,
                    ctx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_contribution(window, cx))),
        );
        // While the map lights frames, its tooltip says the rail is its key.
        let can_map = self.can_show_map(cx);
        let map_tip = if !can_map {
            say!(cx, "view-bar-depth-map-unavailable")
        } else if self.lights_map(cx) && painting {
            say!(cx, "view-bar-depth-map-key-retouch")
        } else if self.lights_map(cx) {
            say!(cx, "view-bar-depth-map-key")
        } else {
            say!(cx, "view-bar-depth-map")
        };
        let map = Button::new("depth-map")
            .icon(IconName::Mountain)
            .small()
            .ghost()
            .selected(map_on)
            .disabled(!can_map)
            .tooltip_with_action(map_tip, &ToggleMap, ctx)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_map(window, cx)));
        h_flex()
            .flex_shrink_0()
            .gap_0p5()
            .items_center()
            .child(
                h_flex()
                    .items_center()
                    .child(div().debug_selector(|| "view-compare".into()).child(compare))
                    .child(div().debug_selector(|| "view-layouts".into()).child(layouts)),
            )
            .children(swap)
            .child(contribution)
            .child(map)
    }

    /// The stack on screen's other renders, newest first, to compare or
    /// paint from.
    pub(crate) fn other_stack_renders(
        &self,
        cx: &gpui::App,
    ) -> Vec<deptheos_app::library::history::Render> {
        let history = self.renders.read(cx);
        let active = history.active(cx).cloned();
        let stack =
            active.as_ref().and_then(|id| history.find(id)).map_or(&self.stack, |r| &r.stack);
        let mut renders: Vec<_> = history
            .renders()
            .iter()
            .filter(|r| r.stack == *stack && Some(&r.id) != active.as_ref())
            .cloned()
            .collect();
        renders.sort_by_key(|r| std::cmp::Reverse(r.number));
        renders
    }

    /// [`Self::other_stack_renders`] as `(id, "#3 ★ · name", "Depth · R8 S2.5 B3")`
    /// for two-line menu rows.
    pub(super) fn stack_render_rows(
        &self,
        cx: &gpui::App,
    ) -> Vec<(deptheos_app::library::history::RenderId, SharedString, SharedString)> {
        self.other_stack_renders(cx)
            .into_iter()
            .map(|r| {
                let w = crate::words::of(cx);
                let name = deptheos_app::library::history::render_name(
                    w,
                    r.number,
                    r.name.as_deref(),
                    r.picked,
                );
                (r.id.clone(), name.into(), r.config.summary(w).into())
            })
            .collect()
    }

    /// [`Self::other_stack_renders`] labeled "#3 ★ · sharper · Depth · R8 S2.5 B3".
    #[cfg(test)]
    pub(super) fn stack_renders(
        &self,
        cx: &gpui::App,
    ) -> Vec<(deptheos_app::library::history::RenderId, SharedString)> {
        self.other_stack_renders(cx)
            .into_iter()
            .map(|r| {
                let w = crate::words::of(cx);
                let summary = r.config.summary(w);
                let label = deptheos_app::library::history::render_with_summary(
                    w,
                    r.number,
                    r.name.as_deref(),
                    r.picked,
                    &summary,
                );
                (r.id.clone(), label.into())
            })
            .collect()
    }

    /// → / ← (or PgDn / PgUp): the next or previous frame on the rail.
    pub(super) fn step_frame(
        &mut self,
        delta: isize,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_result(cx) {
            self.scrub_by(delta, window, cx);
        }
    }

    /// Focus mode (Tab): frames, strip and inspector hide; the title and
    /// view bars stay.
    pub(crate) fn toggle_focus_mode(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.set_chrome(self.chrome.toggled_focus(), cx);
        // The focused row may be hidden: the window keeps focus so Tab returns.
        window.focus(&self.focus_handle, cx);
    }
}

impl AppView {
    /// The strip is under the photo: not in an empty window, Focus Mode, or
    /// Retouch (where renders are Paint From's).
    pub(crate) fn strip_shown(&self, cx: &App) -> bool {
        !self.chrome.is_focus() && !self.is_empty(cx) && !self.is_retouching()
    }

    /// Render Info over the photo while open and the strip isn't drawn (Focus
    /// Mode, Retouch).
    pub(super) fn render_info_over_photo(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.strip_shown(cx) || self.screen != super::Screen::Stacking {
            return None;
        }
        let info = self.renders.update(cx, |r, cx| r.info_over_photo(cx))?;
        Some(
            div()
                .absolute()
                .top_3()
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(info)
                .into_any_element(),
        )
    }
}

/// Two-line render row for dropdown menus: render glyph + `Method #N ★` on
/// line 1, muted algorithm settings summary on line 2.
fn render_menu_item(title: SharedString, subtitle: Option<SharedString>) -> PopupMenuItem {
    PopupMenuItem::element(move |_, cx| {
        h_flex()
            .gap_2()
            .items_center()
            .py_0p5()
            .child(Icon::new(AppIcon::Render).small().text_color(cx.theme().muted_foreground))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child(title.clone()))
                    .when_some(subtitle.clone(), |el, line| {
                        el.child(
                            div().text_xs().text_color(cx.theme().muted_foreground).child(line),
                        )
                    }),
            )
    })
}
