//! The adjustment stages' numbers as the shaders take them, and the WGSL
//! module of the Rust constants the shaders read, written from the Rust
//! values when a pipeline is built so the two can't drift.

use std::fmt::Write;

use glam::Vec4;

use super::standard;
use crate::chroma::{self, Chroma};
use crate::curve::{self, Curve};
use crate::local_tone;
use crate::vignette::{self, Vig};
use crate::white;

/// What the resolve step works out from the sliders for the shaders
/// (`crate::resolve`). With every stage `None` a linear stack draws
/// exactly as the SDK renders it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Stages {
    /// A standard stack's white balance, in linear ProPhoto.
    pub(crate) white_balance: Option<crate::white::Mat3>,
    pub(crate) exposure: Option<Exposure>,
    /// Highlights and Shadows.
    pub(crate) local_tone: Option<LocalTone>,
    /// Contrast, Whites and Blacks.
    pub(crate) curve: Option<Curve>,
    /// Vibrance and Saturation.
    pub(crate) chroma: Option<Chroma>,
    pub(crate) vignette: Option<Vig>,
}

impl Stages {
    /// Which stages after the tone curve run (1, 2 and 4 the curve's, 8
    /// chroma, 16 the vignette) and their numbers: the curve's operators'
    /// weights, then Vibrance and Saturation.
    pub(super) fn late(&self) -> (u32, Vec4) {
        let (_, curve, weights) = pack_curve(self.curve.as_ref());
        let flag = u32::from(self.chroma.is_some()) << 3 | u32::from(self.vignette.is_some()) << 4;
        let (vibrance, saturation) = self.chroma.map_or((0.0, 0.0), |c| (c.vibrance, c.saturation));
        (curve | flag, Vec4::new(weights.x, weights.y, vibrance, saturation))
    }

    /// The vignette's numbers: its amount, start, width and how far it
    /// takes the frame's shape (`adjust.wgsl`).
    pub(super) fn vignette(&self) -> Vec4 {
        self.vignette.map_or(Vec4::ZERO, |v| Vec4::new(v.amount, v.start, v.width, v.aspect_mix))
    }
}

/// Exposure's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Exposure {
    /// Stops the SDK's ramp and darkening curve take on a linear stack, in
    /// place of its baseline.
    pub(crate) ramp: f64,
    /// The linear gain after the ramp.
    pub(crate) gain: f64,
    /// Where the shoulder starts, below 1; 1 clamps at white.
    pub(crate) knee: f64,
}

impl Exposure {
    pub(super) fn gain_knee(self) -> Vec4 {
        Vec4::new(self.gain as f32, self.knee as f32, 0.0, 0.0)
    }
}

/// The local tone's numbers, the sliders that read neighbors: each one's
/// strength ([`crate::local_tone`]), and the stops exposure shifts the
/// base by. One at 0 takes no part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LocalTone {
    pub(crate) highlights: f32,
    pub(crate) shadows: f32,
    pub(crate) ev: f32,
    pub(crate) clarity: f32,
    pub(crate) texture: f32,
    pub(crate) sharpen: Option<local_tone::Sharpen>,
}

/// The flag bits of the local tone's terms, beside exposure's 1 in the
/// uniform's `flags.z`: Highlights and Shadows, Clarity, Texture,
/// Sharpening.
pub(super) const TONE_TERM: u32 = 2;
pub(super) const CLARITY_TERM: u32 = 4;
pub(super) const TEXTURE_TERM: u32 = 8;
pub(super) const SHARPEN_TERM: u32 = 16;

impl LocalTone {
    pub(super) fn packed(self) -> Vec4 {
        Vec4::new(self.highlights, self.shadows, self.ev, self.clarity)
    }

    /// The fine terms' numbers: Texture's strength, then Sharpening's
    /// strength, limit and Masking.
    pub(super) fn detail(self) -> Vec4 {
        let s = self.sharpen.map_or([0.0; 3], |s| [s.strength, s.limit, s.masking]);
        Vec4::new(self.texture, s[0], s[1], s[2])
    }

    /// The planes the terms read.
    pub(super) fn needs(self) -> local_tone::Needs {
        let terms = self.terms();
        local_tone::Needs {
            base: terms & TONE_TERM != 0,
            clarity: terms & CLARITY_TERM != 0,
            texture: terms & TEXTURE_TERM != 0,
            sharpen: self.sharpen.map(|s| s.sigma),
        }
    }

    /// Which terms run, as flag bits.
    pub(super) fn terms(self) -> u32 {
        let on = |v: f32, bit: u32| if v != 0.0 { bit } else { 0 };
        (on(self.highlights, TONE_TERM) | on(self.shadows, TONE_TERM))
            | on(self.clarity, CLARITY_TERM)
            | on(self.texture, TEXTURE_TERM)
            | if self.sharpen.is_some() { SHARPEN_TERM } else { 0 }
    }
}

/// The import path of [`constants_module`].
pub(super) const CONSTANTS_MODULE: &str = "deptheos::render_constants";

