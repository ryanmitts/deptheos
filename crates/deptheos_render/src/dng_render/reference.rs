//! The standard stage's per-pixel math once more, in f64, taking the
//! resolved constants: what `standard.wgsl` computes, for the tests only.
//! The linear stages' reference is the DNG SDK (ADR-130).

use super::standard::{CODES, StandardConstants};

/// One stored pixel through `constants`, as unorm values of the output.
pub(super) fn standard(constants: &StandardConstants, stored: [u16; 3]) -> [f64; 3] {
    let u = &constants.uniform;
    let linear: [f64; 3] =
        std::array::from_fn(|c| f64::from(constants.decode[c * CODES + usize::from(stored[c])]));
    let to = [u.to_prophoto_c0, u.to_prophoto_c1, u.to_prophoto_c2];
    let from = [u.from_prophoto_c0, u.from_prophoto_c1, u.from_prophoto_c2];
    let times = |columns: [glam::Vec4; 3], v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|row| (0..3).map(|c| f64::from(columns[c][row]) * v[c]).sum())
    };
    let out = times(from, times(to, linear)).map(|v| v.clamp(0.0, 1.0));
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
                let color = Color::Standard(icc.as_ref());
                let constants = resolve_adjusted(color, &Adjustments::NEUTRAL, output);
                let Kind::Standard(standard) = &constants.0 else { unreachable!() };
                let gpu = drawn(&ctx, &constants, &pixels);
                let worst = pixels
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .zip(gpu.as_chunks::<3>().0)
                    .flat_map(|(&stored, gpu)| {
                        let reference = super::standard(standard, stored);
                        (0..3).map(move |c| (gpu[c] - reference[c]).abs() * 65535.0)
                    })
                    .fold(0.0, f64::max);
                let name = icc.as_ref().and_then(IccProfile::name);
                assert!(worst <= bound, "{name:?} into {output:?}: {worst:.2} codes off");
            }
        }
    }
}
