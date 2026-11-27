//! The per-pixel math once more, in f64, taking the resolved constants:
//! what `standard.wgsl` and `dng_render.wgsl` compute, for the tests only.
//! The SDK is the reference for the linear stages it has (ADR-130); this
//! one follows them for a profile without HSV tables, so it checks the
//! adjustment stages between them.

use glam::Vec4;

use super::params::DngColorParams;
use super::standard::{CODES, StandardConstants};
use crate::chroma::Chroma;
use crate::curve::Curve;

/// `columns` (a WGSL `mat3x3f`'s) times `v`.
fn times(columns: [Vec4; 3], v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|row| (0..3).map(|c| f64::from(columns[c][row]) * v[c]).sum())
}

/// One stored pixel, at `at` of a `frame`-sized picture, through
/// `constants`, as unorm values of the output.
pub(super) fn standard(
    constants: &StandardConstants,
    stored: [u16; 3],
    at: [u32; 2],
    frame: [u32; 2],
) -> [f64; 3] {
    let u = &constants.uniform;
    let linear: [f64; 3] =
        std::array::from_fn(|c| f64::from(constants.decode[c * CODES + usize::from(stored[c])]));
    let to = [u.to_prophoto_c0, u.to_prophoto_c1, u.to_prophoto_c2];
    let from = [u.from_prophoto_c0, u.from_prophoto_c1, u.from_prophoto_c2];
    let mut prophoto = times(to, linear);
    if u.flags.z != 0 {
        prophoto = exposure(prophoto, u.exposure_gain_knee);
    }
    if let Some(curve) = &constants.curve {
        prophoto = curve.apply(prophoto);
    }
    prophoto = chroma(u.flags.w, u.late, prophoto);
    prophoto = vignette(u.flags.w, [u.vignette, u.vignette_window], at, frame, prophoto);
    let out = times(from, prophoto).map(|v| v.clamp(0.0, 1.0));
    if u.flags.y != 0 {
        // The nearest code through the profile's own curves, which never
        // decrease.
        std::array::from_fn(|c| {
            let table = &constants.decode[c * CODES..(c + 1) * CODES];
            let above = table.partition_point(|&v| f64::from(v) < out[c]).min(CODES - 1);
            let gap = |i: usize| (f64::from(table[i]) - out[c]).abs();
            let code = if above > 0 && gap(above - 1) <= gap(above) { above - 1 } else { above };
            code as f64 / 65535.0
        })
    } else {
        out.map(srgb_encode)
    }
}

/// One camera pixel (unorm), at `at` of a `frame`-sized picture, of a
/// linear stack with no HSV tables through its uniform and composed tone
/// curve, as sRGB unorm values.
pub(super) fn linear(
    u: &DngColorParams,
    tone: &[f32],
    curve: Option<&Curve>,
    camera: [f64; 3],
    at: [u32; 2],
    frame: [u32; 2],
) -> [f64; 3] {
    assert_eq!(u.flags.x + u.flags.y, 0, "the reference has no HSV tables");
    let black = f64::from(u.range_inv.w);
    let normalized: [f64; 3] = std::array::from_fn(|c| {
        ((camera[c] - black) * f64::from(u.range_inv[c])).min(f64::from(u.camera_white[c]))
    });
    let matrix = [u.cam_to_prophoto_c0, u.cam_to_prophoto_c1, u.cam_to_prophoto_c2];
    let prophoto = times(matrix, normalized).map(|v| v.clamp(0.0, 1.0));
    let [black, slope, radius, q_scale] = u.exposure.to_array().map(f64::from);
    let adjusted = u.flags.z != 0;
    let ramp = |x: f64| {
        if x <= black - radius {
            0.0
        } else if x >= black + radius {
            let y = (x - black) * slope;
            if adjusted { y.max(0.0) } else { y.clamp(0.0, 1.0) }
        } else {
            q_scale * (x - (black - radius)).powi(2)
        }
    };
    let mut prophoto = prophoto.map(ramp);
    if adjusted {
        prophoto = exposure(prophoto, u.exposure_gain_knee);
    }
    let mut toned = hue_preserving(prophoto, |v| tone_sample(tone, v));
    if let Some(curve) = curve {
        toned = curve.apply(toned);
    }
    toned = chroma(u.flags.w, u.late, toned);
    toned = vignette(u.flags.w, [u.vignette, u.vignette_window], at, frame, toned);
    let pro_to_output = [u.pro_to_output_c0, u.pro_to_output_c1, u.pro_to_output_c2];
    let power = f64::from(u.pro_to_output_c0.w);
    times(pro_to_output, toned).map(|v| match power {
        0.0 => srgb_encode(v.clamp(0.0, 1.0)),
        _ => v.clamp(0.0, 1.0).powf(power),
    })
}

