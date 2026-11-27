//! Adjust's crop tool on the desktop (ADR-136): C opens it over the
//! picture uncropped, its handles drag the crop (`viewer/input.rs`), the
//! panel's section picks the aspect, turns, flips and resets; Done or
//! Return keeps the crop, Esc puts back the reframe it opened on. Each
//! drag is set while it moves and committed on release; each turn, flip,
//! reset or shape is one commit.

use deptheos_app::adjust::crop::{Aspect, CropFrame};
use deptheos_app::reframe::Reframe;
use gpui::prelude::*;
use gpui::{Context, FontWeight, div};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Selectable as _, Sizable as _, h_flex, v_flex};

use super::AppView;
use super::actions::*;
use crate::adjust::AdjustTool;
use crate::words::say;

impl AppView {
    /// The value's reframe now, while Adjust is open on a render.
    fn reframe_now(&self, cx: &gpui::App) -> Option<Reframe> {
        self.adjusting()?;
        Some(self.adjust.read(cx).status()?.value.reframe)
    }

    /// `reframe` as one change, if it is one.
    fn reframe_to(&mut self, reframe: Reframe, cx: &mut Context<Self>) {
        if self.reframe_now(cx).is_some_and(|now| now != reframe) {
            tracing::debug!("crop: {reframe:?}");
            let adjust = self.adjust.read(cx);
            adjust.set_reframe(reframe);
            adjust.commit();
        }
    }

    /// The crop tool's aspect, while it is open.
    fn crop_aspect(&self, cx: &gpui::App) -> Option<Aspect> {
        match self.adjust.read(cx).tool() {
            AdjustTool::Crop { aspect, .. } => Some(aspect),
            _ => None,
        }
    }

    /// C or the Crop button: the crop tool open, or kept and closed.
    pub(super) fn toggle_crop(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.reframe_now(cx) else { return };
        if self.crop_aspect(cx).is_some() {
            return self.crop_done(cx);
        }
        // A crop already made keeps its shape free; none starts at the
        // picture's own.
        let aspect = if opened.crop.is_some() { Aspect::Free } else { Aspect::Original };
        self.set_adjust_tool(AdjustTool::Crop { opened, aspect }, cx);
    }

    /// Done or Return: the crop kept, the sliders back.
    pub(super) fn crop_done(&mut self, cx: &mut Context<Self>) {
        if self.crop_aspect(cx).is_some() {
            self.set_adjust_tool(AdjustTool::Sliders, cx);
        }
    }

    /// Esc in the crop tool: the reframe it opened on, back as one change.
    /// `false` when the tool isn't open.
    pub(super) fn cancel_crop(&mut self, cx: &mut Context<Self>) -> bool {
        let AdjustTool::Crop { opened, .. } = self.adjust.read(cx).tool() else { return false };
        self.reframe_to(opened, cx);
        self.set_adjust_tool(AdjustTool::Sliders, cx);
        true
    }

    /// ⌘[ or ⌘], or the buttons: a quarter turn as shown, the aspect
    /// turned with the picture.
    pub(super) fn turn(&mut self, right: bool, cx: &mut Context<Self>) {
        let Some(now) = self.reframe_now(cx) else { return };
        self.reframe_to(if right { now.turned_right() } else { now.turned_left() }, cx);
        if let AdjustTool::Crop { opened, aspect } = self.adjust.read(cx).tool() {
            self.set_adjust_tool(AdjustTool::Crop { opened, aspect: aspect.turned() }, cx);
        }
    }

    /// A mirror as shown, left to right or top to bottom.
    pub(super) fn flip(&mut self, vertical: bool, cx: &mut Context<Self>) {
        let Some(now) = self.reframe_now(cx) else { return };
        let flipped = if vertical { now.flipped_vertically() } else { now.flipped_horizontally() };
        self.reframe_to(flipped, cx);
    }

    /// Reset Crop: no crop, turn or flip.
    pub(super) fn reset_crop(&mut self, cx: &mut Context<Self>) {
        self.reframe_to(Reframe::NONE, cx);
        if let AdjustTool::Crop { opened, .. } = self.adjust.read(cx).tool() {
            self.set_adjust_tool(AdjustTool::Crop { opened, aspect: Aspect::Original }, cx);
        }
    }

    /// An aspect picked: the crop the largest of its shape inside the one
    /// now, centered.
    pub(super) fn set_aspect(&mut self, aspect: Aspect, cx: &mut Context<Self>) {
        let AdjustTool::Crop { opened, .. } = self.adjust.read(cx).tool() else { return };
        self.set_adjust_tool(AdjustTool::Crop { opened, aspect }, cx);
        let (Some(now), Some(picture)) = (self.reframe_now(cx), self.viewer.read(cx).picture())
        else {
            return;
        };
        let frame = CropFrame::of(picture, now);
        self.reframe_to(frame.reframed(frame.shaped(frame.area(), aspect)), cx);
    }

