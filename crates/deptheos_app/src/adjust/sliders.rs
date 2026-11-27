//! The Adjust panel's sliders, as both apps draw them: in groups and in
//! order, each with its numbers ([`deptheos_render::slider`]), how it reads
//! and sets an [`Adjustments`] value, how its value is written, and its
//! label and help (`adjust-*` in `deptheos_app.ftl`). A panel and the bridge
//! need no code of their own per slider.

use deptheos_dng::Temperature;
use deptheos_render::slider::{self, SliderSpec};
use deptheos_render::{Adjustments, WhiteBalance};

use crate::library::adjustments::rules::StackKind;
use crate::runs::session::{CompletedStack, StackColor};
use crate::words::{Words, say};

/// One adjustment slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slider {
    Temperature,
    Tint,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Clarity,
    Texture,
    Vibrance,
    Saturation,
    /// Sharpening's Amount.
    Sharpening,
    SharpenRadius,
    SharpenDetail,
    SharpenMasking,
    /// Vignette's Amount.
    Vignette,
    VignetteMidpoint,
}

/// A heading of the panel, over its sliders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    WhiteBalance,
    Tone,
    Presence,
    Sharpening,
    Vignette,
}

/// What a render's sliders read against: its kind, whether it has color,
/// and a camera RAW stack's as-shot white, where its white balance
/// sliders start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    pub kind: StackKind,
    /// Whether the color sliders show: white balance, Vibrance and
    /// Saturation ([`CompletedStack::has_color`]).
    pub color: bool,
    pub as_shot: Option<Temperature>,
}

impl Panel {
    /// The panel of `stack`.
    pub fn of(stack: &CompletedStack) -> Self {
        let kind = match stack.color {
            StackColor::Linear { .. } => StackKind::CameraRaw,
            StackColor::Srgb { .. } => StackKind::Standard,
        };
        Self { kind, color: stack.has_color(), as_shot: stack.as_shot() }
    }

    /// The groups shown, in order, each with its sliders in order. A stack
    /// with no color has no White Balance, Vibrance or Saturation.
    pub fn groups(&self) -> Vec<(Group, Vec<Slider>)> {
        Group::ALL
            .into_iter()
            .map(|g| (g, g.sliders().iter().copied().filter(|s| self.shows(*s)).collect()))
            .filter(|(_, sliders): &(Group, Vec<Slider>)| !sliders.is_empty())
            .collect()
    }

    /// Whether `slider` shows on this panel.
    pub fn shows(&self, slider: Slider) -> bool {
        self.color || !slider.is_color()
    }

    /// The temperature and tint as shot, on this kind's scale.
    fn as_shot_white(&self) -> (f64, f64) {
        match (self.kind, self.as_shot) {
            (StackKind::CameraRaw, Some(t)) => {
                (slider::TEMPERATURE_KELVIN.clamp(t.kelvin), slider::TINT_RAW.clamp(t.tint))
            }
            (StackKind::CameraRaw, None) => {
                (slider::TEMPERATURE_KELVIN.default, slider::TINT_RAW.default)
            }
            (StackKind::Standard, _) => {
                (slider::TEMPERATURE_RELATIVE.default, slider::TINT_RELATIVE.default)
            }
        }
    }

    /// The temperature and tint `white` reads as here.
    fn white(&self, white: WhiteBalance) -> (f64, f64) {
        match (self.kind, white) {
            (StackKind::CameraRaw, WhiteBalance::Kelvin { temperature, tint })
            | (StackKind::Standard, WhiteBalance::Relative { temperature, tint }) => {
                (temperature, tint)
            }
            // As shot, or a value on the other kind's scale, which draws as shot.
            _ => self.as_shot_white(),
        }
    }

    /// `temperature` and `tint` as this kind's white balance; as shot
    /// when both are.
    fn white_balance(&self, temperature: f64, tint: f64) -> WhiteBalance {
        if (temperature, tint) == self.as_shot_white() {
            return WhiteBalance::AsShot;
        }
        match self.kind {
            StackKind::CameraRaw => WhiteBalance::Kelvin { temperature, tint },
            StackKind::Standard => WhiteBalance::Relative { temperature, tint },
        }
    }
}

