//! The grid search's scorer: a candidate's difference metric over the
//! sample points, a few candidates at a time, and the match statistics the
//! found step is judged by.
//!
//! The vector score is bit for bit the scalar one ([`Similarity::back`],
//! [`Plane::bilinear`]): no fused multiply-add, f64 sums added in order.

use super::{MAX_MATCH_RATIO, Samples, Similarity};
use crate::FEATURELESS_STD;
use crate::error::AlignmentError;
use crate::plane::Plane;
use rten_simd::ops::{FloatOps, MaskOps, NumOps, ToFloat};
use rten_simd::{Isa, Mask, Simd, SimdOp};

/// Standard deviation of the frame-minus-reference difference over the
/// sample points that land inside the reference; `f32::MAX` when none do.
#[cfg(test)]
fn difference_score(reference: &Plane, candidate: &Similarity, points: &Samples) -> f32 {
    crate::simd::dispatch(Score { reference, candidates: [candidate], points })[0]
}

/// [`difference_score`] of each of `candidates` into `scores`; a score does
/// not depend on which others it is grouped with.
pub(super) fn difference_scores(
    reference: &Plane,
    candidates: &[Similarity],
    points: &Samples,
    scores: &mut [f32],
) {
    let (groups, rest) = candidates.as_chunks::<SCORE_GROUP>();
    let (out, out_rest) = scores.as_chunks_mut::<SCORE_GROUP>();
    for (group, out) in groups.iter().zip(out) {
        let candidates = group.each_ref();
        *out = crate::simd::dispatch(Score { reference, candidates, points });
    }
    for (candidate, out) in rest.iter().zip(out_rest) {
        *out = crate::simd::dispatch(Score { reference, candidates: [candidate], points })[0];
    }
}

/// Candidates per task: [`SCORE_GROUP`] when every thread can take two
/// groups, otherwise one, so a small round still spreads over the threads.
pub(super) fn score_task_size(total: usize) -> usize {
    if total >= 2 * SCORE_GROUP * rayon::current_num_threads() { SCORE_GROUP } else { 1 }
}

/// Candidates [`Score`] takes at once: interleaving four chains of
/// dependent f64 additions keeps the adder busy.
const SCORE_GROUP: usize = 4;

/// [`difference_score`] of `K` candidates: a vector of samples at a time
/// is read from the reference, then the differences are added to each
/// candidate's f64 sums one sample at a time, in order.
///
/// A sample outside the reference adds `+0.0` instead of being skipped,
/// which changes nothing: a sum starting at `+0.0` never becomes `−0.0`.
struct Score<'a, const K: usize> {
    reference: &'a Plane,
    candidates: [&'a Similarity; K],
    points: &'a Samples,
}

/// One candidate's constants, as vectors for [`BilinearSampler`].
struct Candidate<S> {
    sn: S,
    cs: S,
    inv_m: S,
    src_cx: S,
    src_cy: S,
    dst_cx: S,
    dst_cy: S,
}

impl<S: Simd> Candidate<S> {
    #[inline(always)]
    fn splat<O: NumOps<f32, Simd = S>>(
        fo: O,
        c: &Similarity,
        sn: f32,
        cs: f32,
        inv_m: f32,
    ) -> Self {
        Self {
            sn: fo.splat(sn),
            cs: fo.splat(cs),
            inv_m: fo.splat(inv_m),
            src_cx: fo.splat(c.src_cx),
            src_cy: fo.splat(c.src_cy),
            dst_cx: fo.splat(c.dst_cx),
            dst_cy: fo.splat(c.dst_cy),
        }
    }
}

/// Vector bilinear sampler over `reference`, bit for bit
/// [`Similarity::back`] and [`Plane::bilinear`].
struct BilinearSampler<'a, I: Isa> {
    data: &'a [f32],
    width: usize,
    zero: I::F32,
    max_x: I::F32,
    max_y: I::F32,
    zero_i: I::I32,
    one_i: I::I32,
    last_x: I::I32,
    last_y: I::I32,
    width_i: I::I32,
    taps: [[f32; crate::simd::MAX_F32_LANES]; 4],
}

