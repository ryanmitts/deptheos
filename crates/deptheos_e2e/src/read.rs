//! Reading a file and shrinking it: JPEG, PNG and TIFF by the `image`
//! crate, RAW and DNG through the engine.

use std::fmt;
use std::path::Path;

use deptheos_dng::{DngImage, RenderParams, WhiteXy};
use image::{DynamicImage, ImageReader};
use rayon::prelude::*;

/// A picture shrunk by a whole factor, RGB as encoded, 0 to 1.
#[derive(Clone)]
pub struct Small {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
}

impl Small {
    pub(crate) fn gray(&self) -> Vec<f32> {
        self.px.iter().map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).collect()
    }

    pub(crate) fn shrink(&self, k: usize) -> Self {
        let (w, h) = (self.w / k, self.h / k);
        let px = (0..h)
            .into_par_iter()
            .flat_map_iter(|y| {
                (0..w).map(move |x| {
                    let mut sum = [0f32; 3];
                    for dy in 0..k {
                        for dx in 0..k {
                            let p = self.px[(y * k + dy) * self.w + x * k + dx];
                            for c in 0..3 {
                                sum[c] += p[c];
                            }
                        }
                    }
                    sum.map(|s| s / (k * k) as f32)
                })
            })
            .collect();
        Self { w, h, px }
    }
}

/// A file as read: its full size, bits per channel, and shrunk.
pub struct Read {
    pub width: u32,
    pub height: u32,
    pub bits: u8,
    pub small: Small,
    /// The color science it was rendered with: a RAW frame's or a DNG's.
    pub color: Option<RenderParams>,
    /// The white that color science renders at, when it has one.
    pub white: Option<WhiteXy>,
}

/// A RAW frame's or DNG's color science, and the white it renders at.
type Color = Option<(RenderParams, Option<WhiteXy>)>;

/// Whether `path` is camera RAW or a DNG, which the engine reads.
pub(crate) fn is_raw(path: &Path) -> bool {
    deptheos_core::io::is_raw_file(path)
}

/// A picture as opened: by the `image` crate, or for RAW and DNG
/// [`open_raw`].
fn open_image(path: &Path) -> Result<(DynamicImage, Color), String> {
    if is_raw(path) {
        return open_raw(path);
    }
    let mut reader = ImageReader::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    reader.no_limits();
    let image = reader.decode().map_err(|e| format!("{} doesn't open: {e}", path.display()))?;
    Ok((image, None))
}

/// A RAW frame or DNG rendered to 16-bit sRGB by the DNG SDK, with its
/// color science. A linear DNG renders from its own negative, as a DNG
/// reader shows it, not through the code that wrote it. Camera RAW and a
/// mosaic DNG decode through [`deptheos_app::decode_linear_with_metadata`],
/// as the app stacks them.
fn open_raw(path: &Path) -> Result<(DynamicImage, Color), String> {
    use deptheos_core::io;
    let failed = |e: &dyn fmt::Display| format!("{} doesn't open: {e}", path.display());
    if io::is_dng(path) && !io::is_mosaic_dng(path) {
        let dng = DngImage::read_metadata(path).map_err(|e| failed(&e))?;
        let (w, h) = (dng.width(), dng.height());
        let pixels =
            deptheos_dng::sdk_render_file_srgb(path, None, [0, 0, w, h]).map_err(|e| failed(&e))?;
        let color = color_of(&dng).map_err(|e| failed(&e))?;
        return Ok((rgb16(path, w, h, pixels)?, Some(color)));
    }
    let (image, metadata) =
        deptheos_app::decode_linear_with_metadata(path, deptheos_app::DcpPreference::default())
            .map_err(|e| failed(&e))?;
    let image = image.into_rgb16();
    let (w, h) = image.dimensions();
    let srgb = metadata.render_linear_to_srgb(image.as_raw(), w, h, 3).map_err(|e| failed(&e))?;
    let color = color_of(&metadata).map_err(|e| failed(&e))?;
    let pixels = match srgb.channels() {
        3 => srgb.pixels().to_vec(),
        1 => srgb.pixels().iter().flat_map(|&v| [v; 3]).collect(),
        n => return Err(failed(&format!("rendered with {n} channels"))),
    };
    Ok((rgb16(path, srgb.width(), srgb.height(), pixels)?, Some(color)))
}