impl Group {
    pub const ALL: [Self; 5] =
        [Self::WhiteBalance, Self::Tone, Self::Presence, Self::Sharpening, Self::Vignette];

    /// Its sliders, in order.
    pub fn sliders(self) -> &'static [Slider] {
        use Slider::*;
        match self {
            Self::WhiteBalance => &[Temperature, Tint],
            Self::Tone => &[Exposure, Contrast, Highlights, Shadows, Whites, Blacks],
            Self::Presence => &[Clarity, Texture, Vibrance, Saturation],
            Self::Sharpening => &[Sharpening, SharpenRadius, SharpenDetail, SharpenMasking],
            Self::Vignette => &[Vignette, VignetteMidpoint],
        }
    }

    /// Its heading: "White Balance".
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::WhiteBalance => say!(words, "adjust-group-white-balance"),
            Self::Tone => say!(words, "adjust-group-tone"),
            Self::Presence => say!(words, "adjust-group-presence"),
            Self::Sharpening => say!(words, "adjust-group-sharpening"),
            Self::Vignette => say!(words, "adjust-group-vignette"),
        }
    }
}

/// How a slider's value is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    /// Stops, signed, to two places: "+0.35 EV".
    Stops,
    /// Kelvins, whole: "5200 K".
    Kelvin,
    /// Pixels, to one place: "1.0 px".
    Pixels,
    /// A whole number, signed when the slider runs both ways: "+25".
    Whole,
}

impl Slider {
    pub const ALL: [Self; 18] = [
        Self::Temperature,
        Self::Tint,
        Self::Exposure,
        Self::Contrast,
        Self::Highlights,
        Self::Shadows,
        Self::Whites,
        Self::Blacks,
        Self::Clarity,
        Self::Texture,
        Self::Vibrance,
        Self::Saturation,
        Self::Sharpening,
        Self::SharpenRadius,
        Self::SharpenDetail,
        Self::SharpenMasking,
        Self::Vignette,
        Self::VignetteMidpoint,
    ];

    /// Whether it works on color: a stack with none doesn't show it.
    pub fn is_color(self) -> bool {
        matches!(self, Self::Temperature | Self::Tint | Self::Vibrance | Self::Saturation)
    }

    /// Its numbers on `panel`: white balance's depend on the kind.
    pub fn spec(self, panel: &Panel) -> SliderSpec {
        let raw = panel.kind == StackKind::CameraRaw;
        match self {
            Self::Temperature if raw => slider::TEMPERATURE_KELVIN,
            Self::Temperature => slider::TEMPERATURE_RELATIVE,
            Self::Tint if raw => slider::TINT_RAW,
            Self::Tint => slider::TINT_RELATIVE,
            Self::Exposure => slider::EXPOSURE,
            Self::Contrast => slider::CONTRAST,
            Self::Highlights => slider::HIGHLIGHTS,
            Self::Shadows => slider::SHADOWS,
            Self::Whites => slider::WHITES,
            Self::Blacks => slider::BLACKS,
            Self::Clarity => slider::CLARITY,
            Self::Texture => slider::TEXTURE,
            Self::Vibrance => slider::VIBRANCE,
            Self::Saturation => slider::SATURATION,
            Self::Sharpening => slider::SHARPENING,
            Self::SharpenRadius => slider::SHARPEN_RADIUS,
            Self::SharpenDetail => slider::SHARPEN_DETAIL,
            Self::SharpenMasking => slider::SHARPEN_MASKING,
            Self::Vignette => slider::VIGNETTE,
            Self::VignetteMidpoint => slider::VIGNETTE_MIDPOINT,
        }
    }

    /// Its value in `a`, within its range. White balance as shot reads as
    /// the as-shot temperature and tint.
    pub fn get(self, a: &Adjustments, panel: &Panel) -> f64 {
        let v = match self {
            Self::Temperature => panel.white(a.white_balance).0,
            Self::Tint => panel.white(a.white_balance).1,
            Self::Exposure => a.exposure,
            Self::Contrast => a.contrast,
            Self::Highlights => a.highlights,
            Self::Shadows => a.shadows,
            Self::Whites => a.whites,
            Self::Blacks => a.blacks,
            Self::Clarity => a.clarity,
            Self::Texture => a.texture,
            Self::Vibrance => a.vibrance,
            Self::Saturation => a.saturation,
            Self::Sharpening => a.sharpening.amount,
            Self::SharpenRadius => a.sharpening.radius,
            Self::SharpenDetail => a.sharpening.detail,
            Self::SharpenMasking => a.sharpening.masking,
            Self::Vignette => a.vignette.amount,
            Self::VignetteMidpoint => a.vignette.midpoint,
        };
        self.spec(panel).clamp(v)
    }