impl<'a, I: Isa> BilinearSampler<'a, I> {
    #[inline(always)]
    fn new(isa: I, reference: &'a Plane) -> Self {
        let (fo, io) = (isa.f32(), isa.i32());
        let (width, height) = (reference.width, reference.height);
        // The tap indices are computed in i32.
        assert!(reference.data.len() <= i32::MAX as usize, "reference too large for the score");
        Self {
            data: &reference.data,
            width,
            zero: fo.zero(),
            max_x: fo.splat((width - 1) as f32),
            max_y: fo.splat((height - 1) as f32),
            zero_i: io.zero(),
            one_i: io.splat(1),
            last_x: io.splat(width as i32 - 1),
            last_y: io.splat(height as i32 - 1),
            width_i: io.splat(width as i32),
            taps: [[0.0f32; crate::simd::MAX_F32_LANES]; 4],
        }
    }

    /// Back-transform `(x, y)` through `c` and sample `reference`. `inside`
    /// is `None` when every lane is within `[0, w-1) × [0, h-1)` (the fast
    /// path), else the mask of lanes within `[0, w-1] × [0, h-1]`.
    #[inline(always)]
    fn sample(
        &mut self,
        isa: I,
        c: &Candidate<I::F32>,
        x: I::F32,
        y: I::F32,
    ) -> (I::F32, Option<I::M32>) {
        let (fo, io, mo) = (isa.f32(), isa.i32(), isa.m32());
        let lanes = fo.len();
        let data = self.data;
        let width = self.width;
        // `back`; its `−dx·sn + dy·cs` is `dy·cs − dx·sn` bit for bit.
        let dx = fo.sub(x, c.dst_cx);
        let dy = fo.sub(y, c.dst_cy);
        let sx = fo.add(fo.mul(fo.add(fo.mul(dx, c.cs), fo.mul(dy, c.sn)), c.inv_m), c.src_cx);
        let sy = fo.add(fo.mul(fo.sub(fo.mul(dy, c.cs), fo.mul(dx, c.sn)), c.inv_m), c.src_cy);
        // `bilinear`: read when inside `[0, w−1] × [0, h−1]` (false for
        // NaN), where truncation is the scalar `as usize`.
        let xi = fo.to_int_trunc(sx);
        let yi = fo.to_int_trunc(sy);
        let fx = fo.sub(sx, io.to_float(xi));
        let fy = fo.sub(sy, io.to_float(yi));
        let above_zero = mo.and(fo.ge(sx, self.zero), fo.ge(sy, self.zero));
        // Usually every lane is short of the last column and row, and
        // needs no clamps.
        let interior = mo.and(above_zero, mo.and(fo.lt(sx, self.max_x), fo.lt(sy, self.max_y)));
        let inside = if mo.all(interior) {
            let corner = io.add(io.mul(yi, self.width_i), xi).to_array();
            for (l, &k) in corner.as_ref().iter().enumerate() {
                let k = k as usize;
                // SAFETY: column `k % width` is below `width − 1`
                // and row `k / width` below `height − 1`.
                unsafe {
                    self.taps[0][l] = *data.get_unchecked(k);
                    self.taps[1][l] = *data.get_unchecked(k + 1);
                    self.taps[2][l] = *data.get_unchecked(k + width);
                    self.taps[3][l] = *data.get_unchecked(k + width + 1);
                }
            }
            None
        } else {
            let inside = mo.and(above_zero, mo.and(fo.le(sx, self.max_x), fo.le(sy, self.max_y)));
            // No-ops inside; outside they keep the loads in bounds.
            let x0 = io.min(io.max(xi, self.zero_i), self.last_x);
            let y0 = io.min(io.max(yi, self.zero_i), self.last_y);
            let x1 = io.min(io.add(x0, self.one_i), self.last_x);
            let y1 = io.min(io.add(y0, self.one_i), self.last_y);
            let (r0, r1) = (io.mul(y0, self.width_i), io.mul(y1, self.width_i));
            let index = [io.add(r0, x0), io.add(r0, x1), io.add(r1, x0), io.add(r1, x1)];
            for (t, index) in self.taps.iter_mut().zip(index) {
                for (t, &k) in t.iter_mut().zip(index.to_array().as_ref()) {
                    // SAFETY: every index is that of a column and a row clamped
                    // into the reference, which has at most i32::MAX elements.
                    *t = unsafe { *data.get_unchecked(k as usize) };
                }
            }
            Some(inside)
        };
        let [p00, p10, p01, p11] = self.taps.each_ref().map(|t| fo.load(&t[..lanes]));
        let top = fo.add(p00, fo.mul(fo.sub(p10, p00), fx));
        let bottom = fo.add(p01, fo.mul(fo.sub(p11, p01), fx));
        let canvas = fo.add(top, fo.mul(fo.sub(bottom, top), fy));
        (canvas, inside)
    }
}

