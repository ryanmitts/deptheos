//! How the image passes run, kept apart from what they compute: the
//! drivers that split rows across rayon and compile them with
//! [`vectorized`] ([`par_rows`], [`par_bands`]), and the row kernels the
//! separable filters share ([`add_weighted_row`], [`RowTaps`]).
//!
//! All are `#[inline(always)]`; closures handed to [`par_rows`] must be
//! too, for [`vectorized`] to compile them for the dispatched instruction
//! set.

use super::vectorized;
use rayon::prelude::*;

/// Run `row(y, out_row)` for every `width`-long row of `out`, in parallel
/// and vectorized. `row` must be `#[inline(always)]`.
#[inline(always)]
pub(crate) fn par_rows(out: &mut [f32], width: usize, row: impl Fn(usize, &mut [f32]) + Sync) {
    out.par_chunks_mut(width).enumerate().for_each(|(y, out_row)| {
        vectorized(
            #[inline(always)]
            || row(y, out_row),
        )
    });
}

/// Run `band(y0, out_band, scratch)` for every band of `rows` rows of
/// `out`, in parallel, each thread reusing its own `scratch()`. Not
/// vectorized: `band` wraps what it wants in [`vectorized`].
#[inline(always)]
pub(crate) fn par_bands<S>(
    out: &mut [f32],
    width: usize,
    rows: usize,
    scratch: impl Fn() -> S + Sync + Send,
    band: impl Fn(usize, &mut [f32], &mut S) + Sync + Send,
) {
    out.par_chunks_mut(width * rows)
        .enumerate()
        .for_each_init(scratch, |s, (b, out_band)| band(b * rows, out_band, s));
}

/// `acc[i] += src[i] · w`: one tap of a separable filter, a row at a time.
/// A multiply then an add, never fused, so every instruction set gives the
/// same values.
#[inline(always)]
pub(crate) fn add_weighted_row<T: Copy + Into<f32>>(acc: &mut [f32], src: &[T], w: f32) {
    for (a, &v) in acc.iter_mut().zip(src) {
        *a += v.into() * w;
    }
}

/// One destination pixel's taps: the first source pixel and the weights
/// from there on.
pub(crate) type Tap = (usize, Vec<f32>);

/// A resizer's horizontal taps, one [`Tap`] per destination pixel, and the
/// run where they repeat (same weights, each pixel `stride` source pixels
/// on). At an integer downscale the run is nearly every pixel and is summed
/// a tap at a time across pixels, which vectorizes: it halves the Keys
/// resize and takes 11–16 % off ECC's gray resize.
pub(crate) struct RowTaps {
    pub(super) taps: Vec<Tap>,
    /// Destination pixels `run.start..run.end` repeat `taps[run.start]`.
    pub(super) run: std::ops::Range<usize>,
    pub(super) stride: usize,
}

impl RowTaps {
    /// `taps` with the longest run repeating the middle pixel's, cut so the
    /// strided reads stay inside the `src_len` row.
    pub(crate) fn new(taps: Vec<Tap>, src_len: usize) -> Self {
        let none = |taps| Self { taps, run: 0..0, stride: 0 };
        let mid = taps.len() / 2;
        let (Some(&(m_start, ref weights)), Some(&(next, _))) = (taps.get(mid), taps.get(mid + 1))
        else {
            return none(taps);
        };
        let Some(stride) = next.checked_sub(m_start).filter(|&s| s > 0) else {
            return none(taps);
        };
        let same = |x: usize| {
            let (start, w) = &taps[x];
            // Bits, not values: the run must give the same sums.
            start.wrapping_sub(m_start) == (x.wrapping_sub(mid)).wrapping_mul(stride)
                && w.len() == weights.len()
                && w.iter().zip(weights).all(|(a, b)| a.to_bits() == b.to_bits())
        };
        if !same(mid + 1) {
            return none(taps);
        }
        let mut first = mid;
        while first > 0 && same(first - 1) {
            first -= 1;
        }
        let mut end = mid + 1;
        while end < taps.len() && same(end) {
            end += 1;
        }
        // The strided sum reads up to `groups` whole groups of `stride`.
        let groups = weights.len().div_ceil(stride);
        let first_start = taps[first].0;
        while end > first && first_start + (end - first - 1 + groups) * stride > src_len {
            end -= 1;
        }
        Self { taps, run: first..end, stride }
    }

    /// Filter `src` into `out`, each pixel's taps summed in order, in the
    /// run or not.
    #[inline(always)]
    pub(crate) fn filter(&self, src: &[f32], out: &mut [f32]) {
        let run = self.run.clone();
        for x in (0..run.start).chain(run.end..out.len()) {
            let (start, weights) = &self.taps[x];
            out[x] =
                src[*start..*start + weights.len()].iter().zip(weights).map(|(v, w)| v * w).sum();
        }
        if run.is_empty() {
            return;
        }
        let (start, weights) = &self.taps[run.start];
        let out = &mut out[run];
        let src = &src[*start..];
        match self.stride {
            2 => strided_taps::<2>(src, weights, out),
            3 => strided_taps::<3>(src, weights, out),
            4 => strided_taps::<4>(src, weights, out),
            5 => strided_taps::<5>(src, weights, out),
            6 => strided_taps::<6>(src, weights, out),
            8 => strided_taps::<8>(src, weights, out),
            _ => {
                for (x, o) in out.iter_mut().enumerate() {
                    let src = &src[x * self.stride..][..weights.len()];
                    *o = src.iter().zip(weights).map(|(v, w)| v * w).sum();
                }
            }
        }
    }
}

/// `out[x] = Σⱼ src[S·x + j]·weights[j]`, a tap at a time across `out`:
/// the same operations in the same order as each pixel's own sum. It starts
/// from the first product, as `Iterator::sum` (from `-0.0`) does. `src`
/// holds `S·(out.len() + ⌈taps / S⌉ − 1)` values.
#[inline(always)]
fn strided_taps<const S: usize>(src: &[f32], weights: &[f32], out: &mut [f32]) {
    let (groups, _) = src.as_chunks::<S>();
    let n = out.len();
    // Tap j = q·S + r: group q, lane r.
    for (q, w) in weights.chunks(S).enumerate() {
        let groups = &groups[q..q + n];
        if let Ok(w) = <&[f32; S]>::try_from(w) {
            // A whole group: every lane of each load is a tap.
            for (o, g) in out.iter_mut().zip(groups) {
                let mut acc = if q == 0 { g[0] * w[0] } else { *o + g[0] * w[0] };
                for r in 1..S {
                    acc += g[r] * w[r];
                }
                *o = acc;
            }
        } else {
            for (r, &w) in w.iter().enumerate() {
                for (o, g) in out.iter_mut().zip(groups) {
                    *o = if q == 0 && r == 0 { g[r] * w } else { *o + g[r] * w };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// `strided_taps` relies on it: a sum starts from `-0.0`.
    #[test]
    fn sums_start_from_negative_zero() {
        assert!([-0.0f32].iter().sum::<f32>().is_sign_negative());
        assert!(std::iter::empty::<f32>().sum::<f32>().is_sign_negative());
    }
}
