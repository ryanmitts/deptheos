//! The strip's pieces: day dividers, batch headers, render tiles, failures,
//! renaming in place, and loading the thumbnails in view.

use std::sync::Arc;
use std::time::SystemTime;

use deptheos_app::StackerConfig;
use deptheos_app::library::history::{self, Render as Saved, RenderId};
use gpui::prelude::*;
use gpui::{
    App, ClickEvent, Context, Div, Focusable as _, FontWeight, ObjectFit, RenderImage,
    SharedString, Window, div, img, px,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Escape as InputEscape, Input, InputEvent, InputState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};

use super::labels::{age, frame_range, frames_summary};
use super::menus::{TileFlags, tile_menu};
use super::{
    Renaming, Renders, RendersEvent, THUMB_BORDER, THUMB_HEIGHT, THUMB_WIDTH, TILE_PADDING,
};
use crate::AppIcon;
use crate::look::{Fill, Pickable as _, overlay_surface};
use crate::r#type::TypeStyled as _;
use crate::words::{DesktopWords, Wording as _, say};
use crate::work;

impl Renders {
    /// Rename a render on its tile, from its own name, or empty with "#4" as
    /// placeholder; never the folder's name.
    pub(crate) fn start_rename(
        &mut self,
        id: &RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_rename(true, window, cx);
        let Some(render) = self.find(id) else { return };
        let name = render.name.clone().unwrap_or_default();
        let placeholder = history::render_name(crate::words::of(cx), render.number, None, false);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        input.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.select_all(window, cx);
        });
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.finish_rename(true, window, cx)
                    }
                    _ => {}
                },
            );
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.renaming = Some(Renaming { render: id.clone(), input, _events: events });
        cx.notify();
    }

    /// Done editing: keep the text as the name (empty: none), or drop it
    /// unless `keep`.
    fn finish_rename(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Renaming { render, input, .. }) = self.renaming.take() else { return };
        cx.notify();
        if !keep {
            return;
        }
        let text = input.read(cx).value().trim().to_string();
        let Some(entry) = self.renders.iter_mut().find(|r| r.id == render) else { return };
        let name = (!text.is_empty()).then_some(text);
        if entry.name == name {
            return;
        }
        let old = std::mem::replace(&mut entry.name, name.clone());
        let store = self.store.clone();
        let words = cx.wording().clone();
        let lead = say!(cx, "render-rename-failed");
        cx.spawn_in(window, async move |this, cx| {
            let written = name.clone();
            let renamed = work::store({
                let render = render.clone();
                move || store.rename_render(&render, written.as_deref())
            })
            .await;
            if let Err(why) = crate::notices::worked(&words, &lead, renamed) {
                // The old name comes back, unless it was renamed again.
                this.update(cx, |this, cx| {
                    if let Some(entry) = this.renders.iter_mut().find(|r| r.id == render)
                        && entry.name == name
                    {
                        entry.name = old;
                    }
                    this.fail(why, cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn request_compare(&mut self, id: &RenderId, cx: &mut Context<Self>) {
        if !self.is_locked() && !self.is_active(id, cx) {
            cx.emit(RendersEvent::Compare(id.clone()));
        }
    }

    /// A tile click shows its render; the working version stays. Comparing,
    /// the comparison goes on; clicking the compared render swaps the two.
    pub(crate) fn click_tile(
        &mut self,
        id: &RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_locked() {
            return;
        }
        if self.is_active(id, cx) {
            // On screen: it stays, not a render opening since.
            if self.is_pending() {
                self.cancel_open();
                cx.notify();
            }
            return;
        }
        if self.pending_id() == Some(id) {
            return;
        }
        if self.compare.as_ref() == Some(id)
            && let Some(shown) = self.active(cx).cloned()
        {
            self.compare = Some(shown);
        }
        self.open(id, window, cx);
    }

    /// The base tile's second line while edited: "3 changes".
    pub(super) fn base_tile_line(&self, cx: &App) -> Option<String> {
        let on = self.on_screen.as_ref()?;
        let base = self.find(on.working.base.as_ref()?)?;
        self.showing_for(base)?.base_tile_line(crate::words::of(cx))
    }

    /// A render tile: thumbnail, title, settings, what it stacked. Click
    /// shows, ⌘-click compares, right-click opens its menu. The render shown
    /// is ringed; the base carries Edited while differing, `base_line` under.
    pub(super) fn render_tile(
        &self,
        render: &Saved,
        base_line: Option<&String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = cx.theme();
        let id = render.id.clone();
        let active = self.is_active(&id, cx);
        let is_base = self.on_screen.as_ref().and_then(|o| o.working.base.as_ref()) == Some(&id);
        let changes = base_line.filter(|_| is_base);
        let edited = changes.is_some();
        let edited_color = crate::params::edited_color(cx);
        let opening = self.pending_id() == Some(&id);
        let compared = self.compare.as_ref() == Some(&id);
        // Comparing, A is the pinned reference and B the result on screen.
        let tag = match (compared, active && self.compare.is_some()) {
            (true, _) => Some((say!(cx, "tile-tag-a"), say!(cx, "tile-tag-a-tip"))),
            (false, true) => Some((say!(cx, "tile-tag-b"), say!(cx, "tile-tag-b-tip"))),
            _ => None,
        };
        let width = thumb_width(render.width, render.height);
        let detail = if opening {
            say!(cx, "tile-opening")
        } else if let Some(changes) = changes {
            changes.clone()
        } else {
            render.config.summary(crate::words::of(cx))
        };
        let locked = self.is_locked();
        let (picked, number) = (render.picked, render.number);
        let can_delete = self.can_delete();
        // One ring, on the render shown, moving when an open lands; the
        // opening render says so, with a spinner once slow.
        let shown = active;
        let selected = shown;
        let this = cx.weak_entity();
        let menu_id = id.clone();
        let editing = self.renaming.as_ref().filter(|r| r.render == id).map(|r| r.input.clone());
        let title_line = match editing {
            Some(input) => div()
                .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                    this.finish_rename(false, window, cx)
                }))
                .child(Input::new(&input).xsmall())
                .into_any_element(),
            None => render_line(render_title(cx.wording(), render)).into_any_element(),
        };
        let left_out = render.left_out_message(crate::words::of(cx));
        let marks = Marks {
            opening,
            shown,
            compared,
            warned: left_out.is_some(),
            edited: edited.then_some(edited_color),
            tag,
        };
        let w = cx.wording();
        let mut tip = say!(
            w,
            "tile-tip",
            range = frame_range(&render.frames, &render.included),
            frames = frames_summary(w, &render.included, render.linear, &render.options),
            age = age(w, render.created, SystemTime::now())
        );
        if let Some(left_out) = left_out {
            tip = say!(w, "tile-tip-left-out", tip = tip, note = left_out);
        }
        // Without the settings line, the tooltip says them first.
        if self.compact && !opening {
            tip = say!(
                w,
                "tile-tip-compact",
                method = render.config.kind().label(w.shared()),
                settings = render.config.summary(w.shared()),
                tip = tip
            );
        }
        let tip = SharedString::from(tip);
        let tile = v_flex()
            .id(SharedString::from(format!("render-{}", id.as_str())))
            .flex_shrink_0()
            .w(px(width + 2.0 * (THUMB_BORDER + TILE_PADDING)))
            .p(px(TILE_PADDING))
            .gap_0p5()
            .rounded_md()
            .pickable(selected, !locked, cx)
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(self.tile_thumbnail(&id, width, marks, cx))
            .child(title_line)
            // In a narrow window the tooltip keeps the settings.
            .when(!self.compact || opening, |el| {
                el.child(
                    div().text_xs().text_color(theme.muted_foreground).truncate().child(detail),
                )
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.modifiers().secondary() {
                    this.request_compare(&id, cx);
                } else {
                    this.click_tile(&id, window, cx);
                }
            }))
            .context_menu(move |menu, _, cx| {
                let none = (false, 0, false, false);
                let (pyramid, others, revert, shown) = this.upgrade().map_or(none, |p| {
                    let p = p.read(cx);
                    let made = p.find(&menu_id);
                    let retouching = p.on_screen.as_ref().is_some_and(|o| o.retouching);
                    let revert = !retouching
                        && made
                            .and_then(|r| p.showing_for(r))
                            .is_some_and(|s| s.revert().is_some());
                    (
                        made.is_some_and(|r| matches!(r.config, StackerConfig::Pyramid(_))),
                        p.others_of(&menu_id).len(),
                        revert,
                        p.is_active(&menu_id, cx),
                    )
                });
                let flags = TileFlags {
                    picked,
                    locked,
                    can_delete,
                    pyramid,
                    others,
                    number,
                    revert,
                    shown,
                };
                tile_menu(menu, this.clone(), menu_id.clone(), flags, cx.wording())
            });
        // Render Info opens over its tile: the strip is the window's foot.
        match self.info_for(&render.id, cx.wording()) {
            Some(card) => {
                self.info_popover(card, tile.into_any_element(), gpui::Anchor::BottomLeft, cx)
            }
            None => tile.into_any_element(),
        }
    }

    /// A `width`-wide thumbnail framed by `marks`: ringed when shown, dimmed
    /// with a spinner while slow, neutral ring when compared, with the
    /// left-out warning, Edited pill and A/B tag.
    fn tile_thumbnail(&self, id: &RenderId, width: f32, marks: Marks, cx: &App) -> Div {
        let theme = cx.theme();
        let thumbnail = match self.thumbnails.get(id) {
            Some(image) => img(image)
                .w(px(width))
                .h(px(THUMB_HEIGHT))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div().w(px(width)).h(px(THUMB_HEIGHT)).bg(theme.muted).into_any_element(),
        };
        div()
            .relative()
            .rounded_sm()
            .overflow_hidden()
            .border(px(THUMB_BORDER))
            // The render shown keeps the ring until another's open lands.
            .border_color(if marks.shown {
                theme.ring
            } else if marks.compared {
                // Neutral: amber means a warning, the accent what is shown.
                theme.input
            } else {
                gpui::transparent_black()
            })
            .child(thumbnail)
            .when(marks.opening && self.open_is_slow(), |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::opaque_grey(0.0, 0.35))
                        .child(Spinner::new().color(gpui::opaque_grey(1.0, 0.9))),
                )
            })
            .when(marks.warned, |el| {
                el.child(
                    overlay_pill(cx).px_0p5().py_0p5().bottom_1().right_1().child(
                        Icon::new(IconName::TriangleAlert).xsmall().text_color(theme.warning),
                    ),
                )
            })
            .when_some(marks.edited, |el, color| {
                el.child(edited_pill(cx, color).bottom_1().left_1())
            })
            .when_some(marks.tag, |el, (tag, tip)| {
                el.child(
                    overlay_pill(cx)
                        .id("compare-tag")
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .top_1()
                        .left_1()
                        .child(tag),
                )
            })
    }

    /// Read unread thumbnails in view (and a strip's width either side), from
    /// the last frame's layout; `strip_ids` is this frame's.
    pub(super) fn load_visible(
        &mut self,
        strip_ids: Vec<Option<RenderId>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.scroll.bounds();
        let offset = self.scroll.offset();
        let margin = bounds.size.width;
        let (left, right) = (bounds.left() - offset.x - margin, bounds.right() - offset.x + margin);
        let wanted: Vec<RenderId> = self
            .strip_ids
            .iter()
            .enumerate()
            .filter_map(|(ix, id)| {
                let id = id.as_ref()?;
                let tile = self.scroll.bounds_for_item(ix)?;
                let seen = tile.right() >= left && tile.left() <= right;
                seen.then(|| id.clone())
            })
            .collect();
        // Laid out anew: look again once this frame has placed the tiles.
        if strip_ids != self.strip_ids {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        self.strip_ids = strip_ids;
        self.request_thumbnails(wanted, cx);
    }
}

