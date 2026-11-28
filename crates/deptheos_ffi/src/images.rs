//! Display images: 8-bit BGRA pixels for Core Graphics, each naming the
//! space its colors are in.

use deptheos_app::PresentedImage;

/// 8-bit BGRA pixels, rows tightly packed, opaque, in `space`: the image
/// is tagged with it.
#[derive(Clone, Debug, uniffi::Record)]
pub struct DisplayImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub space: ScreenSpace,
}

/// The colors a picture for the screen is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenSpace {
    /// sRGB, and a standard stack's own values with Color Management off,
    /// shown as they are.
    Srgb,
    DisplayP3,
}

impl From<deptheos_app::ScreenSpace> for ScreenSpace {
    fn from(space: deptheos_app::ScreenSpace) -> Self {
        match space {
            deptheos_app::ScreenSpace::Srgb => Self::Srgb,
            deptheos_app::ScreenSpace::DisplayP3 => Self::DisplayP3,
        }
    }
}

impl From<ScreenSpace> for deptheos_app::ScreenSpace {
    fn from(space: ScreenSpace) -> Self {
        match space {
            ScreenSpace::Srgb => Self::Srgb,
            ScreenSpace::DisplayP3 => Self::DisplayP3,
        }
    }
}

impl From<deptheos_render::OutputSpace> for ScreenSpace {
    fn from(space: deptheos_render::OutputSpace) -> Self {
        deptheos_app::ScreenSpace::of(space).into()
    }
}

/// Sets the space the screen shows: Display P3 on a wide-gamut screen,
/// else sRGB (the default). Pictures presented from then on are drawn in
/// it; the UI, the histogram, exports and thumbnails stay sRGB.
#[uniffi::export]
pub fn set_screen_space(space: ScreenSpace) {
    deptheos_app::engine::present::set_screen(space.into());
}

/// `image` at most `max_side` long: the smallest level at least that long
/// (else the largest), scaled down to it. The first level that fits would
/// show a result just over twice `max_side` at under half: 8256 px at 2064.
pub(crate) fn level_image(image: &PresentedImage, max_side: u32) -> Option<DisplayImage> {
    levels_image(&image.levels, max_side, image.space().into())
}

/// [`level_image`] of `levels`, finest first: some of an image's levels,
/// in `space`.
pub(crate) fn levels_image(
    levels: &[deptheos_app::TiledLevel],
    max_side: u32,
    space: ScreenSpace,
) -> Option<DisplayImage> {
    let long = |l: &&deptheos_app::TiledLevel| l.width.max(l.height);
    let level = levels.iter().rev().find(|l| long(l) >= max_side).or(levels.first())?.assemble();
    let (width, height) = deptheos_app::fit_long_edge(level.width, level.height, max_side);
    if (width, height) == (level.width, level.height) {
        return Some(DisplayImage { width, height, bgra: level.pixels, space });
    }
    let bgra = scale_bgra(&level.pixels, level.width, level.height, width, height)?;
    Some(DisplayImage { width, height, bgra, space })
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

/// `rgb`, in sRGB (a thumbnail), for the screen.
pub(crate) fn rgb_to_display(rgb: &image::RgbImage) -> DisplayImage {
    let image = deptheos_app::CpuImage::from_rgb(rgb);
    let (width, height) = (image.width, image.height);
    DisplayImage { width, height, bgra: image.pixels, space: ScreenSpace::Srgb }
}
