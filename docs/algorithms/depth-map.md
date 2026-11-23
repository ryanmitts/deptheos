# Depth-map stacking

The depth stacker takes a focus stack and returns one image in which each
pixel comes from the frames that were sharpest there. Pass 1 measures focus
in every frame on a grid at most half the frame size, records per pixel the
frame that scored highest, and smooths that depth map where the pick was
unsure. Pass 2 reads the frames again and blends them through a Laplacian
pyramid, each level weighted by the depth map's distance to each frame.

## Inputs and outputs

Inputs, in `crates/deptheos_core/src/depth/mod.rs` (pass 1 and the
public API; pass 2 is in `blend.rs`, and the blur, map-grid and pyramid
curves in `curves.rs`):

- `StackInfo`: `total_frames` (N, left-out frames included) and `is_linear`.
- Pass 1: each frame once, in stack order, through `accumulate`, with an
  optional `FrameAlignment` (an affine reference-to-frame transform and a
  `brightness` factor). `skip_frame` leaves a frame out but keeps its index.
- Pass 2: each stacked frame once more, in any order, through
  `blend_frame` with its pass-1 index.
- `DepthConfig`, the three user settings, used as given (validated
  against these ranges by `DepthConfig::validate`): `radius`, 1 to 50,
  default 8, sets the focus-metric blurs (step 3); `smoothing`, 0 to 20,
  default 4, 0 off, sets the depth-map smoothing (step 7); `blend_range`,
  0 to 10, default 3, sets the feather power (step 8).

Outputs, after `finalize`:

- `result_texture`: rgba32float, full size, values in [0, 1], in the
  frames' own encoding (no sRGB conversion in pass 2).
- `frame_map`: the full-resolution depth map, r32float, values in [0, 1]
  on the frame-plane scale below.

## The algorithm

Frame `i` (0-based, stack order) sits at plane `p_i = i / (N − 1)`, with
`step = 1 / (N − 1)` (`FramePlane::new` in `plane.rs`; a one-frame stack uses
N = 2, so its frame is at 0). Left-out frames keep their planes.

1. **Map grid** (`map_grid`). Everything in pass 1 runs on a grid of
   ```
   scale = min(MAP_SCALE, MAX_MAP_SIDE / max(W, H))
   w = max(1, trunc(W·scale + MAP_SIDE_BIAS)),  h likewise
   ```
   with `MAP_SCALE = 0.5`, `MAX_MAP_SIDE = 6000`, `MAP_SIDE_BIAS = 0.01`.
   The cap bounds the metric's cost and the smoothing sweeps' length.

2. **Gray and downscale** (`focus.wgsl`: `gray_downscale_h_kernel`, then
   `gray_downscale_v_kernel`). Gray is `(r + g + b) / 3`, after sRGB
   encoding when `is_linear`. Each axis is filtered separately with the
   Keys cubic `keys_weight` (`KEYS_C = 0.6121`, `warp_common.wgsl`)
   stretched by `ratio = source size / grid size`:
   ```
   c = (x + 0.5)·ratio − 0.5
   out(x) = Σ_j g(j)·k((j − c)/ratio) / Σ_j k((j − c)/ratio),
            j in [ceil(c − 2·ratio), floor(c + 2·ratio)] ∩ image
   ```
   Taps outside the image are dropped; the sum is 0 if the weights sum
   below 1e-6.

