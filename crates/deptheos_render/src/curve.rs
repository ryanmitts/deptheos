//! Contrast, Whites and Blacks: one curve on linear ProPhoto, after a
//! camera RAW stack's profile tone curve and after a standard stack's
//! exposure. Each is lightcraft's operator as it stands (MIT OR
//! Apache-2.0):
//!
//! - a tone map on luminance in a gamma-2.2 domain, identity at neutral,
//!   the pixel scaled by its ratio and pulled toward its luminance where a
//!   channel passes white (craft-libs `crates/develop/src/tone.rs`,
//!   `ToneMap::display`): a standard stack's three sliders, a camera RAW
//!   stack's Whites;
//! - on a camera RAW stack, then, Contrast and Blacks as fitted operators
//!   on sRGB-encoded ProPhoto, per channel or mixed with luminance
//!   (craft-libs `crates/develop/src/light_v3.rs`), from the fitted
//!   constants in [`crate::curve_fits`].
//!
//! The tables are made here, once per draw, and read by `adjust.wgsl`'s
//! `curve`; [`Curve::apply`] is the same stage in f64 for the tests.

use crate::curve_fits as fits;

/// The tone map's middle gray.
pub(crate) const GREY: f32 = 0.18;
/// The tone map's table spans these stops around gray in [`TONE_N`]
/// entries.
pub(crate) const TONE_MIN_EV: f32 = -14.0;
pub(crate) const TONE_MAX_EV: f32 = 10.0;
pub(crate) const TONE_N: usize = 4096;
/// Entries of a fitted operator's table over encoded values 0 to 1.
pub(crate) const OP_N: usize = 1024;
/// Linear ProPhoto's luminance (the Y row of ProPhoto to XYZ).
pub(crate) const PRO_Y: [f32; 3] = [0.288_040_2, 0.711_874_1, 0.000_085_7];

/// The curve of one draw: the tone map, then each fitted operator.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Curve {
    pub(crate) tone: Option<Vec<f32>>,
    /// Contrast's, then Blacks'.
    pub(crate) ops: [Option<Operator>; 2],
}

/// A fitted operator at one amount: `lut[i]` is the linear value of the
/// curve at encoded `i / (OP_N - 1)`; `weight` is the per-channel result's
/// share against the luminance result's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Operator {
    pub(crate) weight: f32,
    pub(crate) lut: Vec<f32>,
}

/// One fit, as lightcraft lays it out: 11 values of the positive shape at
/// the knots but the last, 11 of the negative, `ln τ` and `ln q` of each
/// sign's amount law, then each sign's mix logit when mixed.
struct Fit {
    params: &'static [f64],
    /// Anchored at black too: Contrast moves tones both ways.
    signed: bool,
    mixed: bool,
}

const CONTRAST: Fit = Fit { params: &fits::CONTRAST, signed: true, mixed: false };
const BLACKS: Fit = Fit { params: &fits::BLACKS, signed: false, mixed: true };

impl Curve {
    /// A camera RAW stack's: Whites as the tone map, then the fitted
    /// Contrast and Blacks. `None` with all three at 0.
    pub(crate) fn raw(contrast: f64, whites: f64, blacks: f64) -> Option<Self> {
        let tone = (whites != 0.0).then(|| tone_display(0.0, whites, 0.0));
        let ops = [Operator::new(&CONTRAST, contrast), Operator::new(&BLACKS, blacks)];
        (tone.is_some() || ops.iter().any(Option::is_some)).then_some(Self { tone, ops })
    }

    /// A standard stack's: all three as the tone map. `None` with all
    /// three at 0.
    pub(crate) fn standard(contrast: f64, whites: f64, blacks: f64) -> Option<Self> {
        let neutral = contrast == 0.0 && whites == 0.0 && blacks == 0.0;
        let tone = (!neutral).then(|| tone_display(contrast, whites, blacks));
        tone.map(|tone| Self { tone: Some(tone), ops: [None, None] })
    }

    /// The curve on a linear ProPhoto pixel, in f64 over the same tables.
    #[cfg(test)]
    pub(crate) fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let mut p = match &self.tone {
            Some(tone) => tone_pixel(tone, p),
            None => p,
        };
        for op in self.ops.iter().flatten() {
            p = op.apply(p);
        }
        p
    }
}

/// The widest shoulder into white: from 0.95 it reaches white at 1.05.
const SHOULDER: f32 = 0.05;

