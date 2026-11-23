//! One level of ECC: the iterations that refine a warp of the frame onto
//! the template at one resolution ([`find_transform_ecc`]), with their
//! vectorized sums. The schedule over the levels is the parent's.

use crate::error::AlignmentError;
use crate::image_ops::{PackedWarp, gaussian_blur};
use crate::plane::Plane;
use crate::simd;
use nalgebra::{Affine2, SMatrix, SVector};
use rayon::prelude::*;
use rten_simd::ops::NumOps;
use rten_simd::{Isa, Simd, SimdOp};
use std::hint::black_box;

/// Pixels per rayon task: lanes sum in f32 within it, tasks merge in f64 in
/// row order. Part of the result: another size (or SIMD lane count) moves
/// the transforms by ~0.002 px.
const CHUNK: usize = 65536;

/// Pixels of a row [`ChunkFusedOp`]'s passes take in turn, so their reads
/// stay in L1. A multiple of every lane count; it does not regroup any sum.
const BLOCK: usize = 512;

/// Fail when less than this fraction of the template has a source pixel.
pub(crate) const MIN_OVERLAP: f64 = 0.25;

// Chunks merge, and the 6×6 system is solved, in f64.
type Mat6 = SMatrix<f64, 6, 6>;
type Vec6 = SVector<f64, 6>;

/// Pixel coordinates for the Jacobian, centered and normalized:
/// `u = (x − cx)/L`, `v = (y − cy)/L` with `L` the level's longer side, so
/// the system stays well conditioned (raw coordinates make the x and
/// constant columns almost parallel). The unknowns are `L·ΔA` and
/// `Δt + ΔA·c`; [`Coords::apply`] converts back.
#[derive(Clone, Copy)]
struct Coords {
    cx: f32,
    cy: f32,
    inv_l: f32,
}

impl Coords {
    fn new(width: usize, height: usize) -> Self {
        Self {
            cx: width as f32 / 2.0,
            cy: height as f32 / 2.0,
            inv_l: 1.0 / width.max(height).max(1) as f32,
        }
    }

    /// `u = (x − cx)·inv_l` for `x` in `0..width`, once per level.
    fn u_row(self, width: usize) -> Vec<f32> {
        let Self { cx, inv_l, .. } = self;
        (0..width).map(|x| (x as f32 - cx) * inv_l).collect()
    }

    /// Add the solved step to the warp, in pixel coordinates.
    fn apply(&self, warp: &mut Affine2<f32>, q: &Vec6) {
        let l = 1.0 / self.inv_l as f64;
        let (da, dc, db, dd) = (q[0] / l, q[1] / l, q[2] / l, q[3] / l);
        let (cx, cy) = (self.cx as f64, self.cy as f64);
        let m = warp.matrix_mut_unchecked();
        m[(0, 0)] += da as f32;
        m[(1, 0)] += dc as f32;
        m[(0, 1)] += db as f32;
        m[(1, 1)] += dd as f32;
        m[(0, 2)] += (q[4] - da * cx - db * cy) as f32;
        m[(1, 2)] += (q[5] - dc * cx - dd * cy) as f32;
    }
}

// ---------------------------------------------------------------------------
// One ECC iteration in a single sweep: warp, masked statistics, Hessian,
// projections and correlation, a row at a time through an L1-resident
// scratch row, so no warped planes are stored (that cost twice the time).
// ---------------------------------------------------------------------------

/// Sums over a block of rows. Image and template are shifted by a constant
/// `c` (≈ their mean) so the f32 lane sums stay small; the exact masked
/// means are restored in f64.
struct FusedAccum {
    hessian: Mat6,
    img_proj: Vec6,
    tmpl_proj: Vec6,
    /// Σ J over the valid pixels: the mean correction of the projections.
    jac_sum: Vec6,
    correlation: f64,
    count: f64,
    sum_t: f64,
    sum_t2: f64,
    sum_i: f64,
    sum_i2: f64,
}

impl FusedAccum {
    fn zero() -> Self {
        Self {
            hessian: Mat6::zeros(),
            img_proj: Vec6::zeros(),
            tmpl_proj: Vec6::zeros(),
            jac_sum: Vec6::zeros(),
            correlation: 0.0,
            count: 0.0,
            sum_t: 0.0,
            sum_t2: 0.0,
            sum_i: 0.0,
            sum_i2: f64::default(),
        }
    }

    fn merge(mut self, o: Self) -> Self {
        self.hessian += o.hessian;
        self.img_proj += o.img_proj;
        self.tmpl_proj += o.tmpl_proj;
        self.jac_sum += o.jac_sum;
        self.correlation += o.correlation;
        self.count += o.count;
        self.sum_t += o.sum_t;
        self.sum_t2 += o.sum_t2;
        self.sum_i += o.sum_i;
        self.sum_i2 += o.sum_i2;
        self
    }