3. **Focus metric** (`focus.wgsl`; host order in `FocusKernels::encode`).
   Gaussian pre-blur `b = G_pre * g`, then the Weber bandpass
   (`bandpass_kernel`):
   ```
   f = clamp(BANDPASS_GAIN · |g − b| / (DARK_FLOOR + max(g, b)), 0, 1)
   ```
   with `BANDPASS_GAIN = 1.1`, `DARK_FLOOR = 0.1`, then a Gaussian
   post-blur `F = G_post * f`, which pools the metric before the pick.
   Both blurs (`gaussian_blur_h_kernel`, `gaussian_blur_v_kernel`) use taps
   `exp(−i² / (2σ²))` for `|i| ≤ max(1, ceil(BLUR_HALF_WIDTH_SIGMAS·σ))`
   (`blur_half_width`, `BLUR_HALF_WIDTH_SIGMAS = 3`),
   clamp to the edge and divide by the weight sum. Their widths come from
   `radius` through the IIR mapping (`iir_k1`, `iir_sigma` in
   `crate::focus`, `focus.rs`; the factors in `curves.rs`):
   ```
   k1(r) = IIR_K1_BASE^(1.5 / r)            (0 when r ≤ 0)
   w1    = max(2, floor(Q·k1(r)) rounded down to even);  k1 = w1 / Q
   σ(k1) = (1 + k1) / ((1 − k1)·√2)
   σ_pre  = σ(k1(radius · RADIUS_BLUR_SCALE · PRE_BLUR_FACTOR))
   σ_post = σ(k1(radius · RADIUS_BLUR_SCALE · POST_BLUR_FACTOR))
   ```
   (`iir_k1`, `iir_sigma`, `iir_pre_blur_sigma`, `iir_post_blur_sigma`),
   with `IIR_K1_BASE = 0.045999825`, `Q = IIR_QUANTIZATION = 4096`,
   `RADIUS_BLUR_SCALE = 2.4`, `PRE_BLUR_FACTOR = 0.5` and
   `POST_BLUR_FACTOR = 2`.
   `σ(k1)` is the standard deviation of the smoothing filter of step 7 at a
   constant `k1`; the Gaussian stands in for it. At radius 8, σ_pre ≈ 2.99
   and σ_post ≈ 11.72 map pixels (test `iir_parameter_mapping`).

4. **Warp the focus map** (`warped_focus` in `depth.wgsl`), as the pick
   reads it; an unaligned frame goes through the identity, which bilinear
   reproduces exactly. The metric runs on the frame's own grid and only the
   finished map is resampled: warping the frame first would make the metric
   depend on subpixel alignment phase. The full-size transform `[A | t]`
   becomes, for per-axis `S = diag(ratio)` and `c = (½, ½)`,
   `[S⁻¹AS | S⁻¹(t + (A − I)(S − I)c)]` (`WarpParams::on_grid`).
   Sampling is bilinear; positions outside `[0, w−1] × [0, h−1]` read 0,
   so a frame cannot win where it has no data. No brightness correction.

5. **Pick** (`depth_wta_accumulate`). Before the first frame, `best_focus`
   is −1 and `depth_index` and `opacity_map` are 0 (`allocate_and_init`).
   Per frame and pixel:
   ```
   sum += F
   if F > best:  best = F;  depth = p_i
   ```
   The strict `>` keeps the earlier frame on an exact tie, which happens in
   flat regions where every frame scores 0.

6. **Confidence** (`encode_confidence`). With `n` the frames stacked:
   ```
   c0 = clamp(|sum / max(n, 1) − best|, 0, 1)                (opacity_diff)
   ```
   `histogram_accumulate` counts `c0` into `HISTOGRAM_BINS = 1024` bins,
   `bin = min(floor(1024·c0), 1023)`. `histogram_percentile` (one thread)
   takes `t = max(1, floor(0.01 · w·h))` and finds `lo`, the first bin from
   the bottom whose running count reaches `t`, and `hi`, likewise from the
   top. Bins are coded as their lower edge, `bin << 6` on a 16-bit scale:
   ```
   offset    = 64·lo / 65535
   inv_range = 65535 / (64·(hi − lo))
   c = clamp((c0 − offset)·inv_range, 0, 1) ^ CONFIDENCE_GAMMA
   ```
   If `hi ≤ lo`, offset is 0 and inv_range 1 (`normalize_opacity`,
   `CONFIDENCE_GAMMA = 0.5`). The square root lifts mid confidences. `c`
   is written to `map_b`, the smoothing guide.