impl<const K: usize> SimdOp for Score<'_, K> {
    type Output = [f32; K];

    #[inline(always)]
    fn eval<I: Isa>(self, isa: I) -> [f32; K] {
        let Score { reference, candidates, points } = self;
        let fo = isa.f32();
        let lanes = fo.len();
        let mut sampler = BilinearSampler::new(isa, reference);
        let trig = candidates.map(|c| (c.sin_cos(), c.inv_magnification()));
        let consts: [Candidate<_>; K] = std::array::from_fn(|k| {
            let (c, ((sn, cs), inv_m)) = (candidates[k], trig[k]);
            Candidate::splat(fo, c, sn, cs, inv_m)
        });
        let zero = fo.zero();

        let mut count = [0usize; K];
        let mut sum = [0.0f64; K];
        let mut sum_sq = [0.0f64; K];
        // Differences into `diffs` (zero outside), then into the f64 sums.
        let full = points.len() - points.len() % lanes;
        let mut diffs = [[0.0f32; crate::simd::MAX_F32_LANES]; K];
        for i in (0..full).step_by(lanes) {
            let x = fo.load(&points.xs[i..i + lanes]);
            let y = fo.load(&points.ys[i..i + lanes]);
            let value = fo.load(&points.values[i..i + lanes]);
            for ((c, diffs), count) in consts.iter().zip(&mut diffs).zip(&mut count) {
                let (canvas, inside) = sampler.sample(isa, c, x, y);
                let d = fo.sub(value, canvas);
                match inside {
                    None => {
                        fo.store(d, &mut diffs[..lanes]);
                        *count += lanes;
                    }
                    Some(inside) => {
                        fo.store(fo.select(d, zero, inside), &mut diffs[..lanes]);
                        *count += inside.to_array().into_iter().filter(|&b| b).count();
                    }
                }
            }
            // The K chains of additions interleave.
            #[allow(clippy::needless_range_loop)]
            for l in 0..lanes {
                for k in 0..K {
                    let d = diffs[k][l] as f64;
                    sum[k] += d;
                    sum_sq[k] += d * d;
                }
            }
        }
        // The samples past the last vector, scalar; then the score.
        std::array::from_fn(|k| {
            let (c, ((sn, cs), inv_m)) = (candidates[k], trig[k]);
            let (mut count, mut sum, mut sum_sq) = (count[k], sum[k], sum_sq[k]);
            for (x, y, value) in points.iter().skip(full) {
                let (sx, sy) = c.back(x, y, sn, cs, inv_m);
                if let Some(canvas) = reference.bilinear(sx, sy) {
                    let d = (value - canvas) as f64;
                    count += 1;
                    sum += d;
                    sum_sq += d * d;
                }
            }
            if count == 0 {
                return f32::MAX;
            }
            let count = count as f64;
            let mean = sum / count;
            ((sum_sq / count - mean * mean).max(0.0)).sqrt() as f32
        })
    }
}

