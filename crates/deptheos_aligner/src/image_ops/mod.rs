//! The aligners' CPU image processing: gray conversion, resizing, blur
//! and warping.
//!
//! - ECC: Rec. 709 gray at every level ([`gray_resize`]), blurred
//!   ([`blur`]), warped each iteration ([`PackedWarp`]).
//! - Grid: one equal-weight gray map, Keys cubic ([`keys_resize`]).
//!
//! [`rows`] holds what the passes share. The warp is explicit SIMD
//! because LLVM does not vectorize it well (ADR-112). Every pass gives the
//! same values on every instruction set: element-wise loops, sums in a
//! fixed order, no fused multiply-add.

mod blur;
mod gray_resize;
mod keys_resize;
mod rows;

pub(crate) use blur::gaussian_blur;
#[cfg(test)]
pub(crate) use gray_resize::image_to_grayscale;
pub(crate) use gray_resize::{image_to_grayscale_resized, resize_plane};
#[cfg(test)]
pub(crate) use keys_resize::image_to_gray_mean;
pub(crate) use keys_resize::{GrayRows, resize_keys_scaled};

use crate::error::AlignmentError;
pub(crate) use crate::simd::vectorized;
use crate::simd::{blend4, iota_f32, transpose_store4};
use image::DynamicImage;
use nalgebra::Affine2;
use rten_simd::ops::{FloatOps, NumOps, ToFloat};
use rten_simd::{Isa, Simd};

/// The aligners read 8- or 16-bit RGB only (`deptheos_core::io` loads every
/// source as RGB); converting here could silently quantize.
fn unsupported_layout(image: &DynamicImage) -> AlignmentError {
    AlignmentError::ProcessingFailed(format!(
        "unsupported pixel layout {:?}; convert to RGB8 or RGB16 first",
        image.color()
    ))
}

// ---------------------------------------------------------------------------
// Image warping (parallel bilinear backward-mapping)
// ---------------------------------------------------------------------------

/// Interleave three same-sized planes for [`PackedWarp`].
#[cfg(test)]
pub(crate) fn pack_planes(
    a: &crate::plane::Plane,
    b: &crate::plane::Plane,
    c: &crate::plane::Plane,
) -> Vec<[f32; 4]> {
    use rayon::prelude::*;
    let mut out = vec![[0.0f32; 4]; a.data.len()];
    out.par_chunks_mut(1 << 14)
        .zip(a.data.par_chunks(1 << 14))
        .zip(b.data.par_chunks(1 << 14))
        .zip(c.data.par_chunks(1 << 14))
        .for_each(|(((out, a), b), c)| {
            for (((o, &a), &b), &c) in out.iter_mut().zip(a).zip(b).zip(c) {
                *o = [a, b, c, 0.0];
            }
        });
    out
}

/// Bilinear backward warp of a packed `[image, gx, gy, _]` source: a
/// pixel's taps are four 16-byte loads instead of twelve scattered ones.
pub(crate) struct PackedWarp<'a> {
    src: &'a [[f32; 4]],
    src_w: usize,
    src_h: usize,
    m: nalgebra::Matrix3<f32>,
}

impl<'a> PackedWarp<'a> {
    pub(crate) fn new(src: &'a [[f32; 4]], src_w: usize, transform: &Affine2<f32>) -> Self {
        // The SIMD interior computes tap indices in i32.
        assert!(src.len() <= i32::MAX as usize, "source too large for the warp");
        Self { src, src_w, src_h: src.len() / src_w.max(1), m: *transform.matrix() }
    }

    /// Warp row `dst_y` of the destination: the three planes and the valid
    /// mask.
    #[inline(always)]
    pub(crate) fn row<I: Isa>(
        &self,
        isa: I,
        dst_y: usize,
        a: &mut [f32],
        b: &mut [f32],
        c: &mut [f32],
        v: &mut [f32],
    ) {
        let (lo, hi) = self.edges(dst_y, a, b, c, v);
        self.interior(isa, dst_y, lo, hi, a, b, c, v);
    }

    /// Source coordinates of destination pixel `(dx, dst_y)`.
    #[inline(always)]
    fn source(&self, dx: usize, dst_y: usize) -> (f32, f32) {
        let m = &self.m;
        let (dx, dy) = (dx as f32, dst_y as f32);
        let sx = m[(0, 0)] * dx + (m[(0, 1)] * dy + m[(0, 2)]);
        let sy = m[(1, 0)] * dx + (m[(1, 1)] * dy + m[(1, 2)]);
        (sx, sy)
    }

