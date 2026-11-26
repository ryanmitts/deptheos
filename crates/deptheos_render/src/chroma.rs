//! Vibrance and Saturation: lightcraft's chroma scaling in OkLCh, as it
//! stands (MIT OR Apache-2.0: craft-libs `crates/develop/src/colorops.rs`),
//! on linear ProPhoto after the curve. Vibrance raises low chroma most,
//! and less near skin's hue when raised; Saturation scales every chroma.
//!
//! OkLab is Ottosson's ("A perceptual color space for image processing",
//! 2020), taken from linear ProPhoto through XYZ with its white adapted
//! to D65 (Bradford), as craft-libs `crates/color-transform/src/oklab.rs`
//! builds it for any space, and white set exactly on its neutral axis.

use crate::white::{self, Mat3};

/// XYZ (D65) to approximate cone responses (Ottosson's M1).
const XYZ_TO_LMS: Mat3 = [
    [0.8189330101, 0.3618667424, -0.1288597137],
    [0.0329845436, 0.9293118715, 0.0361456387],
    [0.0482003018, 0.2643662691, 0.6338517070],
];
/// Cube-rooted cone responses to Lab (Ottosson's M2).
pub(crate) const LMS_TO_LAB: Mat3 = [
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
];
/// Linear sRGB to cone responses, published with M1 and M2.
const SRGB_TO_LMS: Mat3 = [
    [0.4122214708, 0.5363325363, 0.0514459929],
    [0.2119034982, 0.6806995451, 0.1073969566],
    [0.0883024619, 0.2817188376, 0.6299787005],
];
const D65: [f64; 2] = [0.3127, 0.3290];

/// Chroma below which Vibrance acts, falling to none at it.
pub(crate) const VIBRANCE_REACH: f64 = 0.22;
/// How much less a raised Vibrance acts at skin's hue, and over what hue
/// width (radians).
pub(crate) const SKIN_PROTECTION: f64 = 0.6;
pub(crate) const SKIN_WIDTH: f64 = 0.35;
/// Vibrance's gain at its end and the lowest chroma.
pub(crate) const VIBRANCE_GAIN: f64 = 1.2;

/// Vibrance and Saturation, each -1 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Chroma {
    pub(crate) vibrance: f32,
    pub(crate) saturation: f32,
}

impl Chroma {
    /// From the sliders, -100 to 100; `None` with both at 0.
    pub(crate) fn new(vibrance: f64, saturation: f64) -> Option<Self> {
        let (vibrance, saturation) = ((vibrance / 100.0) as f32, (saturation / 100.0) as f32);
        (vibrance != 0.0 || saturation != 0.0).then_some(Self { vibrance, saturation })
    }

    /// The stage on a linear ProPhoto pixel, in f64.
    #[cfg(test)]
    pub(crate) fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let (to_lms, from_lms) = lms_matrices();
        let lms = white::apply(&to_lms, p).map(f64::cbrt);
        let [l, a, b] = white::apply(&LMS_TO_LAB, lms);
        let k = self.scale(a.hypot(b), b.atan2(a));
        let lab = [l, a * k, b * k];
        let lms = white::apply(&white::invert(&LMS_TO_LAB).unwrap(), lab).map(|v| v * v * v);
        white::apply(&from_lms, lms)
    }

    /// The factor on chroma `c` at hue `h`.
    #[cfg(test)]
    fn scale(&self, c: f64, h: f64) -> f64 {
        if c <= 0.0 {
            return 1.0;
        }
        let (vibrance, saturation) = (f64::from(self.vibrance), f64::from(self.saturation));
        let mut scaled = c;
        if vibrance != 0.0 {
            let low = 1.0 - (c / VIBRANCE_REACH).clamp(0.0, 1.0);
            let skin = match vibrance > 0.0 {
                true => {
                    let off = wrap(h - skin_hue()) / SKIN_WIDTH;
                    1.0 - SKIN_PROTECTION * (-off * off).exp()
                }
                false => 1.0,
            };
            scaled *= (1.0 + vibrance * low * low * skin * VIBRANCE_GAIN).max(0.0);
        }
        scaled *= (1.0 + saturation).max(0.0);
        scaled / c
    }
}

