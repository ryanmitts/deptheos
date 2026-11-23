//! The grid aligner's gray map: the equal-weight gray `(r + g + b) / 3`
//! ([`GrayRows`], read a row at a time) resized with a Keys cubic
//! ([`resize_keys_scaled`]). Every sum takes its taps in the order of the
//! plain per-pixel loop in the tests, so the map is that loop's bit for bit.

use super::rows::{RowTaps, Tap, add_weighted_row, par_bands};
use super::{unsupported_layout, vectorized};
use crate::error::AlignmentError;
use crate::plane::Plane;
use image::DynamicImage;
use rayon::prelude::*;

/// Where the grid's resize reads gray rows from: a gray plane, or an RGB
/// image converted to equal-weight gray (not Rec. 709 luma) as it is read.
#[derive(Clone, Copy)]
pub(crate) enum GrayRows<'a> {
    Plane(&'a Plane),
    Image(&'a DynamicImage),
}

impl GrayRows<'_> {
    pub(crate) fn size(&self) -> (usize, usize) {
        match self {
            GrayRows::Plane(p) => (p.width, p.height),
            GrayRows::Image(i) => (i.width() as usize, i.height() as usize),
        }
    }

    fn check(&self) -> Result<(), AlignmentError> {
        match self {
            GrayRows::Plane(_)
            | GrayRows::Image(DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgb16(_)) => Ok(()),
            GrayRows::Image(other) => Err(unsupported_layout(other)),
        }
    }

    /// Row `y` into `out` (one row wide).
    fn fill(&self, y: usize, out: &mut [f32]) {
        fn mean<T: Copy + Into<f32>>(src: &[T], out: &mut [f32], scale: f32) {
            for (rgb, o) in src.as_chunks::<3>().0.iter().zip(out.iter_mut()) {
                *o = (rgb[0].into() + rgb[1].into() + rgb[2].into()) * scale;
            }
        }
        let w = out.len();
        match self {
            GrayRows::Plane(p) => out.copy_from_slice(&p.data[y * w..(y + 1) * w]),
            GrayRows::Image(DynamicImage::ImageRgb8(i)) => {
                mean(&i.as_raw()[y * w * 3..(y + 1) * w * 3], out, 1.0 / (3.0 * 255.0))
            }
            GrayRows::Image(DynamicImage::ImageRgb16(i)) => {
                mean(&i.as_raw()[y * w * 3..(y + 1) * w * 3], out, 1.0 / (3.0 * 65535.0))
            }
            GrayRows::Image(_) => unreachable!("checked"),
        }
    }
}

/// The equal-weight gray of a whole image, for tests.
#[cfg(test)]
pub(crate) fn image_to_gray_mean(image: &DynamicImage) -> Result<Plane, AlignmentError> {
    let rows = GrayRows::Image(image);
    rows.check()?;
    let (w, h) = rows.size();
    let mut out = Plane::new(w, h);
    out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| rows.fill(y, row));
    Ok(out)
}

/// Keys cubic with `B = 0, C = 0.6121`, slightly sharper than `C = 0.5`.
fn keys_kernel(x: f32) -> f32 {
    const C: f32 = 0.6121;
    let a = x.abs();
    if a < 1.0 {
        (2.0 - C) * a * a * a - (3.0 - C) * a * a + 1.0
    } else if a < 2.0 {
        -C * a * a * a + 5.0 * C * a * a - 8.0 * C * a + 4.0 * C
    } else {
        0.0
    }
}