    /// Fill the pixels outside the interior with checked taps and return
    /// the interior `[lo, hi)`, where no tap is clamped: found in f64, then
    /// confirmed at both ends (the f32 maps are monotone in `dx`).
    fn edges(
        &self,
        dst_y: usize,
        row_a: &mut [f32],
        row_b: &mut [f32],
        row_c: &mut [f32],
        row_v: &mut [f32],
    ) -> (usize, usize) {
        let (src, src_w, src_h, m) = (self.src, self.src_w, self.src_h, &self.m);
        let (bound_x, bound_y) = (src_w as f32, src_h as f32);
        let dy = dst_y as f32;
        let sx_base = m[(0, 1)] * dy + m[(0, 2)];
        let sy_base = m[(1, 1)] * dy + m[(1, 2)];
        let n = row_a.len().min(row_b.len()).min(row_c.len()).min(row_v.len());
        let within = |dx: usize, bound_x: f32, bound_y: f32| {
            let (sx, sy) = self.source(dx, dst_y);
            sx >= 0.0 && sy >= 0.0 && sx < bound_x && sy < bound_y
        };
        let inside = |dx: usize| within(dx, bound_x, bound_y);
        let (inner_x, inner_y) = (bound_x - 1.0, bound_y - 1.0);
        let inner = |dx: usize| within(dx, inner_x, inner_y);
        let (mut lo, mut hi) = interior_range(
            [
                (m[(0, 0)] as f64, sx_base as f64, inner_x as f64),
                (m[(1, 0)] as f64, sy_base as f64, inner_y as f64),
            ],
            n,
        );
        while lo < hi && !inner(lo) {
            lo += 1;
        }
        while lo < hi && !inner(hi - 1) {
            hi -= 1;
        }
        for dx in (0..lo).chain(hi..n) {
            if !inside(dx) {
                row_a[dx] = 0.0;
                row_b[dx] = 0.0;
                row_c[dx] = 0.0;
                row_v[dx] = 0.0;
                continue;
            }
            let (sx, sy) = self.source(dx, dst_y);
            let x0 = sx.floor() as usize;
            let y0 = sy.floor() as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let y1 = (y0 + 1).min(src_h - 1);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let w = weights(fx, fy);
            let p = blend4(
                w,
                [
                    src[y0 * src_w + x0],
                    src[y0 * src_w + x1],
                    src[y1 * src_w + x0],
                    src[y1 * src_w + x1],
                ],
            );
            (row_a[dx], row_b[dx], row_c[dx], row_v[dx]) = (p[0], p[1], p[2], 1.0);
        }
        (lo, hi)
    }