/// An angle within -π to π.
#[cfg(test)]
fn wrap(a: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

/// Linear ProPhoto to cone responses, and back.
pub(crate) fn lms_matrices() -> (Mat3, Mat3) {
    let (to_xyz, _) = white::prophoto();
    let d50 = white::xy_to_xyz(white::PROPHOTO[3]);
    let adapt = white::bradford_checked(d50, white::xy_to_xyz(D65)).expect("D50 and D65 adapt");
    let to_lms = white::mul(&XYZ_TO_LMS, &white::mul(&adapt, &to_xyz));
    // White lands on equal cone responses, OkLab's neutral axis, which
    // M1 at D65's chromaticity misses by 1e-4: so gray keeps no chroma
    // for Vibrance to raise.
    let white = white::apply(&to_lms, [1.0; 3]);
    let to_lms: Mat3 = std::array::from_fn(|row| to_lms[row].map(|v| v / white[row]));
    (to_lms, white::invert(&to_lms).expect("the cone matrix inverts"))
}

/// The OkLCh hue (radians) of skin: sRGB's hue 25° at full saturation.
pub(crate) fn skin_hue() -> f64 {
    // HSV 25°, full: red 1, green 25/60, blue 0, then sRGB's curve undone.
    let decode = |e: f64| if e <= 0.04045 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) };
    let linear = [1.0, 25.0 / 60.0, 0.0].map(decode);
    let lms = white::apply(&SRGB_TO_LMS, linear).map(f64::cbrt);
    let [_, a, b] = white::apply(&LMS_TO_LAB, lms);
    b.atan2(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lch(p: [f64; 3]) -> [f64; 3] {
        let (to_lms, _) = lms_matrices();
        let [l, a, b] = white::apply(&LMS_TO_LAB, white::apply(&to_lms, p).map(f64::cbrt));
        [l, a.hypot(b), b.atan2(a)]
    }

    /// ProPhoto's white is OkLab's neutral, and gray keeps no chroma.
    #[test]
    fn gray_has_no_chroma_and_stays_gray() {
        for v in [0.02, 0.18, 1.0] {
            let [_, c, _] = lch([v; 3]);
            // Within M2's b row, which sums to 4e-8.
            assert!(c < 1e-7, "{v}: {c}");
            let out = Chroma::new(80.0, 50.0).unwrap().apply([v; 3]);
            assert!(out.iter().all(|o| (o - v).abs() < 1e-6), "{v}: {out:?}");
        }
    }

    #[test]
    fn saturation_scales_chroma_and_keeps_lightness_and_hue() {
        let p = [0.4, 0.25, 0.1];
        let [l, c, h] = lch(p);
        let more = lch(Chroma::new(0.0, 50.0).unwrap().apply(p));
        assert!((more[1] - 1.5 * c).abs() < 1e-9 && (more[0] - l).abs() < 1e-9, "{more:?}");
        assert!((more[2] - h).abs() < 1e-9);
        let gray = lch(Chroma::new(0.0, -100.0).unwrap().apply(p));
        assert!(gray[1] < 1e-9, "{gray:?}");
    }

    /// Vibrance raises a muted color more than a vivid one, and a muted
    /// color at skin's hue less than one away from it.
    #[test]
    fn vibrance_favors_muted_colors_and_spares_skin() {
        let vibrance = Chroma::new(60.0, 0.0).unwrap();
        let gain = |p| lch(vibrance.apply(p))[1] / lch(p)[1];
        let (muted, vivid) = ([0.20, 0.18, 0.22], [0.6, 0.05, 0.4]);
        assert!(gain(muted) > gain(vivid), "{} {}", gain(muted), gain(vivid));
        let skin = lch([0.42, 0.30, 0.25]);
        assert!(wrap(skin[2] - skin_hue()).abs() < 0.3, "a skin tone's hue {}", skin[2]);
        let away = [0.25, 0.30, 0.42];
        assert!((lch(away)[1] - skin[1]).abs() < 0.03);
        assert!(gain([0.42, 0.30, 0.25]) < gain(away));
    }
}