/// Per-destination taps with support `radius`: destination `X` is centered
/// at `(X + 0.5)/scale − 0.5`, the kernel stretched when downscaling,
/// out-of-image taps dropped and the rest renormalized.
fn resample_taps(src_len: usize, dst_len: usize, scale: f32, radius: f32) -> Vec<Tap> {
    let k = scale.min(1.0);
    let n_taps = (2.0 * radius / k + 1.0).floor() as i64;
    (0..dst_len)
        .map(|x| {
            let center = (x as f32 + 0.5) / scale - 0.5;
            let first = (center - radius / k).ceil() as i64;
            let in_image = first.max(0)..(first + n_taps).min(src_len as i64);
            let mut weights: Vec<f32> = in_image
                .clone()
                .map(|idx| {
                    let d = (k * (center - idx as f32)).abs();
                    if d > radius { 0.0 } else { keys_kernel(d) }
                })
                .collect();
            let sum = weights.iter().fold(0.0f32, |s, &w| s + w);
            if weights.is_empty() || sum.abs() < 1e-12 {
                let idx = (center.floor().max(0.0) as usize).min(src_len - 1);
                return (idx, vec![1.0]);
            }
            for w in &mut weights {
                *w /= sum;
            }
            (in_image.start as usize, weights)
        })
        .collect()
}

/// Resize gray by `scale` with the Keys cubic: `trunc(dim·scale + 0.01)` a
/// side, vertical pass first, clamped to `[0, 1]`.
pub(crate) fn resize_keys_scaled(src: GrayRows<'_>, scale: f32) -> Result<Plane, AlignmentError> {
    src.check()?;
    let (src_w, src_h) = src.size();
    if src_w == 0 || src_h == 0 {
        return Err(AlignmentError::ResizeFailed("empty source".into()));
    }
    if scale == 1.0 {
        let mut out = Plane::new(src_w, src_h);
        out.data.par_chunks_mut(src_w).enumerate().for_each(|(y, row)| src.fill(y, row));
        return Ok(out);
    }
    let dst_w = ((src_w as f32 * scale + 0.01) as usize).max(1);
    let dst_h = ((src_h as f32 * scale + 0.01) as usize).max(1);
    let resize = KeysResize::new(src, (src_w, src_h), (dst_w, dst_h), scale);
    let mut out = vec![0.0f32; dst_h * dst_w];
    par_bands(
        &mut out,
        dst_w,
        KeysResize::BAND,
        || resize.scratch(),
        |y0, band, scratch| resize.band(y0, band, scratch),
    );
    Ok(Plane::from_vec(dst_w, dst_h, out))
}

/// One [`resize_keys_scaled`]: its source, sizes and taps.
struct KeysResize<'a> {
    src: GrayRows<'a>,
    src_w: usize,
    dst_w: usize,
    v_taps: Vec<Tap>,
    h_taps: RowTaps,
    /// The most source rows one band's vertical taps span.
    max_band_rows: usize,
}

/// A thread's buffers for [`KeysResize::band`], reused across its bands.
struct BandScratch {
    /// The gray of the source rows a band reads.
    gray: Vec<f32>,
    /// The band after the vertical pass: its rows at the source's width.
    mid: Vec<f32>,
}

impl<'a> KeysResize<'a> {
    /// Destination rows per band; a band converts each source row once.
    const BAND: usize = 32;

    fn new(
        src: GrayRows<'a>,
        (src_w, src_h): (usize, usize),
        (dst_w, dst_h): (usize, usize),
        scale: f32,
    ) -> Self {
        let v_taps = resample_taps(src_h, dst_h, scale, 2.0);
        let h_taps = RowTaps::new(resample_taps(src_w, dst_w, scale, 2.0), src_w);
        let max_band_rows = v_taps
            .chunks(Self::BAND)
            .map(|taps| {
                let (lo, hi) = source_rows(taps);
                hi - lo + 1
            })
            .max()
            .unwrap_or(1);
        Self { src, src_w, dst_w, v_taps, h_taps, max_band_rows }
    }

    fn scratch(&self) -> BandScratch {
        BandScratch {
            gray: vec![0.0; self.max_band_rows * self.src_w],
            mid: vec![0.0; Self::BAND.min(self.v_taps.len()) * self.src_w],
        }
    }

