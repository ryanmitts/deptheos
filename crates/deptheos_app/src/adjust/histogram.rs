//! The live histogram (ADR-138): counted on the CPU from a small draw of
//! the whole picture under the crop, from the reduced copy, so it is the
//! same at any zoom or pan ([`super::GpuCopies::histogram`]). Its clipping
//! counts the pixels at 8-bit 0 or 255 per channel; when a drag ends they
//! are counted again on the GPU over the full redraw ([`clipping`]), so a
//! highlight a pixel wide, which the reduced copy averages away, counts.

use deptheos_render::dng_render::{Clipped, Region};
use deptheos_render::reframe::Rect;

use crate::engine::Engine;
use crate::{Error, Result};

/// Bins per channel: one per 8-bit display value.
pub const BINS: usize = 256;

/// The long side of the small draw the histogram is counted from.
pub const SMALL_LONG: u32 = 512;

/// The picture under the crop, by display value.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    /// Pixels at each 8-bit value: red, green, blue.
    pub channels: [[u32; BINS]; 3],
    /// Pixels at each value of Rec. 709 luma of the display values.
    pub luminance: [u32; BINS],
    /// Pixels counted.
    pub pixels: u64,
    pub clipping: Clipping,
}

/// Pixels showing a channel at 8-bit 0 (`shadows`) or 255
/// (`highlights`), red first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clipping {
    pub shadows: [u64; 3],
    pub highlights: [u64; 3],
    pub counted: Counted,
}

/// What a [`Clipping`] was counted over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counted {
    /// The histogram's small draw: a tick's.
    Small,
    /// Every pixel of the settled full picture.
    Full,
}

impl Clipping {
    /// Whether any channel of any pixel shows at black.
    pub fn shadows_clipped(&self) -> bool {
        self.shadows.iter().any(|&n| n > 0)
    }

    /// Whether any channel of any pixel shows at white.
    pub fn highlights_clipped(&self) -> bool {
        self.highlights.iter().any(|&n| n > 0)
    }
}

impl Histogram {
    /// Counted from `rows` of 8-bit RGB.
    pub(crate) fn count<'a>(rows: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut histogram = Self {
            channels: [[0; BINS]; 3],
            luminance: [0; BINS],
            pixels: 0,
            clipping: Clipping { shadows: [0; 3], highlights: [0; 3], counted: Counted::Small },
        };
        for row in rows {
            for &[r, g, b] in row.as_chunks::<3>().0 {
                let rgb = [r, g, b];
                for (c, &v) in rgb.iter().enumerate() {
                    histogram.channels[c][usize::from(v)] += 1;
                    histogram.clipping.shadows[c] += u64::from(v == 0);
                    histogram.clipping.highlights[c] += u64::from(v == 255);
                }
                let luma = (2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b) + 5000)
                    / 10_000;
                histogram.luminance[luma as usize] += 1;
                histogram.pixels += 1;
            }
        }
        histogram
    }

    /// This histogram with `clipping` counted over the full picture.
    pub fn with_clipping(self, clipping: Clipping) -> Self {
        Self { clipping, ..self }
    }
}

/// The clipping of `full`, a settled full redraw of a `frame`-sized
/// picture in display values, under `crop` (stored pixels; none is the
/// whole picture): every pixel, counted on the GPU.
pub fn clipping(engine: &Engine, full: &wgpu::Texture, crop: Option<Rect>) -> Result<Clipping> {
    let window = match crop {
        Some(c) => Region { x: c.x, y: c.y, width: c.width, height: c.height },
        None => Region::whole(full.width(), full.height()),
    };
    let fits = window.width > 0
        && window.height > 0
        && window.x + window.width <= full.width()
        && window.y + window.height <= full.height();
    if !fits {
        return Err(Error::InvalidState(format!("crop {window:?} outside the picture")));
    }
    let Clipped { shadows, highlights } =
        engine.color().clipped(engine.gpu(), full, window).map_err(|e| Error::Core(e.into()))?;
    Ok(Clipping {
        shadows: shadows.map(u64::from),
        highlights: highlights.map(u64::from),
        counted: Counted::Full,
    })
}

/// `crop` (stored pixels of a `frame`-sized picture) in the pixels of a
/// `size` draw of it: the pixels whose centers it holds, at least one.
pub(crate) fn scaled_window(crop: Option<Rect>, frame: [u32; 2], size: [u32; 2]) -> Region {
    let Some(c) = crop else { return Region::whole(size[0], size[1]) };
    let axis = |at: u32, len: u32, from: u32, to: u32| {
        let s = f64::from(to) / f64::from(from);
        let start = ((f64::from(at) * s).round() as u32).min(to - 1);
        let end = ((f64::from(at + len) * s).round() as u32).clamp(start + 1, to);
        (start, end - start)
    };
    let (x, width) = axis(c.x, c.width, frame[0], size[0]);
    let (y, height) = axis(c.y, c.height, frame[1], size[1]);
    Region { x, y, width, height }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pixel_lands_in_one_bin_of_each_channel() {
        let row: Vec<u8> = [[0, 128, 255], [10, 10, 10], [255, 255, 255]].concat();
        let h = Histogram::count([&row[..], &row[..]]);
        assert_eq!(h.pixels, 6);
        assert_eq!((h.channels[0][0], h.channels[1][128], h.channels[2][255]), (2, 2, 4));
        assert_eq!((h.luminance[10], h.luminance[255]), (2, 2));
        assert_eq!(h.clipping.shadows, [2, 0, 0]);
        assert_eq!(h.clipping.highlights, [2, 2, 4]);
        assert!(h.clipping.shadows_clipped() && h.clipping.highlights_clipped());
        let none = Histogram::count([&[10u8, 20, 30][..]]);
        assert!(!none.clipping.shadows_clipped() && !none.clipping.highlights_clipped());
    }

    #[test]
    fn a_crop_scales_to_the_small_draw() {
        assert_eq!(scaled_window(None, [900, 600], [450, 300]), Region::whole(450, 300));
        let crop = Rect { x: 100, y: 50, width: 400, height: 300 };
        let r = scaled_window(Some(crop), [900, 600], [450, 300]);
        assert_eq!(r, Region { x: 50, y: 25, width: 200, height: 150 });
        let tiny = Rect { x: 899, y: 599, width: 1, height: 1 };
        let r = scaled_window(Some(tiny), [900, 600], [90, 60]);
        assert_eq!((r.width, r.height), (1, 1));
        assert!(r.x < 90 && r.y < 60);
    }
}
