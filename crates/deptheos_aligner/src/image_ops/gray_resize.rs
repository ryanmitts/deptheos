//! ECC's gray (Rec. 709 luma in `[0, 1]`) and its bilinear resize.
//!
//! - [`image_to_grayscale_resized`]: an RGB frame straight to the finest
//!   level's gray, without the full-resolution gray. It reads every pixel
//!   of the frame, so it is written for speed.
//! - [`resize_plane`]: each coarser level from the one above.
//! - [`image_to_grayscale`]: the full-resolution gray.
//!
//! Both resizes share [`bilinear_taps`], so they agree up to the rounding
//! of taking the luma before or after the vertical pass (within `1e-4`).
//! Every sum takes its terms in the order of the plain per-pixel loop in
//! the tests, so it matches that loop bit for bit on every instruction set.

use super::rows::{RowTaps, Tap, add_weighted_row, par_bands};
use super::{unsupported_layout, vectorized};
use crate::error::AlignmentError;
use crate::plane::Plane;
use image::DynamicImage;
use rayon::prelude::*;

/// Rec. 709 luma in `[0, 1]` (ECC's gray), from RGB8 or RGB16.
#[tracing::instrument(skip_all)]
pub(crate) fn image_to_grayscale(image: &DynamicImage) -> Result<Plane, AlignmentError> {
    // The weights as an array, not a closure: a closure defeats
    // auto-vectorization here.
    fn luma<T: Copy + Into<f32> + Sync>(data: &[T], width: u32, height: u32, max: f32) -> Plane {
        let k = crate::REC709_LUMA.map(|w| w / max);
        let row = width as usize;
        let mut out = vec![0.0f32; row * height as usize];
        out.par_chunks_mut(row).enumerate().for_each(|(y, out_row)| {
            let src = &data[y * row * 3..(y + 1) * row * 3];
            for (rgb, o) in src.as_chunks::<3>().0.iter().zip(out_row.iter_mut()) {
                *o = rgb[0].into() * k[0] + rgb[1].into() * k[1] + rgb[2].into() * k[2];
            }
        });
        Plane::from_vec(row, height as usize, out)
    }
    let (w, h) = (image.width(), image.height());
    match image {
        DynamicImage::ImageRgb8(img) => Ok(luma(img.as_raw(), w, h, 255.0)),
        DynamicImage::ImageRgb16(img) => Ok(luma(img.as_raw(), w, h, 65535.0)),
        other => Err(unsupported_layout(other)),
    }
}

/// [`image_to_grayscale`] resized to `dst_w`×`dst_h` as [`resize_plane`]
/// does, without the full-resolution gray: each destination row filters
/// interleaved RGB vertically, takes the luma, then filters horizontally.
/// That saves a 180 MB gray plane on a 45 MP frame.
pub(crate) fn image_to_grayscale_resized(
    image: &DynamicImage,
    dst_w: usize,
    dst_h: usize,
) -> Result<Plane, AlignmentError> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    if (dst_w, dst_h) == (w, h) {
        return image_to_grayscale(image);
    }
    let size = [w, h, dst_w, dst_h];
    match image {
        DynamicImage::ImageRgb8(img) => Ok(gray_resized(img.as_raw(), size, 255.0)),
        DynamicImage::ImageRgb16(img) => Ok(gray_resized(img.as_raw(), size, 65535.0)),
        other => Err(unsupported_layout(other)),
    }
}

/// [`image_to_grayscale_resized`] of interleaved RGB `data` whose channels
/// run to `max`, `[src_w, src_h, dst_w, dst_h]`.
fn gray_resized<T: Channel>(
    data: &[T],
    [src_w, src_h, dst_w, dst_h]: [usize; 4],
    max: f32,
) -> Plane {
    let filter = GrayRowFilter {
        data,
        row_len: src_w * 3,
        luma: crate::REC709_LUMA.map(|w| w / max),
        h_taps: RowTaps::new(bilinear_taps(src_w, dst_w), src_w),
    };
    let v_taps = bilinear_taps(src_h, dst_h);
    const BAND: usize = 16;
    let mut out = vec![0.0f32; dst_w * dst_h];
    par_bands(
        &mut out,
        dst_w,
        BAND,
        || (vec![0.0f32; src_w * 3], vec![0.0f32; src_w]),
        |y0, band, (acc, mid)| {
            vectorized(
                #[inline(always)]
                || {
                    for (row, taps) in band.chunks_mut(dst_w).zip(&v_taps[y0..]) {
                        filter.row(taps, acc, mid, row);
                    }
                },
            )
        },
    );
    Plane::from_vec(dst_w, dst_h, out)
}

