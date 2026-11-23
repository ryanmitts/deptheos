//! SIMD dispatch for the aligner, on the widest instruction set the CPU has
//! (`rten-simd` picks it at run time), and the few vector helpers with no
//! portable form.
//!
//! - [`dispatch`] runs a [`SimdOp`]: the ECC sums, the grid's scores, the
//!   warp.
//! - [`vectorized`] runs a plain closure compiled for that instruction set,
//!   so LLVM's own vectorized loops use the wide vectors too.

use rten_simd::ops::NumOps;
use rten_simd::{Isa, SimdOp};

/// Maximum `f32` lanes across all supported SIMD instruction sets (AVX-512 = 16).
pub(crate) const MAX_F32_LANES: usize = 16;

/// Run `op` on the CPU's widest instruction set.
#[inline(always)]
pub(crate) fn dispatch<Op: SimdOp>(op: Op) -> Op::Output {
    op.dispatch()
}

/// Run `f` compiled for the widest SIMD instruction set, so its loops use
/// AVX2 or AVX-512 rather than SSE2. `f` must be `#[inline(always)]`.
/// Element-wise loops give the same values on every instruction set (ECC's
/// lane sums do not; see `ecc::solve::CHUNK`).
#[inline(always)]
pub(crate) fn vectorized<R>(f: impl FnOnce() -> R) -> R {
    struct Op<F>(F);
    impl<R, F: FnOnce() -> R> SimdOp for Op<F> {
        type Output = R;

        #[inline(always)]
        fn eval<I: Isa>(self, _: I) -> R {
            (self.0)()
        }
    }
    dispatch(Op(f))
}

/// Lane indices `[0.0, 1.0, ..., (lanes - 1) as f32]` for `isa.f32()`.
#[inline(always)]
pub(crate) fn iota_f32<I: Isa>(isa: I) -> I::F32 {
    const IOTA: [f32; MAX_F32_LANES] =
        [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
    let ops = isa.f32();
    let lanes = ops.len();
    debug_assert!(lanes <= MAX_F32_LANES);
    ops.load(&IOTA[..lanes])
}

/// The four taps `[p00, p10, p01, p11]` blended by `w`, as one 128-bit
/// vector: LLVM leaves the plain-array form scalar. SSE2 and NEON are
/// baseline, so no detection is needed.
#[inline(always)]
pub(crate) fn blend4(w: [f32; 4], p: [[f32; 4]; 4]) -> [f32; 4] {
    // Elsewhere the plain form below overwrites the zeros.
    #[cfg_attr(
        not(any(target_arch = "x86_64", target_arch = "aarch64")),
        allow(unused_assignments)
    )]
    let mut out = [0.0f32; 4];
    #[cfg(target_arch = "x86_64")]
    // SAFETY: SSE2 is baseline; every load and the store is of a [f32; 4].
    unsafe {
        use std::arch::x86_64::*;
        let tap = |k: usize| _mm_mul_ps(_mm_set1_ps(w[k]), _mm_loadu_ps(p[k].as_ptr()));
        let r = _mm_add_ps(_mm_add_ps(_mm_add_ps(tap(0), tap(1)), tap(2)), tap(3));
        _mm_storeu_ps(out.as_mut_ptr(), r);
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: NEON is baseline; every load and the store is of a [f32; 4].
    unsafe {
        use std::arch::aarch64::*;
        let tap = |k: usize| vmulq_n_f32(vld1q_f32(p[k].as_ptr()), w[k]);
        let r = vaddq_f32(vaddq_f32(vaddq_f32(tap(0), tap(1)), tap(2)), tap(3));
        vst1q_f32(out.as_mut_ptr(), r);
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        out = std::array::from_fn(|k| {
            w[0] * p[0][k] + w[1] * p[1][k] + w[2] * p[2][k] + w[3] * p[3][k]
        });
    }
    out
}

/// Four pixels `[a, b, c, _]` from [`blend4`] stored into the first four
/// elements of `a`, `b` and `c`: a 4×4 transpose and three vector stores
/// instead of twelve single-lane stores.
///
/// In `std::arch` because `rten-simd` has no `f32` lane shuffles and no
/// fixed four-lane vector.
#[inline(always)]
pub(crate) fn transpose_store4(p: [[f32; 4]; 4], a: &mut [f32], b: &mut [f32], c: &mut [f32]) {
    let (a, b, c) = (&mut a[..4], &mut b[..4], &mut c[..4]);
    #[cfg(target_arch = "x86_64")]
    // SAFETY: SSE2 is baseline; every load is of a [f32; 4] and every
    // store of four elements of a slice at least that long.
    unsafe {
        use std::arch::x86_64::*;
        let r = |k: usize| _mm_loadu_ps(p[k].as_ptr());
        let (ab01, ab23) = (_mm_unpacklo_ps(r(0), r(1)), _mm_unpacklo_ps(r(2), r(3)));
        let (c01, c23) = (_mm_unpackhi_ps(r(0), r(1)), _mm_unpackhi_ps(r(2), r(3)));
        _mm_storeu_ps(a.as_mut_ptr(), _mm_movelh_ps(ab01, ab23));
        _mm_storeu_ps(b.as_mut_ptr(), _mm_movehl_ps(ab23, ab01));
        _mm_storeu_ps(c.as_mut_ptr(), _mm_movelh_ps(c01, c23));
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: NEON is baseline; every load is of a [f32; 4] and every store
    // of four elements of a slice at least that long.
    unsafe {
        use std::arch::aarch64::*;
        let r = |k: usize| vld1q_f32(p[k].as_ptr());
        // [a0 a1 c0 c1], [b0 b1 _ _] and the same of pixels 2 and 3.
        let (ac01, b01) = (vtrn1q_f32(r(0), r(1)), vtrn2q_f32(r(0), r(1)));
        let (ac23, b23) = (vtrn1q_f32(r(2), r(3)), vtrn2q_f32(r(2), r(3)));
        let d = vreinterpretq_f64_f32;
        let low = |u, v| vreinterpretq_f32_f64(vzip1q_f64(d(u), d(v)));
        let high = |u, v| vreinterpretq_f32_f64(vzip2q_f64(d(u), d(v)));
        vst1q_f32(a.as_mut_ptr(), low(ac01, ac23));
        vst1q_f32(b.as_mut_ptr(), low(b01, b23));
        vst1q_f32(c.as_mut_ptr(), high(ac01, ac23));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    for (k, p) in p.iter().enumerate() {
        (a[k], b[k], c[k]) = (p[0], p[1], p[2]);
    }
}