    /// The interior `[lo, hi)` of a row, a vector of pixels at a time, with
    /// the scalar path's arithmetic operation for operation. The top-left
    /// tap is clamped to the source less its last column and row, so the
    /// unchecked loads are in bounds whatever the float math gives; `edges`
    /// makes the clamp a no-op.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn interior<I: Isa>(
        &self,
        isa: I,
        dst_y: usize,
        lo: usize,
        hi: usize,
        row_a: &mut [f32],
        row_b: &mut [f32],
        row_c: &mut [f32],
        row_v: &mut [f32],
    ) {
        if lo >= hi {
            return;
        }
        let (fo, io) = (isa.f32(), isa.i32());
        let lanes = fo.len();
        let (src, src_w, m) = (self.src, self.src_w, &self.m);
        // An interior has a source at least two pixels wide and high, so
        // these do not underflow.
        let (max_x, max_y) = (src_w - 2, self.src_h - 2);
        let dy = dst_y as f32;
        let a_v = fo.splat(m[(0, 0)]);
        let c_v = fo.splat(m[(1, 0)]);
        let sxb = fo.splat(m[(0, 1)] * dy + m[(0, 2)]);
        let syb = fo.splat(m[(1, 1)] * dy + m[(1, 2)]);
        let iota = iota_f32(isa);
        let one = fo.one();
        let zero_i = io.zero();
        let max_xi = io.splat(max_x as i32);
        let max_yi = io.splat(max_y as i32);
        let w_i = io.splat(src_w as i32);
        // Source pixels `i` and `i + 1`.
        let pair = |i: usize| -> [[f32; 4]; 2] {
            // SAFETY: `i` is clamped to a pixel left of the source's last
            // column and above its last row (see above).
            unsafe { [*src.get_unchecked(i), *src.get_unchecked(i + 1)] }
        };

        let mut x = lo;
        while x + lanes <= hi {
            // 1. Source positions, as `source` computes them.
            let dx = fo.add(iota, fo.splat(x as f32));
            let sx = fo.add(fo.mul(a_v, dx), sxb);
            let sy = fo.add(fo.mul(c_v, dx), syb);
            // 2. Whole and fractional parts (positions are ≥ 0, so
            // truncation is the floor) and the weights.
            let xi = fo.to_int_trunc(sx);
            let yi = fo.to_int_trunc(sy);
            let fx = fo.sub(sx, io.to_float(xi));
            let fy = fo.sub(sy, io.to_float(yi));
            let (gx, gy) = (fo.sub(one, fx), fo.sub(one, fy));
            let w00 = fo.mul(gy, gx).to_array();
            let w10 = fo.mul(gy, fx).to_array();
            let w01 = fo.mul(fy, gx).to_array();
            let w11 = fo.mul(fy, fx).to_array();
            // 3. The clamped top-left tap's index and the one below it.
            let x0 = io.min(io.max(xi, zero_i), max_xi);
            let y0 = io.min(io.max(yi, zero_i), max_yi);
            let i00 = io.add(io.mul(y0, w_i), x0);
            let (i00, i01) = (i00.to_array(), io.add(i00, w_i).to_array());
            // 4. Four pixels at a time, blended and stored with one
            // transpose (every lane count is a multiple of 4).
            let (oa, ob) = (&mut row_a[x..x + lanes], &mut row_b[x..x + lanes]);
            let oc = &mut row_c[x..x + lanes];
            for g in (0..lanes).step_by(4) {
                let p = std::array::from_fn(|k| {
                    let l = g + k;
                    let w = [w00.as_ref()[l], w10.as_ref()[l], w01.as_ref()[l], w11.as_ref()[l]];
                    let [p00, p10] = pair(i00.as_ref()[l] as usize);
                    let [p01, p11] = pair(i01.as_ref()[l] as usize);
                    blend4(w, [p00, p10, p01, p11])
                });
                transpose_store4(p, &mut oa[g..g + 4], &mut ob[g..g + 4], &mut oc[g..g + 4]);
            }
            x += lanes;
        }
        // The last pixels, fewer than a vector, one at a time.
        for dx in x..hi {
            let (sx, sy) = self.source(dx, dst_y);
            // `sx`, `sy` ≥ 0 here, so truncation is the floor.
            let (xt, yt) = (sx as usize, sy as usize);
            let w = weights(sx - xt as f32, sy - yt as f32);
            let i00 = yt.min(max_y) * src_w + xt.min(max_x);
            let ([p00, p10], [p01, p11]) = (pair(i00), pair(i00 + src_w));
            let p = blend4(w, [p00, p10, p01, p11]);
            (row_a[dx], row_b[dx], row_c[dx]) = (p[0], p[1], p[2]);
        }
        row_v[lo..hi].fill(1.0);
    }
}

/// The bilinear weights `[w00, w10, w01, w11]` of fractions `fx`, `fy`.
#[inline(always)]
fn weights(fx: f32, fy: f32) -> [f32; 4] {
    let (gx, gy) = (1.0 - fx, 1.0 - fy);
    [gy * gx, gy * fx, fy * gx, fy * fx]
}