/// A channel type of [`gray_resized`]'s sources, and its vertical pass.
trait Channel: Copy + Into<f32> + Sync {
    /// `acc[i] = 0 + rows₀[i]·weights[0] + rows₁[i]·weights[1] + …`, in
    /// that order; by default a source row at a time.
    #[inline(always)]
    fn vertical(rows: &[Self], weights: &[f32], acc: &mut [f32]) {
        acc.fill(0.0);
        for (src, &w) in rows.chunks_exact(acc.len()).zip(weights) {
            add_weighted_row(acc, src, w);
        }
    }
}

// `u8` keeps the default: strips measured a few percent slower (aarch64).
impl Channel for u8 {}

impl Channel for u16 {
    /// A strip of `acc` at a time, its sums in registers through every
    /// source row (same operations, same order): about a fifth faster.
    #[inline(always)]
    fn vertical(rows: &[u16], weights: &[f32], acc: &mut [f32]) {
        const STRIP: usize = 32;
        let row_len = acc.len();
        let (strips, tail) = acc.as_chunks_mut::<STRIP>();
        for (s, strip) in strips.iter_mut().enumerate() {
            let mut a = [0.0f32; STRIP];
            for (j, &w) in weights.iter().enumerate() {
                let src = rows[j * row_len + s * STRIP..].first_chunk::<STRIP>().unwrap();
                for (a, v) in a.iter_mut().zip(src.map(f32::from)) {
                    *a += v * w;
                }
            }
            *strip = a;
        }
        let done = row_len - tail.len();
        tail.fill(0.0);
        for (j, &w) in weights.iter().enumerate() {
            add_weighted_row(tail, &rows[j * row_len + done..(j + 1) * row_len], w);
        }
    }
}

/// One destination row of [`image_to_grayscale_resized`].
struct GrayRowFilter<'a, T> {
    data: &'a [T],
    row_len: usize,
    luma: [f32; 3],
    h_taps: RowTaps,
}

impl<T: Channel> GrayRowFilter<'_, T> {
    /// Filter the source rows the vertical taps name into `out`; `acc` (RGB)
    /// and `mid` (gray) are scratch rows.
    #[inline(always)]
    fn row(&self, (start, weights): &Tap, acc: &mut [f32], mid: &mut [f32], out: &mut [f32]) {
        let row_len = self.row_len;
        T::vertical(&self.data[start * row_len..(start + weights.len()) * row_len], weights, acc);
        let k = self.luma;
        for (m, rgb) in mid.iter_mut().zip(acc.as_chunks::<3>().0) {
            *m = rgb[0] * k[0] + rgb[1] * k[1] + rgb[2] * k[2];
        }
        self.h_taps.filter(mid, out);
    }
}

/// Per-destination taps of the bilinear (triangle) filter: destination `x`
/// is centered at source `(x + 0.5)·scale`, the triangle stretched when
/// downscaling, the weights normalized.
fn bilinear_taps(src_len: usize, dst_len: usize) -> Vec<Tap> {
    let scale = src_len as f64 / dst_len as f64;
    let filter_scale = scale.max(1.0);
    (0..dst_len)
        .map(|x| {
            let center = (x as f64 + 0.5) * scale;
            let first = (center - filter_scale).floor().max(0.0) as usize;
            let end = ((center + filter_scale).ceil() as usize).min(src_len);
            let mut weights: Vec<f64> = (first..end)
                .map(|i| (1.0 - ((i as f64 + 0.5 - center) / filter_scale).abs()).max(0.0))
                .collect();
            // Trim zero taps at the ends, keeping at least one.
            let lead = weights.iter().take_while(|&&w| w == 0.0).count().min(weights.len() - 1);
            weights.drain(..lead);
            while weights.len() > 1 && weights.last() == Some(&0.0) {
                weights.pop();
            }
            let sum: f64 = weights.iter().sum();
            if sum <= 0.0 {
                return ((center as usize).min(src_len - 1), vec![1.0]);
            }
            let weights = weights.iter().map(|w| (w / sum) as f32).collect();
            (first + lead, weights)
        })
        .collect()
}

