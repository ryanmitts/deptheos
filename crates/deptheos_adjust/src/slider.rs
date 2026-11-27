//! Each adjustment slider's numbers: its range, step, default and track.
//! Labels and help are messages in `deptheos_app.ftl`; what a value does
//! to the picture is the process version's.
//!
//! The ranges and the Kelvin track are lightcraft's (MIT OR Apache-2.0:
//! lightcraft 5bdb4e7a6 `crates/develop/src/controls.rs`).

/// How a value sits along the slider's track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    /// Evenly from end to end.
    Linear,
    /// Kelvins: even from the minimum to [`KELVIN_CENTER`] on the left
    /// half, logarithmic above it on the right.
    Kelvin,
}

/// One slider's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderSpec {
    pub min: f64,
    pub max: f64,
    /// The neutral value, and what a value that isn't a finite number reads as.
    pub default: f64,
    pub step: f64,
    pub track: Track,
}

/// The Kelvin track's middle.
pub const KELVIN_CENTER: f64 = 7000.0;
/// Where the Kelvin track's right half turns logarithmic: close to the
/// left half's slope just past the middle, so the thumb moves evenly.
const KELVIN_KNEE: f64 = 1000.0;

const fn centered(min: f64, max: f64, step: f64) -> SliderSpec {
    SliderSpec { min, max, default: 0.0, step, track: Track::Linear }
}

/// Exposure, in stops.
pub const EXPOSURE: SliderSpec = centered(-5.0, 5.0, 0.01);
/// Temperature of a camera RAW stack, in kelvins. Its neutral is the
/// render's as-shot temperature; 6500 K stands in without one.
pub const TEMPERATURE_KELVIN: SliderSpec =
    SliderSpec { min: 2000.0, max: 50_000.0, default: 6500.0, step: 50.0, track: Track::Kelvin };
/// Tint of a camera RAW stack.
pub const TINT_RAW: SliderSpec = centered(-150.0, 150.0, 1.0);
/// Temperature of a standard stack, relative: see [`crate::temperature`].
pub const TEMPERATURE_RELATIVE: SliderSpec = centered(-100.0, 100.0, 1.0);
/// Tint of a standard stack, relative.
pub const TINT_RELATIVE: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const CONTRAST: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const HIGHLIGHTS: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const SHADOWS: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const WHITES: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const BLACKS: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const CLARITY: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const TEXTURE: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const VIBRANCE: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const SATURATION: SliderSpec = centered(-100.0, 100.0, 1.0);
pub const VIGNETTE: SliderSpec = centered(-100.0, 100.0, 1.0);
/// Where the vignette starts, from the center out.
pub const VIGNETTE_MIDPOINT: SliderSpec =
    SliderSpec { min: 0.0, max: 100.0, default: 50.0, step: 1.0, track: Track::Linear };
pub const SHARPENING: SliderSpec =
    SliderSpec { min: 0.0, max: 150.0, default: 0.0, step: 1.0, track: Track::Linear };
/// Sharpening's Radius, in the picture's pixels.
pub const SHARPEN_RADIUS: SliderSpec =
    SliderSpec { min: 0.5, max: 3.0, default: 1.0, step: 0.1, track: Track::Linear };
/// Sharpening's Detail: higher keeps more of large edges' sharpening.
pub const SHARPEN_DETAIL: SliderSpec =
    SliderSpec { min: 0.0, max: 100.0, default: 25.0, step: 1.0, track: Track::Linear };
/// Sharpening's Masking: higher sharpens only stronger edges.
pub const SHARPEN_MASKING: SliderSpec =
    SliderSpec { min: 0.0, max: 100.0, default: 0.0, step: 1.0, track: Track::Linear };

impl SliderSpec {
    /// `v` within the range; the default if it isn't a finite number.
    pub fn clamp(&self, v: f64) -> f64 {
        if v.is_finite() { v.clamp(self.min, self.max) } else { self.default }
    }

    /// `v` on the nearest step, within the range.
    pub fn snap(&self, v: f64) -> f64 {
        let v = self.clamp(v);
        self.clamp(self.min + ((v - self.min) / self.step).round() * self.step)
    }