    /// One pixel's terms in f64, for the pixels past a row's last vector.
    #[inline(always)]
    fn add_pixel(&mut self, m: f32, [gx, gy]: [f32; 2], [u, v]: [f32; 2], [i, t]: [f32; 2]) {
        let m = m as f64;
        let (gx, gy, u, v) = (gx as f64, gy as f64, u as f64, v as f64);
        let iz = m * i as f64;
        let tz = m * t as f64;
        let j = [gx * u, gy * u, gx * v, gy * v, gx, gy];
        for i in 0..6 {
            self.hessian[(i, i)] += j[i] * j[i];
            for jj in (i + 1)..6 {
                self.hessian[(i, jj)] += j[i] * j[jj];
            }
            self.img_proj[i] += j[i] * iz;
            self.tmpl_proj[i] += j[i] * tz;
            self.jac_sum[i] += j[i];
        }
        self.correlation += iz * tz;
        self.count += m;
        self.sum_t += tz;
        self.sum_t2 += tz * tz;
        self.sum_i += iz;
        self.sum_i2 += iz * iz;
    }
}

/// [`FusedAccum`]'s sums in one f32 per SIMD lane.
struct LaneSums<F> {
    hess: [[F; 6]; 6],
    ip: [F; 6],
    tp: [F; 6],
    js: [F; 6],
    corr: F,
    cnt: F,
    st: F,
    st2: F,
    si: F,
    si2: F,
}

impl<F: Simd<Elem = f32>> LaneSums<F> {
    fn new(zero: F) -> Self {
        let z = zero;
        Self {
            hess: [[z; 6]; 6],
            ip: [z; 6],
            tp: [z; 6],
            js: [z; 6],
            corr: z,
            cnt: z,
            st: z,
            st2: z,
            si: z,
            si2: z,
        }
    }

    /// Each sum's lanes added in f64, then `tail`'s sums merged in.
    #[allow(clippy::needless_range_loop)]
    #[inline(always)]
    fn finish(self, tail: FusedAccum) -> FusedAccum {
        let sum = |v: F| -> f64 { v.to_array().as_ref().iter().map(|&x| x as f64).sum() };
        let Self { hess, ip, tp, js, corr, cnt, st, st2, si, si2 } = self;
        let mut acc = FusedAccum::zero();
        for i in 0..6 {
            acc.hessian[(i, i)] = sum(hess[i][i]);
            for jj in (i + 1)..6 {
                acc.hessian[(i, jj)] = sum(hess[i][jj]);
            }
            acc.img_proj[i] = sum(ip[i]);
            acc.tmpl_proj[i] = sum(tp[i]);
            acc.jac_sum[i] = sum(js[i]);
        }
        acc.correlation = sum(corr);
        acc.count = sum(cnt);
        acc.sum_t = sum(st);
        acc.sum_t2 = sum(st2);
        acc.sum_i = sum(si);
        acc.sum_i2 = sum(si2);
        acc.merge(tail)
    }
}

/// One task of [`fused_iteration`]: template rows `row_start..row_end`,
/// each warped from the frame ([`PackedWarp::row`]) and summed into a
/// [`FusedAccum`]. `ur` is each column's `u` ([`Coords`]), `shift` the
/// constant `c` ([`FusedAccum`]).
struct ChunkFusedOp<'a> {
    warp: &'a PackedWarp<'a>,
    template: &'a [f32],
    ur: &'a [f32],
    shift: f32,
    coords: Coords,
    width: usize,
    row_start: usize,
    row_end: usize,
}

/// One row's buffers in [`ChunkFusedOp`].
struct RowBufs<'a> {
    img: &'a [f32],
    gx: &'a [f32],
    gy: &'a [f32],
    valid: &'a [f32],
    tmpl: &'a [f32],
    /// `u` of each pixel (the same in every row).
    u: &'a [f32],
}

impl RowBufs<'_> {
    /// The Jacobian `[gx·u, gy·u, gx·v, gy·v, gx, gy]` at `x`.
    #[inline(always)]
    fn jacobian<I: Isa>(&self, isa: I, v: I::F32, x: usize) -> [I::F32; 6] {
        let ops = isa.f32();
        let (gx, gy, u) =
            (ops.load(&self.gx[x..]), ops.load(&self.gy[x..]), ops.load(&self.u[x..]));
        [ops.mul(gx, u), ops.mul(gy, u), ops.mul(gx, v), ops.mul(gy, v), gx, gy]
    }

    /// Image and template at `x`, shifted and masked, and the mask.
    #[inline(always)]
    fn values<I: Isa>(&self, isa: I, shift: I::F32, x: usize) -> (I::F32, I::F32, I::F32) {
        let ops = isa.f32();
        let m = ops.load(&self.valid[x..]);
        let iz = ops.mul(m, ops.sub(ops.load(&self.img[x..]), shift));
        let tz = ops.mul(m, ops.sub(ops.load(&self.tmpl[x..]), shift));
        (iz, tz, m)
    }
}