fn color_of(dng: &DngImage) -> deptheos_dng::Result<(RenderParams, Option<WhiteXy>)> {
    Ok((dng.render_params()?, dng.as_shot_white()?))
}

/// The metadata a RAW frame or DNG renders with, as [`open_raw`] reads it.
pub(crate) fn color_source(path: &Path) -> Result<DngImage, String> {
    use deptheos_core::io;
    let failed = |e: &dyn fmt::Display| format!("{}: {e}", path.display());
    if io::is_dng(path) && !io::is_mosaic_dng(path) {
        return DngImage::read_metadata(path).map_err(|e| failed(&e));
    }
    deptheos_app::decode_linear_with_metadata(path, deptheos_app::DcpPreference::default())
        .map(|(_, metadata)| metadata)
        .map_err(|e| failed(&e))
}

fn rgb16(path: &Path, w: u32, h: u32, pixels: Vec<u16>) -> Result<DynamicImage, String> {
    image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(w, h, pixels)
        .map(DynamicImage::ImageRgb16)
        .ok_or_else(|| format!("{}: the render isn't {w} × {h}", path.display()))
}

/// Read `path`, shrunk by `k`.
pub fn read(path: &Path, k: usize) -> Result<Read, String> {
    read_by(path, |_, _| k)
}

/// Read `path`, shrunk by the factor `k` gives for its full size.
pub(crate) fn read_by(path: &Path, k: impl FnOnce(u32, u32) -> usize) -> Result<Read, String> {
    let (image, color) = open_image(path)?;
    let (color, white) = match color {
        Some((params, white)) => (Some(params), white),
        None => (None, None),
    };
    let bits = match &image {
        DynamicImage::ImageLuma8(_)
        | DynamicImage::ImageLumaA8(_)
        | DynamicImage::ImageRgb8(_)
        | DynamicImage::ImageRgba8(_) => 8,
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => 32,
        _ => 16,
    };
    let (width, height) = (image.width(), image.height());
    let k = k(width, height);
    let small = match &image {
        DynamicImage::ImageRgb8(b) => shrink(b.as_raw(), width as usize, 3, 255.0, k),
        DynamicImage::ImageRgba8(b) => shrink(b.as_raw(), width as usize, 4, 255.0, k),
        DynamicImage::ImageRgb16(b) => shrink(b.as_raw(), width as usize, 3, 65535.0, k),
        DynamicImage::ImageRgba16(b) => shrink(b.as_raw(), width as usize, 4, 65535.0, k),
        other => {
            let b = other.to_rgb16();
            shrink(b.as_raw(), width as usize, 3, 65535.0, k)
        }
    };
    Ok(Read { width, height, bits, small, color, white })
}

fn shrink<T: Copy + Into<f32> + Sync>(
    raw: &[T],
    width: usize,
    channels: usize,
    max: f32,
    k: usize,
) -> Small {
    let height = raw.len() / (width * channels);
    let (w, h) = (width / k, height / k);
    let px = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..w).map(move |x| {
                let mut sum = [0f32; 3];
                for dy in 0..k {
                    let row = (y * k + dy) * width;
                    for dx in 0..k {
                        let i = (row + x * k + dx) * channels;
                        for c in 0..3 {
                            sum[c] += raw[i + c].into();
                        }
                    }
                }
                sum.map(|s| s / (max * (k * k) as f32))
            })
        })
        .collect();
    Small { w, h, px }
}
