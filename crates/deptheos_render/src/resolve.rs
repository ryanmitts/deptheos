//! The resolve step: a stack's color, its adjustments and an output space
//! in, how the draw goes out. Pure, with no GPU, so a table test reads it
//! as the behavior.

use deptheos_adjust::Adjustments;
use deptheos_core::io::IccProfile;
use deptheos_dng::RenderParams;

use crate::dng_render::Constants;

/// Where a draw's colors go. The screen is sRGB, or a standard stack's
/// own profile while Color Management is off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputSpace {
    Srgb,
    /// A standard stack's own profile, its values passed through when
    /// unadjusted; a linear stack's render is sRGB.
    StackProfile,
}

/// A stack's color, as the draw needs it.
#[derive(Clone, Copy)]
pub enum Color<'a> {
    /// Linear camera RGB, rendered through the reference frame's color
    /// science.
    Linear(&'a RenderParams),
    /// Stored values in `profile` (`None`: sRGB).
    Standard(Option<&'a IccProfile>),
}

/// How stored pixels become the output's colors.
#[derive(Debug, PartialEq)]
pub enum Draw<'a> {
    /// On the GPU through the color render, into [`Constants::output`].
    Rendered(Box<Constants>),
    /// The stored values as they are, converted on the CPU from `profile`
    /// (`None`: sRGB) to `output`.
    Stored { profile: Option<&'a IccProfile>, output: OutputSpace },
}

impl Draw<'_> {
    /// The space the drawn values are in.
    pub fn output(&self) -> OutputSpace {
        match self {
            Draw::Rendered(constants) => constants.output(),
            Draw::Stored { output, .. } => *output,
        }
    }
}

/// How `color` draws under `adjustments` (`None`: the stored picture, as
/// source frames and Retouch show it) into `output`. A neutral adjustment
/// draws as none does: a standard stack's stored values on the CPU.
pub fn resolve<'a>(
    color: Color<'a>,
    adjustments: Option<&Adjustments>,
    output: OutputSpace,
) -> Draw<'a> {
    match (color, adjustments) {
        (Color::Standard(profile), None) => Draw::Stored { profile, output },
        (Color::Standard(profile), Some(a)) if a.is_neutral() => Draw::Stored { profile, output },
        (color, Some(adjustments)) => Draw::Rendered(resolve_adjusted(color, adjustments, output)),
        (color @ Color::Linear(_), None) => {
            Draw::Rendered(resolve_adjusted(color, &Adjustments::NEUTRAL, output))
        }
    }
}

/// The constants `color` draws with on the GPU under `adjustments`, neutral
/// ones too: the path an adjusted stack takes, which the tests and the
/// redraw command take for a standard stack before any slider is set.
pub fn resolve_adjusted(
    color: Color<'_>,
    _adjustments: &Adjustments,
    output: OutputSpace,
) -> Box<Constants> {
    Box::new(match color {
        Color::Linear(params) => Constants::linear(params),
        Color::Standard(profile) => Constants::standard(profile, output),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> RenderParams {
        RenderParams { baseline_exposure: 0.35, ..RenderParams::srgb_identity() }
    }

    /// Neutral draws exactly as no adjustments, for both kinds and every
    /// output.
    #[test]
    fn neutral_is_none() {
        let (params, profile) = (params(), IccProfile::adobe_rgb_compatible());
        let outputs = [OutputSpace::Srgb, OutputSpace::StackProfile];
        for color in
            [Color::Linear(&params), Color::Standard(None), Color::Standard(Some(&profile))]
        {
            for output in outputs {
                let neutral = resolve(color, Some(&Adjustments::NEUTRAL), output);
                assert_eq!(neutral, resolve(color, None, output));
            }
        }
    }

    /// A linear stack renders on the GPU whatever the output, from its own
    /// parameters; a standard one draws as stored, toward the output.
    #[test]
    fn each_kind_draws_its_way() {
        let (params, profile) = (params(), IccProfile::adobe_rgb_compatible());
        let srgb = OutputSpace::Srgb;
        assert_eq!(
            resolve(Color::Linear(&params), None, srgb),
            Draw::Rendered(Box::new(Constants::linear(&params)))
        );
        assert_ne!(
            resolve(Color::Linear(&params), None, srgb),
            resolve(Color::Linear(&RenderParams::srgb_identity()), None, srgb)
        );
        let own = OutputSpace::StackProfile;
        assert_eq!(
            resolve(Color::Standard(Some(&profile)), None, own),
            Draw::Stored { profile: Some(&profile), output: own }
        );
    }

    /// The adjusted path draws a matrix-and-curves standard stack on the
    /// GPU, in its own profile when asked, with no CPU pass first.
    #[test]
    fn an_adjusted_standard_stack_draws_on_the_gpu() {
        let profile = IccProfile::adobe_rgb_compatible();
        let neutral = Adjustments::NEUTRAL;
        for (output, drawn) in [
            (OutputSpace::StackProfile, OutputSpace::StackProfile),
            (OutputSpace::Srgb, OutputSpace::Srgb),
        ] {
            let constants = resolve_adjusted(Color::Standard(Some(&profile)), &neutral, output);
            assert_eq!(constants.output(), drawn);
            assert!(constants.linearizer().is_none());
        }
    }
}
