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
    };

    pub fn process(&self) -> ProcessVersion {
        self.process
    }

    /// These values read in `process`.
    pub fn in_process(self, process: ProcessVersion) -> Self {
        Self { process, ..self }
    }

    /// Whether every slider is at its default.
    pub fn is_neutral(&self) -> bool {
        let Self { process: _, exposure, white_balance } = self;
        at_default(&slider::EXPOSURE, *exposure) && white_balance.is_as_shot()
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
    }

    #[test]
    fn only_known_versions_read() {
        assert_eq!(ProcessVersion::known(1), Some(ProcessVersion::V1));
        assert_eq!(ProcessVersion::known(0), None);
        assert_eq!(ProcessVersion::known(ProcessVersion::LATEST.number() + 1), None);
    }
}
