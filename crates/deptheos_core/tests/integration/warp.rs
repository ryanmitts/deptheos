//! `TextureWarper`: the standalone warp+crop used to bring a source frame
//! into a stacked result's geometry for comparison.

use deptheos_core::Affine2;
use deptheos_core::warp::TextureWarper;
use deptheos_gpu::GpuContext;
use image::math::Rect;
use image::{ImageBuffer, Rgb};
use nalgebra::{Matrix3, Vector2};

use crate::common::{download_rgba, upload_rgb8 as upload};

const W: u32 = 96;
const H: u32 = 64;

/// Smooth gradients, so an integer-shift Lanczos reproduces pixels exactly
/// and every pixel is distinguishable from its neighbors.
fn scene() -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    ImageBuffer::from_fn(W, H, |x, y| Rgb([(x * 2) as u8, (y * 3) as u8, ((x + y) % 256) as u8]))
}

/// Warp output (in the context's source format) → RGB u8 (rounded),
/// row-major.
fn download_rgb8(ctx: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 3]> {
    assert_eq!(texture.format(), ctx.source_format());
    download_rgba(ctx, texture)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            [
                (p[0] * 255.0).round() as u8,
                (p[1] * 255.0).round() as u8,
                (p[2] * 255.0).round() as u8,
            ]
        })
        .collect()
}

fn assert_matches(
    actual: &[[u8; 3]],
    out_w: u32,
    out_h: u32,
    expected: impl Fn(u32, u32) -> Rgb<u8>,
) {
    for y in 0..out_h {
        for x in 0..out_w {
            let got = actual[(y * out_w + x) as usize];
            let want = expected(x, y).0;
            assert_eq!(got, want, "pixel ({x}, {y})");
        }
    }
}

#[test]
fn identity_warp_reproduces_source() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let image = scene();
    let source = upload(&ctx, &image);
    let warped = TextureWarper::new(&ctx).warp(&ctx, &source, None, None);
    assert_eq!((warped.width(), warped.height()), (W, H));
    let out = download_rgb8(&ctx, &warped);
    assert_matches(&out, W, H, |x, y| *image.get_pixel(x, y));
}

#[test]
fn crop_selects_the_rectangle() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let image = scene();
    let source = upload(&ctx, &image);
    let rect = Rect { x: 10, y: 20, width: 50, height: 30 };
    let warped = TextureWarper::new(&ctx).warp(&ctx, &source, None, Some(rect));
    assert_eq!((warped.width(), warped.height()), (rect.width, rect.height));
    let out = download_rgb8(&ctx, &warped);
    assert_matches(&out, rect.width, rect.height, |x, y| *image.get_pixel(x + rect.x, y + rect.y));
}

#[test]
fn integer_translation_then_crop_composes() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let image = scene();
    let source = upload(&ctx, &image);
    // Reference → frame: frame pixel = reference pixel + (3, 2), the
    // aligner's backward-sampling convention.
    let transform =
        Affine2::from_matrix_unchecked(Matrix3::new_translation(&Vector2::new(3.0, 2.0)));
    let rect = Rect { x: 4, y: 6, width: 40, height: 20 };
    let warped = TextureWarper::new(&ctx).warp(&ctx, &source, Some(&transform.into()), Some(rect));
    let out = download_rgb8(&ctx, &warped);
    assert_matches(&out, rect.width, rect.height, |x, y| {
        *image.get_pixel(x + rect.x + 3, y + rect.y + 2)
    });
}

/// Odd pixel counts give a byte length that is not a multiple of 4; the
/// uploader must pad the staging write rather than trip wgpu validation.
#[test]
fn odd_pixel_count_rgb8_uploads() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let image = ImageBuffer::from_fn(3, 3, |x, y| Rgb([(x * 40) as u8, (y * 40) as u8, 7]));
    let source = upload(&ctx, &image);
    // The identity warp reads back in the context's source format.
    let warped = TextureWarper::new(&ctx).warp(&ctx, &source, None, None);
    let out = download_rgb8(&ctx, &warped);
    assert_matches(&out, 3, 3, |x, y| *image.get_pixel(x, y));
}

/// The brightness factor scales the sampled color in the source's space
/// and clamps at 1.0.
#[test]
fn brightness_factor_scales_and_clamps() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let image = scene();
    let source = upload(&ctx, &image);
    let alignment = deptheos_core::FrameAlignment::new(Affine2::identity(), 2.0);
    let warped = TextureWarper::new(&ctx).warp(&ctx, &source, Some(&alignment), None);
    let out = download_rgb8(&ctx, &warped);
    assert_matches(&out, W, H, |x, y| {
        let p = image.get_pixel(x, y).0;
        Rgb(p.map(|c| (c as u32 * 2).min(255) as u8))
    });
}
