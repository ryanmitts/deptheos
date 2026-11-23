# ECC alignment

The ECC aligner takes two frames of a sequence, the previous one and this one,
and returns the affine transform from the previous frame's pixel coordinates
to this frame's, or an error when the pair does not align. It maximizes the
enhanced correlation coefficient ρ of Evangelidis & Psarakis (2008) with their
forward additive update, coarse to fine, on the CPU. The solver is derived
from OpenCV's `findTransformECC` (credited under References).

## Inputs and outputs

- **Frames.** RGB8 or RGB16 images, all the size of frame 0. Each becomes a
  gray `Plane` (row-major `f32`, values 0–1) at every level of the schedule.
- **Model.** A full affine warp, six parameters, held as `Affine2<f32>`:
  `W = [a00 a01 tx; a10 a11 ty]`. Brightness is not modeled: ECC always
  reports a brightness factor of 1.
- **Direction.** Within one level, `W` maps a template pixel `(x, y)` to the
  input position `W·(x, y, 1)` that is sampled for it (template → input, the
  backward sampling matrix). The template is the previous frame, so a step
  maps previous → this frame. `(x, y)` is the integer pixel index; between
  levels, pixel centers sit at `x + 0.5`.
- **Output.** `Aligner::align_prepared` returns `AlignedFrame { transform, brightness }`
  with `transform` mapping frame 0 (the reference) → this frame in
  full-resolution pixels. `Aligner::crop` gives the rectangle every
  frame so far covers, in reference pixels, when it is at least a quarter
  of the frame.
- **ρ.** Each level reports the ρ of the warp it returns (or, when it
  converged, of the warp one sub-tolerance update before it). The values are
  logged at debug level ("ρ by level") and used for the acceptance check;
  `Aligner::align` does not return them.

## The algorithm

Lowercase names are fields of `EccConfig::default()` (`ecc/mod.rs`);
uppercase constants are in `ecc/solve.rs`, but `FEATURELESS_STD` and
`REC709_LUMA`, which are in `lib.rs`.

1. **Gray and the level schedule** (`ecc/mod.rs:EccConfig::level_resolutions`,
   `EccAligner::prepare`). With `S = max(width, height)` and
   `top = min(fine_max_dimension, S)`, the level resolutions start at
   `min(coarse_max_dimension, top)` and double while the double is below
   `top`; `top` is appended if it is not already last. With the defaults (256,
   2048) a 6000×4000 frame gets 256, 512, 1024, 2048, and a 1600-px frame gets
   256, 512, 1024, 1600; a frame of 256 px or less gets one level. A level of
   resolution `r` is `round(width·s) × round(height·s)`, at least 1×1, with
   `s = min(r/S, 1)`. Only the finest level is made from the frame
   (`image_ops/gray_resize.rs:image_to_grayscale_resized`): Rec. 709 luma
   `0.2126 R + 0.7152 G + 0.0722 B` (`REC709_LUMA`) over 255 or 65535, resized by a bilinear
   (triangle) convolution whose destination pixel `x` is centered at source
   `(x + 0.5)·scale`. Each coarser level is the unblurred level above it,
   resized with the same bilinear convolution (`resize_plane`).
