//! A DNG export's as-shot white against the case's white balance.

use deptheos_adjust::WhiteBalance;
use deptheos_dng::{Temperature, WhiteXy};

use crate::{Frames, Report, Set};

/// How far a DNG's as-shot white may be from the one asked for, in mireds
/// (a million over the kelvins) and in the SDK's tint. The white goes
/// through AsShotNeutral's rationals and back, which moves it by under
/// 0.05 of either (`a_dngs_white_is_the_one_asked_for`); half a mired is
/// half a 50 K step at the Kelvin slider's middle, and half a tint step.
pub(crate) const WHITE_MIREDS: f64 = 0.5;
pub(crate) const WHITE_TINT: f64 = 0.5;

/// The DNG's as-shot white (`export`, read back through the SDK) is the
/// case's Kelvin white balance, or with none the as-shot white of one of
/// its frames.
pub(crate) fn white_check(
    report: &mut Report,
    export: Option<WhiteXy>,
    asked: WhiteBalance,
    set: &Set,
    frames: &Frames,
) {
    let Some(export) = export else {
        report.check("white", false, "the DNG has no as-shot white");
        return;
    };
    let ours = Temperature::of(export);
    let said = |t: Temperature| format!("{:.0} K, tint {:+.1}", t.kelvin, t.tint);
    let within = format!("within {WHITE_MIREDS} mired and {WHITE_TINT} tint");
    match asked {
        WhiteBalance::Kelvin { temperature, tint } => {
            let theirs = Temperature { kelvin: temperature, tint };
            report.check(
                "white",
                near(ours, theirs),
                format!("{}; {} asked for ({within})", said(ours), said(theirs)),
            );
        }
        WhiteBalance::Relative { .. } => {
            report.check("white", false, "a relative white balance asked of a DNG, not kelvins");
        }
        WhiteBalance::AsShot => {
            let nearest = frames
                .whites
                .iter()
                .enumerate()
                .filter_map(|(i, w)| w.map(|w| (i, Temperature::of(w))))
                .min_by(|a, b| distance(ours, a.1).total_cmp(&distance(ours, b.1)));
            let Some((i, theirs)) = nearest else {
                report.check("white", false, "no RAW or DNG frame with a white to compare with");
                return;
            };
            let name = set.frames[i].file_name().unwrap_or_default().to_string_lossy();
            report.check(
                "white",
                near(ours, theirs),
                format!("{}; {name} as shot {} ({within})", said(ours), said(theirs)),
            );
        }
    }
}

fn mireds(t: Temperature) -> f64 {
    1e6 / t.kelvin
}

fn near(a: Temperature, b: Temperature) -> bool {
    (mireds(a) - mireds(b)).abs() <= WHITE_MIREDS && (a.tint - b.tint).abs() <= WHITE_TINT
}

/// For picking the nearest frame: each part over its tolerance.
fn distance(a: Temperature, b: Temperature) -> f64 {
    ((mireds(a) - mireds(b)) / WHITE_MIREDS).abs().max(((a.tint - b.tint) / WHITE_TINT).abs())
}
