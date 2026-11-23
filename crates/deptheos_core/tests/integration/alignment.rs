//! End-to-end alignment + stacking: a shifted frame must be warped back
//! onto the reference by the ECC transform before accumulation.

use deptheos_aligner::{AlignMode, Aligner};
use deptheos_core::pyramid::{PyramidConfig, PyramidStacker};
use deptheos_core::{StackInfo, io};
use deptheos_gpu::GpuContext;
use image::{DynamicImage, ImageBuffer, Rgb, Rgba};

use crate::common::{align, download_result};

const SIZE: u32 = 512;
const SHIFT_X: i64 = 8;
const SHIFT_Y: i64 = 5;

/// A natural image (the aligner's fruits fixture), resized. ECC needs
/// aperiodic structure; checkerboards trap it in false minima.
fn scene() -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../deptheos_aligner/fixtures/fruits.jpg");
    image::open(path)
        .expect("fixture")
        .resize_exact(SIZE, SIZE, image::imageops::FilterType::Triangle)
        .to_rgb8()
}

/// Shift content by (dx, dy) pixels with edge clamping.
fn shifted(src: &ImageBuffer<Rgb<u8>, Vec<u8>>, dx: i64, dy: i64) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
        let sx = (x as i64 + dx).clamp(0, SIZE as i64 - 1) as u32;
        let sy = (y as i64 + dy).clamp(0, SIZE as i64 - 1) as u32;
        *src.get_pixel(sx, sy)
    })
}

#[test]
fn aligned_stack_matches_reference() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    let reference = scene();
    let frame_a = DynamicImage::ImageRgb8(reference.clone());
    let frame_b = DynamicImage::ImageRgb8(shifted(&reference, SHIFT_X, SHIFT_Y));

    let mut chain = Aligner::new(AlignMode::Ecc);
    let mut stacker = PyramidStacker::new(
        ctx.clone(),
        PyramidConfig::default(),
        StackInfo { total_frames: 2, is_linear: false },
    );

    let mut t = nalgebra::Affine2::identity();
    for frame in [&frame_a, &frame_b] {
        t = align(&mut chain, frame).expect("align").transform;
        stacker.accumulate(frame, Some(&t.into())).expect("accumulate");
    }

    // `shifted` moves content left-up, so the backward map that rebuilds
    // the reference is the negative translation.
    let (tx, ty) = (t[(0, 2)], t[(1, 2)]);
    assert!(
        (tx + SHIFT_X as f32).abs() < 0.5 && (ty + SHIFT_Y as f32).abs() < 0.5,
        "recovered translation ({tx:.2}, {ty:.2}) should be ~(-{SHIFT_X}, -{SHIFT_Y})"
    );

    stacker.finalize().expect("finalize");
    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    assert_eq!((width, height), (SIZE, SIZE));
    let pixels: &[f32] = bytemuck::cast_slice(&bytes);

    // Both frames are sharp, so the interior (past the clamped border)
    // matches the reference unless the warp misapplied the transform.
    let margin = 32u32;
    let mut total = 0.0f64;
    let mut count = 0u64;
    for y in margin..SIZE - margin {
        for x in margin..SIZE - margin {
            let base = ((y * width + x) * 4) as usize;
            let p = reference.get_pixel(x, y);
            for c in 0..3 {
                total += (pixels[base + c] as f64 - p[c] as f64 / 255.0).abs();
                count += 1;
            }
        }
    }
    let diff = total / count as f64;
    assert!(diff < 0.03, "aligned stack deviates from reference: mean abs diff {diff:.4}");
}

/// A 16-bit RGBA source aligns exactly like its RGB twin: its opaque alpha
/// is dropped (ADR-132). The low byte carries detail, so an 8-bit step
/// would move the result.
#[test]
fn rgba16_source_aligns_like_its_rgb_twin() {
    let dir = tempfile::tempdir().unwrap();
    let reference = scene();
    let mut rgb_paths = Vec::new();
    let mut rgba_paths = Vec::new();
    for (i, frame) in [reference.clone(), shifted(&reference, SHIFT_X, SHIFT_Y)].iter().enumerate()
    {
        let rgb16 = ImageBuffer::<Rgb<u16>, _>::from_fn(SIZE, SIZE, |x, y| {
            let low = ((x * 7 + y * 13) % 251) as u16;
            Rgb(frame.get_pixel(x, y).0.map(|v| v as u16 * 256 + low))
        });
        let rgba16 = ImageBuffer::<Rgba<u16>, _>::from_fn(SIZE, SIZE, |x, y| {
            let [r, g, b] = rgb16.get_pixel(x, y).0;
            Rgba([r, g, b, u16::MAX])
        });
        let rgb_path = dir.path().join(format!("rgb{i}.png"));
        let rgba_path = dir.path().join(format!("rgba{i}.png"));
        rgb16.save(&rgb_path).unwrap();
        rgba16.save(&rgba_path).unwrap();
        rgb_paths.push(rgb_path);
        rgba_paths.push(rgba_path);
    }

    let align = |paths: &[std::path::PathBuf]| {
        let mut chain = Aligner::new(AlignMode::Ecc);
        let mut last = None;
        for path in paths {
            let image = io::load_image(path, io::CameraRaw::LibRaw).unwrap();
            assert!(matches!(image, DynamicImage::ImageRgb16(_)), "{:?}", image.color());
            last = Some(align(&mut chain, &image).expect("align").transform);
        }
        last.unwrap()
    };
    let rgb = align(&rgb_paths);
    let rgba = align(&rgba_paths);
    assert_eq!(rgb.matrix(), rgba.matrix());
    assert!((rgb[(0, 2)] + SHIFT_X as f32).abs() < 0.5, "{:?}", rgb.matrix());
}
