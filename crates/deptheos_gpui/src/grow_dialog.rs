//! The Export Grow Sequence… dialog: which way focus comes in, and how the
//! frames are written. Export picks the folder (the app view checks room);
//! frames go into a new subfolder named after the render.
//!
//! [`FrameControls`] are the frame options shared with Batch's turntable:
//! format, size and 16-bit, starting from the export settings.

use deptheos_app::SaveOptions;
use deptheos_app::library::export::ExportFormat;
use deptheos_app::runs::sequence::{self, Direction, FrameFormat, FrameOptions, GrowOptions};
use gpui::prelude::*;
use gpui::{App, Context, EventEmitter, SharedString, Window, div, px};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use crate::app::ModalDialog;
use crate::choice::{Choice, ChoiceSelect, choice_select, chosen};
use crate::form::{footer, heading, note, radio_row};
use crate::words::{Wording as _, say};

/// Frame format, size and bit depth, as dropdowns and a checkbox.
pub struct FrameControls {
    format: ChoiceSelect<FrameFormat>,
    size: ChoiceSelect<Option<u32>>,
    /// The export settings these start from, with 16-bit as ticked here.
    save: SaveOptions,
    /// The source can be written with 16 bits; an 8-bit one leaves the
    /// 16-bit checkbox off. Not known for a batch's stacks: it says yes.
    sixteen_bit_source: bool,
}

/// The long edges offered besides the full size: the 4K and 1080p widths
/// videos go out at.
const SIZES: [u32; 3] = [3840, 2560, 1920];

impl FrameControls {
    /// Starting from the export settings `save`, in `format`.
    pub fn new(format: FrameFormat, save: SaveOptions, window: &mut Window, cx: &mut App) -> Self {
        let formats = FrameFormat::ALL.iter().map(|&f| Choice::new(f.label(), f)).collect();
        let mut sizes = vec![Choice::new(say!(cx, "grow-full-size"), None)];
        sizes.extend(
            SIZES
                .iter()
                .map(|&s| Choice::new(say!(cx, "grow-long-edge", pixels = s.to_string()), Some(s))),
        );
        Self {
            format: choice_select(formats, &format, window, cx),
            size: choice_select(sizes, &None, window, cx),
            save,
            sixteen_bit_source: true,
        }
    }

    pub fn options(&self, cx: &App) -> FrameOptions {
        FrameOptions {
            format: chosen(&self.format, cx).unwrap_or_default(),
            save: self.save,
            long_edge: chosen(&self.size, cx).flatten(),
        }
    }

    /// Whether 16 bits can be ticked: JPEG has none, and an 8-bit source
    /// has none to write.
    fn sixteen_bit_allowed(&self, cx: &App) -> bool {
        let format = ExportFormat::from(chosen(&self.format, cx).unwrap_or_default());
        !format.is_depth_fixed(self.sixteen_bit_source)
    }

    /// The format and size dropdowns and the 16-bit checkbox, in the view
    /// `V` that holds these controls, which `slot` finds them in.
    pub fn render<V: 'static>(
        &self,
        id: &'static str,
        slot: fn(&mut V) -> &mut FrameControls,
        cx: &mut Context<V>,
    ) -> impl IntoElement + use<V> {
        let allowed = self.sixteen_bit_allowed(cx);
        let sixteen = say!(cx, "grow-sixteen-bit");
        let on_sixteen = cx.listener(move |this, checked: &bool, _, cx| {
            slot(this).save.sixteen_bit = *checked;
            cx.notify();
        });
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(110.0)).child(Select::new(&self.format).small()))
                    .child(div().w(px(170.0)).child(Select::new(&self.size).small())),
            )
            .child(
                Checkbox::new(SharedString::from(format!("{id}-sixteen")))
                    .label(sixteen)
                    .checked(self.save.sixteen_bit && allowed)
                    .disabled(!allowed)
                    .on_click(on_sixteen),
            )
    }
}

/// What the dialog says about the render.
#[derive(Clone, Debug)]
pub struct GrowInfo {
    pub name: String,
    /// Frames it stacked: the sequence's length.
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    /// Its frames can be written with 16 bits (a RAW stack, or 16-bit
    /// sources).
    pub sixteen_bit_source: bool,
    pub has_paint: bool,
}

pub enum GrowDialogEvent {
    Export(GrowOptions),
    Cancel,
}

pub struct GrowDialog {
    info: GrowInfo,
    direction: Direction,
    frames: FrameControls,
}

impl EventEmitter<GrowDialogEvent> for GrowDialog {}