2. **Pre-blur** (`ecc/solve.rs:EccInput::new`, `blur`, `image_ops/blur.rs:gaussian_blur`).
   Level 0 uses `coarse_blur_sigma = 0.0`, no blur ("resize already low-passed
   it"); every other level uses `blur_sigma = 0.8`. The blur is separable,
   `2·ceil(3σ) + 1` taps (7 at σ = 0.8) of `exp(−d²/2σ²)` normalized to 1,
   edges clamped. The blurred plane is the frame's input at that level and,
   once the frame has aligned, the next frame's template.
3. **Gradients** (`ecc/solve.rs:pack_with_gradients`). On the blurred input,
   `gx = 0.5·(I[x+1] − I[x−1])`, `gy = 0.5·(I[y+1] − I[y−1])`; at an edge the
   edge pixel stands in for its missing neighbor, so `gx[0] = 0.5·(I[1] −
   I[0])`. A 1-px-wide level has `gx = 0`. Blur, `gx` and `gy` are interleaved
   (`pack_with_gradients`) so that one bilinear fetch reads all three.
4. **Warp and mask** (`image_ops/mod.rs:PackedWarp::row`). For template pixel
   `(x, y)` the source is `(sx, sy) = (a00 x + a01 y + tx, a10 x + a11 y +
   ty)`. The pixel is valid (`m = 1`) when `0 ≤ sx < W_in` and
   `0 ≤ sy < H_in`; it is then sampled bilinearly from `I`, `gx` and `gy`,
   with the `+1` tap clamped to the last row or column. An invalid pixel gets
   `m = 0` and zero image and gradients. The gradients are warped, not
   recomputed on the warped image.
5. **One pass of sums** (`ecc/solve.rs:fused_iteration`, `ChunkFusedOp`). Each
   iteration sweeps the template once, a row at a time, accumulating with a
   shift `c` (the mean of every 61st template pixel, `SHIFT_SAMPLE_STRIDE`):
   ```
   i = m·(I_w − c)   t = m·(T − c)   N = Σ m
   Σi  Σi²  Σt  Σt²  Σ i·t   Σ J   Σ J·i   Σ J·t   H = Σ JᵀJ
   ```
   Each warped row is summed in four passes over its buffers (`BLOCK`
   pixels at a time), so each pass's accumulators fit in registers; every
   sum takes the same terms in the same order as in a single pass.
6. **Jacobian in centered, normalized coordinates** (`ecc/solve.rs:Coords`). With
   `cx = w/2`, `cy = h/2`, `L = max(w, h)` of the level, `u = (x − cx)/L`,
   `v = (y − cy)/L`, the Jacobian row of a pixel is
   ```
   J = [gx·u, gy·u, gx·v, gy·v, gx, gy]
   ```
   so the solved vector is `q = (L·Δa00, L·Δa10, L·Δa01, L·Δa11,
   Δtx + Δa00·cx + Δa01·cy, Δty + Δa10·cx + Δa11·cy)`. `J` is zero wherever
   `m = 0`, because the warped gradients are.
7. **Overlap check.** If `N < MIN_OVERLAP · w·h` (0.25) the level fails
   ("frames overlap on …% of the template").
8. **Zero mean and norms.** With `μi = Σi/N`, `μt = Σt/N`:
   ```
   ‖ī‖ = sqrt(max(Σi²/N − μi², 0))·sqrt(N)     (‖t̄‖ likewise)
   Jᵀī = ΣJ·i − ΣJ·μi     Jᵀt̄ = ΣJ·t − ΣJ·μt     C = Σ i·t − N·μi·μt
   ```
   Only valid pixels count, for the template's statistics as for the input's.
   The code comment gives the reason: counting the zero border instead biases
   the fixed point toward the frame center, which a chain under focus
   breathing accumulates as drift at the corners.
9. **Hessian.** `H` (6×6, symmetric; the upper triangle is summed and
   mirrored) is inverted with `try_inverse` in f64. A singular `H` fails the
   level.
10. **ρ.** `ρ = C / (‖ī‖·‖t̄‖)`. A non-finite ρ fails the level.
11. **Best ρ and stall.** If ρ beats the best so far, the warp and ρ become the
    best and the stall count resets; otherwise the count grows. On the last
    iteration of the budget, or once the count reaches `STALL_ITERATIONS` (3),
    the loop stops here and returns the best warp and its ρ (`Flow::Stop`).
12. **λ.** With `H⁻¹Jᵀī` computed once:
    ```
    λn = ‖ī‖² − (Jᵀī)ᵀ H⁻¹ (Jᵀī)
    λd = C    − (Jᵀt̄)ᵀ H⁻¹ (Jᵀī)
    λ  = λn / λd
    ```
    If `λd` is NaN or `λd ≤ 0` the level fails ("Images may be uncorrelated or
    non-overlapped"). This is the paper's first case (eq. 16, which holds when
    `îᵣᵀīw > îᵣᵀP_G īw`); its second case (eqs. 17–18) is not implemented.
13. **Update** (`Coords::apply`). `q = H⁻¹·(λ·Jᵀt̄ − Jᵀī)`, which is
    `H⁻¹Jᵀ(λ·t̄ − ī)`. It is added to the warp in pixel units:
    `Δa00 = q0/L`, `Δa10 = q1/L`, `Δa01 = q2/L`, `Δa11 = q3/L`,
    `Δtx = q4 − Δa00·cx − Δa01·cy`, `Δty = q5 − Δa10·cx − Δa11·cy`, computed
    in f64 and added to the f32 matrix. A non-finite result fails the level.
14. **Step tolerance** (`corner_displacement`). The step is the largest
    distance any of the corners `(0,0)`, `(w,0)`, `(0,h)`, `(w,h)` moves
    between the old and new warp, in level pixels. If it is below
    `max_step` (0.02) the level has converged and returns the updated warp with
    the ρ measured before the update (`Flow::Converged`). Otherwise the next
    iteration measures at the new warp. The loop runs at most `max_iter`
    iterations: `coarse_max_iters` (25) at level 0, `fine_max_iters` (50) at
    the finest level, `mid_max_iters` (30) between (`criteria_for_level`; a
    one-level schedule uses 25). The last only
    measures, so a budget of `n` applies at most `n − 1` updates.
15. **Between levels** (`ecc/mod.rs:pairwise`, `rect.rs:rescale_affine`). The
    full-resolution transform starts at the identity. For level `k`,
    `sx = level_w/width`, `sy = level_h/height`; the seed is
    `C·T·C⁻¹` with `C = [sx 0 (sx−1)/2; 0 sy (sy−1)/2]` (the map
    `x' = s·(x + 0.5) − 0.5`), and the level's result is carried back with
    `(1/sx, 1/sy)`. The template at every level is the previous frame's
    blurred plane at that level.
16. **Acceptance** (`pairwise`, `EccConfig::implausibility`). A level whose ρ
    is below `min_correlation` (0.6) fails the pair at once, before finer
    levels run. After the finest level the full-resolution step `[a b tx; c d
    ty]` must be plausible: `det = ad − bc > 0`; scale `√det` within
    `max_scale_pct` (5 %) of 1; shear and anisotropy `max(|a−d|, |b+c|) /
    (2√det)` at most `max_scale_pct`; rotation `atan2(c − b, a + d)` at most
    `max_rotation_deg` (5°); the shift of the frame center, `|a·cx + b·cy +
    tx − cx| / width` (and likewise in y), at most `max_shift_pct` (5 %).
17. **The sequence** (`lib.rs:Aligner::align_prepared`, `Sequence`). Frame 0
    is accepted when its finest blurred level has a standard deviation of at
    least `FEATURELESS_STD` (1/512); a flatter frame is an error and the next
    frame is tried as frame 0. Every later frame must have frame 0's size and
    is aligned to the previous frame that aligned. On success the step is
    composed as `last = step · last` (reference → previous, then previous →
    this), the frame becomes the reference, and the valid area is intersected
    with the frame's rectangle mapped by `last⁻¹`. On any error nothing
    changes: the next frame aligns against the last good one. The grid
    aligner shares this sequence handling.

## Numerical notes

- **f32 lanes, f64 merges.** Within a chunk each SIMD lane accumulates its
  own f32 sums, with fused multiply-add where the instruction set has it;
  pixels past the last full vector of a row accumulate in f64. Lane sums are
  converted to f64 at the end of the chunk; chunks merge, and the 6×6 system
  is solved, in f64.
- **Conditioning.** The code comment states that in raw pixel coordinates the
  system at the 2048 level "is conditioned far beyond f32's precision": the x
  and constant columns are almost parallel (`Σgx²·x ≈ 1024·Σgx²` at
  2048 px). The centered, normalized `u`, `v` give the six columns comparable
  magnitudes and make the translation columns nearly orthogonal to the rest.
- **The shift `c`.** Subtracting a sampled template mean before squaring keeps
  the f32 lane sums small; the exact masked means are restored in f64
  (step 8), so the shift does not change the statistics.
- **Chunking and determinism.** `CHUNK = 65536` pixels per rayon task
  (`max(CHUNK / width, 1)` rows). Partial sums are collected in row order and
  folded sequentially, so the result does not depend on rayon's scheduling.
  The grouping is part of the result: the `CHUNK` comment says a different
  chunk size or lane count moves the transforms by about 0.002 px.
- **Same values everywhere.** The warp's SIMD interior repeats the scalar
  arithmetic without fused multiply-add, so every instruction set gives the
  same warped values. `corner_displacement` uses `sqrt(dx² + dy²)`, not
  `hypot`, which comes from each platform's C runtime and can differ in the
  last bit.

## Properties and limits

- ρ ignores a uniform gain and offset between the frames, but not uneven
  brightness changes such as vignetting (`lib.rs` module doc). ECC needs
  texture.
- The paper's eq. (16) uses the column-zero-mean Jacobian `Ḡ` in `ḠᵀḠ`; this
  code uses `H = JᵀJ` of the uncentered `J`. The projections are
  the same either way, because `ī` and `t̄` are zero-mean.
- A shift past the coarse level's capture range must fail, not align wrongly:
  100 px on a 1600-px frame, about 16 px at the 256 level, is an error
  (`shift_beyond_basin_is_an_error`).
- The ρ floor: per the `min_correlation` comment, correctly aligned frames of
  real stacks reach ρ ≥ 0.93 at every level, while wrong answers on shifted
  and flipped frames usually stay at or below about 0.58. A smooth circuit
  board against its mirror image scores 0.65–0.67, and only the plausibility
  limits reject it.
- Per their comment, real stacks move ≤ 0.5 %, ≤ 1 % and ≤ 0.03° per frame,
  and wrong answers seen in testing were off by more than 5 % in scale.
- Chaining accumulates error: `breathing_chain_does_not_drift` allows 0.2 px
  at the corners after 24 frames of 0.5 % magnification each.
- A failed frame is left out by the stacking session, and the session's
  reverse check (`deptheos_app/src/runs/session.rs`, ADR-112) restarts with
  the frames reversed when the second frame is magnified relative to the
  first by more than `REVERSE_DEADBAND` (0.1 %).

## References

- G. D. Evangelidis and E. Z. Psarakis, "Parametric Image Alignment Using
  Enhanced Correlation Coefficient Maximization", *IEEE Transactions on
  Pattern Analysis and Machine Intelligence* 30(10), 1858–1865, 2008,
  doi:10.1109/TPAMI.2008.113. Author's version: HAL hal-00864385. The
  criterion is eq. (4), the linearized warp eqs. (6)–(7), Theorem I eqs.
  (10)–(15), the update this code implements eq. (16), the case it does not
  eqs. (17)–(18) (Lemma I), and the forward additive algorithm Table I.
- G. D. Evangelidis and E. Z. Psarakis, "An ECC Based Iterative Algorithm for
  Photometric Invariant Projective Registration", *International Journal on
  Artificial Intelligence Tools* 18(1), 121–139, 2009,
  doi:10.1142/S021821300900007X (HAL hal-00864389). It applies the 2008
  results to projective warps; its eq. (17) is the same update.
- OpenCV, `findTransformECC`,
  <https://github.com/opencv/opencv/blob/4.x/modules/video/src/ecc.cpp>.
  OpenCV is Apache-2.0 since 4.5; `ecc.cpp` carries the Intel License
  Agreement ("Copyright (C) 2000, Intel Corporation"). The solver here is
  derived from it and used under that license; the notice is in
  [THIRD_PARTY_NOTICES.txt](../../THIRD_PARTY_NOTICES.txt).

## Decisions

- [ADR-112](../decisions/112-alignment-on-the-cpu-each-frame-to-the-one-before.md):
  alignment runs on the CPU, in SIMD Rust on a bounded thread pool, with
  two aligners, each aligning a frame to the one before it.
- [ADR-121](../decisions/121-a-frame-that-fails-is-left-out-and-frames-are-not-scored.md):
  a frame that cannot be aligned is left out.