/// Accumulate rows `$rows` of the upper-triangular Hessian in one
/// register-bounded pass; `black_box` keeps `sums` in memory between passes.
macro_rules! accum_hess_rows {
    ($ops:expr, $isa:expr, $bufs:expr, $v:expr, $xs:expr, $sums:expr, [$($row:literal),+]) => {{
        let s = black_box(&mut $sums);
        let mut h = [$(s.hess[$row]),+];
        for x0 in $xs {
            let j = $bufs.jacobian($isa, $v, x0);
            let mut r = 0;
            $(
                for jj in $row..6 {
                    h[r][jj] = $ops.mul_add(j[$row], j[jj], h[r][jj]);
                }
                r += 1;
            )+
            let _ = r;
        }
        [$(s.hess[$row]),+] = h;
    }};
}

impl SimdOp for ChunkFusedOp<'_> {
    type Output = FusedAccum;

    #[allow(clippy::needless_range_loop)]
    #[inline(always)]
    fn eval<I: Isa>(self, isa: I) -> FusedAccum {
        let ops = isa.f32();
        let lanes = ops.len();
        let w = self.width;
        let simd_w = (w / lanes) * lanes;
        let shift_v = ops.splat(self.shift);
        let Coords { cx, cy, inv_l } = self.coords;
        let ur = &self.ur[..simd_w];

        let mut row_bufs = vec![0.0f32; 4 * w];
        let (img, rest) = row_bufs.split_at_mut(w);
        let (gxr, rest) = rest.split_at_mut(w);
        let (gyr, valid) = rest.split_at_mut(w);

        let mut sums = LaneSums::new(ops.zero());
        let mut tail = FusedAccum::zero();

        for row in self.row_start..self.row_end {
            // 1. Warp the row: image, gradients and the valid mask.
            self.warp.row(isa, row, img, gxr, gyr, valid);
            let base = row * w;
            let tmpl_row = &self.template[base..base + w];
            let v_f = (row as f32 - cy) * inv_l;
            let v = ops.splat(v_f);
            let bufs = RowBufs { img, gx: gxr, gy: gyr, valid, tmpl: tmpl_row, u: ur };
            // 2. The sums in four passes per block, each with only the
            // accumulators that fit in registers (one pass spills). Terms
            // arrive in the same order, so the values match one pass.
            for block in (0..simd_w).step_by(BLOCK) {
                let xs = (block..(block + BLOCK).min(simd_w)).step_by(lanes);
                accum_hess_rows!(ops, isa, bufs, v, xs.clone(), sums, [0, 1]);
                accum_hess_rows!(ops, isa, bufs, v, xs.clone(), sums, [2, 3, 4, 5]);
                let s = black_box(&mut sums);
                let (mut ip, mut tp) = (s.ip, s.tp);
                for x0 in xs.clone() {
                    let j = bufs.jacobian(isa, v, x0);
                    let (iz, tz, _) = bufs.values(isa, shift_v, x0);
                    for i in 0..6 {
                        ip[i] = ops.mul_add(j[i], iz, ip[i]);
                        tp[i] = ops.mul_add(j[i], tz, tp[i]);
                    }
                }
                (s.ip, s.tp) = (ip, tp);
                let s = black_box(&mut sums);
                let (mut js, [mut corr, mut cnt, mut st, mut st2, mut si, mut si2]) =
                    (s.js, [s.corr, s.cnt, s.st, s.st2, s.si, s.si2]);
                for x0 in xs {
                    let j = bufs.jacobian(isa, v, x0);
                    let (iz, tz, m) = bufs.values(isa, shift_v, x0);
                    for i in 0..6 {
                        js[i] = ops.add(js[i], j[i]);
                    }
                    corr = ops.mul_add(iz, tz, corr);
                    cnt = ops.add(cnt, m);
                    st = ops.add(st, tz);
                    st2 = ops.mul_add(tz, tz, st2);
                    si = ops.add(si, iz);
                    si2 = ops.mul_add(iz, iz, si2);
                }
                s.js = js;
                [s.corr, s.cnt, s.st, s.st2, s.si, s.si2] = [corr, cnt, st, st2, si, si2];
            }
            // 3. The pixels past the last full vector, in f64.
            for x in simd_w..w {
                let shifted = [img[x] - self.shift, tmpl_row[x] - self.shift];
                let uv = [(x as f32 - cx) * inv_l, v_f];
                tail.add_pixel(valid[x], [gxr[x], gyr[x]], uv, shifted);
            }
        }
        sums.finish(tail)
    }
}