/// lightcraft's display tone map at peak 1: a table over [`TONE_N`] steps
/// of scene stops around gray, of display-linear luminance.
///
/// Its short shoulder into white is as wide as Whites lifts white, up to
/// lightcraft's: Contrast and Blacks keep white at 1, and a small lift
/// gets a small shoulder, so the first step from neutral moves white by
/// no more than the lift.
fn tone_display(contrast: f64, whites: f64, blacks: f64) -> Vec<f32> {
    let (c, w, b) = ((contrast / 100.0) as f32, (whites / 100.0) as f32, (blacks / 100.0) as f32);
    let m = GREY.powf(1.0 / 2.2);
    // Where Whites takes white, the top of what reaches the shoulder.
    let lifted = (1.0 + w.max(0.0) * 0.12 * 0.5).powf(2.2) - 1.0;
    let half = lifted.min(SHOULDER);
    (0..TONE_N)
        .map(|i| {
            let ev = TONE_MIN_EV + (TONE_MAX_EV - TONE_MIN_EV) * i as f32 / (TONE_N - 1) as f32;
            let y = GREY * ev.exp2();
            let mut p = y.powf(1.0 / 2.2);
            if p <= 1.0 {
                // An S-curve anchored at 0, gray and 1.
                p += c * 0.35 * (p - m) * (1.0 - (2.0 * p - 1.0).powi(2));
                let up = smooth(0.45, 1.0, p);
                p += w * 0.12 * up * (1.0 - p * 0.5);
                let lo = 1.0 - smooth(0.0, 0.45, p);
                p += b * 0.07 * lo * (p * 2.0).min(1.0);
            } else {
                p += w * 0.12 * 0.5;
            }
            let mut o = p.max(0.0).powf(2.2);
            // The shoulder: slope 1 at 1 - half, reaching 1 at 1 + half.
            if half > 0.0 && o > 1.0 - half {
                let d = (o - (1.0 - half)).min(2.0 * half);
                o = 1.0 - half + d - d * d / (4.0 * half);
            }
            o.clamp(0.0, 1.0)
        })
        .collect()
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The tone map's table at scene luminance `y`: linear below its range.
#[cfg(test)]
fn tone_at(lut: &[f32], y: f64) -> f64 {
    if y <= 0.0 {
        return 0.0;
    }
    let (min, max, grey) = (f64::from(TONE_MIN_EV), f64::from(TONE_MAX_EV), f64::from(GREY));
    let ev = (y / grey).log2();
    let f = ((ev - min) / (max - min)).clamp(0.0, 1.0) * (TONE_N - 1) as f64;
    let i = (f as usize).min(TONE_N - 2);
    let t = f - i as f64;
    let v = f64::from(lut[i]) + (f64::from(lut[i + 1]) - f64::from(lut[i])) * t;
    if ev < min { v * (y / (grey * min.exp2())) } else { v }
}

/// The tone map on a pixel: scaled by the map's ratio at its luminance,
/// then pulled toward that luminance as far as a channel passes white.
#[cfg(test)]
fn tone_pixel(lut: &[f32], p: [f64; 3]) -> [f64; 3] {
    let y = luminance(p);
    let o = tone_at(lut, y);
    let d = if y > 1e-9 { p.map(|v| v * o / y) } else { [0.0; 3] };
    let top = d[0].max(d[1]).max(d[2]);
    if top <= 1.0 {
        return d;
    }
    let t = ((top - 1.0) / (top - o).max(1e-6)).clamp(0.0, 1.0);
    d.map(|v| v + (o - v) * t)
}

#[cfg(test)]
fn luminance(p: [f64; 3]) -> f64 {
    p.iter().zip(PRO_Y).map(|(v, w)| v * f64::from(w)).sum()
}

/// The amount law: 1 at an amount of 100, linear as `τ` grows,
/// saturating as it shrinks.
fn law((tau, q): (f64, f64), amount: f64) -> f64 {
    let t = (amount.abs() / 100.0).min(1.0);
    let tau = tau.clamp(1e-3, 1e3);
    ((1.0 - (-t / tau).exp()) / (1.0 - (-1.0 / tau).exp())).powf(q)
}

/// Piecewise linear on the knots, constant beyond the ends.
fn interp(v: f64, d: &[f64; 12]) -> f64 {
    let k = &fits::KNOTS;
    if v <= k[0] {
        return d[0];
    }
    for i in 1..k.len() {
        if v <= k[i] {
            let t = (v - k[i - 1]) / (k[i] - k[i - 1]);
            return d[i - 1] + t * (d[i] - d[i - 1]);
        }
    }
    d[k.len() - 1]
}

/// A fit unpacked: each sign's shape and amount law, and its mix weight.
struct Unpacked {
    shapes: [[f64; 12]; 2],
    laws: [(f64, f64); 2],
    weights: [f64; 2],
}

impl Fit {
    fn unpack(&self) -> Unpacked {
        let p = self.params;
        let shape = |offset: usize| {
            let mut d = [0.0; 12];
            d[..11].copy_from_slice(&p[offset..offset + 11]);
            if self.signed {
                d[0] = 0.0;
            }
            d
        };
        let sigmoid = |x: f64| 1.0 / (1.0 + (-x).exp());
        let weights = match self.mixed {
            true => [sigmoid(p[26]), sigmoid(p[27])],
            false => [1.0, 1.0],
        };
        Unpacked {
            shapes: [shape(0), shape(11)],
            laws: [(p[22].exp(), p[23].exp()), (p[24].exp(), p[25].exp())],
            weights,
        }
    }
}

impl Unpacked {
    /// The curve at `amount` on encoded `v`.
    fn curve(&self, amount: f64, v: f64) -> f64 {
        match amount {
            0.0 => v,
            a if a > 0.0 => v + law(self.laws[0], a) * interp(v, &self.shapes[0]),
            a => (v - law(self.laws[1], a) * interp(v, &self.shapes[1])).max(0.0),
        }
    }
}

impl Operator {
    /// `fit` at `amount` (-100 to 100), `None` at 0. The table is the
    /// curve's running maximum, so no two tones swap.
    fn new(fit: &Fit, amount: f64) -> Option<Self> {
        let a = if amount.is_finite() { amount.clamp(-100.0, 100.0) } else { 0.0 };
        if a == 0.0 {
            return None;
        }
        let u = fit.unpack();
        let weight = (if a > 0.0 { u.weights[0] } else { u.weights[1] }) as f32;
        let mut top = 0.0f64;
        let lut = (0..OP_N)
            .map(|i| {
                top = top.max(u.curve(a, i as f64 / (OP_N - 1) as f64));
                srgb_decode(top) as f32
            })
            .collect();
        Some(Self { weight, lut })
    }

    #[cfg(test)]
    fn eval(&self, v: f64) -> f64 {
        let f = v.clamp(0.0, 1.0) * (OP_N - 1) as f64;
        let i = (f as usize).min(OP_N - 2);
        let t = f - i as f64;
        f64::from(self.lut[i]) + (f64::from(self.lut[i + 1]) - f64::from(self.lut[i])) * t
    }

    /// One linear value: above 1 it passes, below 0 it keeps its distance
    /// below the image of 0.
    #[cfg(test)]
    fn channel(&self, x: f64) -> f64 {
        match x {
            x if x > 1.0 => x,
            x if x < 0.0 => f64::from(self.lut[0]) + x,
            x => self.eval(srgb_encode(x)),
        }
    }

    #[cfg(test)]
    fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let per_channel = p.map(|c| self.channel(c));
        let w = f64::from(self.weight);
        if w == 1.0 {
            return per_channel;
        }
        let y = luminance(p);
        let scaled = match y.is_nan() || y <= 1e-6 || y > 1.0 {
            true => p,
            false => p.map(|c| c * self.channel(y) / y),
        };
        std::array::from_fn(|k| w * per_channel[k] + (1.0 - w) * scaled[k])
    }
}

