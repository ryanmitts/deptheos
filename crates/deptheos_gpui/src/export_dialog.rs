//! The Export sheet (⌘E): thumbnail and name, format, bit depth and color,
//! file name, destination, and Show in Finder after. Export Again (⇧⌘E)
//! reuses the choices without it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, RenderImage, SharedString, Window, div, px,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use deptheos_app::ExportSpace;
use deptheos_app::library::export::{ExportFormat, strip_extension, with_extension};
use deptheos_app::library::history::RenderId;

use crate::app::ModalDialog;
use crate::app::actions::{ExportAgain, KEY_CONTEXT, bound_keys};
use crate::file_dialog::{Picker, file_dialog, pick_folder};
use crate::form::{Segment, Segmented, note, reveal_when_done_label};
use crate::renders::Renders;
use crate::words::{DesktopWords, Wording as _, say};

/// Where an export goes, as remembered for the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportPlace {
    /// The folder of the render's first frame.
    #[default]
    Frames,
    /// The folder the last export went to.
    Last,
}

impl ExportPlace {
    pub const ALL: [Self; 2] = [Self::Frames, Self::Last];

    pub fn key(self) -> &'static str {
        match self {
            Self::Frames => "frames",
            Self::Last => "last",
        }
    }

    pub fn label(self, w: &DesktopWords) -> String {
        match self {
            Self::Frames => say!(w, "export-place-frames"),
            Self::Last => say!(w, "export-place-last"),
        }
    }
}

/// What the sheet says about the render, and where it could go.
#[derive(Clone)]
pub struct ExportInfo {
    /// "Export #4".
    pub title: SharedString,
    /// "canon1 · Depth · R8 S2.5 · retouched".
    pub subtitle: SharedString,
    /// The render whose thumbnail the sheet shows, read as it draws.
    pub thumbnail: Option<(Entity<Renders>, RenderId)>,
    pub color: ExportColor,
    /// The stack has 16 bits to write (`StackResult::has_sixteen_bit`); for
    /// 8-bit frames, 8 bits is the only choice unless it is adjusted.
    pub sixteen_bit_source: bool,
    /// The render has an adjustment set: 16 bits are offered, a RAW
    /// render picks its space, and DNG carries only part of them.
    pub adjusted: bool,
    /// A standard stack's profile is drawn through tables: adjusted, it is
    /// written in sRGB (ADR-133).
    pub adjusted_in_srgb: bool,
    /// The file name without its extension: "canon1-4".
    pub base_name: String,
    pub frames_dir: Option<PathBuf>,
    pub last_dir: Option<PathBuf>,
}

/// What the render's pixels are, as the sheet tells it.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportColor {
    /// A RAW or DNG stack: DNG is offered, and 16 bits are real.
    Linear,
    /// Standard frames in sRGB (or with no profile).
    Srgb,
    /// Frames in another color space, kept in the file: "Display P3", or
    /// `None` for an unnamed profile.
    Profile(Option<SharedString>),
}

impl ExportColor {
    pub fn is_linear(&self) -> bool {
        matches!(self, Self::Linear)
    }
}

/// What Export was pressed with.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportChoice {
    pub format: ExportFormat,
    pub sixteen_bit: bool,
    /// The space an adjusted RAW render is written in.
    pub space: ExportSpace,
    /// The destination; a chosen folder becomes the last one used.
    pub dir: PathBuf,
    pub place: ExportPlace,
    /// The file name, with its extension.
    pub name: String,
    pub reveal: bool,
}

impl ExportChoice {
    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }
}

pub enum ExportDialogEvent {
    Export(ExportChoice),
    Cancel,
}

/// Where the sheet saves: the remembered places, or a folder chosen in it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Remembered(ExportPlace),
    Chosen,
}

pub struct ExportDialog {
    info: ExportInfo,
    format: ExportFormat,
    sixteen_bit: bool,
    space: ExportSpace,
    place: Place,
    chosen: Option<PathBuf>,
    reveal: bool,
    name: Entity<InputState>,
    picker: Picker,
    /// The free name is being looked up: Export is busy. Set by the sheet's
    /// owner ([`Self::set_finding`]).
    finding: bool,
}

impl EventEmitter<ExportDialogEvent> for ExportDialog {}

