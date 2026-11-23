# Pyramid stacking

The pyramid stacker takes N aligned frames of one scene, each sharp at a
different depth, and returns one image that keeps each frame's sharp detail.
Every frame is split into a Laplacian pyramid; at each level and pixel the
frames' Laplacians are mixed by a soft winner-take-all on a relative
Laplacian focus metric, the coarsest level is averaged, and the mixed pyramid
is collapsed. Frames are streamed one at a time and none is kept.

Code: `crates/deptheos_core/src/pyramid/{mod.rs,pyramid.wgsl}`, with the
shared kernels in `crates/deptheos_core/src/shaders/`.

## Inputs and outputs

In, per frame (`PyramidStacker::accumulate`):

- An RGB frame of W×H pixels, values in [0, 1]. Every frame must have the
  first frame's size. `StackInfo::is_linear` says whether the frames are
  linear (RAW) or sRGB-encoded; `StackInfo::total_frames` is N.
- Optionally a `FrameAlignment`: an affine transform M from reference pixel
  coordinates to this frame's pixel coordinates (pixel j centered at j), and
  an exposure factor `brightness` (1.0 for none).
- `PyramidStacker::skip_frame` leaves the next frame out. It keeps its index
  and plane, so later frames keep theirs.

One setting: `PyramidConfig::smoothness`, in `SMOOTHNESS_RANGE` = [1, 10],
step `SMOOTHNESS_STEP` = 0.1, default `DEFAULT_SMOOTHNESS` = 4.0. Values
outside the range are refused (`PyramidConfig::validate`).

Out, after `finalize` (`State::Finalized`):

- `result`: the stacked image, rgba32float, W×H, clamped to [0, 1] and, for
  linear input, decoded back to linear.
- `winners`: the winner record, r32float, W×H; each pixel holds the
  `FramePlane` value `index / (max(N, 2) − 1)` of the frame chosen for it.

`collapse()` gives the stack so far after any frame without ending the stack.

## The algorithm

Notation: `G_l` is Gaussian level l of the current frame, `L_l` its
Laplacian, `EXPAND` the upsample of step 4, `n = num_levels`, `R =
record_level`. Steps 1 and 2 run once per stack, 3 to 10 once per frame (one
command submission, in this order), 11 and 12 at the end.

1. **Plan** (`num_levels`, `exponent_schedule`, `focus_blur`, called from
   `allocate_and_init`). From the first frame's size and the setting:
   - `n = max(floor(log2(min(W, H) / LEVEL_FLOOR_PX)) + 1, MIN_LEVELS)`
     (`num_levels`), with `LEVEL_FLOOR_PX = 16`, `MIN_LEVELS = 2`. Level
     sizes halve rounding up (`accum.rs:level_sizes`), so the coarsest
     level's short side ends up between 16 and 32 pixels (inclusive) for
     frames at least 32 pixels on a side.
   - `R = min(RECORD_LEVEL, n − 2)`, with `RECORD_LEVEL = 1`.
   - WTA exponent per level (`exponent_schedule`), with s the setting:
     ```
     e_0     = max(EXPONENT_BASE − s, MIN_EXPONENT)          (13 − s)
     e_last  = max(e_0 · COARSEST_EXPONENT_RATIO, MIN_EXPONENT)
     e_l     = e_0 + (e_last − e_0) · l / (n − 1)
     ```
     `EXPONENT_BASE = 13`, `COARSEST_EXPONENT_RATIO = 0.5`,
     `MIN_EXPONENT = 1`. s = 1 gives 12 to 6, s = 4 gives 9 to 4.5, s = 10
     gives 3 to 1.5. The coarsest level does not use its exponent.
   - Focus blur (`focus_blur`): `b = max(trunc(s) − 1, 0)`. If b = 0 there
     is no blur; otherwise `σ = iir_blur_sigma(b / BLUR_RADIUS_DIVISOR)`
     (`focus.rs`), with `BLUR_RADIUS_DIVISOR = 5`, and half-width
     `ceil(BLUR_HALF_WIDTH_SIGMAS·σ)`, `BLUR_HALF_WIDTH_SIGMAS = 3`
     (`focus.rs:blur_half_width`). `iir_blur_sigma(r)` is
     `(1 + k1) / ((1 − k1)·√2)` with `k1 = IIR_K1_BASE^(1.5 / r)`,
     `IIR_K1_BASE = 0.045999825`, quantized to an even multiple of
     1/`IIR_QUANTIZATION` (4096) and at least 2/4096. Across s = 2…10 this gives σ ≈ 0.708…0.825 and a
     half-width of 3 (`FOCUS_HALO = 3` is the most the shader tile holds).
