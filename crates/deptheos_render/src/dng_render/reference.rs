//! The per-pixel math once more, in f64, taking the resolved constants:
//! what `standard.wgsl` and `dng_render.wgsl` compute, for the tests only.
//! The SDK is the reference for the linear stages it has (ADR-130); this
//! one follows them for a profile without HSV tables, so it checks the
//! adjustment stages between them.

use glam::Vec4;

use super::params::DngColorParams;
use super::standard::{CODES, StandardConstants};

/// `columns` (a WGSL `mat3x3f`'s) times `v`.
fn times(columns: [Vec4; 3], v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|row| (0..3).map(|c| f64::from(columns[c][row]) * v[c]).sum())
}

/// One stored pixel through `constants`, as unorm values of the output.
pub(super) fn standard(constants: &StandardConstants, stored: [u16; 3]) -> [f64; 3] {
    let u = &constants.uniform;
    let linear: [f64; 3] =
        std::array::from_fn(|c| f64::from(constants.decode[c * CODES + usize::from(stored[c])]));
    let to = [u.to_prophoto_c0, u.to_prophoto_c1, u.to_prophoto_c2];
    let from = [u.from_prophoto_c0, u.from_prophoto_c1, u.from_prophoto_c2];
    let mut prophoto = times(to, linear);
    if u.flags.z != 0 {
        prophoto = exposure(prophoto, u.exposure_gain_knee);
    }
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

/// One camera pixel (unorm) of a linear stack with no HSV tables through
/// its uniform and composed tone curve, as sRGB unorm values.
pub(super) fn linear(u: &DngColorParams, tone: &[f32], camera: [f64; 3]) -> [f64; 3] {
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
    let toned = hue_preserving(prophoto, |v| tone_sample(tone, v));
    let pro_to_srgb = [u.pro_to_srgb_c0, u.pro_to_srgb_c1, u.pro_to_srgb_c2];
    times(pro_to_srgb, toned).map(|v| srgb_encode(v.clamp(0.0, 1.0)))
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

    use super::super::{DngRenderPipeline, Kind, PreparedRender, Region, Source};
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
        std::iter::once(neutral).chain(exposure).chain(white).collect()
    }

    /// The largest gap between the shader's output and `reference`'s, in
    /// 16-bit codes.
    fn worst(pixels: &[u16], gpu: &[f64], reference: impl Fn([u16; 3]) -> [f64; 3]) -> f64 {
        pixels
            .as_chunks::<3>()
            .0
            .iter()
            .zip(gpu.as_chunks::<3>().0)
            .flat_map(|(&stored, gpu)| {
                let reference = reference(stored);
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
            for (output, bound) in
                [(OutputSpace::Srgb, SRGB_BOUND), (OutputSpace::StackProfile, OWN_BOUND)]
            {
                for adjustments in adjustments() {
                    let color = Color::Standard(icc.as_ref());
                    let constants = resolve_adjusted(color, &adjustments, output);
                    let Kind::Standard(standard) = &constants.0 else { unreachable!() };
                    let gpu = drawn(&ctx, &constants, &pixels);
                    let worst = worst(&pixels, &gpu, |stored| super::standard(standard, stored));
                    let name = icc.as_ref().and_then(IccProfile::name);
                    assert!(
                        worst <= bound,
                        "{name:?} into {output:?} under {adjustments:?}: {worst:.2} codes off"
                    );
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
        for baseline in [0.35, -0.5] {
            let params = linear_params(baseline);
            for adjustments in adjustments() {
                let color = Color::Linear(&params);
                let constants = resolve_adjusted(color, &adjustments, OutputSpace::Srgb);
                let Kind::Linear { uniform, tone, .. } = &constants.0 else { unreachable!() };
                let gpu = drawn(&ctx, &constants, &pixels);
                let worst = worst(&pixels, &gpu, |stored| {
                    super::linear(uniform, tone, stored.map(|v| f64::from(v) / 65535.0))
                });
                assert!(
                    worst <= LINEAR_BOUND,
                    "baseline {baseline} under {adjustments:?}: {worst:.2} codes off"
                );
            }
        }
    }
}