impl Renders {
    /// The stack running: "#5" filling as it goes.
    pub(super) fn render_running_tile(
        &self,
        number: u32,
        progress: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let width = THUMB_HEIGHT * 1.5;
        v_flex()
            .id("running-tile")
            .flex_shrink_0()
            .w(px(width + 2.0 * (THUMB_BORDER + TILE_PADDING)))
            .p(px(TILE_PADDING))
            .gap_0p5()
            .child(
                v_flex()
                    .w(px(width + 2.0 * THUMB_BORDER))
                    .h(px(THUMB_HEIGHT + 2.0 * THUMB_BORDER))
                    .px_3()
                    .gap_1()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .border(px(THUMB_BORDER))
                    .border_color(theme.ring)
                    .bg(theme.muted)
                    .child(div().text_xs().tabular().child(say!(
                        cx,
                        "unit-percent",
                        value = format!("{:.0}", progress * 100.0)
                    )))
                    .child(
                        div().w_full().h(px(3.0)).rounded_full().bg(theme.border).child(
                            div()
                                .h_full()
                                .rounded_full()
                                .w(gpui::relative(progress.clamp(0.0, 1.0)))
                                .bg(theme.primary),
                        ),
                    ),
            )
            .child(render_line(history::render_name(crate::words::of(cx), number, None, false)))
            .child(
                div().text_xs().text_color(theme.muted_foreground).child(say!(cx, "tile-stacking")),
            )
    }
}