2. **Allocate and clear** (`allocate_and_init`, `clear_accumulators`). Per
   level: a Gaussian scratch texture, an accumulator `best_rgb` (vec4f per
   pixel: Laplacian sums in rgb, weight in a) cleared to 0, and, below the
   coarsest level, `best_focus` cleared to `FOCUS_SENTINEL = −2`. Also the
   winner record, its running best `winners_best` (cleared to −2) at level
   R's size, and the prior pyramid for levels R to n − 2.
3. **Ingest and warp** (`pyramid.wgsl:warp_rgba`). For each reference
   pixel p, sample the frame at `M·(p, 1)` with Lanczos-3
   (`warp_common.wgsl:lanczos_sample`): 6×6 taps of
   `3·sin(πx)·sin(πx/3) / (π²x²)`, the position first clamped into
   `[0, size − 1]`, taps outside the frame dropped and the rest
   renormalized, the result clamped to [0, 1]. Then `rgb·brightness`,
   clamped to [0, 1] (`apply_exposure`), then, for linear input, the sRGB
   transfer curve (`linear_to_srgb3`). The result is `G_0`. Lanczos, not
   bilinear: bilinear is a low-pass whose strength varies with subpixel
   phase, so it would soften every aligned frame and bias the
   winner-take-all toward frames that land on whole pixels. The warp also
   runs with no transform (identity). Outside the frame's footprint this
   leaves the edge pixels stretched outward; step 6 keeps those pixels
   from carrying weight.
4. **Gaussian pyramid** (`pyramid.wgsl:downsample`, for l = 0…n − 2):
   ```
   G_{l+1}(x, y) = Σ_{i,j=−2..2} g_i·g_j · G_l(clamp(2x + i), clamp(2y + j))
   g = GAUSSIAN = [1, 4, 6, 4, 1] / 16
   ```
   Coordinates are clamped to the level. `EXPAND`
   (`laplacian.wgsl:gaussian_upsample`) uses the same kernel with gain 4:
   per axis, an even fine position 2c takes coarse pixels c − 1, c, c + 1
   with weights (1, 6, 1)/16 and an odd one 2c + 1 takes c, c + 1 with
   (4, 4)/16; the 2-D weight is the product, times 4, coarse indices
   clamped. The Laplacian is `L_l = G_l − EXPAND(G_{l+1})`.
5. **Focus metric** (`laplacian.wgsl:focus_metric`, called from
   `focus_wta` for every level below the coarsest), with `U = EXPAND(G_{l+1})`:
   ```
   F_raw = mean over r, g, b of |L_l| / (max(U, 0) + DARK_FLOOR)
   ```
   `DARK_FLOOR = 0.1` (`metric.wgsl`). If there is a focus blur, `F_raw`
   is blurred with a separable, normalized Gaussian of σ and half-width
   from step 1, edges clamped (`row_blur`, `column_blur_rows`).
6. **Margin mask** (`warp_common.wgsl:footprint_weight` via `level_mask`).
   For level pixel x, with `q = M·(x·2^l, 1)` in frame pixels:
   ```
   d = min(q.x, q.y, W − 1 − q.x, H − 1 − q.y)
   m = clamp(d / 2^l, 0, 1)
   ```
   m = 0 where the frame has no data, with a one-level-pixel ramp inside
   the footprint.