impl GrowDialog {
    pub fn new(info: GrowInfo, mut frames: FrameControls) -> Self {
        frames.sixteen_bit_source = info.sixteen_bit_source;
        Self { info, direction: Direction::BackToFront, frames }
    }

    /// Whether the 16-bit checkbox is ticked and can be changed.
    #[cfg(test)]
    pub fn sixteen_bit_checkbox(&self, cx: &App) -> (bool, bool) {
        let allowed = self.frames.sixteen_bit_allowed(cx);
        (self.frames.save.sixteen_bit && allowed, allowed)
    }

    fn options(&self, cx: &App) -> GrowOptions {
        GrowOptions { direction: self.direction, every: 1, frames: self.frames.options(cx) }
    }

    /// "66 frames · 8138 × 5449 · about 5.4 GB".
    fn summary(&self, cx: &App) -> String {
        let options = self.frames.options(cx);
        let (w, h) = options.output_size(self.info.width, self.info.height);
        let sixteen = options.sixteen_bit_from(self.info.sixteen_bit_source);
        let bytes = sequence::estimate_bytes(
            self.info.frames,
            self.info.width,
            self.info.height,
            &options,
            sixteen,
        );
        let (n, size) = (self.info.frames, format!("{w} × {h}"));
        let bytes = deptheos_app::words::units::size_text(bytes, crate::words::of(cx));
        say!(cx, "grow-summary", n = n, size = size, bytes = bytes)
    }
}

impl Render for GrowDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let frames = self.frames.render("grow", |this: &mut Self| &mut this.frames, cx);
        let w = cx.wording().clone();
        v_flex()
            .w_full()
            .gap_3()
            .child(div().text_sm().child(say!(w, "grow-intro", name = self.info.name.as_str())))
            .child(heading(say!(w, "grow-direction-heading")))
            .child(radio_row(
                RadioGroup::vertical("grow-direction"),
                &Direction::ALL,
                |d| match d {
                    Direction::BackToFront => say!(w, "grow-direction-back-to-front"),
                    Direction::FrontToBack => say!(w, "grow-direction-front-to-back"),
                },
                self.direction,
                |_| true,
                cx.listener(|this, direction: &Direction, _, cx| {
                    this.direction = *direction;
                    cx.notify();
                }),
            ))
            .child(heading(say!(w, "grow-frames-heading")))
            .child(frames)
            .when(self.info.has_paint, |el| el.child(note(say!(w, "grow-no-retouching"), muted)))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_sm().child(self.summary(cx)))
                    .child(note(say!(w, "grow-written"), muted)),
            )
            .child(footer(
                "grow-cancel",
                cx.listener(|_, _, _, cx| cx.emit(GrowDialogEvent::Cancel)),
                Button::new("grow-export")
                    .label(say!(w, "grow-export"))
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
            ))
    }
}

impl ModalDialog for GrowDialog {
    const WIDTH: f32 = 460.0;

    fn title(cx: &App) -> SharedString {
        say!(cx, "grow-dialog-title").into()
    }

    /// Export…, which goes on to ask where.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let options = self.options(cx);
        cx.emit(GrowDialogEvent::Export(options))
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::test_support::open;

    fn dialog(cx: &mut TestAppContext, sixteen_bit_source: bool) -> gpui::Entity<GrowDialog> {
        let info = GrowInfo {
            name: "bee".into(),
            frames: 3,
            width: 192,
            height: 128,
            sixteen_bit_source,
            has_paint: false,
        };
        let (_, dialog) = open(cx, |window, cx| {
            let frames = FrameControls::new(FrameFormat::Tiff, SaveOptions::default(), window, cx);
            cx.new(|_| GrowDialog::new(info, frames))
        });
        dialog
    }

    /// An 8-bit source's frames are written at 8 bits: the 16-bit checkbox
    /// is off and can't be ticked, where it showed ticked.
    #[gpui::test]
    fn sixteen_bits_are_off_for_an_eight_bit_source(cx: &mut TestAppContext) {
        let eight = dialog(cx, false);
        eight.read_with(cx, |d, cx| {
            assert_eq!(d.sixteen_bit_checkbox(cx), (false, false));
            assert!(!d.options(cx).frames.sixteen_bit_from(false));
        });
    }

    /// A 16-bit source keeps the checkbox, ticked as the export settings are.
    #[gpui::test]
    fn sixteen_bits_stay_for_a_sixteen_bit_source(cx: &mut TestAppContext) {
        let sixteen = dialog(cx, true);
        sixteen.read_with(cx, |d, cx| assert_eq!(d.sixteen_bit_checkbox(cx), (true, true)));
    }
}