/// Vibrance and Saturation when the late stages (`late`) hold them.
fn chroma(stages: u32, late: Vec4, prophoto: [f64; 3]) -> [f64; 3] {
    match stages & 8 {
        0 => prophoto,
        _ => Chroma { vibrance: late.z, saturation: late.w }.apply(prophoto),
    }
}

/// The vignette when the late stages hold it, through its port in f32
/// (`crate::vignette`): its numbers and its window (`[vignette,
/// vignette_window]`), at pixel `at` of a `frame`-sized picture.
fn vignette(
    stages: u32,
    [v, window]: [Vec4; 2],
    at: [u32; 2],
    frame: [u32; 2],
    prophoto: [f64; 3],
) -> [f64; 3] {
    if stages & 16 == 0 {
        return prophoto;
    }
    let vig = crate::vignette::Vig {
        amount: v.x,
        start: v.y,
        width: v.z,
        aspect_mix: v.w,
        power: crate::vignette::POWER,
    };
    let window = match window.z {
        0.0 => [0.0, 0.0, frame[0] as f32, frame[1] as f32],
        _ => window.to_array(),
    };
    let t = vig.reach(window, (at[0] as f32, at[1] as f32));
    vig.apply(prophoto.map(|c| c as f32), t).map(f64::from)
}

/// The composed tone curve at `v`, lerped between its samples.
fn tone_sample(tone: &[f32], v: f64) -> f64 {
    let x = v.clamp(0.0, 1.0) * (tone.len() - 1) as f64;
    let i = (x.floor() as usize).min(tone.len() - 1);
    let next = (i + 1).min(tone.len() - 1);
    let f = (x - i as f64).clamp(0.0, 1.0);
    f64::from(tone[i]) * (1.0 - f) + f64::from(tone[next]) * f
}

/// `curve` on the largest and smallest channel, the middle one kept in
/// proportion between them (the SDK's hue-preserving tone curve).
fn hue_preserving(rgb: [f64; 3], curve: impl Fn(f64) -> f64) -> [f64; 3] {
    let (max, min) = (rgb[0].max(rgb[1]).max(rgb[2]), rgb[0].min(rgb[1]).min(rgb[2]));
    let (top, bottom) = (curve(max), curve(min));
    if max - min < 1e-10 {
        return [top; 3];
    }
    let scale = (top - bottom) / (max - min);
    rgb.map(|v| bottom + (v - min) * scale)
}

/// Exposure's gain, then its shoulder (`adjust.wgsl`).
fn exposure(prophoto: [f64; 3], gain_knee: Vec4) -> [f64; 3] {
    let (gain, knee) = (f64::from(gain_knee.x), f64::from(gain_knee.y));
    prophoto.map(|v| {
        let v = v * gain;
        if v <= knee {
            return v;
        }
        let gap = 1.0 - knee;
        1.0 - gap * gap / (gap + v - knee)
    })
}

