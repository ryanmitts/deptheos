//! A render's adjustments as plain data: what each slider is set to, the
//! process version that says what those values mean, each slider's numbers
//! ([`slider`]), the relative temperature scale ([`temperature`]) and the
//! reframe ([`reframe`]). No GPU and no other Deptheos crate; the color
//! render (`deptheos_render`) turns a value into the shader's constants.

pub mod reframe;
pub mod slider;
pub mod temperature;

pub use reframe::{Flip, Reframe, Turn};

/// What a set of adjustment values means. A saved value names its version;
/// version 1 never changes once it ships, and a change of look is a new
/// version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessVersion(u32);

impl ProcessVersion {
    pub const V1: Self = Self(1);
    /// The version a render that starts neutral takes.
    pub const LATEST: Self = Self::V1;

    /// The version numbered `n`, if this build knows it.
    pub fn known(n: u32) -> Option<Self> {
        (1..=Self::LATEST.0).contains(&n).then_some(Self(n))
    }

    pub fn number(self) -> u32 {
        self.0
    }
}

/// One render's adjustments. Every slider at its default is neutral, which
/// draws the stored picture exactly as it draws with no adjustments. A
/// value that isn't a finite number reads as its slider's default.
#[derive(Clone, Debug, PartialEq)]
pub struct Adjustments {
    /// What the values mean.
    pub process: ProcessVersion,
    /// Stops ([`slider::EXPOSURE`]).
    pub exposure: f64,
    pub white_balance: WhiteBalance,
    /// [`slider::CONTRAST`], [`slider::HIGHLIGHTS`], [`slider::SHADOWS`],
    /// [`slider::WHITES`], [`slider::BLACKS`].
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
    /// [`slider::CLARITY`], [`slider::TEXTURE`].
    pub clarity: f64,
    pub texture: f64,
    pub sharpening: Sharpening,
    /// [`slider::VIBRANCE`], [`slider::SATURATION`].
    pub vibrance: f64,
    pub saturation: f64,
}

/// Sharpening: its Amount ([`slider::SHARPENING`]), and its Radius,
/// Detail and Masking ([`slider::SHARPEN_RADIUS`],
/// [`slider::SHARPEN_DETAIL`], [`slider::SHARPEN_MASKING`]), which change
/// nothing while the amount is 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sharpening {
    pub amount: f64,
    pub radius: f64,
    pub detail: f64,
    pub masking: f64,
}

impl Sharpening {
    pub const NONE: Self = Self {
        amount: slider::SHARPENING.default,
        radius: slider::SHARPEN_RADIUS.default,
        detail: slider::SHARPEN_DETAIL.default,
        masking: slider::SHARPEN_MASKING.default,
    };

    /// Whether it sharpens nothing.
    pub fn is_none(self) -> bool {
        at_default(&slider::SHARPENING, self.amount)
    }
}

/// What a render's white is set to. A camera RAW stack takes kelvins,
/// a standard stack the relative scale; a value of the other kind reads
/// as as shot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WhiteBalance {
    /// The stack's own: a camera RAW stack's as-shot white, a standard
    /// stack's values as stored.
    AsShot,
    /// A camera RAW stack's, in kelvins ([`slider::TEMPERATURE_KELVIN`])
    /// and the DNG SDK's tint ([`slider::TINT_RAW`]), positive toward
    /// magenta.
    Kelvin { temperature: f64, tint: f64 },
    /// A standard stack's, on the relative scale
    /// ([`slider::TEMPERATURE_RELATIVE`], [`temperature`]), positive
    /// warmer, and its tint ([`slider::TINT_RELATIVE`]), positive toward
    /// magenta.
    Relative { temperature: f64, tint: f64 },
}

impl WhiteBalance {
    /// Whether this draws the stack's own white. Kelvins always draw
    /// through the SDK's white point, as-shot values too.
    pub fn is_as_shot(self) -> bool {
        match self {
            Self::AsShot => true,
            Self::Kelvin { .. } => false,
            Self::Relative { temperature, tint } => {
                at_default(&slider::TEMPERATURE_RELATIVE, temperature)
                    && at_default(&slider::TINT_RELATIVE, tint)
            }
        }
    }
}