/// `src` resized to `new_width`×`new_height` with [`bilinear_taps`].
#[tracing::instrument(skip_all)]
pub(crate) fn resize_plane(
    src: &Plane,
    new_width: usize,
    new_height: usize,
) -> Result<Plane, AlignmentError> {
    if (src.width, src.height) == (new_width, new_height) {
        return Ok(src.clone());
    }
    let h_taps = RowTaps::new(bilinear_taps(src.width, new_width), src.width);
    let v_taps = bilinear_taps(src.height, new_height);
    const BAND: usize = 16;
    let mut out = vec![0.0f32; new_width * new_height];
    par_bands(
        &mut out,
        new_width,
        BAND,
        || vec![0.0f32; src.width],
        |y0, band, mid| {
            vectorized(
                #[inline(always)]
                || {
                    for (row, (start, weights)) in band.chunks_mut(new_width).zip(&v_taps[y0..]) {
                        mid.fill(0.0);
                        for (sy, &w) in (*start..).zip(weights) {
                            add_weighted_row(mid, src.row(sy), w);
                        }
                        h_taps.filter(mid, row);
                    }
                },
            )
        },
    );
    Ok(Plane::from_vec(new_width, new_height, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fused gray + resize as a plain per-pixel loop, to check against
    /// bit for bit.
    fn reference<T: Copy + Into<f32> + Sync>(
        data: &[T],
        src_w: usize,
        src_h: usize,
        dst_w: usize,
        dst_h: usize,
        max: f32,
    ) -> Plane {
        struct Filter<'a, T> {
            data: &'a [T],
            row_len: usize,
            luma: [f32; 3],
            h_taps: Vec<(usize, Vec<f32>)>,
        }
        impl<T: Copy + Into<f32>> Filter<'_, T> {
            #[inline(always)]
            fn row(
                &self,
                (start, weights): &(usize, Vec<f32>),
                acc: &mut [f32],
                mid: &mut [f32],
                out: &mut [f32],
            ) {
                let row_len = self.row_len;
                acc.fill(0.0);
                for (j, &w) in weights.iter().enumerate() {
                    let src = &self.data[(start + j) * row_len..(start + j + 1) * row_len];
                    for (a, &v) in acc.iter_mut().zip(src) {
                        *a += v.into() * w;
                    }
                }
                let k = self.luma;
                for (m, rgb) in mid.iter_mut().zip(acc.as_chunks::<3>().0) {
                    *m = rgb[0] * k[0] + rgb[1] * k[1] + rgb[2] * k[2];
                }
                for (o, (start, weights)) in out.iter_mut().zip(&self.h_taps) {
                    *o = mid[*start..*start + weights.len()]
                        .iter()
                        .zip(weights)
                        .map(|(v, w)| v * w)
                        .sum();
                }
            }
        }
        let filter = Filter {
            data,
            row_len: src_w * 3,
            luma: crate::REC709_LUMA.map(|w| w / max),
            h_taps: bilinear_taps(src_w, dst_w),
        };
        let v_taps = bilinear_taps(src_h, dst_h);
        const BAND: usize = 16;
        let mut out = vec![0.0f32; dst_w * dst_h];
        out.par_chunks_mut(dst_w * BAND).enumerate().for_each(|(b, band)| {
            let mut acc = vec![0.0f32; src_w * 3];
            let mut mid = vec![0.0f32; src_w];
            vectorized(
                #[inline(always)]
                || {
                    for (row, taps) in band.chunks_mut(dst_w).zip(&v_taps[b * BAND..]) {
                        filter.row(taps, &mut acc, &mut mid, row);
                    }
                },
            )
        });
        Plane::from_vec(dst_w, dst_h, out)
    }

    /// Pseudo-random channel values over the whole range of `T`.
    fn noise(len: usize, seed: u64) -> impl Iterator<Item = u64> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..len).map(move |_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        })
    }

    /// Integer factors (each strided case and one without), fractional,
    /// odd and tiny sizes, and upscaling.
    const SIZES: [(usize, usize, usize, usize); 22] = [
        (1024, 683, 256, 171),
        (2048, 1366, 512, 342),
        (1500, 1000, 512, 341),
        (600, 400, 200, 133),
        (128, 20, 64, 10),
        (192, 30, 64, 10),
        (320, 50, 64, 10),
        (384, 60, 64, 10),
        (448, 70, 64, 10),
        (512, 80, 64, 10),
        (576, 90, 64, 10),
        (1000, 667, 341, 227),
        (101, 77, 33, 25),
        (37, 23, 9, 5),
        (5, 3, 2, 1),
        (7, 1, 3, 1),
        (1, 9, 1, 4),
        (3, 3, 1, 1),
        (2, 2, 1, 1),
        (10, 7, 23, 15),
        (64, 64, 63, 31),
        (9, 1000, 2, 300),
    ];

    fn assert_bits(got: &Plane, want: &Plane, what: &str) {
        assert_eq!((got.width, got.height), (want.width, want.height), "{what}");
        if let Some(i) =
            (0..want.data.len()).find(|&i| got.data[i].to_bits() != want.data[i].to_bits())
        {
            panic!("{what}: pixel {i}: {} != {}", got.data[i], want.data[i]);
        }
    }

    /// Every value is the reference's, bit for bit, for 8- and 16-bit
    /// sources, noise, black and white.
    #[test]
    fn gray_resized_matches_the_reference_bit_for_bit() {
        fn check<T: Channel>(data: &[T], size: [usize; 4], max: f32, what: &str) {
            let [sw, sh, dw, dh] = size;
            let want = reference(data, sw, sh, dw, dh, max);
            assert_bits(&gray_resized(data, size, max), &want, &format!("{what} {size:?}"));
        }
        for (i, &(sw, sh, dw, dh)) in SIZES.iter().enumerate() {
            let n = sw * sh * 3;
            let size = [sw, sh, dw, dh];
            let u8s: Vec<u8> = noise(n, i as u64).map(|v| v as u8).collect();
            let u16s: Vec<u16> = noise(n, i as u64).map(|v| v as u16).collect();
            check(&u8s, size, 255.0, "u8 noise");
            check(&u16s, size, 65535.0, "u16 noise");
            check(&vec![0u8; n], size, 255.0, "u8 black");
            check(&vec![u16::MAX; n], size, 65535.0, "u16 white");
        }
    }

    /// At 8192 → 2048 (4×) every destination pixel but a few at each end
    /// is in the strided run.
    #[test]
    fn integer_downscale_is_one_run() {
        let taps = RowTaps::new(bilinear_taps(8192, 2048), 8192);
        assert_eq!(taps.stride, 4);
        assert!(taps.run.start <= 2 && taps.run.end >= 2046, "{:?}", taps.run);
        // A fractional factor has no repeating taps.
        let taps = RowTaps::new(bilinear_taps(6000, 2048), 6000);
        assert!(taps.run.len() <= 1, "{:?}", taps.run);
    }

    /// The fused gray + resize is the gray resized with `resize_plane`, for
    /// integer and fractional factors, 8- and 16-bit sources.
    #[test]
    fn fused_gray_resize_matches_gray_then_resize() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/spider.jpg");
        let rgb8 = DynamicImage::ImageRgb8(image::open(path).unwrap().to_rgb8());
        let rgb16 = DynamicImage::ImageRgb16(rgb8.to_rgb16());
        for image in [&rgb8, &rgb16] {
            let gray = image_to_grayscale(image).unwrap();
            let (w, h) = (gray.width, gray.height);
            for (dw, dh) in [(w / 4, h / 4), (w * 2 / 7, h * 2 / 7), (w, h)] {
                let fused = image_to_grayscale_resized(image, dw, dh).unwrap();
                let reference = resize_plane(&gray, dw, dh).unwrap();
                let worst = fused
                    .data
                    .iter()
                    .zip(&reference.data)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0f32, f32::max);
                assert!(worst < 1e-4, "{dw}x{dh}: max difference {worst}");
            }
        }
    }
}