/// The sRGB curve (IEC 61966-2-1).
fn srgb_encode(v: f64) -> f64 {
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use deptheos_core::io::IccProfile;
    use deptheos_core::upload::ImageUploader;
    use deptheos_gpu::GpuContext;
    use deptheos_gpu::gpu_utils::download_texture_with;

    use super::super::{Constants, DngRenderPipeline, Kind, PreparedRender, Region, Source};
    use crate::{Adjustments, Color, OutputSpace, resolve_adjusted};

    const SIZE: [u32; 2] = [256, 96];

    /// Every channel's full ramp, a soft two-color gradient, saturated
    /// patches and a hashed texture.
    fn scenes() -> Vec<u16> {
        let [w, h] = SIZE;
        let mut pixels = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let ramp = x * 257;
                let hash = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663);
                let rgb = match y / 16 {
                    0 => [ramp, 0, 0],
                    1 => [0, ramp, 0],
                    2 => [0, 0, ramp],
                    3 => [ramp, ramp, ramp],
                    4 => [ramp, 65_535 - ramp, (ramp + 20_000) % 65_536],
                    _ => [hash % 65_536, (hash >> 8) % 65_536, (hash >> 16) % 65_536],
                };
                pixels.extend(rgb.map(|v| v.min(65_535) as u16));
            }
        }
        pixels
    }

    fn drawn(ctx: &GpuContext, constants: &super::super::Constants, pixels: &[u16]) -> Vec<f64> {
        let [w, h] = SIZE;
        let mut uploader = ImageUploader::new(ctx);
        let prep = uploader.prepare_rgb16(ctx, pixels, w, h).unwrap();
        let input = ImageUploader::create_target(ctx, w, h, "reference_source");
        let mut encoder = ctx.encoder("reference_upload");
        prep.dispatch_to_rgba(ctx, &mut encoder, &input);
        ctx.submit(encoder);
        let output = DngRenderPipeline::create_output(ctx, w, h);
        let prepared = PreparedRender::new(ctx, constants);
        let whole = Region::whole(w, h);
        DngRenderPipeline::new(ctx).draw(ctx, &prepared, Source::whole(&input), whole, &output);
        let mut values = Vec::new();
        download_texture_with(ctx, &output, [0, 0], [w, h], |row| match output.format() {
            wgpu::TextureFormat::Rgba16Unorm => {
                values.extend(row.as_chunks::<8>().0.iter().flat_map(|t| {
                    [0, 2, 4].map(|i| f64::from(u16::from_le_bytes([t[i], t[i + 1]])) / 65535.0)
                }))
            }
            _ => values.extend(row.as_chunks::<16>().0.iter().flat_map(|t| {
                [0, 4, 8].map(|i| f64::from(f32::from_le_bytes(t[i..i + 4].try_into().unwrap())))
            })),
        })
        .unwrap();
        values
    }

    /// How far the shader may be from the f64 reference, in 16-bit codes:
    /// one code of rounding through sRGB's curve, and in a profile's own
    /// curves the few codes f32 moves a channel where the curve is flat
    /// near black.
    const SRGB_BOUND: f64 = 1.0;
    const OWN_BOUND: f64 = 16.0;
    /// A linear stack's: the tone curve's lerp in f32.
    const LINEAR_BOUND: f64 = 2.0;
    /// Into a pure power (Adobe RGB's, ProPhoto's), whose slope is
    /// unbounded at black: f32's rounding of the darkest values, magnified.
    const POWER_BOUND: f64 = 4.0;

    /// Through the tone map: where a channel passes white by a hair, the
    /// pull toward luminance divides by that hair, and moves the codes
    /// the stages before it rounded by up to 8 times as many.
    const TONE_MAP_BOUND: f64 = 16.0;

    /// With Vibrance or Saturation: OkLab's cube root, a `pow` in f32,
    /// and its cube on the way back. In a profile's own curves, a pure
    /// power with no linear toe (Adobe RGB's), the channels chroma takes
    /// toward 0 turn that rounding into tens of codes: under a third of an
    /// 8-bit code.
    const CHROMA_BOUND: f64 = 2.0;
    const CHROMA_OWN_BOUND: f64 = 96.0;
    /// Into ProPhoto, whose gamut keeps the colors chroma takes far out,
    /// and whose pure power (no linear toe) magnifies their rounding at
    /// black.
    const CHROMA_PROPHOTO_BOUND: f64 = 8.0;

    /// With a vignette, into a pure power (Adobe RGB's): the darkest
    /// values it takes darker sit where the power is steepest, in a
    /// standard stack's own profile and in an export.
    const VIGNETTE_OWN_BOUND: f64 = 24.0;
    const VIGNETTE_POWER_BOUND: f64 = 6.0;

    /// `bound` for `adjustments`, wider through the tone map (a standard
    /// stack's curve, a camera RAW stack's Whites), through OkLab, and
    /// `[chroma_bound, vignette_bound]` through those stages.
    fn bound(bound: f64, wider: [f64; 2], adjustments: &Adjustments, standard: bool) -> f64 {
        let [chroma_bound, vignette_bound] = wider;
        let chroma = adjustments.vibrance != 0.0 || adjustments.saturation != 0.0;
        let bound = if chroma { bound.max(chroma_bound) } else { bound };
        let bound = if adjustments.vignette.is_none() { bound } else { bound.max(vignette_bound) };
        let curve = adjustments.contrast != 0.0 || adjustments.blacks != 0.0;
        let tone_map = adjustments.whites != 0.0 || (standard && curve);
        if tone_map { bound.max(TONE_MAP_BOUND) } else { bound }
    }

    /// The adjustments each test draws: neutral, then each slider's
    /// values across its range.
    fn adjustments() -> Vec<Adjustments> {
        let neutral = Adjustments::NEUTRAL;
        let exposure = [-2.0, -0.5, 0.01, 1.0, 3.0]
            .map(|exposure| Adjustments { exposure, ..neutral.clone() });
        let white = [(60.0, 0.0), (-80.0, 30.0), (0.0, -100.0)].map(|(temperature, tint)| {
            let white_balance = deptheos_adjust::WhiteBalance::Relative { temperature, tint };
            Adjustments { white_balance, exposure: 0.5, ..neutral.clone() }
        });
        let curve = [(60.0, 0.0, 0.0), (0.0, 70.0, 0.0), (0.0, 0.0, -60.0), (-50.0, -40.0, 50.0)]
            .map(|(contrast, whites, blacks)| Adjustments {
                contrast,
                whites,
                blacks,
                ..neutral.clone()
            });
        let chroma = [(60.0, 0.0), (-80.0, 0.0), (0.0, 50.0), (0.0, -100.0), (40.0, 30.0)]
            .map(|(vibrance, saturation)| Adjustments { vibrance, saturation, ..neutral.clone() });
        let vignette = [(-100.0, 0.0), (-40.0, 50.0), (70.0, 80.0), (100.0, 100.0)].map(
            |(amount, midpoint)| Adjustments {
                vignette: crate::Vignette { amount, midpoint },
                exposure: 0.3,
                ..neutral.clone()
            },
        );
        let all = std::iter::once(neutral).chain(exposure).chain(white).chain(curve);
        all.chain(chroma).chain(vignette).collect()
    }

    /// A reframe's rectangle off the picture's center, which a vignette
    /// is centered on.
    const CROP: crate::reframe::Rect =
        crate::reframe::Rect { x: 40, y: 10, width: 150, height: 70 };

    /// `constants` as drawn, then centered on [`CROP`] when it has a
    /// vignette.
    fn framings(constants: Box<Constants>, adjustments: &Adjustments) -> Vec<Constants> {
        match adjustments.vignette.is_none() {
            true => vec![*constants],
            false => vec![(*constants).clone(), constants.framed(CROP)],
        }
    }

    /// The largest gap between the shader's output and `reference`'s, in
    /// 16-bit codes.
    /// `reference` takes each stored pixel and where it is.
    fn worst(
        pixels: &[u16],
        gpu: &[f64],
        reference: impl Fn([u16; 3], [u32; 2]) -> [f64; 3],
    ) -> f64 {
        pixels
            .as_chunks::<3>()
            .0
            .iter()
            .zip(gpu.as_chunks::<3>().0)
            .enumerate()
            .flat_map(|(i, (&stored, gpu))| {
                let at = [i as u32 % SIZE[0], i as u32 / SIZE[0]];
                let reference = reference(stored, at);
                (0..3).map(move |c| (gpu[c] - reference[c]).abs() * 65535.0)
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn the_shader_matches_the_f64_reference() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let pixels = scenes();
        let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let profiles = [
            None,
            Some(IccProfile::from_bytes(p3).unwrap()),
            Some(IccProfile::adobe_rgb_compatible()),
        ];
        for icc in &profiles {
            for (output, bound, wider) in [
                (OutputSpace::Srgb, SRGB_BOUND, [CHROMA_BOUND, SRGB_BOUND]),
                (OutputSpace::StackProfile, OWN_BOUND, [CHROMA_OWN_BOUND, VIGNETTE_OWN_BOUND]),
            ] {
                for adjustments in adjustments() {
                    let color = Color::Standard(icc.as_ref());
                    let resolved = resolve_adjusted(color, &adjustments, output);
                    for constants in framings(resolved, &adjustments) {
                        let Kind::Standard(standard) = &constants.kind else { unreachable!() };
                        let gpu = drawn(&ctx, &constants, &pixels);
                        let worst = worst(&pixels, &gpu, |stored, at| {
                            super::standard(standard, stored, at, SIZE)
                        });
                        let name = icc.as_ref().and_then(IccProfile::name);
                        assert!(
                            worst <= self::bound(bound, wider, &adjustments, true),
                            "{name:?} into {output:?} under {adjustments:?}: {worst:.2} codes off"
                        );
                    }
                }
            }
        }
    }

    /// Render parameters with no HSV tables: a clipped camera white, a
    /// black level, a curved tone curve and `baseline` stops.
    fn linear_params(baseline: f64) -> deptheos_dng::RenderParams {
        deptheos_dng::RenderParams {
            camera_white: [1.0, 0.92, 0.97],
            black_level: 512.0,
            baseline_exposure: baseline,
            shadow_scale: 1.0,
            tone_curve: (0..=256)
                .map(|i| {
                    let x = i as f32 / 256.0;
                    x * x * (3.0 - 2.0 * x) * 0.5 + x * 0.5
                })
                .collect(),
            ..deptheos_dng::RenderParams::srgb_identity()
        }
    }

    #[test]
    fn the_linear_shader_matches_the_f64_reference() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let pixels = scenes();
        let outputs = [
            (OutputSpace::Srgb, LINEAR_BOUND, [CHROMA_BOUND, LINEAR_BOUND]),
            (OutputSpace::DisplayP3, LINEAR_BOUND, [CHROMA_BOUND, LINEAR_BOUND]),
            (OutputSpace::AdobeRgb, POWER_BOUND, [CHROMA_BOUND, VIGNETTE_POWER_BOUND]),
            (OutputSpace::ProPhoto, POWER_BOUND, [CHROMA_PROPHOTO_BOUND, VIGNETTE_POWER_BOUND]),
        ];
        for ((baseline, adjustments), (output, linear_bound, wider)) in [0.35, -0.5]
            .into_iter()
            .flat_map(|b| adjustments().into_iter().map(move |a| (b, a)))
            .flat_map(|ba| outputs.map(move |o| (ba.clone(), o)))
        {
            let params = linear_params(baseline);
            let resolved = resolve_adjusted(Color::Linear(&params), &adjustments, output);
            for constants in framings(resolved, &adjustments) {
                assert_eq!(constants.output(), output);
                let Kind::Linear { uniform, tone, curve, .. } = &constants.kind else {
                    unreachable!()
                };
                let gpu = drawn(&ctx, &constants, &pixels);
                let worst = worst(&pixels, &gpu, |stored, at| {
                    let camera = stored.map(|v| f64::from(v) / 65535.0);
                    super::linear(uniform, tone, curve.as_ref(), camera, at, SIZE)
                });
                assert!(
                    worst <= bound(linear_bound, wider, &adjustments, false),
                    "baseline {baseline} into {output:?} under {adjustments:?}: {worst:.2} codes off"
                );
            }
        }
    }
}
