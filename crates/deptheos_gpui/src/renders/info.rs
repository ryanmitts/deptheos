//! Render Info: one render's record, read-only, in a popover from its tile
//! (or over the photo without the strip). Its rows are the engine's
//! ([`render_setup`]), with the working version's value under any that
//! differs. No buttons: Revert lives with the working version.

use std::rc::Rc;
use std::sync::Arc;

use chrono::{DateTime, Local};
use deptheos_app::library::history::{Render as Saved, RenderId};
use deptheos_app::library::stacks::{SetupSection, render_setup, stacked_at};
use gpui::prelude::*;
use gpui::{AnyElement, App, Context, FontWeight, RenderImage, SharedString, Window, div, px};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Selectable, h_flex, v_flex};

use super::Renders;
use super::tiles::{render_title, thumb_box};
use crate::words::{DesktopWords, Wording as _, say};

/// What Render Info shows for one render, read when it is drawn.
pub(super) struct InfoCard {
    /// "#3", "#3 · sharper".
    title: String,
    /// "Stacked Nov 14, 2026 at 10:42 AM", with "★" when starred.
    stacked: String,
    /// How the working version differs from it, when it does.
    change: Option<String>,
    thumbnail: Option<Arc<RenderImage>>,
    sections: Vec<SetupSection>,
}

/// A trigger for the popover that draws what it is given: the tile, or
/// nothing over the photo.
#[derive(IntoElement)]
struct Trigger(AnyElement);

impl Selectable for Trigger {
    fn selected(self, _: bool) -> Self {
        self
    }

    fn is_selected(&self) -> bool {
        false
    }
}

impl RenderOnce for Trigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.0
    }
}

impl Renders {
    /// The only way [`Self::info`] changes.
    pub(crate) fn set_info(&mut self, info: Option<RenderId>, cx: &mut Context<Self>) {
        if self.info == info {
            return;
        }
        tracing::debug!("render info: {:?} -> {info:?}", self.info);
        self.info = info;
        cx.notify();
    }

    /// Where Render Info is.
    #[cfg(test)]
    pub(crate) fn info(&self) -> Option<&RenderId> {
        self.info.as_ref()
    }

    /// Render Info closes once its render is deleted or its stack is off
    /// screen.
    pub(super) fn close_stale_info(&mut self, cx: &mut Context<Self>) {
        let Some(id) = &self.info else { return };
        let stack = self.on_screen.as_ref().map(|o| &o.working.stack);
        if self.find(id).is_none_or(|r| Some(&r.stack) != stack) {
            self.set_info(None, cx);
        }
    }

    /// Render Info's card for `id`, while it is open on it.
    pub(super) fn info_for(&self, id: &RenderId, w: &DesktopWords) -> Option<InfoCard> {
        if self.info.as_ref() != Some(id) {
            return None;
        }
        self.find(id).map(|render| self.info_card(render, w))
    }

    /// What Render Info says about `render`.
    fn info_card(&self, render: &Saved, w: &DesktopWords) -> InfoCard {
        let mut title_render = render.clone();
        title_render.picked = false;
        let title = render_title(w, &title_render);
        let at = DateTime::<Local>::from(render.created);
        let date = say!(
            w,
            "render-info-date",
            day = crate::dates::day_with_year(at.date_naive()),
            time = crate::dates::time(at)
        );
        let stacked = stacked_at(&date, w.shared());
        let stacked =
            if render.picked { say!(w, "render-info-starred", stacked = stacked) } else { stacked };
        let showing = self.showing_for(render);
        let change = showing.and_then(|s| s.change_line(w.shared()));
        let sections = match &self.on_screen {
            Some(on) => render_setup(render, &on.working, on.compared, w.shared()),
            None => Vec::new(),
        };
        InfoCard { title, stacked, change, thumbnail: self.thumbnail(&render.id), sections }
    }

    /// Render Info over the photo, centered under its top edge, for when
    /// the strip isn't drawn (Focus Mode).
    pub(crate) fn info_over_photo(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(id) = &self.info else { return None };
        let card = self.info_for(id, cx.wording())?;
        let anchor = div().size(px(1.0)).into_any_element();
        Some(self.info_popover(card, anchor, gpui::Anchor::TopCenter, cx))
    }

    /// `trigger` with Render Info's popover open on it: `anchor` says
    /// which side it opens on. A click outside it or Esc closes it.
    pub(super) fn info_popover(
        &self,
        card: InfoCard,
        trigger: AnyElement,
        anchor: gpui::Anchor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.weak_entity();
        let card = Rc::new(card);
        Popover::new("render-info")
            .anchor(anchor)
            .open(true)
            .on_open_change(move |open, _, cx| {
                if !*open {
                    this.update(cx, |r, cx| r.set_info(None, cx)).ok();
                }
            })
            .trigger(Trigger(trigger))
            .content(move |_, _, cx| info_card(&card, cx))
            .into_any_element()
    }
}

/// The card: the thumbnail beside the title, when it was stacked and the
/// change line, then each section's rows, scrolling when they are many.
fn info_card(card: &InfoCard, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (muted, border) = (theme.muted_foreground, theme.border);
    let edited = crate::params::edited_color(cx);
    let head = h_flex()
        .gap_3()
        .items_start()
        .child(thumb_box(card.thumbnail.clone(), theme.muted).w(px(72.0)).h(px(48.0)).rounded_sm())
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .debug_selector(|| "render-info-title".into())
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(card.title.clone()),
                )
                .child(div().text_xs().text_color(muted).child(card.stacked.clone()))
                .when_some(card.change.clone(), |el, change| {
                    el.child(div().text_xs().child(change))
                }),
        );
    let sections = card.sections.iter().map(|section| {
        let rows = section.rows.iter().map(|row| {
            let label = SharedString::from(row.label.clone());
            v_flex()
                .gap_0p5()
                .py_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1p5()
                                .items_center()
                                .child(div().min_w_0().truncate().child(label))
                                .when(row.differs(), |el| {
                                    el.child(
                                        div()
                                            .size(px(6.0))
                                            .flex_shrink_0()
                                            .rounded_full()
                                            .bg(edited),
                                    )
                                }),
                        )
                        .child(div().flex_shrink_0().text_color(muted).child(row.value.clone())),
                )
                .when_some(row.working.clone(), |el, working| {
                    el.child(div().text_xs().text_color(muted).child(working))
                })
        });
        v_flex()
            .gap_0p5()
            .pt_2()
            .border_t_1()
            .border_color(border)
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child(section.title.clone()))
            .children(rows)
    });
    v_flex().debug_selector(|| "render-info".into()).w(px(320.0)).gap_2().child(head).child(
        v_flex()
            .id("render-info-rows")
            .max_h(px(360.0))
            .overflow_y_scroll()
            .gap_2()
            .children(sections),
    )
}