/// What a tile's thumbnail is marked with.
#[derive(Clone)]
struct Marks {
    /// Opening: "Opening…" under it, with a spinner once it is slow.
    opening: bool,
    /// The render shown: ringed.
    shown: bool,
    /// Reference A: a neutral ring.
    compared: bool,
    /// Frames were left out of it.
    warned: bool,
    /// The working version's base, edited since: the edited dot's color.
    edited: Option<gpui::Hsla>,
    /// "A" or "B" while comparing, with its tooltip.
    tag: Option<(String, String)>,
}

pub(super) fn thumb_width(width: u32, height: u32) -> f32 {
    let aspect = if height == 0 { 1.5 } else { width as f32 / height as f32 };
    (THUMB_HEIGHT * aspect).clamp(THUMB_WIDTH.0, THUMB_WIDTH.1).round()
}

/// A render's title on tiles and the details pane: "#4", "#4 · Final",
/// "#4 ★ · Final".
pub(crate) fn render_title(w: &DesktopWords, render: &Saved) -> String {
    history::render_name(w.shared(), render.number, render.name.as_deref(), render.picked)
}

/// A tile's first line: the render glyph, then its title.
pub(crate) fn render_line(title: String) -> Div {
    h_flex()
        .gap_1()
        .items_center()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(Icon::new(AppIcon::Render).xsmall().flex_shrink_0())
        .child(div().min_w_0().truncate().child(title))
}

/// A thumbnail box: `image` covering it, or `blank`; the caller sizes it.
pub(crate) fn thumb_box(image: Option<Arc<RenderImage>>, blank: gpui::Hsla) -> Div {
    div()
        .overflow_hidden()
        .bg(blank)
        .when_some(image, |el, image| el.child(img(image).size_full().object_fit(ObjectFit::Cover)))
}

/// A small HUD label over a thumbnail ([`Fill::Hud`]); the caller places it.
pub(super) fn overlay_pill(cx: &App) -> Div {
    overlay_surface(Fill::Hud, cx)
        .absolute()
        .flex()
        .flex_row()
        .px_1p5()
        .gap_1()
        .items_center()
        .text_xs()
}

/// "• Edited" over the base's thumbnail while the working version differs.
pub(super) fn edited_pill(cx: &App, edited: gpui::Hsla) -> Div {
    overlay_pill(cx)
        .child(div().size(px(5.0)).rounded_full().bg(edited))
        .child(say!(cx, "tile-edited"))
}