impl ExportDialog {
    /// The line under the sheet's title.
    #[cfg(test)]
    pub fn subtitle(&self) -> &str {
        &self.info.subtitle
    }

    /// The render's thumbnail, once read.
    pub fn thumbnail(&self, cx: &App) -> Option<Arc<RenderImage>> {
        let (renders, id) = self.info.thumbnail.as_ref()?;
        renders.read(cx).thumbnail(id)
    }

    /// From the last choices: `format` (TIFF when DNG can't be had),
    /// `sixteen_bit`, `space`, `place` and `reveal`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        info: ExportInfo,
        format: ExportFormat,
        sixteen_bit: bool,
        space: ExportSpace,
        place: ExportPlace,
        reveal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let format = format.for_stack(info.color.is_linear());
        let name = format!("{}.{}", info.base_name, format.extension());
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(name, window, cx);
            input
        });
        // The last folder is offered only once there is one.
        let place = match place {
            ExportPlace::Last if info.last_dir.is_none() => ExportPlace::Frames,
            place => place,
        };
        Self {
            info,
            format,
            sixteen_bit,
            space,
            place: Place::Remembered(place),
            chosen: None,
            reveal,
            name,
            picker: Picker::default(),
            finding: false,
        }
    }

    /// Export shown busy while the free name is looked up.
    pub fn set_finding(&mut self, finding: bool, cx: &mut Context<Self>) {
        if self.finding != finding {
            self.finding = finding;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn is_finding(&self) -> bool {
        self.finding
    }

    fn dir(&self) -> Option<&Path> {
        match self.place {
            Place::Remembered(ExportPlace::Frames) => self.info.frames_dir.as_deref(),
            Place::Remembered(ExportPlace::Last) => self.info.last_dir.as_deref(),
            Place::Chosen => self.chosen.as_deref(),
        }
    }

    /// What Export would export, while it can.
    pub fn choice(&self, cx: &App) -> Option<ExportChoice> {
        let dir = self.dir()?.to_path_buf();
        let name = with_extension(self.name.read(cx).value().trim(), self.format)?;
        Some(ExportChoice {
            format: self.format,
            sixteen_bit: self.sixteen_bit(),
            space: self.space(),
            dir,
            place: match self.place {
                Place::Remembered(place) => place,
                Place::Chosen => ExportPlace::Last,
            },
            name,
            reveal: self.reveal,
        })
    }

    fn set_format(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        if format == self.format {
            return;
        }
        // The name follows the format: its extension changes with it.
        let name = self.name.read(cx).value().to_string();
        let stem = strip_extension(&name).to_string();
        self.format = format;
        let name = format!("{stem}.{}", format.extension());
        self.name.update(cx, |input, cx| input.set_value(name, window, cx));
        cx.notify();
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = self.dir().map(Path::to_path_buf);
        let dialog =
            file_dialog(window, start.as_deref()).set_title(say!(cx, "file-dialog-export-to"));
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            || pick_folder(dialog),
            |this, picked, _, cx| {
                if let Some(dir) = picked {
                    this.chosen = Some(dir);
                    this.place = Place::Chosen;
                }
                cx.notify();
            },
            window,
            cx,
        );
    }

    #[cfg(test)]
    pub fn pick_format(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_format(format, window, cx);
    }
}

/// Today as a pattern's {date} gives it (`deptheos_app::library::export::file_stem`).
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// What a format keeps, under the format buttons.
fn format_note(w: &DesktopWords, format: ExportFormat) -> String {
    match format {
        ExportFormat::Tiff => say!(w, "export-format-note-tiff"),
        ExportFormat::Jpeg => say!(w, "export-format-note-jpeg"),
        ExportFormat::Png => say!(w, "export-format-note-png"),
        ExportFormat::Dng => say!(w, "export-format-note-dng"),
    }
}

/// A space's radio, by name for tests.
fn space_id(space: ExportSpace) -> &'static str {
    match space {
        ExportSpace::Srgb => "export-space-srgb",
        ExportSpace::DisplayP3 => "export-space-display-p3",
        ExportSpace::AdobeRgb => "export-space-adobe-rgb",
        ExportSpace::ProPhoto => "export-space-prophoto",
    }
}

