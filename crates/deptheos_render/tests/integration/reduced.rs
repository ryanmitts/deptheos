//! The reduced copy: the mean of the linear values each texel covers, and
//! at full size the same picture as the source draws, both kinds.

use deptheos_dng::RenderParams;
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;
use deptheos_render::dng_render::{Constants, DngRenderPipeline};
use deptheos_render::{Adjustments, Color, OutputSpace, resolve_adjusted};
use image::{DynamicImage, ImageBuffer, Rgb};

fn upload(ctx: &GpuContext, [w, h]: [u32; 2], pixels: Vec<u16>) -> wgpu::Texture {
    let buffer = ImageBuffer::<Rgb<u16>, _>::from_raw(w, h, pixels).unwrap();
    crate::common::upload(ctx, &DynamicImage::ImageRgb16(buffer))
}

/// A half float's value.
fn half(bits: u16) -> f32 {
    let (sign, exp, frac) = (bits >> 15, (bits >> 10) & 0x1f, f32::from(bits & 0x3ff));
    let magnitude = match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + frac / 1024.0) * 2f32.powi(i32::from(e) - 15),
    };
    if sign == 1 { -magnitude } else { magnitude }
}

fn read_half(ctx: &GpuContext, texture: &wgpu::Texture) -> Vec<f32> {
    let mut bytes = Vec::new();
    let size = [texture.width(), texture.height()];
    download_texture_with(ctx, texture, [0, 0], size, |row| bytes.extend_from_slice(row)).unwrap();
    bytes.as_chunks::<2>().0.iter().map(|b| half(u16::from_le_bytes(*b))).collect()
}

fn standard(adjustments: &Adjustments) -> Constants {
    *resolve_adjusted(Color::Standard(None), adjustments, OutputSpace::Srgb)
}

/// Each texel of a picture twice the reduced copy's long side is the mean
/// of the four it covers, decoded: black and white average to linear half,
/// not to the code halfway.
#[test]
fn a_texel_is_the_mean_of_the_linear_values_it_covers() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let [w, h] = [8192u32, 4];
    let pixels: Vec<u16> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let v = if (x + y) % 2 == 0 { 0 } else { u16::MAX };
            [v, v, v]
        })
        .collect();
    let source = upload(&ctx, [w, h], pixels);
    let constants = standard(&Adjustments::NEUTRAL);
    let reduced = DngRenderPipeline::new(&ctx).reduce(&ctx, &source, &constants);
    assert_eq!([reduced.width(), reduced.height()], [4096, 2]);
    for (i, v) in read_half(&ctx, &reduced).chunks(4).enumerate() {
        assert_eq!(&v[..3], &[0.5; 3], "texel {i}");
    }
}

/// A picture no longer than the reduced copy's long side keeps its size,
/// and drawn from it under the same sliders it is the picture the source
/// draws, within half floats' rounding: under 8 codes of 65535 on average,
/// one 8-bit step at most, where a value crosses a step of the curve's
/// table.
#[test]
fn at_full_size_the_reduced_copy_draws_as_the_source() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let [w, h] = [300u32, 200];
    let pixels: Vec<u16> =
        (0..w * h * 3).map(|i| ((i * 2_654_435_761u32) >> 16) as u16 | 0x0400).collect();
    let source = upload(&ctx, [w, h], pixels);
    let pipeline = DngRenderPipeline::new(&ctx);
    let adjustments = Adjustments { exposure: 0.6, contrast: 30.0, ..Adjustments::NEUTRAL };
    let raw = RenderParams::srgb_identity();
    let kinds = [
        ("standard", standard(&adjustments)),
        ("linear", *resolve_adjusted(Color::Linear(&raw), &adjustments, OutputSpace::Srgb)),
    ];
    for (kind, constants) in kinds {
        let reduced = pipeline.reduce(&ctx, &source, &constants);
        assert_eq!([reduced.width(), reduced.height()], [w, h], "{kind}");
        let full = crate::common::download_rgba(&ctx, &pipeline.render(&ctx, &source, &constants));
        let drawn = pipeline.render(&ctx, &reduced, &constants.clone().reduced());
        let codes: Vec<u32> = full
            .iter()
            .zip(crate::common::download_rgba(&ctx, &drawn))
            .map(|(a, b)| ((a - b).abs() * 65535.0).round() as u32)
            .collect();
        let worst = codes.iter().max().unwrap();
        let mean = codes.iter().sum::<u32>() as f64 / codes.len() as f64;
        assert!(*worst <= 257 && mean < 8.0, "{kind}: {worst} codes apart, {mean:.2} on average");
    }
}
