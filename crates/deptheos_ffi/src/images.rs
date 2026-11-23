//! Display images: 8-bit BGRA pixels for Core Graphics.

use deptheos_app::PresentedImage;

/// 8-bit BGRA pixels, rows tightly packed, opaque.
#[derive(Clone, Debug, uniffi::Record)]
pub struct DisplayImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// `image` at most `max_side` long: the smallest level at least that long
/// (else the largest), scaled down to it. The first level that fits would
/// show a result just over twice `max_side` at under half: 8256 px at 2064.
pub(crate) fn level_image(image: &PresentedImage, max_side: u32) -> Option<DisplayImage> {
    levels_image(&image.levels, max_side)
}

/// [`level_image`] of `levels`, finest first: some of an image's levels.
pub(crate) fn levels_image(
    levels: &[deptheos_app::TiledLevel],
    max_side: u32,
) -> Option<DisplayImage> {
    let long = |l: &&deptheos_app::TiledLevel| l.width.max(l.height);
    let level = levels.iter().rev().find(|l| long(l) >= max_side).or(levels.first())?.assemble();
    let (width, height) = deptheos_app::fit_long_edge(level.width, level.height, max_side);
    if (width, height) == (level.width, level.height) {
        return Some(DisplayImage { width, height, bgra: level.pixels });
    }
    let bgra = scale_bgra(&level.pixels, level.width, level.height, width, height)?;
    Some(DisplayImage { width, height, bgra })
}

/// Resized bilinearly: a level is under twice the size asked, so no more
/// taps are needed.
fn scale_bgra(pixels: &[u8], width: u32, height: u32, to_w: u32, to_h: u32) -> Option<Vec<u8>> {
    use fast_image_resize as fr;
    let src = fr::images::ImageRef::new(width, height, pixels, fr::PixelType::U8x4)
        .inspect_err(|e| tracing::warn!("display level: {e}"))
        .ok()?;
    let mut dst = fr::images::Image::new(to_w, to_h, fr::PixelType::U8x4);
    fr::Resizer::new()
        .resize(
            &src,
            &mut dst,
            &fr::ResizeOptions {
                algorithm: fr::ResizeAlg::Convolution(fr::FilterType::Bilinear),
                cropping: fr::SrcCropping::None,
                mul_div_alpha: false,
            },
        )
        .inspect_err(|e| tracing::warn!("display level: {e}"))
        .ok()?;
    Some(dst.into_vec())
}

pub(crate) fn rgb_to_display(rgb: &image::RgbImage) -> DisplayImage {
    let image = deptheos_app::CpuImage::from_rgb(rgb);
    DisplayImage { width: image.width, height: image.height, bgra: image.pixels }
}