    /// `a` with this slider at `v`, within its range; the process version
    /// stays.
    pub fn set(self, a: &Adjustments, panel: &Panel, v: f64) -> Adjustments {
        let v = self.spec(panel).clamp(v);
        let mut a = a.clone();
        match self {
            Self::Temperature => {
                let (_, tint) = panel.white(a.white_balance);
                a.white_balance = panel.white_balance(v, tint);
            }
            Self::Tint => {
                let (temperature, _) = panel.white(a.white_balance);
                a.white_balance = panel.white_balance(temperature, v);
            }
            Self::Exposure => a.exposure = v,
            Self::Contrast => a.contrast = v,
            Self::Highlights => a.highlights = v,
            Self::Shadows => a.shadows = v,
            Self::Whites => a.whites = v,
            Self::Blacks => a.blacks = v,
            Self::Clarity => a.clarity = v,
            Self::Texture => a.texture = v,
            Self::Vibrance => a.vibrance = v,
            Self::Saturation => a.saturation = v,
            Self::Sharpening => a.sharpening.amount = v,
            Self::SharpenRadius => a.sharpening.radius = v,
            Self::SharpenDetail => a.sharpening.detail = v,
            Self::SharpenMasking => a.sharpening.masking = v,
            Self::Vignette => a.vignette.amount = v,
            Self::VignetteMidpoint => a.vignette.midpoint = v,
        }
        a
    }

    /// Where it rests: its default, or the as-shot white for white
    /// balance.
    pub fn neutral(self, panel: &Panel) -> f64 {
        match self {
            Self::Temperature => panel.as_shot_white().0,
            Self::Tint => panel.as_shot_white().1,
            _ => self.spec(panel).default,
        }
    }

    /// `a` with this slider back where it rests; the process version stays
    /// (ADR-152). White balance becomes as shot once both of its sliders are.
    pub fn reset(self, a: &Adjustments, panel: &Panel) -> Adjustments {
        self.set(a, panel, self.neutral(panel))
    }

    /// Whether it rests in `a`.
    pub fn is_neutral(self, a: &Adjustments, panel: &Panel) -> bool {
        self.get(a, panel) == self.neutral(panel)
    }

    fn shown(self, panel: &Panel) -> Shown {
        match self {
            Self::Exposure => Shown::Stops,
            Self::Temperature if panel.kind == StackKind::CameraRaw => Shown::Kelvin,
            Self::SharpenRadius => Shown::Pixels,
            _ => Shown::Whole,
        }
    }

    /// `v` as the panel writes it, on its step: "+0.35 EV", "5200 K",
    /// "1.0 px", "+25", "50".
    pub fn value_text(self, v: f64, panel: &Panel, words: &Words) -> String {
        let spec = self.spec(panel);
        let v = spec.snap(v);
        // No "-0".
        let v = if v == 0.0 { 0.0 } else { v };
        let signed = spec.min < 0.0;
        let number = |places: usize| match signed && v > 0.0 {
            true => format!("+{v:.places$}"),
            false => format!("{v:.places$}"),
        };
        match self.shown(panel) {
            Shown::Stops => say!(words, "adjust-value-stops", value = number(2)),
            Shown::Kelvin => say!(words, "adjust-value-kelvin", value = format!("{v:.0}")),
            Shown::Pixels => say!(words, "adjust-value-pixels", value = number(1)),
            Shown::Whole => number(0),
        }
    }