/// How well a candidate matches, over the sample points that land inside
/// the reference ([`match_quality`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct Match {
    /// [`difference_score`].
    pub(super) score: f32,
    /// What two unrelated images would score: the root of the summed
    /// variances.
    pub(super) unrelated: f32,
    /// The previous frame's standard deviation over the sample positions.
    pub(super) reference_std: f32,
}

impl Match {
    /// Why the frame is rejected, if it is; a featureless previous frame
    /// gives no verdict.
    pub(super) fn rejection(self) -> Option<AlignmentError> {
        if self.reference_std < FEATURELESS_STD || self.score <= MAX_MATCH_RATIO * self.unrelated {
            return None;
        }
        Some(AlignmentError::NoMatch {
            difference: f64::from(100.0 * self.score / self.unrelated),
            max: f64::from(100.0 * MAX_MATCH_RATIO),
        })
    }
}

/// [`difference_score`] with the statistics [`Match`] reports; `None` when
/// no sample lands in the reference. Once a frame, so a plain loop.
pub(super) fn match_quality(
    reference: &Plane,
    candidate: &Similarity,
    points: &Samples,
) -> Option<Match> {
    let (sn, cs) = candidate.sin_cos();
    let inv_m = candidate.inv_magnification();
    let [mut count, mut n, mut n2, mut r, mut r2, mut d, mut d2] = [0.0f64; 7];
    for (x, y, value) in points.iter() {
        let (sx, sy) = candidate.back(x, y, sn, cs, inv_m);
        let Some(canvas) = reference.bilinear(sx, sy) else {
            continue;
        };
        let (v, c) = (value as f64, canvas as f64);
        count += 1.0;
        n += v;
        n2 += v * v;
        r += c;
        r2 += c * c;
        d += v - c;
        d2 += (v - c) * (v - c);
    }
    if count <= 0.0 {
        return None;
    }
    let var = |s: f64, s2: f64| (s2 / count - (s / count).powi(2)).max(0.0);
    let (new_var, reference_var) = (var(n, n2), var(r, r2));
    Some(Match {
        score: var(d, d2).sqrt() as f32,
        unrelated: (new_var + reference_var).sqrt() as f32,
        reference_std: reference_var.sqrt() as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{IRect, MIN_LAYER_SIZE, Pyramid, sample_points};
    use crate::image_ops::image_to_gray_mean;
    use crate::test_util::{fixture, translation, warped};

    const W: u32 = 1600;
    const H: u32 = 1200;

    fn spider_gray() -> Plane {
        image_to_gray_mean(&fixture("spider.jpg", W, H)).expect("gray")
    }

    /// The scalar score: each sample carried back, read and added alone.
    fn difference_score_reference(
        reference: &Plane,
        candidate: &Similarity,
        points: &Samples,
    ) -> f32 {
        let (sn, cs) = candidate.sin_cos();
        let inv_m = candidate.inv_magnification();
        let mut count = 0.0f64;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        for (x, y, value) in points.iter() {
            let (sx, sy) = candidate.back(x, y, sn, cs, inv_m);
            if let Some(canvas) = reference.bilinear(sx, sy) {
                let d = (value - canvas) as f64;
                count += 1.0;
                sum += d;
                sum_sq += d * d;
            }
        }
        if count <= 0.0 {
            return f32::MAX;
        }
        let mean = sum / count;
        ((sum_sq / count - mean * mean).max(0.0)).sqrt() as f32
    }

    /// The vector score is the scalar one bit for bit, dispatched and
    /// generic: real and tiny planes, tails, samples outside or on the
    /// last row and column, zero and NaN magnification, zero differences.
    #[test]
    fn vector_score_matches_the_scalar_score_bitwise() {
        use rten_simd::isa::GenericIsa;
        let spider = spider_gray();
        let shifted = warped(&spider, &translation(7.3, -4.1), W as usize, H as usize);
        let reference = Pyramid::build(spider.clone(), MIN_LAYER_SIZE);
        let new = Pyramid::build(shifted, MIN_LAYER_SIZE);
        let mut pairs: Vec<(Plane, Plane)> =
            reference.layers.iter().cloned().zip(new.layers.iter().cloned()).collect();
        let ramp = |w: usize, h: usize, k: usize| {
            let data = (0..w * h).map(|i| ((i * (7 + k) + i / w * 13) % 101) as f32 / 101.0);
            Plane::from_vec(w, h, data.collect())
        };
        for (w, h) in [(1, 1), (2, 2), (1, 9), (9, 1), (3, 5), (17, 13), (37, 41)] {
            pairs.push((ramp(w, h, 0), ramp(w, h, 1)));
            pairs.push((ramp(w, h, 2), ramp(w, h, 2)));
        }
        let mut seed = 7u32;
        let mut unit = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut checked = 0;
        for (r, n) in &pairs {
            let (w, h) = (n.width as i64, n.height as i64);
            let whole = IRect { x0: 0, y0: 0, x1: w, y1: h };
            let middle = IRect { x0: w / 10, y0: h / 10, x1: w - w / 10, y1: h - h / 10 };
            for (rect, target) in [(whole, 10_000), (whole, 97), (middle, 1001), (middle, 3)] {
                for ratio in [1.0, 0.97, 1.5] {
                    let points = sample_points(n, rect, target, ratio);
                    let mut candidates = vec![
                        Similarity::identity(n.width, n.height),
                        Similarity {
                            magnification: 0.0,
                            ..Similarity::identity(n.width, n.height)
                        },
                        Similarity {
                            magnification: f32::NAN,
                            ..Similarity::identity(n.width, n.height)
                        },
                        Similarity {
                            dst_cx: 3.0 * w as f32,
                            ..Similarity::identity(n.width, n.height)
                        },
                    ];
                    for _ in 0..12 {
                        let mut c = Similarity::identity(n.width, n.height);
                        c.dst_cx += (unit() - 0.5) * 0.4 * w as f32;
                        c.dst_cy += (unit() - 0.5) * 0.4 * h as f32;
                        c.rotation = (unit() - 0.5) * 20.0;
                        c.magnification = 0.9 + 0.2 * unit();
                        candidates.push(c);
                    }
                    let want: Vec<u32> = candidates
                        .iter()
                        .map(|c| difference_score_reference(r, c, &points).to_bits())
                        .collect();
                    for (c, &want) in candidates.iter().zip(&want) {
                        let score = || Score { reference: r, candidates: [c], points: &points };
                        assert_eq!(score().dispatch()[0].to_bits(), want, "dispatched {c:?}");
                        let generic = score().eval(GenericIsa::new())[0];
                        assert_eq!(generic.to_bits(), want, "generic {c:?}");
                        assert_eq!(difference_score(r, c, &points).to_bits(), want);
                        checked += 1;
                    }
                    // In groups, with a remainder, on both instruction sets.
                    let group: [_; SCORE_GROUP] = std::array::from_fn(|k| &candidates[k + 1]);
                    let score = || Score { reference: r, candidates: group, points: &points };
                    let bits = |s: [f32; SCORE_GROUP]| s.map(f32::to_bits);
                    assert_eq!(bits(score().dispatch()), want[1..=SCORE_GROUP], "dispatched");
                    assert_eq!(bits(score().eval(GenericIsa::new())), want[1..=SCORE_GROUP]);
                    let mut scores = vec![0.0; candidates.len() - 1];
                    difference_scores(r, &candidates[1..], &points, &mut scores);
                    assert!(scores.iter().map(|s| s.to_bits()).eq(want[1..].iter().copied()));
                }
            }
        }
        assert!(checked > 1000, "{checked}");
    }
}