/// The WGSL module of the constants the shaders share with Rust.
pub(super) fn constants_module() -> String {
    let mut wgsl = format!("#define_import_path {CONSTANTS_MODULE}\n\n");
    let mut u32_const = |name: &str, value: usize| {
        let value = u32::try_from(value).expect("a u32 constant");
        writeln!(wgsl, "const {name}: u32 = {value}u;").unwrap();
    };
    u32_const("CODES", standard::CODES);
    u32_const("TONE_N", curve::TONE_N);
    u32_const("OP_N", curve::OP_N);
    u32_const("TONE_TERM", TONE_TERM as usize);
    u32_const("CLARITY_TERM", CLARITY_TERM as usize);
    u32_const("TEXTURE_TERM", TEXTURE_TERM as usize);
    u32_const("SHARPEN_TERM", SHARPEN_TERM as usize);
    for (name, value) in [
        ("STRENGTH", local_tone::STRENGTH),
        ("SHADOW_LOW", local_tone::SHADOW_EDGES[0]),
        ("SHADOW_HIGH", local_tone::SHADOW_EDGES[1]),
        ("HIGHLIGHT_LOW", local_tone::HIGHLIGHT_EDGES[0]),
        ("HIGHLIGHT_HIGH", local_tone::HIGHLIGHT_EDGES[1]),
        ("CLARITY_STRENGTH", local_tone::CLARITY_STRENGTH),
        ("CLARITY_LIMIT", local_tone::CLARITY_LIMIT),
        ("CLARITY_MIDTONES", local_tone::CLARITY_MIDTONES),
        ("CLARITY_FLOOR", local_tone::CLARITY_FLOOR),
        ("TEXTURE_STRENGTH", local_tone::TEXTURE_STRENGTH),
        ("TEXTURE_LIMIT", local_tone::TEXTURE_LIMIT),
        ("TEXTURE_TAME", local_tone::TEXTURE_TAME),
        ("TEXTURE_TAME_LOW", local_tone::TEXTURE_TAME_EDGES[0]),
        ("TEXTURE_TAME_HIGH", local_tone::TEXTURE_TAME_EDGES[1]),
        ("SHARPEN_GAIN", local_tone::SHARPEN_GAIN),
        ("SHARPEN_MASK_SCALE", local_tone::SHARPEN_MASK_SCALE),
        ("SHARPEN_MASK_WIDTH", local_tone::SHARPEN_MASK_WIDTH),
        ("VIGNETTE_POWER", vignette::POWER),
        ("VIGNETTE_LIGHTEN", vignette::LIGHTEN),
        ("GREY", curve::GREY),
        ("TONE_MIN_EV", curve::TONE_MIN_EV),
        ("TONE_MAX_EV", curve::TONE_MAX_EV),
    ] {
        writeln!(wgsl, "const {name}: f32 = {value:?};").unwrap();
    }
    let [r, g, b] = curve::PRO_Y;
    writeln!(wgsl, "const PRO_Y: vec3f = vec3f({r:?}, {g:?}, {b:?});").unwrap();
    let [r, g, b] = curve::tone_y().map(|v| v as f32);
    writeln!(wgsl, "const TONE_Y: vec3f = vec3f({r:?}, {g:?}, {b:?});").unwrap();
    let (to_lms, from_lms) = chroma::lms_matrices();
    let lab_to_lms = white::invert(&chroma::LMS_TO_LAB).expect("M2 inverts");
    for (name, m) in [
        ("PRO_TO_LMS", to_lms),
        ("LMS_TO_PRO", from_lms),
        ("LMS_TO_LAB", chroma::LMS_TO_LAB),
        ("LAB_TO_LMS", lab_to_lms),
        ("PRO_TO_2020", white::prophoto_to_rec2020()),
    ] {
        // WGSL's mat3x3f takes columns.
        let column = |c: usize| {
            format!("vec3f({:?}, {:?}, {:?})", m[0][c] as f32, m[1][c] as f32, m[2][c] as f32)
        };
        writeln!(
            wgsl,
            "const {name}: mat3x3f = mat3x3f({}, {}, {});",
            column(0),
            column(1),
            column(2)
        )
        .unwrap();
    }
    for (name, value) in [
        ("SKIN_HUE", chroma::skin_hue()),
        ("SKIN_PROTECTION", chroma::SKIN_PROTECTION),
        ("SKIN_WIDTH", chroma::SKIN_WIDTH),
        ("VIBRANCE_REACH", chroma::VIBRANCE_REACH),
        ("VIBRANCE_GAIN", chroma::VIBRANCE_GAIN),
    ] {
        writeln!(wgsl, "const {name}: f32 = {:?};", value as f32).unwrap();
    }
    wgsl
}

/// The curve's tables as one buffer (the tone map's, then each present
/// operator's), which stages it has (1 the tone map, 2 Contrast, 4
/// Blacks) and the operators' weights: `curve.wgsl`'s layout.
pub(super) fn pack_curve(curve: Option<&Curve>) -> (Vec<f32>, u32, Vec4) {
    let Some(curve) = curve else { return (Vec::new(), 0, Vec4::ZERO) };
    let mut tables = curve.tone.clone().unwrap_or_default();
    let mut stages = u32::from(curve.tone.is_some());
    let mut weights = [0.0; 4];
    for (i, op) in curve.ops.iter().enumerate() {
        if let Some(op) = op {
            tables.extend_from_slice(&op.lut);
            stages |= 2 << i;
            weights[i] = op.weight;
        }
    }
    (tables, stages, Vec4::from_array(weights))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_are_the_rust_values() {
        let wgsl = constants_module();
        assert!(wgsl.contains("const CODES: u32 = 65536u;"), "{wgsl}");
        assert!(wgsl.contains("const TONE_N: u32 = 4096u;"), "{wgsl}");
        assert!(wgsl.contains("const GREY: f32 = 0.18;"), "{wgsl}");
    }
}
