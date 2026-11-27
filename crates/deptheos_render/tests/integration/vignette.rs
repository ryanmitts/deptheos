//! The vignette is centered on the reframe's rectangle: a crop moves it,
//! a crop draws as that rectangle drawn as a picture of its own, and its
//! center keeps its tones while its corners darken.

use deptheos_dng::RenderParams;
use deptheos_gpu::GpuContext;
use deptheos_render::dng_render::{Constants, DngRenderPipeline};
use deptheos_render::reframe::Rect;
use deptheos_render::{Adjustments, Color, OutputSpace, Vignette, resolve_adjusted};
use image::{DynamicImage, ImageBuffer, Rgb};

use crate::common::{download_rgba, upload};

const SIZE: [u32; 2] = [320, 200];
/// Off the picture's center.
const CROP: Rect = Rect { x: 130, y: 30, width: 150, height: 110 };

/// A soft gradient with a fine texture, so every pixel is its own.
fn picture(size: [u32; 2]) -> Vec<u16> {
    let [w, h] = size;
    (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let hash = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 2048;
            [20_000 + x * 60 + hash, 30_000 + y * 80, 26_000 + hash * 4]
                .map(|v| v.min(65_535) as u16)
        })
        .collect()
}

/// `crop` of a `size`-sized picture.
fn cut(pixels: &[u16], size: [u32; 2], crop: Rect) -> Vec<u16> {
    let row = size[0] as usize * 3;
    (crop.y..crop.y + crop.height)
        .flat_map(|y| {
            let start = y as usize * row + crop.x as usize * 3;
            pixels[start..start + crop.width as usize * 3].iter().copied()
        })
        .collect()
}

fn draw(ctx: &GpuContext, constants: &Constants, size: [u32; 2], pixels: &[u16]) -> Vec<f32> {
    let buffer = ImageBuffer::<Rgb<u16>, _>::from_raw(size[0], size[1], pixels.to_vec()).unwrap();
    let input = upload(ctx, &DynamicImage::ImageRgb16(buffer));
    download_rgba(ctx, &DngRenderPipeline::new(ctx).render(ctx, &input, constants))
}

/// RGBA pixel (`x`, `y`) of a `width`-wide draw.
fn at(drawn: &[f32], width: u32, (x, y): (u32, u32)) -> [f32; 3] {
    let i = (y * width + x) as usize * 4;
    [drawn[i], drawn[i + 1], drawn[i + 2]]
}

fn params() -> RenderParams {
    RenderParams { baseline_exposure: 0.2, ..RenderParams::srgb_identity() }
}

#[test]
fn a_crop_moves_the_vignette_to_its_center() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pixels = picture(SIZE);
    let params = params();
    let vignetted =
        Adjustments { vignette: Vignette { amount: -70.0, midpoint: 0.0 }, ..Adjustments::NEUTRAL };
    for color in [Color::Linear(&params), Color::Standard(None)] {
        let constants = |a: &Adjustments| *resolve_adjusted(color, a, OutputSpace::Srgb);
        let plain = draw(&ctx, &constants(&Adjustments::NEUTRAL), SIZE, &pixels);
        let whole = draw(&ctx, &constants(&vignetted), SIZE, &pixels);
        let framed = draw(&ctx, &constants(&vignetted).framed(CROP), SIZE, &pixels);
        let w = SIZE[0];
        let (picture_center, crop_center) = ((w / 2, SIZE[1] / 2), (205, 85));
        // Each draw keeps its own center's tones and darkens away from it.
        assert_eq!(at(&whole, w, picture_center), at(&plain, w, picture_center));
        assert_eq!(at(&framed, w, crop_center), at(&plain, w, crop_center));
        let darker = |d: &[f32], p| at(d, w, p)[1] < at(&plain, w, p)[1] - 0.02;
        assert!(darker(&framed, picture_center), "the picture's center is off the crop's");
        assert!(darker(&whole, crop_center), "and the crop's off the picture's");
        assert!(darker(&framed, (CROP.x, CROP.y)) && darker(&whole, (0, 0)));

        // The crop's window draws as the crop drawn as a picture of its own.
        let alone_size = [CROP.width, CROP.height];
        let alone = draw(&ctx, &constants(&vignetted), alone_size, &cut(&pixels, SIZE, CROP));
        let window: Vec<f32> = (CROP.y..CROP.y + CROP.height)
            .flat_map(|y| {
                let start = (y * w + CROP.x) as usize * 4;
                framed[start..start + CROP.width as usize * 4].iter().copied()
            })
            .collect();
        assert!(window == alone, "a crop's vignette is the crop's own");
    }
}

/// Darkening and lightening each move the corners their way, and the
/// whole picture's mean with them.
#[test]
fn a_vignette_darkens_or_lightens_the_corners() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pixels = picture(SIZE);
    let params = params();
    for color in [Color::Linear(&params), Color::Standard(None)] {
        let drawn = |amount| {
            let vignette = Vignette { amount, ..Vignette::NONE };
            let a = Adjustments { vignette, ..Adjustments::NEUTRAL };
            draw(&ctx, &resolve_adjusted(color, &a, OutputSpace::Srgb), SIZE, &pixels)
        };
        let (darker, plain, lighter) = (drawn(-50.0), drawn(0.0), drawn(50.0));
        let corner = |d: &[f32]| at(d, SIZE[0], (2, SIZE[1] - 3))[1];
        assert!(corner(&darker) < corner(&plain) && corner(&plain) < corner(&lighter));
    }
}