    /// Where `v` sits on the track, from 0 at the left end to 1 at the right.
    pub fn position(&self, v: f64) -> f64 {
        let v = self.clamp(v);
        let p = match self.kelvin() {
            Some(right) if v > KELVIN_CENTER => {
                0.5 + 0.5 * (1.0 + (v - KELVIN_CENTER) / KELVIN_KNEE).ln() / right
            }
            Some(_) => 0.5 * (v - self.min) / (KELVIN_CENTER - self.min),
            None => (v - self.min) / (self.max - self.min).max(1e-9),
        };
        if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 }
    }

    /// The value at track position `p`, the inverse of [`Self::position`],
    /// not yet on a step.
    pub fn value_at(&self, p: f64) -> f64 {
        let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
        let v = match self.kelvin() {
            Some(_) if p >= 1.0 => self.max,
            Some(right) if p > 0.5 => {
                KELVIN_CENTER + KELVIN_KNEE * (((2.0 * p - 1.0) * right).exp() - 1.0)
            }
            Some(_) => self.min + 2.0 * p * (KELVIN_CENTER - self.min),
            None => self.min + p * (self.max - self.min),
        };
        self.clamp(v)
    }

    /// For a Kelvin track whose range holds the middle: `ln` of its right
    /// half's span. Any other is linear.
    fn kelvin(&self) -> Option<f64> {
        let holds = self.min < KELVIN_CENTER && KELVIN_CENTER < self.max;
        (self.track == Track::Kelvin && holds)
            .then(|| (1.0 + (self.max - KELVIN_CENTER) / KELVIN_KNEE).ln())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [SliderSpec; 20] = [
        EXPOSURE,
        TEMPERATURE_KELVIN,
        TINT_RAW,
        TEMPERATURE_RELATIVE,
        TINT_RELATIVE,
        CONTRAST,
        HIGHLIGHTS,
        SHADOWS,
        WHITES,
        BLACKS,
        CLARITY,
        TEXTURE,
        VIBRANCE,
        SATURATION,
        VIGNETTE,
        VIGNETTE_MIDPOINT,
        SHARPENING,
        SHARPEN_RADIUS,
        SHARPEN_DETAIL,
        SHARPEN_MASKING,
    ];

    #[test]
    fn every_default_is_in_range_and_on_a_step() {
        for spec in ALL {
            assert!(spec.min < spec.max && spec.step > 0.0, "{spec:?}");
            assert_eq!(spec.snap(spec.default), spec.default, "{spec:?}");
        }
    }

    #[test]
    fn a_value_that_is_not_a_number_reads_as_the_default() {
        for spec in ALL {
            for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(spec.clamp(v), spec.default, "{spec:?} {v}");
            }
            assert_eq!(spec.clamp(1e12), spec.max);
            assert_eq!(spec.clamp(-1e12), spec.min);
        }
    }

    #[test]
    fn the_kelvin_track_is_even_below_the_middle_and_logarithmic_above() {
        let t = TEMPERATURE_KELVIN;
        let cases =
            [(2000.0, 0.0), (4500.0, 0.25), (7000.0, 0.5), (15_000.0, 0.79), (50_000.0, 1.0)];
        for (k, p) in cases {
            assert!((t.position(k) - p).abs() < 0.005, "{k} K at {}", t.position(k));
        }
        // Every step survives value → position → value, and the track
        // never runs backwards.
        let mut last = -1.0;
        for k in (2000..=50_000).step_by(50).map(f64::from) {
            let p = t.position(k);
            assert!(p > last, "{k} K");
            last = p;
            assert_eq!(t.snap(t.value_at(p)), k, "{k} K at {p}");
        }
        assert_eq!(
            (t.value_at(-1.0), t.value_at(2.0), t.value_at(f64::NAN)),
            (2000.0, 50_000.0, 2000.0)
        );
        assert_eq!(t.position(f64::NAN), t.position(t.default));
        // A range that doesn't hold the middle falls back to linear.
        let narrow = SliderSpec { min: 7000.0, max: 9000.0, ..t };
        assert_eq!((narrow.position(8000.0), narrow.value_at(0.5)), (0.5, 8000.0));
    }

    #[test]
    fn a_linear_track_is_even() {
        for (v, p) in [(-5.0, 0.0), (-2.5, 0.25), (0.0, 0.5), (1.0, 0.6), (5.0, 1.0)] {
            assert!((EXPOSURE.position(v) - p).abs() < 1e-12, "{v}");
            assert!((EXPOSURE.value_at(p) - v).abs() < 1e-12, "{v}");
        }
        assert_eq!(TEMPERATURE_RELATIVE.position(-50.0), 0.25);
    }
}