fn srgb_decode(e: f64) -> f64 {
    if e <= 0.04045 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) }
}

#[cfg(test)]
fn srgb_encode(v: f64) -> f64 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The port draws lightcraft's own curve values.
    #[test]
    fn the_fits_draw_lightcrafts_curves() {
        for (fit, references) in
            [(&CONTRAST, &fits::CONTRAST_REFERENCE), (&BLACKS, &fits::BLACKS_REFERENCE)]
        {
            let u = fit.unpack();
            for (amount, curve) in references {
                for (v, want) in fits::REF_V.iter().zip(curve) {
                    let got = u.curve(*amount, *v);
                    assert!((got - want).abs() < 1e-9, "{amount} at {v}: {got} for {want}");
                }
            }
        }
    }

    #[test]
    fn neutral_is_no_curve_and_white_stays_white() {
        assert_eq!(Curve::raw(0.0, 0.0, 0.0), None);
        assert_eq!(Curve::standard(0.0, 0.0, 0.0), None);
        for curve in [Curve::raw(60.0, 0.0, -40.0), Curve::standard(60.0, 0.0, -40.0)] {
            // Within the tone map's table at white: 0.006 stops a step.
            let white = curve.unwrap().apply([1.0; 3]);
            assert!(white.iter().all(|v| (v - 1.0).abs() < 5e-4), "{white:?}");
        }
    }

    /// Contrast steepens around gray; Whites lifts the upper tones and
    /// Blacks lowers the lower ones; every curve keeps tones in order.
    #[test]
    fn each_slider_moves_its_tones_and_keeps_their_order() {
        let gray = |curve: Option<Curve>, v: f64| curve.unwrap().apply([v; 3])[1];
        for make in [Curve::raw, Curve::standard] {
            assert!(gray(make(60.0, 0.0, 0.0), 0.05) < 0.05, "contrast darkens shadows");
            assert!(gray(make(60.0, 0.0, 0.0), 0.6) > 0.6, "contrast lightens highlights");
            assert!(gray(make(0.0, 60.0, 0.0), 0.6) > 0.6, "whites lift");
            assert!(gray(make(0.0, 0.0, -60.0), 0.02) < 0.02, "blacks lower");
            assert!(gray(make(0.0, 0.0, 60.0), 0.02) > 0.02, "blacks lift");
            for sliders in [(100.0, 100.0, 100.0), (-100.0, -100.0, -100.0), (50.0, -50.0, 80.0)] {
                let curve = make(sliders.0, sliders.1, sliders.2).unwrap();
                let ramp: Vec<f64> =
                    (0..=1000).map(|i| curve.apply([f64::from(i) / 1000.0; 3])[1]).collect();
                // Within the f32 tables' rounding.
                assert!(ramp.windows(2).all(|w| w[1] >= w[0] - 1e-6), "{sliders:?}");
            }
        }
    }
}
