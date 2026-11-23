//! Helpers shared by the GPU integration tests: synthetic scenes, upload
//! and download, and the comparisons the stacker tests make.

// Not every helper is used on every platform.
#![allow(dead_code)]

use deptheos_core::depth::DepthStacker;
use deptheos_core::upload::ImageUploader;
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;
use image::{DynamicImage, ImageBuffer, Rgb};

pub type Rgb8Image = ImageBuffer<Rgb<u8>, Vec<u8>>;

/// `image` aligned as the next frame of `aligner`'s sequence.
pub fn align(
    aligner: &mut deptheos_aligner::Aligner,
    image: &DynamicImage,
) -> Result<deptheos_aligner::AlignedFrame, deptheos_aligner::AlignmentError> {
    aligner.align_prepared(aligner.preparer().prepare(image)?)
}

/// A whole texture's bytes in its own format, row padding stripped.
pub fn download_texture(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
) -> deptheos_gpu::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let size = [texture.width(), texture.height()];
    download_texture_with(ctx, texture, [0, 0], size, |row| bytes.extend_from_slice(row))?;
    Ok(bytes)
}

/// A `size`² high-frequency checkerboard (8 px squares in two colors):
/// blurring visibly destroys it.
pub fn sharp_scene(size: u32) -> Rgb8Image {
    ImageBuffer::from_fn(size, size, |x, y| {
        let on = ((x / 8) + (y / 8)).is_multiple_of(2);
        if on { Rgb([220, 180, 60]) } else { Rgb([30, 60, 120]) }
    })
}

/// `src` box-blurred (radius 6, edges clamped) on its left or right half,
/// sharp on the other: a frame focused on one side.
pub fn blur_half(src: &Rgb8Image, left: bool) -> Rgb8Image {
    let radius: i64 = 6;
    let (w, h) = src.dimensions();
    ImageBuffer::from_fn(w, h, |x, y| {
        let in_blur_region = if left { x < w / 2 } else { x >= w / 2 };
        if !in_blur_region {
            return *src.get_pixel(x, y);
        }
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let sx = (x as i64 + dx).clamp(0, w as i64 - 1) as u32;
                let sy = (y as i64 + dy).clamp(0, h as i64 - 1) as u32;
                let p = src.get_pixel(sx, sy);
                for c in 0..3 {
                    sum[c] += p[c] as u64;
                }
                count += 1;
            }
        }
        Rgb(sum.map(|s| (s / count) as u8))
    })
}

/// Mean absolute difference between an RGBA float result and an 8-bit
/// reference, over the RGB channels.
pub fn mean_abs_diff(result: &[f32], reference: &Rgb8Image) -> f64 {
    let mut total = 0.0f64;
    let mut count = 0u64;
    for (i, p) in reference.pixels().enumerate() {
        let base = i * 4;
        for c in 0..3 {
            total += (result[base + c] as f64 - p[c] as f64 / 255.0).abs();
            count += 1;
        }
    }
    total / count as f64
}

/// `image` uploaded into a new source texture (`GpuContext::source_format`).
pub fn upload(ctx: &GpuContext, image: &DynamicImage) -> wgpu::Texture {
    let mut uploader = ImageUploader::new(ctx);
    let prep = uploader.prepare(ctx, image).unwrap();
    let texture = ImageUploader::create_target(ctx, prep.width, prep.height, "test_source");
    let mut encoder = ctx.encoder("test_upload");
    prep.dispatch_to_rgba(ctx, &mut encoder, &texture);
    ctx.submit(encoder);
    texture
}

/// [`upload`] for an 8-bit RGB image.
pub fn upload_rgb8(ctx: &GpuContext, image: &Rgb8Image) -> wgpu::Texture {
    upload(ctx, &DynamicImage::ImageRgb8(image.clone()))
}

/// A render/warp output (Rgba16Unorm or Rgba32Float, per the adapter) as
/// RGBA floats.
pub fn download_rgba(ctx: &GpuContext, texture: &wgpu::Texture) -> Vec<f32> {
    let bytes = download_texture(ctx, texture).unwrap();
    match texture.format() {
        wgpu::TextureFormat::Rgba16Unorm => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b) as f32 / 65535.0)
            .collect(),
        wgpu::TextureFormat::Rgba32Float => bytemuck::cast_slice(&bytes).to_vec(),
        other => panic!("unexpected output format {other:?}"),
    }
}

/// [`download_rgba`] as RGB per pixel.
pub fn download_rgb(ctx: &GpuContext, texture: &wgpu::Texture) -> Vec<[f32; 3]> {
    download_rgba(ctx, texture).as_chunks::<4>().0.iter().map(|p| [p[0], p[1], p[2]]).collect()
}

/// A finalized stacker's result (`result_texture`, rgba32float) as bytes,
/// with its size.
pub fn download_result(ctx: &GpuContext, result: Option<&wgpu::Texture>) -> (Vec<u8>, u32, u32) {
    let result = result.expect("result requires finalize()");
    (download_texture(ctx, result).unwrap(), result.width(), result.height())
}

/// The depth map rendered as RGBA8 grayscale bytes at its own size (the
/// live picture, uncapped), with that size.
pub fn download_depth_map(ctx: &GpuContext, stacker: &DepthStacker) -> (Vec<u8>, u32, u32) {
    let map = stacker.render_live(u32::MAX, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    (download_texture(ctx, &map).unwrap(), map.width(), map.height())
}

/// The golden tests' four `w`×`h` frames: detail at two scales from a fixed
/// integer hash, frame k sharp in its own vertical band (k of 4) and
/// box-blurred (5×5, edges clamped) elsewhere.
pub fn golden_frames(w: u32, h: u32) -> [DynamicImage; 4] {
    let noise = |x: u32, y: u32, seed: u32| {
        let mut v = x.wrapping_mul(0x9E37_79B1)
            ^ y.wrapping_mul(0x85EB_CA77)
            ^ seed.wrapping_mul(0xC2B2_AE3D);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2C1B_3C6D);
        v ^= v >> 12;
        (v & 0xff) as u8
    };
    let sharp = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_fn(w, h, |x, y| {
        let fine = noise(x, y, 1);
        let coarse = noise(x / 9, y / 7, 2);
        Rgb([fine, coarse, fine / 2 + coarse / 2])
    });
    let frame = |k: u32| {
        let band = k * w / 4..(k + 1) * w / 4;
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(w, h, |x, y| {
            if band.contains(&x) {
                return *sharp.get_pixel(x, y);
            }
            let mut sum = [0u32; 3];
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as u32;
                    let sy = (y as i32 + dy).clamp(0, h as i32 - 1) as u32;
                    let p = sharp.get_pixel(sx, sy);
                    (0..3).for_each(|c| sum[c] += p[c] as u32);
                }
            }
            Rgb(sum.map(|s| (s / 25) as u8))
        }))
    };
    [frame(0), frame(1), frame(2), frame(3)]
}

/// The golden tests' alignment: a small rotation and a fractional shift,
/// so part of the frame is outside the reference, with an exposure
/// correction.
pub fn golden_alignment() -> deptheos_core::FrameAlignment {
    let (s, c) = 0.02f32.sin_cos();
    deptheos_core::FrameAlignment::new(
        deptheos_core::Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            c, -s, 6.5, s, c, -4.25, 0.0, 0.0, 1.0,
        )),
        1.07,
    )
}
