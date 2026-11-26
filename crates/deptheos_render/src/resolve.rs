//! The resolve step: a stack's color, its adjustments and an output space
//! in, how the draw goes out. Pure, with no GPU, so a table test reads it
//! as the behavior. Each slider's law is here: what its value does to the
//! numbers the shaders take.

use deptheos_adjust::{Adjustments, WhiteBalance, slider};
use deptheos_core::io::IccProfile;
use deptheos_dng::RenderParams;

use crate::curve::Curve;
use crate::dng_render::{Constants, Exposure, Stages};

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
    adjustments: &Adjustments,
    output: OutputSpace,
) -> Box<Constants> {
    let exposure = slider::EXPOSURE.clamp(adjustments.exposure);
    let contrast = slider::CONTRAST.clamp(adjustments.contrast);
    let whites = slider::WHITES.clamp(adjustments.whites);
    let blacks = slider::BLACKS.clamp(adjustments.blacks);
    Box::new(match color {
        Color::Linear(params) => {
            // A camera RAW stack's white is in `params` (the SDK's white point).
            let exposure = linear_exposure(exposure, params.baseline_exposure);
            let curve = Curve::raw(contrast, whites, blacks);
            let stages = Stages { white_balance: None, exposure, curve };
            Constants::linear(params, &stages)
        }
        Color::Standard(profile) => {
            let white_balance = relative_white_balance(adjustments.white_balance);
            let curve = Curve::standard(contrast, whites, blacks);
            let stages = Stages { white_balance, exposure: standard_exposure(exposure), curve };
            Constants::standard(profile, output, &stages)
        }
    })
}

/// A standard stack's white balance on the relative scale: one Bradford
/// adaptation in linear ProPhoto, white's largest channel kept at 1
/// (`crate::white`). As shot, or kelvins, which are a camera RAW stack's,
/// adapt nothing.
fn relative_white_balance(white_balance: WhiteBalance) -> Option<crate::white::Mat3> {
    let WhiteBalance::Relative { temperature, tint } = white_balance else { return None };
    let temperature = slider::TEMPERATURE_RELATIVE.clamp(temperature);
    let tint = slider::TINT_RELATIVE.clamp(tint);
    (temperature != 0.0 || tint != 0.0).then(|| crate::white::relative_matrix(temperature, tint))
}

/// Stops below white where the shoulder starts once exposure has raised
/// the picture far past white; it fades in from a clamp at white as the
/// raise grows ([`knee`]).
pub const SHOULDER_STOPS: f64 = 1.0;

/// The shoulder's knee when exposure raises the picture `raised` stops
/// past what was white: 1 (a clamp) at none, approaching
/// [`SHOULDER_STOPS`] below white as it grows.
fn knee(raised: f64) -> f64 {
    let raised = raised.max(0.0);
    (-SHOULDER_STOPS * (1.0 - (-raised).exp2())).exp2()
}

/// Exposure on a linear stack: `stops` on top of its `baseline`. The law
/// is on the total: the SDK's ramp and darkening curve take as much of it
/// as they can, never less than the baseline's own darkening, and a linear
/// gain takes the rest, so a positive slider renders as the SDK's exposure
/// does and a negative one darkens white as well. The shoulder takes only
/// what is raised past the baseline's own white.
fn linear_exposure(stops: f64, baseline: f64) -> Option<Exposure> {
    if stops == 0.0 {
        return None;
    }
    let total = baseline + stops;
    let ramp = total.max(baseline.min(0.0));
    let raised = ramp.max(0.0) - baseline.max(0.0);
    Some(Exposure { ramp, gain: (total - ramp).exp2(), knee: knee(raised) })
}

/// Exposure on a standard stack: a linear gain of `stops`, its shoulder
/// taking what passes white.
fn standard_exposure(stops: f64) -> Option<Exposure> {
    (stops != 0.0).then(|| Exposure { ramp: 0.0, gain: stops.exp2(), knee: knee(stops) })
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
            Draw::Rendered(Box::new(Constants::linear(&params, &Stages::default())))
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

    /// Exposure takes the total: the SDK's ramp and darkening curve as far
    /// as they go, a gain for the rest, the shoulder for what is raised
    /// past white.
    #[test]
    fn exposure_is_a_law_on_the_total() {
        let e = |stops, baseline| linear_exposure(stops, baseline).unwrap();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
        assert_eq!(linear_exposure(0.0, 0.35), None);
        // Raised: the SDK's own exposure at the total, whatever its sign.
        for (stops, baseline) in [(1.0, 0.35), (0.3, -0.5), (1.0, -0.5)] {
            let got = e(stops, baseline);
            assert!(close(got.ramp, baseline + stops) && close(got.gain, 1.0), "{got:?}");
        }
        assert!(e(1.0, -0.5).knee < 1.0 && close(e(0.3, -0.5).knee, 1.0));
        // Lowered: the ramp unclipped down to a total of 0, then a gain;
        // a negative baseline keeps its own darkening.
        let lowered = e(-0.35, 0.35);
        assert!(close(lowered.ramp, 0.0) && close(lowered.gain, 1.0) && close(lowered.knee, 1.0));
        let darker = e(-1.0, 0.35);
        assert!(close(darker.ramp, 0.0) && close(darker.gain, (-0.65f64).exp2()));
        let negative = e(-1.0, -0.5);
        assert!(close(negative.ramp, -0.5) && close(negative.gain, 0.5));
        // The shoulder fades in from a clamp and never passes its reach.
        assert!(close(knee(0.0), 1.0) && knee(0.01) < 1.0 && knee(0.01) > 0.99);
        assert!(knee(40.0) > (-SHOULDER_STOPS).exp2() - 1e-9);
        assert_eq!(standard_exposure(0.0), None);
        assert!(close(standard_exposure(-1.0).unwrap().gain, 0.5));
        assert!(close(standard_exposure(-1.0).unwrap().knee, 1.0));
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