7. **Focus prior** (`focus_wta`, record level first). At level R the
   unblurred metric is pooled with a Gaussian of `FW_POOL_SIGMA = 1.5` over
   ±3 taps per axis, normalized: this is `P`. `max(P, 0)` is stored as the
   frame's prior at level R; `prior_downsample` builds the coarser prior
   levels with the step-4 kernel; level 0 reads `EXPAND` of the level-R
   prior (`prior_source`: `PRIOR_FROM_POOLED`, `_LEVEL`, `_UPSAMPLE`).
   Each level's focus is then
   ```
   F = max(F_blurred, 0) · (max(prior, 0) + PRIOR_EPSILON)^PRIOR_EXPONENT
   ```
   with `PRIOR_EPSILON = 1e-6` and `PRIOR_EXPONENT = 0.75`. The code says
   why: a frame blurred at a bright edge puts energy into the coarse
   levels, which would pick it and leave a glow around the sharp edge.
8. **Comparison key** (`encode_key`). Frames with data rank above frames
   without, at every pixel:
   ```
   k = F·m                 if m > 0      (k ≥ 0)
   k = −1 / (1 + max(F, 0))  if m = 0    (k in [−1, 0))
   ```
   `key_focus` decodes k back to a focus: k itself if k ≥ 0, else
   `−1/k − 1`. The sentinel −2 decodes to −0.5, which is clamped to 0.
9. **Soft winner-take-all** (`accumulate_soft_wta`), per pixel of levels
   0…n − 2, with running best key B, sums (S, A) and `e = e_l`:
   ```
   f_new = max(key_focus(k), 0)     f_best = max(key_focus(B), 0)
   same  = (k ≥ 0) == (B ≥ 0)
   if k > B:  w = same ? clamp(f_best / max(f_new, 1e-10), 0, 1)^e : 0
              S = S·w + L_l;  A = A·w + 1;  B = k
   else:      w = same ? clamp(f_new / max(f_best, 1e-10), 0, 1)^e : 0
              S = S + L_l·w;  A = A + w
   ```
   (the 1e-10 is the local `THRESHOLD`). The first frame always beats the
   sentinel and starts the sums with weight 1. After any number of frames,
   with K the frames of the best key's class (all frames with m > 0 if any,
   otherwise all frames) and f_i their decoded focus:
   ```
   S / A = Σ_{i∈K} (f_i / f_max)^e · L_i  /  Σ_{i∈K} (f_i / f_max)^e
   ```
   Frames outside K contribute nothing. Where no frame has data, frames
   compete on their unmasked focus, so the stack there blends as it would
   without masks.
10. **Coarsest level** (`pyramid.wgsl:accumulate_sum`), with c the frame's
    `G_{n−1}` and m its mask there:
    ```
    m > 0, A > 0:   S += c·m;  A += m
    m > 0, A ≤ 0:   S = c·m;   A = m      (first frame with data)
    m = 0, A ≤ 0:   S += c;    A −= 1     (no frame with data yet)
    m = 0, A > 0:   unchanged
    ```
    `normalize_coarsest` returns `S / |A|` (S if A = 0): the mask-weighted
    mean of the frames with data, or the plain mean of all frames where none
    has data.
11. **Winner record** (`record_winner`, level R only). With `P` from step 7
    and m from step 6, `k_P = encode_key(P, m)`. If `k_P > winners_best`,
    then `winners_best = k_P` and the frame's plane is written over the
    pixel's 2^R × 2^R block of the full-resolution record (clipped to the
    frame). This is a hard argmax on the pooled metric, not the soft mix of
    step 9.
12. **Collapse** (`collapse`). `normalize_coarsest` writes `G_{n−1}`; then
    for l = n − 2 down to 0, `laplacian_add` writes
    ```
    G_l = S_l / max(A_l, 1) + EXPAND(G_{l+1})
    ```
    into the frame's Gaussian scratch textures. The pass that writes level 0
    applies `finish_rgb` (`accum.wgsl`): clamp to [0, 1], then for linear
    input the inverse sRGB curve; alpha is 1. `finalize` runs a collapse
    and keeps level 0 as the result.