/// One iteration's sums over the whole level, in parallel over row blocks.
fn fused_iteration(
    warp: &PackedWarp<'_>,
    template: &[f32],
    ur: &[f32],
    width: usize,
    height: usize,
    shift: f32,
    coords: Coords,
) -> FusedAccum {
    let rows_per_chunk = (CHUNK / width.max(1)).max(1);
    // Merged in row order, not with `reduce`, so the result does not depend
    // on rayon's scheduling.
    let partials: Vec<FusedAccum> = (0..height)
        .into_par_iter()
        .step_by(rows_per_chunk)
        .map(|row_start| {
            simd::dispatch(ChunkFusedOp {
                warp,
                template,
                ur,
                shift,
                coords,
                width,
                row_start,
                row_end: (row_start + rows_per_chunk).min(height),
            })
        })
        .collect();
    let mut acc = partials.into_iter().fold(FusedAccum::zero(), FusedAccum::merge);
    for i in 0..6 {
        for j in (i + 1)..6 {
            acc.hessian[(j, i)] = acc.hessian[(i, j)];
        }
    }
    acc
}

// ---------------------------------------------------------------------------
// Fused gradient computation + packing
// ---------------------------------------------------------------------------

/// Central-difference gradients (clamped at edges) packed with `src` as
/// `[src, gx, gy, 0.0]` for [`PackedWarp`], in one pass.
fn pack_with_gradients(src: &Plane) -> Vec<[f32; 4]> {
    let (w, h) = (src.width, src.height);
    let s = &src.data;
    let mut out = vec![[0.0f32; 4]; s.len()];
    if w == 0 || h == 0 {
        return out;
    }

    out.par_chunks_mut(w).enumerate().for_each(|(y, out_row)| {
        let row_a = if y == 0 { 0 } else { (y - 1) * w };
        let row_b = if y == h - 1 { (h - 1) * w } else { (y + 1) * w };
        let row = y * w;

        // Edge pixels. A 1 px wide level has no horizontal gradient; `h == 1`
        // gives zero since `row_a == row_b`.
        let gx0 = if w >= 2 { 0.5 * (s[row + 1] - s[row]) } else { 0.0 };
        let gy0 = 0.5 * (s[row_b] - s[row_a]);
        out_row[0] = [s[row], gx0, gy0, 0.0];
        if w >= 2 {
            let gxw = 0.5 * (s[row + w - 1] - s[row + w - 2]);
            let gyw = 0.5 * (s[row_b + w - 1] - s[row_a + w - 1]);
            out_row[w - 1] = [s[row + w - 1], gxw, gyw, 0.0];
        }

        // Interior pixels.
        if w > 2 {
            let mid = &s[row + 1..row + w - 1];
            let (left, right) = (&s[row..row + w - 2], &s[row + 2..row + w]);
            let (above, below) = (&s[row_a + 1..row_a + w - 1], &s[row_b + 1..row_b + w - 1]);
            let dst = &mut out_row[1..w - 1];
            simd::vectorized(
                #[inline(always)]
                || {
                    for (o, (((&m, (&r, &l)), &b), &a)) in dst
                        .iter_mut()
                        .zip(mid.iter().zip(right.iter().zip(left)).zip(below).zip(above))
                    {
                        *o = [m, 0.5 * (r - l), 0.5 * (b - a), 0.0];
                    }
                },
            );
        }
    });

    out
}

// ---------------------------------------------------------------------------
// Criteria
// ---------------------------------------------------------------------------

/// When one level's ECC iterations stop: once a step is below `max_step`
/// (converged), or after `max_iter` iterations; a stall stops them sooner.
#[derive(Debug, Clone)]
pub(crate) struct Criteria {
    /// Converged once an update moves every corner by less than this many
    /// level pixels (negative disables the test). The step, not ρ, says the
    /// warp has settled: ρ flattens long before the warp stops moving.
    pub max_step: f64,
    pub max_iter: i32,
}

// ---------------------------------------------------------------------------
// Main ECC function
// ---------------------------------------------------------------------------

/// Result of ECC alignment.
pub(crate) struct EccResult {
    /// ρ at `transform`; when converged, at the warp one sub-tolerance
    /// update before it.
    pub correlation: f64,
    pub transform: Affine2<f32>,
    /// The step test was met, rather than a stall or the iteration limit.
    pub converged: bool,
    pub iterations: i32,
}

/// The input image blurred and its gradients. Computed once per frame
/// ahead of the chain (`EccAligner::prepare`); the blur then serves as the
/// next frame's template.
pub(crate) struct EccInput {
    pub blurred: Plane,
    /// The blur and its gradients interleaved for [`PackedWarp`].
    pub packed: Vec<[f32; 4]>,
}

impl EccInput {
    /// `image` blurred with `blur_sigma` (none when ≤ 0), and its gradients.
    pub fn new(image: &Plane, blur_sigma: f32) -> Self {
        let blurred = blur(image, blur_sigma);
        let packed = pack_with_gradients(&blurred);
        Self { blurred, packed }
    }
}

/// Stride of the template subsample whose mean is the shift `c` of
/// [`FusedAccum`]; a prime, so it does not alias with the row length.
const SHIFT_SAMPLE_STRIDE: usize = 61;

