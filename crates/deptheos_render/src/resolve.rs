//! The resolve step: a stack's color, its adjustments and an output space
//! in, how the draw goes out. Pure, with no GPU, so a table test reads it
//! as the behavior.

use deptheos_adjust::Adjustments;
use deptheos_core::io::IccProfile;
use deptheos_dng::RenderParams;

use crate::dng_render::Constants;

/// Where a draw's colors go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputSpace {
    /// The screen: sRGB, or a standard stack's values as they are while
    /// Color Management is off.
    Display,
    Srgb,
    /// A standard stack's own profile, its values passed through; a
    /// linear stack's render is sRGB.
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
    /// On the GPU through the color render, which writes sRGB.
    Rendered(Box<Constants>),
    /// The stored values as they are, converted on the CPU from `profile`
    /// (`None`: sRGB) to `output`.
    Stored { profile: Option<&'a IccProfile>, output: OutputSpace },
}

/// How `color` draws under `adjustments` (`None`: the stored picture, as
/// source frames and Retouch show it) into `output`. A neutral adjustment
/// draws as none does.
pub fn resolve<'a>(
    color: Color<'a>,
    _adjustments: Option<&Adjustments>,
    output: OutputSpace,
) -> Draw<'a> {
    match color {
        Color::Linear(params) => Draw::Rendered(Box::new(Constants::linear(params))),
        Color::Standard(profile) => Draw::Stored { profile, output },
    }
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
        let outputs = [OutputSpace::Display, OutputSpace::Srgb, OutputSpace::StackProfile];
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
        assert_eq!(
            resolve(Color::Standard(Some(&profile)), None, OutputSpace::Display),
            Draw::Stored { profile: Some(&profile), output: OutputSpace::Display }
        );
    }
}