7. **Smoothing** (`encode_iir`; `iir_sweep`), skipped
   when `smoothing ≤ 0`. Two feedback coefficients:
   ```
   a_weak   = smoothing · SMOOTHING_WEAK_SCALE           (0.75 · 0.03125)
   a_strong = a_weak · SMOOTHING_STRONG_FACTOR           (64)
   k_weak = k1(a_weak),  k_strong = k1(a_strong)         (iir_smoothing_k1)
   ```
   Per pixel `k = k_strong + (k_weak − k_strong)·clamp(c, 0, 1)` and
   `k2 = (1 − k) / 2`: confidence 1 gives the weak blur, 0 the strong one.
   There is no gradient or edge term. Along one row or column `x[0..n)`:
   ```
   forward:  y_f[m] = k[m]·y_f[m−1] + k2[m]·(x[m−1] + x[m]),
             with y_f[−1] = x[−1] = x[0]
   backward: y_b[m] = k[m]·y_b[m+1] + k2[m]·(x[m+1] + x[m]),
             with y_b[n] = x[n] = x[n−1]
   out[m] = (y_f[m] + y_b[m]) / 2
   ```
   This is a first-order recursive (exponential) filter whose input is the
   mean of two neighboring samples, run in both directions and averaged,
   not cascaded. At a constant `k1 = 0` it is `[¼, ½, ¼]`. One application
   is a horizontal pass then a vertical pass; it runs `SMOOTHING_PASSES`
   = 2 times with the same coefficients and the same guide. Every allowed
   smoothing gives `k_weak` at the floor `2/4096` (σ ≈ 0.71); `k_strong` is `1896/4096`
   (σ ≈ 1.93) at the default 4 and `3510/4096` (σ ≈ 9.18) at 20. Each
   sweep is its own dispatch, reading the previous output as a texture.

8. **Feather** (`encode_feather`, `depth_feather`), run when
   `power = 1 + (10 − blend_range) / 3` (`feather_power`) exceeds 1 and
   more than one frame was stacked. For each step `j` between frames `j`
   and `j + 1`, `spans[j] = [a, b]` are the nearest stacked frames at or
   below `j` and at or above `j + 1` (`feather_spans`), `[0, 0]` when one is
   missing. Per pixel, with `x = z / step`:
   ```
   [a, b] = spans[clamp(floor(x), 0, N − 2)];   unchanged if b ≤ a
   h = (b − a) / 2;   q = a if x − a < h, else b
   d = (z − q·step) / (h·step)
   z' = clamp(q·step + sign(d) · h·step · min(|d|, 1)^power, 0, 1)
   ```
   Depths move toward the nearer stacked frame's plane; power 1 leaves them.

9. **Upsample** (`encode_upsample`, `upsample_bilinear`): bilinear to full
   size at source position `(x + 0.5)·(w / W) − 0.5`, clamped to the edge.

10. **Blend pyramid** (`begin_blend`). Level count `L` starts at 1 and
    grows while the smaller side exceeds `BLEND_LEVEL_FLOOR_PX = 128` and
    `L < BLEND_MAX_LEVELS = 32`, halving and rounding up
    (`blend_pyramid_levels`, `level_sizes`). The depth pyramid is built once:
    `D_0` is the full-size map, `D_{l+1}` a 2×2 box average of `D_l`
    (`box_down2_r`; on an odd width the last column averages 2 values, on
    an odd height the last row is read twice).

11. **Per frame** (`blend_frame`):
    - `G_0` (`blend_warp`): Keys bicubic, `KEYS_C = 0.6121`, 4×4 taps at
      `A·(x, y) + t` (identity when unaligned), the position clamped to the
      pixel-center range, out-of-image taps dropped and the rest
      renormalized, clamped to [0, 1], then times `brightness` and clamped
      again (`keys_sample`, `apply_exposure`).
    - `G_{l+1}` (`gauss_down2`): taps `2X − 3 … 2X + 4` per axis with
      weights `g(1.75), g(1.25), g(0.75), g(0.25), g(0.25), …, g(1.75)`,
      `g(d) = exp(−d² / (2 · 0.666²))` (`GAUSS_HALF`, `DOWN_K` in
      `resample.wgsl`), out-of-image taps dropped and renormalized per axis.
    - `up()` (`gauss2_up`): the same Gaussian at unit scale, center
      `X/2 − 0.25`, four taps per axis chosen by parity (`UP_EVEN_K`,
      `UP_ODD_K`), dropped taps renormalized.
    - Weight (`plane_weight` in `shaders/plane.wgsl`, `PlaneRamp`):
      ```
      w_i(z) = max(0, 1 − |z − p_i| / s),  s = below if z < p_i, else above
      ```
      where `below` and `above` are the plane distances to the nearest
      stacked frame on each side, one `step` where there is none. Across a
      left-out frame the ramps of its neighbors still sum to 1.
    - Accumulate where `w > 0`, with `z = D_l` at the pixel:
      ```
      l < top:  A_l += w·(G_l − up(G_{l+1})),  W_l += w     (blend_level)
      l = top:  A_T += w·G_T,                  W_T += w     (blend_top)
      ```
    The weight depends only on the frame's index: any frame order works.