## Properties and limits

- **Order and prefix independence.** Each frame's weight is relative to the
  running best and the coarsest level is a running masked sum, so a
  collapse after k frames equals a k-frame stack of the same frames, in any
  order, up to float rounding (`collapse` doc comment; tests
  `a_collapse_after_k_frames_is_the_k_frame_stack`,
  `common_shift_leaves_no_band_at_the_boundary`). The winner record is
  order dependent only on exact ties of `k_P` (the first frame keeps them).
- **Memory.** Everything is allocated at the first frame and does not grow
  with N (`gpu_bytes`): per level, a 16-byte Gaussian texel, a 16-byte
  accumulator and (below the coarsest) a 4-byte best key, plus the
  record-level and prior textures and the full-size winner record, which
  come to `BYTES_PER_PIXEL` (Metal's padding included), and the source
  texture (`FrameDepth::source_bytes`). Frames whose level-0 accumulator
  exceeds the GPU's storage binding limit are refused (`check_accum_fits`).
- **The coarsest level is averaged, not selected.** The `num_levels` doc
  comment says exposure drift or residual misalignment there turns into
  low-frequency haze.
- **The result is a mix.** Step 9 blends Laplacians from several frames per
  pixel; the winner record names one frame per 2^R × 2^R block, chosen on a
  pooled metric at level R, so it describes where detail came from rather
  than the exact mix.
- **Masks are a class.** A frame with any data at a pixel (m > 0) always
  outranks frames without, even when its focus is lower.
- **Exponent bound.** Above s = 10 the code comment notes the exponent
  falls below 3; at s = 13 it would be 0 (a plain average) and beyond that
  negative, which gives NaN, so the range is enforced.
- **Linear input** is stacked in sRGB encoding and decoded at the end; the
  shader comment says a linear-space Laplacian over-weights highlights and
  darkens the result. Stacking a linear frame with itself returns it (test
  `linear_round_trip_preserves_values`).

## Related work

- The Gaussian pyramid (REDUCE), EXPAND (gain 4, only taps with integer
  coarse positions), the Laplacian `L_l = g_l − EXPAND(g_{l+1})` and the
  reconstruction `g_l = L_l + EXPAND(g_{l+1})` are those of P. J. Burt and
  E. H. Adelson, "The Laplacian Pyramid as a Compact Image Code", IEEE
  Transactions on Communications, vol. COM-31, no. 4, pp. 532–540, April
  1983. Its generating kernel is a family with ŵ(0) = a; this code uses
  a = 0.375, and clamps at image borders where the paper sizes images to
  `M·2^N + 1`.
- Related: W. Wang and F. Chang, "A Multi-focus Image Fusion Method Based
  on Laplacian Pyramid", Journal of Computers, vol. 6, no. 12, pp.
  2559–2566, December 2011, doi:10.4304/jcp.6.12.2559-2566. It fuses
  multi-focus images in a Laplacian pyramid with the same [1 4 6 4 1]/16
  kernel, but picks one source per coefficient by maximum region energy
  and treats the top level by deviation and entropy; this code mixes all
  frames with soft weights from a relative Laplacian and a focus prior,
  and averages the top level.
- Related: T. Mertens, J. Kautz and F. Van Reeth, "Exposure Fusion"
  (Pacific Graphics 2007). Each Laplacian level is a normalized weighted
  average of the inputs' Laplacians, with weights from a Gaussian pyramid
  of per-pixel quality maps; here the weights are computed per level from
  that level's Laplacian, relative to the best frame and raised to an
  exponent.

## Decisions

- [ADR-120](../decisions/120-the-stackers-are-tuned-by-eye-and-keep-their-own-kernels.md):
  what was tried for the selection and dropped, how `PRIOR_EXPONENT` was
  chosen, and why the pyramid stacker keeps its own kernels rather than
  the depth stacker's.