/// The run `[lo, hi)` of `dx` in `0..n` where every map `k·dx + base` in
/// `maps` lies in `[0, bound)`, in f64 (callers confirm the ends).
fn interior_range(maps: [(f64, f64, f64); 2], n: usize) -> (usize, usize) {
    let (mut lo, mut hi) = (0.0f64, n as f64);
    for (k, base, bound) in maps {
        if k > 0.0 {
            lo = lo.max((-base / k).ceil());
            hi = hi.min(((bound - base) / k).ceil());
        } else if k < 0.0 {
            lo = lo.max(((bound - base) / k).floor() + 1.0);
            hi = hi.min((-base / k).floor() + 1.0);
        } else if base < 0.0 || base >= bound {
            return (0, 0);
        }
    }
    if lo.is_nan() || hi.is_nan() || lo >= hi {
        return (0, 0);
    }
    (lo.max(0.0) as usize, hi.min(n as f64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plane::Plane;
    use rten_simd::SimdOp;

    /// Both resizers put destination pixel `X` at source position
    /// `(X + 0.5)/s − 0.5`, which the level ↔ full-resolution conversions
    /// rely on.
    #[test]
    fn resizers_place_pixel_centers() {
        let (w, h) = (400usize, 8usize);
        let ramp = Plane::from_vec(w, h, (0..w * h).map(|i| (i % w) as f32 / w as f32).collect());
        let s = 0.25f32;
        let bilinear = resize_plane(&ramp, 100, h).unwrap();
        let keys = resize_keys_scaled(GrayRows::Plane(&ramp), s).unwrap();
        for x in [10usize, 50, 90] {
            let want = ((x as f32 + 0.5) / s - 0.5) / w as f32;
            assert!(
                (bilinear.at(x, 4) - want).abs() < 1e-5,
                "bilinear {x}: {} vs {want}",
                bilinear.at(x, 4)
            );
            assert!((keys.at(x, 1) - want).abs() < 1e-5, "keys {x}: {} vs {want}", keys.at(x, 1));
        }
    }

    fn test_planes(w: usize, h: usize) -> [Plane; 3] {
        std::array::from_fn(|k| {
            let data = (0..w * h).map(|i| ((i * (7 + k) + i / w * 13) % 101) as f32 / 101.0);
            Plane::from_vec(w, h, data.collect())
        })
    }

    /// The packed warp matches the scalar warp (`test_util::warp_planes`)
    /// bit for bit, dispatched and generic: odd widths, clamped taps,
    /// mirrored maps, edges mid-row, and sources one or two pixels wide.
    #[test]
    fn packed_warp_matches_three_planes() {
        use crate::test_util::{scale_rotate, translation, warp_planes};
        use rten_simd::isa::GenericIsa;

        /// Every row of the warp as bits, starting as NaN so an unwritten
        /// pixel shows.
        struct Rows<'a>(&'a PackedWarp<'a>, usize, usize);
        impl SimdOp for Rows<'_> {
            type Output = Vec<u32>;

            #[inline(always)]
            fn eval<I: Isa>(self, isa: I) -> Self::Output {
                let Rows(warp, w, h) = self;
                let mut out = [(); 4].map(|_| vec![f32::NAN; w * h]);
                let [a, b, c, v] = &mut out;
                let rows = a.chunks_mut(w).zip(b.chunks_mut(w)).zip(c.chunks_mut(w));
                for (y, (((a, b), c), v)) in rows.zip(v.chunks_mut(w)).enumerate() {
                    warp.row(isa, y, a, b, c, v);
                }
                out.iter().flatten().map(|v| v.to_bits()).collect()
            }
        }
        #[rustfmt::skip]
        let shear = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            1.01, 0.02, -3.7,
            -0.015, 0.99, 5.2,
            0.0, 0.0, 1.0,
        ));
        let mirror = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            -1.0, 0.0, 202.6, 0.0, 1.0, 0.3, 0.0, 0.0, 1.0,
        ));
        let transforms = [
            translation(0.0, 0.0),
            translation(0.6, 0.4),
            translation(-7.3, 2.1),
            translation(5.6, -3.2),
            scale_rotate(1.01, 3.0, 100.0, 30.0),
            scale_rotate(0.5, -20.0, 40.0, 20.0),
            mirror,
            shear,
        ];
        let (mut inside, mut outside) = (false, false);
        for (src_w, src_h) in [(300usize, 200usize), (203, 61), (1, 5), (2, 2), (9, 1)] {
            let [pa, pb, pc] = test_planes(src_w, src_h);
            let packed = pack_planes(&pa, &pb, &pc);
            for (w, h) in [(src_w, src_h), (203, 61), (37, 9), (211, 13), (1, 3)] {
                for t in &transforms {
                    let ([a, b, c], v) = warp_planes([&pa, &pb, &pc], t, w, h);
                    inside |= v.data.contains(&1.0);
                    outside |= v.data.contains(&0.0);
                    let want: Vec<u32> =
                        [a, b, c, v].iter().flat_map(|p| &p.data).map(|v| v.to_bits()).collect();
                    let warp = PackedWarp::new(&packed, src_w, t);
                    let what = format!("{src_w}x{src_h} to {w}x{h} {t:?}");
                    assert!(Rows(&warp, w, h).dispatch() == want, "dispatched {what}");
                    assert!(Rows(&warp, w, h).eval(GenericIsa::new()) == want, "generic {what}");
                }
            }
        }
        assert!(inside && outside, "both inside and outside");
    }
}