12. **Collapse** (`finalize`; `blend_resolve_top`, `collapse_level`):
    ```
    R_T = A_T / W_T;   R_l = A_l / W_l + up(R_{l+1})
    ```
    A pixel with `W = 0` keeps its sum (0). `R_0` is clamped to [0, 1]
    with alpha 1 (`finish_rgb`, no linear decode) and is the result.

## Properties and limits

- **Two passes.** Pass 1 reads every frame once and builds the depth map;
  pass 2 reads every stacked frame once more. Between them the stacker
  keeps the full-resolution depth map (r32float), the upload texture and
  the `FrameLedger` of stacked and left-out frames; the map-grid textures
  are freed before the pyramids are allocated (`begin_blend`). `finalize`
  refuses until every stacked frame is blended.
- **Memory.** Pass 1 holds the frame-size source texture, a gray-rows
  texture (map width × frame height), four r32float map-grid textures
  (`depth_index`, `map_a`, `map_b`, `map_c`) and two map-grid buffers
  (`best_focus`, `opacity_map`); the smoothing sweeps reuse `map_a` and
  `map_c`. The peak is pass 2, estimated by `gpu_bytes`: the source texture
  plus `BYTES_PER_PIXEL` (Metal's padding included), which holds the
  full-size depth map and, per pixel of each pyramid level, 16 bytes
  (frame Gaussian) + 16 (accumulator) + 4 (depth level, above level 0).
  The level-0 accumulator must fit one storage binding
  (`check_accum_fits`).
- **Deterministic.** `k_weak` and `k_strong` are quantized so a setting
  blurs the same on every GPU (`iir_k1`); the per-pixel mix between them is
  not. Separate sweep dispatches keep repeat runs identical (test
  `depth_map_is_deterministic`).
- **Low-texture areas.** Where every frame's metric is near 0 plus noise,
  the pick is close to random; the post-blur pools the metric so a patch
  picks together, and the smoothing that follows cannot repair
  per-pixel noise (comment above `IIR_K1_BASE`). Exact ties go to the
  earliest frame.
- **Thin structures.** The pick is pooled over σ_post (≈ 11.7 map pixels
  at radius 8), so a structure narrower than that is decided with its
  surroundings. At level 0 each pixel draws on at most the two stacked
  frames whose planes bracket its depth, so one pixel cannot hold two
  surfaces at different depths.
- **No edge term in the smoothing** (ADR-120): it does not stop at image
  edges, and the strong blur (σ ≈ 9.2 map pixels per pass at smoothing
  20) reaches only a short way into a large unconfident area.
- **Frame edges.** Pass 1's warped focus map is 0 outside a frame's data;
  pass 2's Keys warp clamps to the edge pixel, with no margin mask.
- **Exposure.** Pass 1 applies no brightness correction (the Weber
  normalization divides out most of it); pass 2 applies `brightness`.

## Related work

- S. K. Nayar and Y. Nakagawa, "Shape from Focus", *IEEE Transactions on
  Pattern Analysis and Machine Intelligence*, vol. 16, no. 8, pp. 824–831,
  August 1994. Section VI describes a coarse depth map that assigns each
  point the stage position whose focus measure is highest, which is the
  per-pixel pick of step 5. This code measures focus with a Weber-normalized
  bandpass pooled by a Gaussian rather than the sum-modified-Laplacian, and
  does not fit a Gaussian to refine depth between frames.
- R. G. Keys, "Cubic Convolution Interpolation for Digital Image
  Processing", *IEEE Transactions on Acoustics, Speech, and Signal
  Processing*, vol. ASSP-29, no. 6, pp. 1153–1160, December 1981. Equation
  (4) is the kernel of `keys_weight` with `a = −KEYS_C`. This code uses
  `a = −0.6121` rather than the paper's `a = −1/2`, and in step 2 stretches
  the kernel to filter a downscale rather than to interpolate.

## Decisions

- [ADR-120](../decisions/120-the-stackers-are-tuned-by-eye-and-keep-their-own-kernels.md):
  what was tried and dropped, including `RADIUS_BLUR_SCALE`, the
  thin-structure override and the hybrid with the pyramid; the
  confidence-guided IIR smoothing on a half-size grid; and why the blend
  keeps its own pyramid kernels.