    /// X: the aspect's width and height swapped.
    pub(super) fn swap_aspect(&mut self, cx: &mut Context<Self>) {
        if let Some(aspect) = self.crop_aspect(cx) {
            self.set_aspect(aspect.turned(), cx);
        }
    }

    /// A crop handle dragged: the reframe shown, a change going on.
    pub(super) fn crop_moved(&mut self, reframe: Reframe, cx: &mut Context<Self>) {
        if self.crop_aspect(cx).is_some() {
            self.adjust.read(cx).set_reframe(reframe);
        }
    }

    /// The crop handle released: the change ends.
    pub(super) fn crop_released(&mut self, cx: &mut Context<Self>) {
        self.adjust.read(cx).commit();
    }

    /// The panel's Crop section: a button opening the tool, or the tool's
    /// aspects, turns, flips, Reset Crop and Done.
    pub(super) fn render_crop_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let ctx = Some(ADJUST_KEY_CONTEXT);
        let heading =
            div().text_xs().font_weight(FontWeight::SEMIBOLD).child(say!(cx, "adjust-crop"));
        let Some(chosen) = self.crop_aspect(cx) else {
            return v_flex().gap_2().child(
                h_flex().justify_between().items_center().child(heading).child(
                    Button::new("adjust-crop-open")
                        .label(say!(cx, "adjust-crop-open"))
                        .xsmall()
                        .outline()
                        .tooltip_with_action(say!(cx, "adjust-crop-tooltip"), &ToggleCrop, ctx)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_crop(cx))),
                ),
            );
        };
        let mut aspects = h_flex().flex_wrap().gap_1();
        for (i, choice) in Aspect::CHOICES.into_iter().enumerate() {
            // A ratio shows the way it is turned now.
            let shown = if choice.same_choice(chosen) { chosen } else { choice };
            let label = match shown {
                Aspect::Original => say!(cx, "adjust-aspect-original"),
                Aspect::Free => say!(cx, "adjust-aspect-free"),
                Aspect::Ratio(w, h) => {
                    say!(cx, "adjust-aspect-ratio", width = w.to_string(), height = h.to_string())
                }
            };
            aspects = aspects.child(
                Button::new(("adjust-aspect", i))
                    .label(label)
                    .xsmall()
                    .ghost()
                    .selected(choice.same_choice(chosen))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_aspect(shown, cx))),
            );
        }
        let swappable = matches!(chosen, Aspect::Ratio(w, h) if w != h);
        aspects = aspects.when(swappable, |row| {
            row.child(
                Button::new("adjust-aspect-swap")
                    .label("⇄")
                    .xsmall()
                    .ghost()
                    .tooltip_with_action(say!(cx, "adjust-aspect-swap-tooltip"), &SwapAspect, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.swap_aspect(cx))),
            )
        });
        let button =
            |id: &'static str, label: String| Button::new(id).label(label).xsmall().outline();
        let turns = h_flex()
            .flex_wrap()
            .gap_1()
            .child(
                button("adjust-turn-left", say!(cx, "adjust-turn-left"))
                    .tooltip_with_action(say!(cx, "adjust-turn-left"), &TurnLeft, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.turn(false, cx))),
            )
            .child(
                button("adjust-turn-right", say!(cx, "adjust-turn-right"))
                    .tooltip_with_action(say!(cx, "adjust-turn-right"), &TurnRight, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.turn(true, cx))),
            )
            .child(
                button("adjust-flip-horizontal", say!(cx, "adjust-flip-horizontal"))
                    .on_click(cx.listener(|this, _, _, cx| this.flip(false, cx))),
            )
            .child(
                button("adjust-flip-vertical", say!(cx, "adjust-flip-vertical"))
                    .on_click(cx.listener(|this, _, _, cx| this.flip(true, cx))),
            );
        let ends = h_flex()
            .justify_between()
            .child(
                Button::new("adjust-reset-crop")
                    .label(say!(cx, "adjust-reset-crop"))
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.reset_crop(cx))),
            )
            .child(
                Button::new("adjust-crop-done")
                    .label(say!(cx, "adjust-crop-done"))
                    .xsmall()
                    .primary()
                    .tooltip_with_action(say!(cx, "adjust-crop-done-tooltip"), &CropDone, ctx)
                    .on_click(cx.listener(|this, _, _, cx| this.crop_done(cx))),
            );
        v_flex().gap_2().child(heading).child(aspects).child(turns).child(ends)
    }
}