impl Adjustments {
    /// Every slider at its default, in the latest process.
    pub const NEUTRAL: Self = Self {
        process: ProcessVersion::LATEST,
        exposure: 0.0,
        white_balance: WhiteBalance::AsShot,
        contrast: 0.0,
        highlights: 0.0,
        shadows: 0.0,
        whites: 0.0,
        blacks: 0.0,
        clarity: 0.0,
        texture: 0.0,
        sharpening: Sharpening::NONE,
        vibrance: 0.0,
        saturation: 0.0,
    };

    pub fn process(&self) -> ProcessVersion {
        self.process
    }

    /// These values read in `process`.
    pub fn in_process(self, process: ProcessVersion) -> Self {
        Self { process, ..self }
    }

    /// Whether it draws as no adjustments do: every slider at its
    /// default, Sharpening's Radius, Detail and Masking aside while its
    /// amount is 0.
    pub fn is_neutral(&self) -> bool {
        let Self {
            process: _,
            exposure,
            white_balance,
            contrast,
            highlights,
            shadows,
            whites,
            blacks,
            clarity,
            texture,
            sharpening,
            vibrance,
            saturation,
        } = self;
        at_default(&slider::EXPOSURE, *exposure)
            && white_balance.is_as_shot()
            && at_default(&slider::CONTRAST, *contrast)
            && at_default(&slider::HIGHLIGHTS, *highlights)
            && at_default(&slider::SHADOWS, *shadows)
            && at_default(&slider::WHITES, *whites)
            && at_default(&slider::BLACKS, *blacks)
            && at_default(&slider::CLARITY, *clarity)
            && at_default(&slider::TEXTURE, *texture)
            && sharpening.is_none()
            && at_default(&slider::VIBRANCE, *vibrance)
            && at_default(&slider::SATURATION, *saturation)
    }
}

/// Whether `v` reads as `spec`'s default.
fn at_default(spec: &slider::SliderSpec, v: f64) -> bool {
    spec.clamp(v) == spec.default
}

impl Default for Adjustments {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_the_latest_process() {
        assert_eq!(Adjustments::NEUTRAL.process(), ProcessVersion::LATEST);
        assert!(Adjustments::default().is_neutral());
    }

    #[test]
    fn a_slider_off_its_default_is_not_neutral() {
        let exposure = |exposure| Adjustments { exposure, ..Adjustments::NEUTRAL };
        assert!(!exposure(0.01).is_neutral());
        assert!(exposure(f64::NAN).is_neutral());
        assert_eq!(exposure(1.0).in_process(ProcessVersion::V1).exposure, 1.0);
        let white = |white_balance| Adjustments { white_balance, ..Adjustments::NEUTRAL };
        let kelvin = WhiteBalance::Kelvin { temperature: 5000.0, tint: 0.0 };
        assert!(!white(kelvin).is_neutral());
        assert!(white(WhiteBalance::Relative { temperature: 0.0, tint: f64::NAN }).is_neutral());
        assert!(!white(WhiteBalance::Relative { temperature: 0.0, tint: 1.0 }).is_neutral());
        let sharpen = |amount, radius| Adjustments {
            sharpening: Sharpening { amount, radius, ..Sharpening::NONE },
            ..Adjustments::NEUTRAL
        };
        assert!(sharpen(0.0, 2.5).is_neutral());
        assert!(!sharpen(40.0, 1.0).is_neutral());
    }

    #[test]
    fn only_known_versions_read() {
        assert_eq!(ProcessVersion::known(1), Some(ProcessVersion::V1));
        assert_eq!(ProcessVersion::known(0), None);
        assert_eq!(ProcessVersion::known(ProcessVersion::LATEST.number() + 1), None);
    }
}