    /// Destination rows `y0..` into `out`, at most [`Self::BAND`].
    fn band(&self, y0: usize, out: &mut [f32], scratch: &mut BandScratch) {
        let (src_w, dst_w) = (self.src_w, self.dst_w);
        let taps = &self.v_taps[y0..y0 + out.len() / dst_w];

        // 1. The gray of the source rows `lo..=hi` that the band's taps read.
        let (lo, hi) = source_rows(taps);
        let gray = &mut scratch.gray[..(hi - lo + 1) * src_w];
        for (sy, row) in (lo..=hi).zip(gray.chunks_mut(src_w)) {
            self.src.fill(sy, row);
        }

        // 2. Vertical, a whole row per tap. 3. Horizontal, then clamped.
        let mid = &mut scratch.mid[..taps.len() * src_w];
        mid.fill(0.0);
        vectorized(
            #[inline(always)]
            || {
                for (mid_row, (start, weights)) in mid.chunks_mut(src_w).zip(taps) {
                    for (sy, &w) in (start - lo..).zip(weights) {
                        add_weighted_row(mid_row, &gray[sy * src_w..(sy + 1) * src_w], w);
                    }
                }
                for (mid_row, row) in mid.chunks_exact(src_w).zip(out.chunks_exact_mut(dst_w)) {
                    self.h_taps.filter(mid_row, row);
                    for o in row {
                        *o = o.clamp(0.0, 1.0);
                    }
                }
            },
        );
    }
}