    /// A value typed into the panel: a number, with or without its sign,
    /// its unit ("EV", "K", "px") or a typographic minus. `None` if it
    /// isn't one; the caller clamps and snaps.
    pub fn parse(self, text: &str) -> Option<f64> {
        let text = text.trim().replace('\u{2212}', "-");
        let number = text
            .trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace())
            .trim_start_matches('+');
        number.parse::<f64>().ok().filter(|v| v.is_finite())
    }

    /// Its name: "Exposure". Under the Sharpening and Vignette headings,
    /// their amounts read "Amount".
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::Temperature => say!(words, "adjust-temperature"),
            Self::Tint => say!(words, "adjust-tint"),
            Self::Exposure => say!(words, "adjust-exposure"),
            Self::Contrast => say!(words, "adjust-contrast"),
            Self::Highlights => say!(words, "adjust-highlights"),
            Self::Shadows => say!(words, "adjust-shadows"),
            Self::Whites => say!(words, "adjust-whites"),
            Self::Blacks => say!(words, "adjust-blacks"),
            Self::Clarity => say!(words, "adjust-clarity"),
            Self::Texture => say!(words, "adjust-texture"),
            Self::Vibrance => say!(words, "adjust-vibrance"),
            Self::Saturation => say!(words, "adjust-saturation"),
            Self::Sharpening => say!(words, "adjust-sharpening-amount"),
            Self::SharpenRadius => say!(words, "adjust-sharpening-radius"),
            Self::SharpenDetail => say!(words, "adjust-sharpening-detail"),
            Self::SharpenMasking => say!(words, "adjust-sharpening-masking"),
            Self::Vignette => say!(words, "adjust-vignette-amount"),
            Self::VignetteMidpoint => say!(words, "adjust-vignette-midpoint"),
        }
    }

    /// What moving it does, one sentence.
    pub fn help(self, words: &Words) -> String {
        match self {
            Self::Temperature => say!(words, "adjust-temperature-help"),
            Self::Tint => say!(words, "adjust-tint-help"),
            Self::Exposure => say!(words, "adjust-exposure-help"),
            Self::Contrast => say!(words, "adjust-contrast-help"),
            Self::Highlights => say!(words, "adjust-highlights-help"),
            Self::Shadows => say!(words, "adjust-shadows-help"),
            Self::Whites => say!(words, "adjust-whites-help"),
            Self::Blacks => say!(words, "adjust-blacks-help"),
            Self::Clarity => say!(words, "adjust-clarity-help"),
            Self::Texture => say!(words, "adjust-texture-help"),
            Self::Vibrance => say!(words, "adjust-vibrance-help"),
            Self::Saturation => say!(words, "adjust-saturation-help"),
            Self::Sharpening => say!(words, "adjust-sharpening-amount-help"),
            Self::SharpenRadius => say!(words, "adjust-sharpening-radius-help"),
            Self::SharpenDetail => say!(words, "adjust-sharpening-detail-help"),
            Self::SharpenMasking => say!(words, "adjust-sharpening-masking-help"),
            Self::Vignette => say!(words, "adjust-vignette-amount-help"),
            Self::VignetteMidpoint => say!(words, "adjust-vignette-midpoint-help"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Platform;

    const RAW: Panel = Panel {
        kind: StackKind::CameraRaw,
        color: true,
        as_shot: Some(Temperature { kelvin: 5201.0, tint: 8.0 }),
    };
    const STANDARD: Panel = Panel { kind: StackKind::Standard, color: true, as_shot: None };
    const GRAY: Panel = Panel { kind: StackKind::Standard, color: false, as_shot: None };

    #[test]
    fn every_slider_shows_in_one_group_and_color_ones_only_with_color() {
        for slider in Slider::ALL {
            let n = Group::ALL.iter().filter(|g| g.sliders().contains(&slider)).count();
            assert_eq!(n, 1, "{slider:?}");
        }
        let shown = |panel: &Panel| -> Vec<Slider> {
            panel.groups().into_iter().flat_map(|(_, s)| s).collect()
        };
        assert_eq!(shown(&RAW), Slider::ALL);
        let gray = shown(&GRAY);
        assert!(gray.iter().all(|s| !s.is_color()));
        assert_eq!(gray.len(), Slider::ALL.len() - 4);
        assert!(!GRAY.groups().iter().any(|(g, _)| *g == Group::WhiteBalance));
    }

    #[test]
    fn every_slider_sets_what_it_gets_and_nothing_else() {
        for panel in [RAW, STANDARD] {
            for slider in Slider::ALL {
                let spec = slider.spec(&panel);
                let v = spec.snap(spec.min + 0.3 * (spec.max - spec.min));
                let set = slider.set(&Adjustments::NEUTRAL, &panel, v);
                assert_eq!(slider.get(&set, &panel), v, "{slider:?}");
                for other in Slider::ALL.into_iter().filter(|o| *o != slider) {
                    assert!(other.is_neutral(&set, &panel), "{slider:?} moved {other:?}");
                }
                assert_eq!(set.process(), Adjustments::NEUTRAL.process());
                let back = slider.reset(&set, &panel);
                assert!(back.is_neutral(), "{slider:?} reset on {panel:?}: {back:?}");
                // Out of range and not a number land in range.
                assert_eq!(slider.get(&slider.set(&set, &panel, 1e9), &panel), spec.max);
                assert_eq!(
                    slider.get(&slider.set(&set, &panel, f64::NAN), &panel),
                    slider.get(&slider.set(&set, &panel, spec.default), &panel)
                );
            }
        }
    }

    #[test]
    fn white_balance_starts_as_shot_and_returns_to_it() {
        let a = Adjustments::NEUTRAL;
        assert_eq!((Slider::Temperature.get(&a, &RAW), Slider::Tint.get(&a, &RAW)), (5201.0, 8.0));
        let warmer = Slider::Temperature.set(&a, &RAW, 6000.0);
        assert_eq!(warmer.white_balance, WhiteBalance::Kelvin { temperature: 6000.0, tint: 8.0 });
        let tinted = Slider::Tint.set(&warmer, &RAW, 20.0);
        let back = Slider::Temperature.reset(&tinted, &RAW);
        assert_eq!(back.white_balance, WhiteBalance::Kelvin { temperature: 5201.0, tint: 20.0 });
        assert_eq!(Slider::Tint.reset(&back, &RAW).white_balance, WhiteBalance::AsShot);
        // A value on the other scale reads, and draws, as shot.
        let relative = Adjustments {
            white_balance: WhiteBalance::Relative { temperature: 40.0, tint: 0.0 },
            ..Adjustments::NEUTRAL
        };
        assert!(Slider::Temperature.is_neutral(&relative, &RAW));
        assert_eq!(Slider::Temperature.get(&relative, &STANDARD), 40.0);
    }

    #[test]
    fn values_are_written_on_their_step_with_their_unit() {
        let w = Words::english(Platform::Desktop);
        let cases = [
            (Slider::Exposure, 0.348, RAW, "+0.35 EV"),
            (Slider::Exposure, -1.2, RAW, "-1.20 EV"),
            (Slider::Exposure, -0.001, RAW, "0.00 EV"),
            (Slider::Temperature, 5201.0, RAW, "5200 K"),
            (Slider::Temperature, 25.0, STANDARD, "+25"),
            (Slider::Tint, -8.0, RAW, "-8"),
            (Slider::Contrast, 0.0, RAW, "0"),
            (Slider::SharpenRadius, 1.04, RAW, "1.0 px"),
            (Slider::VignetteMidpoint, 50.0, RAW, "50"),
            (Slider::Sharpening, 40.0, RAW, "40"),
        ];
        for (slider, v, panel, want) in cases {
            assert_eq!(slider.value_text(v, &panel, &w), want, "{slider:?} {v}");
        }
    }

    #[test]
    fn a_typed_value_reads_with_or_without_its_unit() {
        let cases = [
            ("0.5", Some(0.5)),
            ("+0.5 EV", Some(0.5)),
            ("\u{2212}1.25", Some(-1.25)),
            ("5200 K", Some(5200.0)),
            ("1.5px", Some(1.5)),
            ("", None),
            ("warm", None),
            ("inf", None),
        ];
        for (text, want) in cases {
            assert_eq!(Slider::Exposure.parse(text), want, "{text:?}");
        }
    }

    #[test]
    fn every_slider_and_group_has_its_words() {
        let w = Words::english(Platform::Desktop);
        for slider in Slider::ALL {
            assert!(!slider.label(&w).is_empty() && slider.help(&w).ends_with('.'), "{slider:?}");
        }
        for group in Group::ALL {
            assert!(!group.label(&w).is_empty());
        }
    }
}