/// Largest distance a corner of the `width`×`height` level moves between
/// warps `a` and `b`, in level pixels.
fn corner_displacement(a: &Affine2<f32>, b: &Affine2<f32>, width: usize, height: usize) -> f64 {
    let d = b.matrix().cast::<f64>() - a.matrix().cast::<f64>();
    let (w, h) = (width as f64, height as f64);
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .into_iter()
        .map(|(x, y)| {
            let dx = d[(0, 0)] * x + d[(0, 1)] * y + d[(0, 2)];
            let dy = d[(1, 0)] * x + d[(1, 1)] * y + d[(1, 2)];
            // Not `hypot`, whose last bit differs between platforms.
            (dx * dx + dy * dy).sqrt()
        })
        .fold(0.0, f64::max)
}

fn is_finite(t: &Affine2<f32>) -> bool {
    t.matrix().iter().all(|v| v.is_finite())
}

/// Iterations without a new best ρ after which the loop returns its best
/// warp. Near the optimum the update can cycle above the step tolerance
/// (pixels crossing the overlap's edge change the sums discontinuously).
pub(crate) const STALL_ITERATIONS: i32 = 3;

/// Find the affine transform that aligns `input` with the template, by
/// Enhanced Correlation Coefficient maximization (Evangelidis & Psarakis,
/// "Parametric Image Alignment Using Enhanced Correlation Coefficient
/// Maximization", IEEE PAMI 2008; the λ and update follow OpenCV's
/// `findTransformECC`). Template and input arrive already blurred.
///
/// A sub-tolerance update is applied and ends the loop; a stall or the
/// iteration limit returns the best warp measured.
#[tracing::instrument(skip_all)]
pub(crate) fn find_transform_ecc(
    template_blurred: &Plane,
    input: &EccInput,
    seed_warp_matrix: Option<&Affine2<f32>>,
    criteria: Criteria,
) -> Result<EccResult, AlignmentError> {
    let mut warp_matrix = seed_warp_matrix.copied().unwrap_or_else(Affine2::identity);
    if !is_finite(&warp_matrix) {
        return Err(AlignmentError::EccFailed("non-finite initial warp".to_string()));
    }

    let (tmpl_w, tmpl_h) = (template_blurred.width, template_blurred.height);
    let pixel_count = tmpl_h * tmpl_w;
    let tmpl_slice = &template_blurred.data;
    let coords = Coords::new(tmpl_w, tmpl_h);
    let ur = coords.u_row(tmpl_w);

    // A sampled template mean is close enough for both images ([`FusedAccum`]).
    let shift = {
        let n = tmpl_slice.len().div_ceil(SHIFT_SAMPLE_STRIDE).max(1);
        let sum: f64 = tmpl_slice.iter().step_by(SHIFT_SAMPLE_STRIDE).map(|&v| v as f64).sum();
        (sum / n as f64) as f32
    };

    // ρ and the best warp measured so far.
    let mut best = (f64::NEG_INFINITY, warp_matrix);
    let mut since_best = 0;

    // One iteration from `warp_matrix`; `last` marks the final one.
    let mut iterate = |warp_matrix: Affine2<f32>, last: bool| -> Flow {
        let warp = PackedWarp::new(&input.packed, input.blurred.width, &warp_matrix);
        let acc = fused_iteration(&warp, tmpl_slice, &ur, tmpl_w, tmpl_h, shift, coords);

        // Only pixels whose source lies inside the input count, for both
        // images' statistics; counting the zero border biases the fixed
        // point toward the center, which a chain accumulates as drift.
        if acc.count < MIN_OVERLAP * pixel_count as f64 {
            let percent = 100.0 * acc.count / pixel_count as f64;
            return Flow::Fail(AlignmentError::LittleOverlap { percent });
        }
        let count = acc.count.max(1.0);
        let (im, tm) = (acc.sum_i / count, acc.sum_t / count);
        let img_std = (acc.sum_i2 / count - im * im).max(0.0).sqrt();
        let tmpl_std = (acc.sum_t2 / count - tm * tm).max(0.0).sqrt();
        let sqrt_n = acc.count.sqrt();
        let img_norm = img_std * sqrt_n;
        let tmp_norm = tmpl_std * sqrt_n;
        // Zero-mean the projections and the correlation exactly.
        let img_proj = acc.img_proj - acc.jac_sum * im;
        let tmpl_proj = acc.tmpl_proj - acc.jac_sum * tm;
        let correlation = acc.correlation - acc.count * im * tm;

        let Some(hessian_inv) = acc.hessian.try_inverse() else {
            return Flow::Fail(AlignmentError::EccFailed("Hessian matrix is singular".into()));
        };

        let rho = correlation / (img_norm * tmp_norm);
        if !rho.is_finite() {
            return Flow::Fail(AlignmentError::EccFailed(
                "non-finite correlation coefficient".into(),
            ));
        }
        if rho > best.0 {
            best = (rho, warp_matrix);
            since_best = 0;
        } else {
            since_best += 1;
        }
        if last || since_best >= STALL_ITERATIONS {
            return Flow::Stop;
        }

        let img_proj_hessian = hessian_inv * img_proj;
        let lambda_n = (img_norm * img_norm) - img_proj.dot(&img_proj_hessian);
        let lambda_d = correlation - tmpl_proj.dot(&img_proj_hessian);
        if lambda_d.is_nan() || lambda_d <= 0.0 {
            return Flow::Fail(AlignmentError::Uncorrelated);
        }
        let lambda = lambda_n / lambda_d;

        // J^T · (λ·tmpl_zm − img_zm), from the accumulated projections.
        let error_projection = tmpl_proj * lambda - img_proj;
        let delta_p = hessian_inv * error_projection;
        let mut next = warp_matrix;
        coords.apply(&mut next, &delta_p);
        if !is_finite(&next) {
            return Flow::Fail(AlignmentError::EccFailed("non-finite warp update".into()));
        }
        let step = corner_displacement(&warp_matrix, &next, tmpl_w, tmpl_h);
        if criteria.max_step >= 0.0 && step < criteria.max_step {
            Flow::Converged { rho, warp: next }
        } else {
            Flow::Next(next)
        }
    };

    let mut iterations = 0;
    for iteration in 1..=criteria.max_iter {
        iterations = iteration;
        match iterate(warp_matrix, iteration == criteria.max_iter) {
            Flow::Next(next) => warp_matrix = next,
            Flow::Converged { rho, warp } => {
                return Ok(EccResult {
                    correlation: rho,
                    transform: warp,
                    converged: true,
                    iterations,
                });
            }
            Flow::Stop => break,
            Flow::Fail(e) => return Err(e),
        }
    }

    let (correlation, transform) = best;
    Ok(EccResult { correlation, transform, converged: false, iterations })
}