/// The first and last source rows `taps` read.
fn source_rows(taps: &[Tap]) -> (usize, usize) {
    let lo = taps.iter().map(|t| t.0).min().unwrap_or(0);
    let hi = taps.iter().map(|(start, w)| start + w.len() - 1).max().unwrap_or(0);
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 at 0 and 0 at the other integers.
    #[test]
    fn keys_kernel_interpolates_at_integers() {
        assert!((keys_kernel(0.0) - 1.0).abs() < 1e-6);
        assert!(keys_kernel(1.0).abs() < 1e-6);
        assert!(keys_kernel(2.0).abs() < 1e-6);
    }

    #[test]
    fn half_scale_taps_are_centered() {
        // 2:1 down: 9 taps at 2X−3 … 2X+5, centered at 2X + 0.5, edges dropped.
        let taps = resample_taps(100, 50, 0.5, 2.0);
        assert_eq!((taps[10].0, taps[10].1.len()), (17, 9));
        let sum: f32 = taps[10].1.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
        assert!(taps[49].0 + taps[49].1.len() <= 100);
        assert!(taps[0].1.len() < 9, "out-of-image taps must be dropped");
    }

    #[test]
    fn constant_image_stays_constant_and_sizes_truncate() {
        let src = Plane::from_vec(201, 301, vec![0.4f32; 201 * 301]);
        let out = resize_keys_scaled(GrayRows::Plane(&src), 0.5).unwrap();
        assert_eq!((out.height, out.width), (150, 100));
        for &v in &out.data {
            assert!((v - 0.4).abs() < 1e-5, "{v}");
        }
    }

    /// The resize with every pixel's taps summed one pixel at a time.
    fn resize_keys_scaled_reference(src: GrayRows<'_>, scale: f32) -> Plane {
        let (src_w, src_h) = src.size();
        let dst_w = ((src_w as f32 * scale + 0.01) as usize).max(1);
        let dst_h = ((src_h as f32 * scale + 0.01) as usize).max(1);
        let mut gray = vec![0.0f32; src_w * src_h];
        for (y, row) in gray.chunks_mut(src_w).enumerate() {
            src.fill(y, row);
        }
        let mut mid = vec![0.0f32; dst_h * src_w];
        for (row, (start, taps)) in
            mid.chunks_mut(src_w).zip(resample_taps(src_h, dst_h, scale, 2.0))
        {
            for (sy, w) in (start..).zip(taps) {
                for (o, &v) in row.iter_mut().zip(&gray[sy * src_w..(sy + 1) * src_w]) {
                    *o += v * w;
                }
            }
        }
        let h_taps = resample_taps(src_w, dst_w, scale, 2.0);
        let mut out = vec![0.0f32; dst_h * dst_w];
        for (y, row) in out.chunks_mut(dst_w).enumerate() {
            let mid_row = &mid[y * src_w..(y + 1) * src_w];
            for (x, o) in row.iter_mut().enumerate() {
                let (start, taps) = &h_taps[x];
                let acc: f32 = (*start..).zip(taps).map(|(sx, w)| mid_row[sx] * w).sum();
                *o = acc.clamp(0.0, 1.0);
            }
        }
        Plane::from_vec(dst_w, dst_h, out)
    }

    /// Planes that exercise the last bits and the sign of zero: the
    /// fixture, black with a lit stripe, and values outside `[0, 1]`
    /// (negative and subnormal included).
    fn awkward_planes(w: usize, h: usize) -> Vec<Plane> {
        let spider = crate::test_util::fixture("spider.jpg", w as u32, h as u32);
        let mut seed = 99u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 8
        };
        let odd = [0.0, -0.0, 1.0, -1e-3, 1e-40, -1e-40, 1.5, f32::MIN_POSITIVE, 0.25];
        vec![
            image_to_gray_mean(&spider).unwrap(),
            Plane::from_vec(
                w,
                h,
                (0..w * h).map(|i| if i % w == w / 3 { 1.0 } else { 0.0 }).collect(),
            ),
            Plane::from_vec(w, h, (0..w * h).map(|_| odd[next() as usize % odd.len()]).collect()),
        ]
    }

    /// The uniform run is the per-pixel sum bit for bit, at integer and
    /// fractional downscales and an upscale, for odd and tiny widths.
    #[test]
    fn uniform_run_matches_the_per_pixel_sum_bitwise() {
        let sizes = [(1, 1), (2, 3), (3, 2), (5, 7), (17, 9), (33, 31), (257, 40), (1001, 23)];
        let scales = [0.5, 0.25, 1.0 / 3.0, 0.3, 0.37, 0.7, 2.0];
        let mut uniform = 0;
        for (w, h) in sizes {
            for (k, plane) in awkward_planes(w, h).iter().enumerate() {
                for scale in scales {
                    let got = resize_keys_scaled(GrayRows::Plane(plane), scale).unwrap();
                    let want = resize_keys_scaled_reference(GrayRows::Plane(plane), scale);
                    assert_eq!((got.width, got.height), (want.width, want.height));
                    let bits = |p: &Plane| p.data.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                    assert!(bits(&got) == bits(&want), "{w}x{h} plane {k} scale {scale}");
                    let dst_w = got.width;
                    let h_taps = RowTaps::new(resample_taps(w, dst_w, scale, 2.0), w);
                    uniform += !h_taps.run.is_empty() as usize;
                }
            }
        }
        assert!(uniform > 10, "the uniform run was taken {uniform} times");
        // An RGB image through the gray rows, as the aligner reads it.
        let image = crate::test_util::fixture("spider.jpg", 403, 301);
        let got = resize_keys_scaled(GrayRows::Image(&image), 0.5).unwrap();
        let want = resize_keys_scaled_reference(GrayRows::Image(&image), 0.5);
        assert!(got.data.iter().zip(&want.data).all(|(a, b)| a.to_bits() == b.to_bits()));
    }

    /// At 0.5 every pixel but a few at each edge is in the run.
    #[test]
    fn half_scale_run_covers_the_interior() {
        let taps = RowTaps::new(resample_taps(1000, 500, 0.5, 2.0), 1000);
        assert_eq!((taps.run.clone(), taps.stride), (2..497, 2));
        let weights = &taps.taps[taps.run.start].1;
        assert_eq!(weights.len(), 9);
        assert_eq!(weights[8], 0.0, "the ninth tap's weight is 0 and is kept");
    }
}