fn space_label(w: &DesktopWords, space: ExportSpace) -> String {
    match space {
        ExportSpace::Srgb => say!(w, "export-space-srgb"),
        ExportSpace::DisplayP3 => say!(w, "export-space-display-p3"),
        ExportSpace::AdobeRgb => say!(w, "export-space-adobe-rgb"),
        ExportSpace::ProPhoto => say!(w, "export-space-prophoto"),
    }
}

/// The color line: what the pixels are; non-sRGB frames keep their space,
/// embedded, but for a table-based profile adjusted; a DNG of an adjusted
/// render carries only its white balance and crop.
fn color_note(w: &DesktopWords, format: ExportFormat, info: &ExportInfo) -> SharedString {
    let adjusted = info.adjusted;
    match (format, &info.color) {
        (ExportFormat::Dng, _) if adjusted => say!(w, "export-color-dng-adjusted").into(),
        (ExportFormat::Dng, _) => say!(w, "export-color-dng").into(),
        (_, ExportColor::Profile(_)) if adjusted && info.adjusted_in_srgb => {
            say!(w, "export-color-adjusted-srgb").into()
        }
        (_, ExportColor::Linear) => say!(w, "export-color-developed").into(),
        (_, ExportColor::Srgb) => say!(w, "export-color-srgb").into(),
        (_, ExportColor::Profile(Some(name))) => {
            say!(w, "export-color-profile", profile = name.as_ref()).into()
        }
        (_, ExportColor::Profile(None)) => say!(w, "export-color-unnamed-profile").into(),
    }
}

impl Render for ExportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let w = cx.wording().clone();
        let muted = cx.theme().muted_foreground;
        let linear = self.info.color.is_linear();
        let color = div().text_sm().child(color_note(&w, self.format, &self.info));
        v_flex()
            .w_full()
            .gap_4()
            .child(self.render_header(cx))
            .child(
                row(say!(w, "export-format")).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(h_flex().child(self.render_formats(cx)))
                        .child(note(format_note(&w, self.format), muted))
                        .when(!linear, |el| el.child(note(say!(w, "export-no-dng"), muted))),
                ),
            )
            .child(row(say!(w, "export-bit-depth")).child(self.render_depth(cx)))
            .map(|el| match self.offers_spaces() {
                true => el.child(row(say!(w, "export-color")).child(self.render_spaces(cx))),
                false => el.child(row(say!(w, "export-color")).child(color)),
            })
            .child(
                row(say!(w, "export-name")).child(
                    v_flex()
                        .flex_1()
                        .gap_1()
                        .child(Input::new(&self.name).small())
                        .child(note(say!(w, "export-name-note"), muted)),
                ),
            )
            .child(row(say!(w, "export-save-to")).child(self.render_places(cx)))
            .child(self.render_footer(window, cx))
    }
}

/// A dialog row: its right-aligned label, then the control.
fn row(label: String) -> gpui::Div {
    h_flex()
        .gap_4()
        .items_start()
        .child(div().w(px(80.0)).flex_shrink_0().text_sm().text_right().pt_0p5().child(label))
}