/// What [`find_transform_ecc`]'s loop does after an iteration.
enum Flow {
    /// Measure again at this warp.
    Next(Affine2<f32>),
    /// Below the step tolerance: `warp` (updated) is the answer, with the
    /// `rho` measured before the update.
    Converged {
        rho: f64,
        warp: Affine2<f32>,
    },
    /// Stalled or out of iterations: the best warp measured is the answer.
    Stop,
    Fail(AlignmentError),
}

/// ECC's pre-blur: Gaussian `sigma`, or a copy when `sigma` ≤ 0.
pub(crate) fn blur(image: &Plane, sigma: f32) -> Plane {
    if sigma > 0.0 { gaussian_blur(image, sigma) } else { image.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{compute_rms, translation, warped};

    const MAX_RMS: f64 = 0.1;
    const ECC_ITERATIONS: i32 = 50;

    /// Every iteration runs (no convergence test).
    fn criteria() -> Criteria {
        Criteria { max_step: -1.0, max_iter: ECC_ITERATIONS }
    }

    /// A natural image at 216×216; periodic patterns give false basins.
    fn load_test_image() -> Plane {
        let img = crate::test_util::fixture("fruits.jpg", 216, 216).to_luma32f();
        let (w, h) = img.dimensions();
        Plane::from_vec(w as usize, h as usize, img.into_raw())
    }

    fn check_result(result: &EccResult, ground_truth: &Affine2<f32>, label: &str) {
        let rms = compute_rms(&result.transform, ground_truth);
        assert!(rms < MAX_RMS, "{label}: RMS {rms:.6} exceeds threshold {MAX_RMS}");
        assert!(result.correlation > 0.9, "{label}: correlation {:.4} too low", result.correlation);
    }

    #[test]
    fn test_translation() {
        let test_img = load_test_image();
        for (k, (tx, ty)) in
            [(12.0f32, 17.0f32), (18.5, 11.0), (10.0, 19.5)].into_iter().enumerate()
        {
            let ground_truth = translation(tx, ty);
            let frame = warped(&test_img, &ground_truth, 200, 200);
            let result = find_transform_ecc(
                &blur(&frame, 0.5),
                &EccInput::new(&test_img, 0.5),
                None,
                criteria(),
            )
            .expect("ECC should converge");

            check_result(&result, &ground_truth, &format!("translation[{k}]"));
        }
    }

    #[test]
    fn test_affine() {
        let test_img = load_test_image();
        // Mild enough for the single-level basin.
        let cases = [
            (1.03f32, 0.02f32, 14.0f32, -0.015f32, 0.97f32, 12.0f32),
            (0.98, -0.015, 15.0, 0.01, 1.02, 13.0),
            (1.025, 0.01, 11.0, -0.02, 0.985, 16.0),
        ];
        for (k, (a, b, tx, c, d, ty)) in cases.into_iter().enumerate() {
            let ground_truth = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
                a, b, tx, c, d, ty, 0.0, 0.0, 1.0,
            ));
            let frame = warped(&test_img, &ground_truth, 200, 200);
            let result = find_transform_ecc(
                &blur(&frame, 0.5),
                &EccInput::new(&test_img, 0.5),
                None,
                criteria(),
            )
            .expect("ECC should converge");

            check_result(&result, &ground_truth, &format!("affine[{k}]"));
        }
    }

    /// The same image as template and input.
    #[test]
    fn test_identity() {
        let test_img = load_test_image();
        let result = find_transform_ecc(
            &blur(&test_img, 0.5),
            &EccInput::new(&test_img, 0.5),
            None,
            criteria(),
        )
        .expect("ECC should converge");

        let rms = compute_rms(&result.transform, &Affine2::identity());
        assert!(rms < 0.05, "identity: RMS {rms:.6} should be near zero");
        assert!(
            result.correlation > 0.98,
            "identity: correlation {:.4} should be near 1.0",
            result.correlation
        );
    }

    /// The step test ends the loop once the warp settles.
    #[test]
    fn step_test_converges() {
        let test_img = load_test_image();
        let ground_truth = translation(8.0, 9.0);
        let template = blur(&warped(&test_img, &ground_truth, 200, 200), 0.5);
        let input = EccInput::new(&test_img, 0.5);
        let settled = Criteria { max_step: 0.01, max_iter: ECC_ITERATIONS };
        let result = find_transform_ecc(&template, &input, None, settled).unwrap();
        assert!(result.converged && result.iterations < ECC_ITERATIONS, "{}", result.iterations);
        check_result(&result, &ground_truth, "converged");
    }

    /// A frame reaching the input's edge cycles above the tolerance; the
    /// loop returns its best warp, at that warp's ρ.
    #[test]
    fn stalled_loop_returns_its_best_warp() {
        let test_img = load_test_image();
        let ground_truth = translation(12.0, 17.0);
        let template = blur(&warped(&test_img, &ground_truth, 200, 200), 0.5);
        let input = EccInput::new(&test_img, 0.5);
        let settled = Criteria { max_step: 0.01, max_iter: ECC_ITERATIONS };
        let result = find_transform_ecc(&template, &input, None, settled).unwrap();
        assert!(!result.converged && result.iterations < ECC_ITERATIONS, "{}", result.iterations);
        check_result(&result, &ground_truth, "stalled");
        // Measuring the returned warp again gives the reported ρ.
        let again = Criteria { max_step: -1.0, max_iter: 1 };
        let remeasured =
            find_transform_ecc(&template, &input, Some(&result.transform), again).unwrap();
        assert_eq!(remeasured.correlation, result.correlation);
        assert_eq!(remeasured.transform, result.transform);
    }

    /// Out of iterations: not converged, and a measured warp.
    #[test]
    fn exhausted_budget_returns_a_measured_warp() {
        let test_img = load_test_image();
        let template = blur(&warped(&test_img, &translation(8.0, 9.0), 200, 200), 0.5);
        let input = EccInput::new(&test_img, 0.5);
        let short = Criteria { max_step: 0.01, max_iter: 1 };
        let result = find_transform_ecc(&template, &input, None, short).unwrap();
        assert!(!result.converged && result.iterations == 1);
        assert_eq!(result.transform, Affine2::identity());
    }

    /// A non-finite seed is an error, not a NaN transform.
    #[test]
    fn non_finite_seed_is_an_error() {
        let test_img = load_test_image();
        let input = EccInput::new(&test_img, 0.5);
        let seed = translation(f32::NAN, 0.0);
        assert!(
            find_transform_ecc(&blur(&test_img, 0.5), &input, Some(&seed), criteria()).is_err()
        );
    }

    /// The sums in one pass: what [`ChunkFusedOp`]'s split passes must match.
    struct OnePass<'a>(ChunkFusedOp<'a>);

    impl SimdOp for OnePass<'_> {
        type Output = FusedAccum;

        #[allow(clippy::needless_range_loop)]
        #[inline(always)]
        fn eval<I: Isa>(self, isa: I) -> FusedAccum {
            let OnePass(op) = self;
            let ops = isa.f32();
            let lanes = ops.len();
            let w = op.width;
            let simd_w = (w / lanes) * lanes;
            let shift_v = ops.splat(op.shift);
            let Coords { cx, cy, inv_l } = op.coords;
            let inv_l_v = ops.splat(inv_l);
            let iota = simd::iota_f32(isa);

            let mut img = vec![0.0f32; w];
            let mut gxr = vec![0.0f32; w];
            let mut gyr = vec![0.0f32; w];
            let mut valid = vec![0.0f32; w];

            let mut s = LaneSums::new(ops.zero());
            let mut tail = FusedAccum::zero();

            for row in op.row_start..op.row_end {
                op.warp.row(isa, row, &mut img, &mut gxr, &mut gyr, &mut valid);
                let base = row * w;
                let tmpl_row = &op.template[base..base + w];
                let v_f = (row as f32 - cy) * inv_l;
                let v = ops.splat(v_f);
                for x0 in (0..simd_w).step_by(lanes) {
                    let gx = ops.load(&gxr[x0..]);
                    let gy = ops.load(&gyr[x0..]);
                    let m = ops.load(&valid[x0..]);
                    let u = ops.mul(ops.add(iota, ops.splat(x0 as f32 - cx)), inv_l_v);
                    let iz = ops.mul(m, ops.sub(ops.load(&img[x0..]), shift_v));
                    let tz = ops.mul(m, ops.sub(ops.load(&tmpl_row[x0..]), shift_v));

                    let j =
                        [ops.mul(gx, u), ops.mul(gy, u), ops.mul(gx, v), ops.mul(gy, v), gx, gy];
                    for i in 0..6 {
                        s.hess[i][i] = ops.mul_add(j[i], j[i], s.hess[i][i]);
                        for jj in (i + 1)..6 {
                            s.hess[i][jj] = ops.mul_add(j[i], j[jj], s.hess[i][jj]);
                        }
                        s.ip[i] = ops.mul_add(j[i], iz, s.ip[i]);
                        s.tp[i] = ops.mul_add(j[i], tz, s.tp[i]);
                        s.js[i] = ops.add(s.js[i], j[i]);
                    }
                    s.corr = ops.mul_add(iz, tz, s.corr);
                    s.cnt = ops.add(s.cnt, m);
                    s.st = ops.add(s.st, tz);
                    s.st2 = ops.mul_add(tz, tz, s.st2);
                    s.si = ops.add(s.si, iz);
                    s.si2 = ops.mul_add(iz, iz, s.si2);
                }
                // The last pixels, and the lanes' sums below, as the split
                // passes take them.
                for x in simd_w..w {
                    let shifted = [img[x] - op.shift, tmpl_row[x] - op.shift];
                    let uv = [(x as f32 - cx) * inv_l, v_f];
                    tail.add_pixel(valid[x], [gxr[x], gyr[x]], uv, shifted);
                }
            }
            s.finish(tail)
        }
    }

    fn accum_bits(a: &FusedAccum) -> Vec<u64> {
        let scalars = [a.correlation, a.count, a.sum_t, a.sum_t2, a.sum_i, a.sum_i2];
        (a.hessian.iter().chain(&a.img_proj).chain(&a.tmpl_proj).chain(&a.jac_sum))
            .chain(&scalars)
            .map(|v| v.to_bits())
            .collect()
    }

    /// Split passes match one pass bit for bit, dispatched and generic, with
    /// scalar tails, several blocks and warp edges mid-row.
    #[test]
    fn split_passes_match_one_pass() {
        use crate::test_util::scale_rotate;
        use rten_simd::isa::GenericIsa;
        let pattern = |w: usize, h: usize, k: f32| {
            let data = (0..w * h).map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                0.5 + 0.3 * (x * 0.07 + k).sin() * (y * 0.11 - k).cos() + 0.1 * (x * 0.31).sin()
            });
            Plane::from_vec(w, h, data.collect())
        };
        for (w, h) in [(1101usize, 40usize), (37, 23), (2051, 6), (515, 17)] {
            let input = EccInput::new(&pattern(w, h, 0.3), 0.5);
            let template = blur(&pattern(w, h, 0.0), 0.5);
            let coords = Coords::new(w, h);
            let ur = coords.u_row(w);
            let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
            for t in
                [translation(-7.3, 2.1), scale_rotate(1.01, 3.0, cx, cy), translation(5.6, -3.2)]
            {
                let warp = PackedWarp::new(&input.packed, w, &t);
                let op = || ChunkFusedOp {
                    warp: &warp,
                    template: &template.data,
                    ur: &ur,
                    shift: 0.43,
                    coords,
                    width: w,
                    row_start: 0,
                    row_end: h,
                };
                let want = accum_bits(&OnePass(op()).dispatch());
                assert_eq!(accum_bits(&op().dispatch()), want, "dispatched {w}x{h}");
                let want = accum_bits(&OnePass(op()).eval(GenericIsa::new()));
                assert_eq!(accum_bits(&op().eval(GenericIsa::new())), want, "generic {w}x{h}");
                let count = op().dispatch().count;
                assert!(count > 0.0 && count < (w * h) as f64, "edges inside {w}x{h}: {count}");
            }
        }
    }

    #[test]
    fn pack_with_gradients_matches_separate_planes() {
        let img = load_test_image();
        let blurred = blur(&img, 0.5);
        let packed = pack_with_gradients(&blurred);
        let w = blurred.width;
        let h = blurred.height;
        let src = &blurred.data;
        for y in 0..h {
            let ym1 = y.saturating_sub(1);
            let yp1 = (y + 1).min(h - 1);
            for x in 0..w {
                let xm1 = x.saturating_sub(1);
                let xp1 = (x + 1).min(w - 1);
                let gx = (src[y * w + xp1] - src[y * w + xm1]) * 0.5;
                let gy = (src[yp1 * w + x] - src[ym1 * w + x]) * 0.5;
                assert_eq!(packed[y * w + x], [src[y * w + x], gx, gy, 0.0]);
            }
        }
    }
}