impl ExportDialog {
    /// The render's thumbnail, its title and its subtitle.
    fn render_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        h_flex()
            .gap_3()
            .items_center()
            .child(
                crate::renders::thumb_box(self.thumbnail(cx), theme.muted)
                    .w(px(84.0))
                    .h(px(50.0))
                    .flex_shrink_0()
                    .rounded(px(4.0)),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.info.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(self.info.subtitle.clone()),
                    ),
            )
    }

    /// The formats, one segment each; DNG only for linear frames.
    fn render_formats(&self, cx: &mut Context<Self>) -> Segmented {
        let linear = self.info.color.is_linear();
        let weak = cx.entity().downgrade();
        let format_ix = ExportFormat::ALL.iter().position(|f| *f == self.format);
        Segmented::new("export-format")
            .children(
                ExportFormat::ALL
                    .iter()
                    .map(|f| Segment::new(f.label()).disabled(!f.is_available(linear))),
            )
            .selected_index(format_ix.unwrap_or(0))
            .on_click(move |ix, window, cx| {
                let format = ExportFormat::ALL[*ix];
                weak.update(cx, |this, cx| this.set_format(format, window, cx)).ok();
            })
    }

    /// The depth is fixed by the format (JPEG 8, DNG 16) or by 8-bit frames.
    pub fn depth_fixed(&self) -> bool {
        self.format.is_depth_fixed(self.has_sixteen())
            || self.space == ExportSpace::ProPhoto && self.offers_spaces()
    }

    /// 16 bits can be written: the stack has them, or it is adjusted.
    fn has_sixteen(&self) -> bool {
        self.info.sixteen_bit_source || self.info.adjusted
    }

    /// Whether the file is written with 16 bits, as the radios show.
    pub fn sixteen_bit(&self) -> bool {
        let prophoto = self.offers_spaces() && self.space == ExportSpace::ProPhoto;
        self.format.depth(self.sixteen_bit || prophoto, self.has_sixteen())
    }

    /// An adjusted RAW render, written developed: its space is chosen.
    pub fn offers_spaces(&self) -> bool {
        self.info.color.is_linear() && self.info.adjusted && self.format != ExportFormat::Dng
    }

    /// The space written, as the radios show: ProPhoto only at 16 bits.
    pub fn space(&self) -> ExportSpace {
        let sixteen = self.format.depth(true, self.has_sixteen());
        match self.space {
            ExportSpace::ProPhoto if !sixteen => ExportSpace::AdobeRgb,
            space => space,
        }
    }

    /// The spaces of an adjusted RAW render, ProPhoto only where 16 bits
    /// can be written.
    fn render_spaces(&self, cx: &mut Context<Self>) -> gpui::Div {
        let w = cx.wording().clone();
        let chosen = self.space();
        let sixteen = self.format.depth(true, self.has_sixteen());
        let mut radios = v_flex().gap_1p5();
        for space in ExportSpace::ALL {
            let id = space_id(space);
            radios = radios.child(
                h_flex().child(
                    div().debug_selector(move || id.into()).child(
                        Radio::new(id)
                            .label(space_label(&w, space))
                            .checked(chosen == space)
                            .disabled(space == ExportSpace::ProPhoto && !sixteen)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.space = space;
                                cx.notify();
                            })),
                    ),
                ),
            );
        }
        radios.child(note(say!(w, "export-space-prophoto-note"), cx.theme().muted_foreground))
    }

    /// 16 or 8 bits, fixed for JPEG (8), DNG (16) and 8-bit frames (8).
    fn render_depth(&self, cx: &mut Context<Self>) -> gpui::Div {
        let w = cx.wording().clone();
        let depth_fixed = self.depth_fixed();
        let sixteen = self.sixteen_bit();
        // Each radio in a box of its own size, so a test clicks it by name.
        let named = |id: &'static str, radio: Radio| {
            h_flex().child(div().debug_selector(move || id.into()).child(radio))
        };
        v_flex()
            .gap_1p5()
            .child(named(
                "export-16",
                Radio::new("export-16")
                    .label(say!(w, "export-sixteen-bits"))
                    .tooltip(say!(w, "export-sixteen-bits-tooltip"))
                    .checked(sixteen)
                    .disabled(depth_fixed)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sixteen_bit = true;
                        cx.notify();
                    })),
            ))
            .child(named(
                "export-8",
                Radio::new("export-8")
                    .label(say!(w, "export-eight-bits"))
                    .tooltip(say!(w, "export-eight-bits-tooltip"))
                    .checked(!sixteen)
                    .disabled(depth_fixed)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sixteen_bit = false;
                        cx.notify();
                    })),
            ))
    }

    /// The destination choices, each with its folder under it; then Show
    /// When Done.
    fn render_places(&self, cx: &mut Context<Self>) -> gpui::Div {
        let w = cx.wording().clone();
        let muted = cx.theme().muted_foreground;
        let place_radio = |id: &'static str, label: String, place: Place, dir: Option<&Path>| {
            Radio::new(id)
                .label(label)
                .checked(self.place == place)
                .disabled(dir.is_none() && place != Place::Chosen)
        };
        // The folder sits outside the radio so it can be narrower than its
        // text: a long one is cut in the middle, keeping its name visible.
        // The radio's dot and gap are 24 px.
        let with_folder = |id: &'static str, radio: Radio, dir: Option<&Path>| {
            // The radio in a box of its own size, so a test clicks it by name.
            let radio = h_flex().child(div().debug_selector(move || id.into()).child(radio));
            v_flex().min_w_0().gap_1().child(radio).children(dir.map(|d| {
                div()
                    .pl(px(24.0))
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .text_sm()
                    .text_color(muted)
                    .debug_selector(move || format!("{id}-folder"))
                    .child(crate::folders::short_dir(d))
            }))
        };
        let (frames_dir, last_dir, chosen) = (
            self.info.frames_dir.as_deref(),
            self.info.last_dir.as_deref(),
            self.chosen.as_deref(),
        );
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .child(with_folder(
                "export-frames",
                place_radio(
                    "export-frames",
                    ExportPlace::Frames.label(&w),
                    Place::Remembered(ExportPlace::Frames),
                    frames_dir,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.place = Place::Remembered(ExportPlace::Frames);
                    cx.notify();
                })),
                frames_dir,
            ))
            .child(with_folder(
                "export-last",
                place_radio(
                    "export-last",
                    ExportPlace::Last.label(&w),
                    Place::Remembered(ExportPlace::Last),
                    last_dir,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.place = Place::Remembered(ExportPlace::Last);
                    cx.notify();
                })),
                last_dir,
            ))
            .child(with_folder(
                "export-choose",
                place_radio("export-choose", say!(w, "button-choose"), Place::Chosen, chosen)
                    .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                chosen,
            ))
            .child(
                div().pt_1().child(
                    Checkbox::new("export-reveal")
                        .label(reveal_when_done_label(cx))
                        .checked(self.reveal)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.reveal = *checked;
                            cx.notify();
                        })),
                ),
            )
    }

    /// The key that exports again, Cancel and Export.
    fn render_footer(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let w = cx.wording().clone();
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let can_export = self.choice(cx).is_some();
        let again = bound_keys(&ExportAgain, KEY_CONTEXT, window);
        h_flex()
            .pt_3()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(border)
            .child(div().flex_1().text_xs().text_color(muted).child(say!(
                w,
                "export-again-note",
                keys = again
            )))
            .child(
                Button::new("export-cancel")
                    .label(say!(w, "button-cancel"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ExportDialogEvent::Cancel))),
            )
            .child(
                div().debug_selector(|| "export-go-button".into()).child(
                    Button::new("export-go")
                        .label(say!(w, "export-button"))
                        .primary()
                        .loading(self.finding)
                        .disabled(!can_export)
                        .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                ),
            )
    }
}

impl ModalDialog for ExportDialog {
    const WIDTH: f32 = 500.0;

    /// Export, with a name that can be used.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.finding {
            return;
        }
        if let Some(choice) = self.choice(cx) {
            cx.emit(ExportDialogEvent::Export(choice));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(color: ExportColor, sixteen_bit_source: bool, adjusted: bool) -> ExportInfo {
        ExportInfo {
            title: "Export #1".into(),
            subtitle: "canon1 · Pyramid".into(),
            thumbnail: None,
            color,
            sixteen_bit_source,
            adjusted,
            adjusted_in_srgb: false,
            base_name: "canon1-1".into(),
            frames_dir: Some(std::env::temp_dir()),
            last_dir: None,
        }
    }

    /// The color line names a non-sRGB color space, which the file keeps;
    /// adjusted, a table-based profile writes sRGB and a DNG says what of
    /// the adjustments it carries.
    #[test]
    fn color_note_names_the_frames_space() {
        let w = DesktopWords::english();
        let note = |format, color: ExportColor, adjusted: bool, srgb: bool| {
            let info = ExportInfo { adjusted_in_srgb: srgb, ..info(color, true, adjusted) };
            super::color_note(&w, format, &info).to_string()
        };
        let p3 = || ExportColor::Profile(Some("Display P3".into()));
        assert_eq!(note(ExportFormat::Tiff, p3(), false, false), "Display P3, like the frames");
        assert_eq!(note(ExportFormat::Tiff, p3(), true, false), "Display P3, like the frames");
        assert_eq!(
            note(ExportFormat::Jpeg, ExportColor::Srgb, false, false),
            "sRGB, like the frames"
        );
        assert_eq!(
            note(ExportFormat::Png, ExportColor::Profile(None), false, false),
            "An unnamed color profile, like the frames"
        );
        assert_eq!(
            note(ExportFormat::Tiff, ExportColor::Linear, false, false),
            "sRGB, developed through the frames' camera profile"
        );
        let table = note(ExportFormat::Tiff, ExportColor::Profile(None), true, true);
        assert_eq!(table, say!(w, "export-color-adjusted-srgb"));
        let dng = note(ExportFormat::Dng, ExportColor::Linear, true, false);
        assert_eq!(dng, say!(w, "export-color-dng-adjusted"));
        assert_eq!(
            note(ExportFormat::Dng, ExportColor::Linear, false, false),
            say!(w, "export-color-dng")
        );
    }

    fn open(
        cx: &mut gpui::TestAppContext,
        info: ExportInfo,
        format: ExportFormat,
        sixteen: bool,
        space: ExportSpace,
    ) -> (gpui::AnyWindowHandle, Entity<ExportDialog>) {
        crate::test_support::open(cx, |window, cx| {
            cx.new(|cx| {
                ExportDialog::new(
                    info,
                    format,
                    sixteen,
                    space,
                    ExportPlace::Frames,
                    false,
                    window,
                    cx,
                )
            })
        })
    }

    fn pick(
        cx: &mut gpui::TestAppContext,
        window: gpui::AnyWindowHandle,
        dialog: &Entity<ExportDialog>,
        format: ExportFormat,
    ) {
        cx.update_window(window, |_, window, cx| {
            dialog.update(cx, |d, cx| d.pick_format(format, window, cx))
        })
        .unwrap();
    }

    /// 16 bits only where the stack has them or it is adjusted: 8-bit
    /// frames unadjusted write 8, radios fixed; otherwise the choice is
    /// kept, but for JPEG (8) and DNG (16).
    #[gpui::test]
    fn sixteen_bits_where_the_stack_has_them_or_is_adjusted(cx: &mut gpui::TestAppContext) {
        for (source, adjusted) in [(false, false), (true, false), (false, true)] {
            let has = source || adjusted;
            let info = info(ExportColor::Srgb, source, adjusted);
            let (window, dialog) = open(cx, info, ExportFormat::Tiff, true, ExportSpace::Srgb);
            let seen = |cx: &mut gpui::TestAppContext| {
                dialog.read_with(cx, |d, cx| {
                    (d.depth_fixed(), d.sixteen_bit(), d.choice(cx).unwrap().sixteen_bit)
                })
            };
            assert_eq!(seen(cx), (!has, has, has), "TIFF, 16 chosen last: {source} {adjusted}");
            for (format, sixteen) in [(ExportFormat::Jpeg, false), (ExportFormat::Png, has)] {
                pick(cx, window, &dialog, format);
                assert_eq!(seen(cx).1, sixteen, "{format:?}");
            }
        }
    }

    /// An adjusted RAW render offers the four spaces, ProPhoto only at 16
    /// bits; unadjusted, or as DNG, it offers none and writes as before.
    #[gpui::test]
    fn an_adjusted_raw_render_picks_its_space(cx: &mut gpui::TestAppContext) {
        let raw = |adjusted| info(ExportColor::Linear, true, adjusted);
        let (window, dialog) =
            open(cx, raw(true), ExportFormat::Tiff, false, ExportSpace::ProPhoto);
        let seen = |cx: &mut gpui::TestAppContext| {
            dialog.read_with(cx, |d, cx| {
                let c = d.choice(cx).unwrap();
                (d.offers_spaces(), c.space, c.sixteen_bit, d.depth_fixed())
            })
        };
        assert_eq!(seen(cx), (true, ExportSpace::ProPhoto, true, true), "ProPhoto holds 16 bits");
        pick(cx, window, &dialog, ExportFormat::Jpeg);
        assert_eq!(seen(cx), (true, ExportSpace::AdobeRgb, false, true), "JPEG has no ProPhoto");
        pick(cx, window, &dialog, ExportFormat::Dng);
        assert!(!seen(cx).0, "a DNG is linear: no space to pick");

        let (_, plain) = open(cx, raw(false), ExportFormat::Tiff, true, ExportSpace::DisplayP3);
        assert!(!plain.read_with(cx, |d, _| d.offers_spaces()), "unadjusted: sRGB as before");
        let standard = info(ExportColor::Srgb, true, true);
        let (_, std) = open(cx, standard, ExportFormat::Tiff, true, ExportSpace::DisplayP3);
        assert!(!std.read_with(cx, |d, _| d.offers_spaces()), "a standard stack keeps its profile");
    }
}
