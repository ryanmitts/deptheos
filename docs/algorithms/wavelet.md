# Wavelet stacking

The wavelet stacker merges the frames' luma in a complex Daubechies wavelet
domain, keeping at each detail coefficient the frame with the most energy,
inverts the merged coefficients to a gray target, and gives each pixel the
color of the frame whose luma is closest to that target. It is adapted from
Petteri Aimonen's focus-stack, which implements the method of Forster, Van De
Ville, Berent, Sage & Unser (2004). The wavelet is complex, as in that paper,
because a complex coefficient's magnitude varies far less with an edge's
position than a real wavelet's, so the energy the selection compares is
steadier from frame to frame. Frames are streamed: the stacker holds the
running winners, not every frame's coefficients.

Paths below are under `crates/deptheos_core/src/`; `mod.rs` and `wavelet.wgsl`
are in `wavelet/`.

## Inputs and outputs

- In: each frame twice, once per pass (`FrameInput`), with an optional
  `FrameAlignment`: an affine map from reference to frame pixel coordinates and
  a `brightness` factor. `StackInfo` gives `total_frames` and `is_linear`.
  `WaveletConfig` gives `denoise` and `consistency`.
- Lifecycle (`State` in `mod.rs`): `Uninitialized` → `Accumulating` (first
  `allocate`) → `Reassigning` (`end_pass_one`, or `Failed` if it fails
  after the merge changed the winners in place) → `Finalized`
  (`finalize`).
  Pass 1 is `accumulate` per frame; `skip_frame` leaves a frame out and keeps
  its index. Pass 2 is `blend_frame` once per stacked frame, in any order.
- Out: `result_texture` (rgba32float, frame size, in the source's encoding, so
  linear for linear input, alpha 1); `frame_map` (r32float, each pixel's
  `FramePlane` value, `index / (max(total_frames, 2) − 1)`); `render_live`:
  the live picture (step 5) through both passes, the frame map once
  finalized.
- Refused: a shorter side under `MIN_SIDE` (4) px, and frames whose largest
  state buffer exceeds the device's storage binding (`check_state_fits`).

## The algorithm

1. **Plan** (`mod.rs:plan`). Halve the shorter side (integer division) while
   it is above `MIN_COARSE` (8) and fewer than `MAX_LEVELS` (9) levels are
   planned; use at least 1 level. Pad width and height up to a multiple of
   `2^levels`. Examples (`plan` tests): 6000×4000 → 9 levels, 6144×4096;
   509×381 → 6 levels, 512×384.
2. **Ingest** (`wavelet.wgsl:ingest`). For each padded pixel `(x, y)`, mirror
   coordinates beyond the frame without repeating the edge pixel
   (`reflect_idx`: `i ≥ size` → `2·size − i − 2`), then sample the warped frame
   there (`sample_frame` → `warp_lanczos`, `shaders/warp_common.wgsl`):
   Lanczos-3 at `M·(x, y, 1)`, the position clamped to the frame (edge smear),
   output clamped to [0, 1], times `brightness`, clamped again. Luma is
   `dot(v, LUMA)` with `LUMA = (0.2126, 0.7152, 0.0722)` (Rec. 709, the
   aligner's `REC709_LUMA`), where `v` is the sample, sRGB-encoded first when
   the source is linear (`apply_srgb`, `shaders/color.wgsl:linear_to_srgb3`).
   Luma lies in [0, 1]; it is stored as a complex value with imaginary part 0.
3. **Forward transform** (`wavelet.wgsl:forward`, dispatched in
   `mod.rs:accumulate`). A level's input is the padded luma at level 0, else
   the previous level's lowpass band. One 1D analysis step over a line
   `x` of length `N`, for `k < N/2`:

   ```
   lo[k] = Σ_{i=0..5} x[mirror(2k − 2 + i)] · LOWPASS[i]   (complex product)
   hi[k] = Σ_{i=0..5} x[mirror(2k − 2 + i)] · HIGHPASS[i]
   ```

   The taps `LOWPASS`, `HIGHPASS` (complex Daubechies, 6 taps) are at the top
   of `wavelet.wgsl`. The boundary is half-sample symmetric at both ends
   (`mirror`: `−1 → 0`, `N → N − 1`, the edge sample repeated). `LOWPASS` is symmetric and `HIGHPASS` antisymmetric about the
   middle of their taps, so the coefficients of the mirrored line are
   mirrored too (`lo[−1] = lo[0]`, `hi[−1] = −hi[0]`, and the same past the
   far end): `N/2` of each describe it, and an edge's coefficients see that
   edge's own picture.
   Horizontal first (into `temp_low`, `temp_high`), then vertical: `temp_low`
   gives the lowpass band and `LH`, `temp_high` gives `HL` and `HH`
   (`mod.rs:level`). All scratch is rg32float.
4. **Selection** (`wavelet.wgsl:wta`, every level, every frame). For subband
   `s` at coefficient `p` of level `l` (0 finest), the energy is

   ```
   E_s(p) = m(p) · Σ_{dx,dy ∈ {−1,0,1}} 2^(2−|dx|−|dy|) · |c_s(p + (dx,dy))|²
   ```

   weights 4 (center), 2 (edges), 1 (corners), neighbors clamped to the
   level's grid (`energy3x3`). The weight `m` is the footprint test: 1 when
   the coefficient's center `k·scale + (scale − 1)/2` (`coeff_center`,
   `scale = 2^(l+1)`) maps through the warp to within half a pixel of the
   frame's outermost pixel centers (`on_frame`), else `OFF_FRAME` (1e-3).
   The level's `best` buffer holds `(re, im, E, frame)` per subband and
   coefficient, the frame being its accumulation index; buffers start at 0.
   If `E > best.E`, the frame's center coefficient `c_s(p)` replaces the
   winner. With `consistency > 0`, a `runner` buffer keeps the runner-up the
   same way (`wta_subband`). Ties keep the earlier frame (strict `>`).
5. **Live picture** (`live_planes`, level 0 only, when a picture is asked
   for). Per coefficient, the plane of the frame its strongest subband's
   winner came from, cropped to `ceil(w/2)×ceil(h/2)`; `end_pass_one` takes
   it once more before the merge and pass 2 shows that.
6. **Coarsest band** (`wavelet.wgsl:ll_accumulate`, after the coarsest
   level). Per coefficient it sums `c` and counts the frames in one of two
   classes, by the footprint test (`on_frame`) at `scale = 2^levels`: the
   frames with data there, or, while none has come, the frames without
   (the count kept negative). The first frame with data replaces the
   frames without, and later frames without data add nothing, as the
   pyramid stacker's coarsest level sums (`accumulate_sum`). At the merge,
   `ll_normalize` writes `LL = Σ c / max(|count|, 1)`: the mean of the
   frames with data, or of every frame where none has any.
7. **Consistency filters** (`mod.rs:encode_filters`, each level, coarse to
   fine, on the winners in place). Only with `consistency ≥ 1`:
   `subband_consistency`. A subband with `E = 0` does not vote. When two of
   `LH`, `HL`, `HH` name the same frame and the third does not, the third takes
   its runner-up if the runner-up has `E > 0` and is that frame. With
   `consistency ≥ 2`: `snapshot_frames` copies each subband's winning frames,
   then `neighbor_consistency` visits each coefficient not on the level grid's
   border. If its frame is strictly above all four neighbors' (left, right,
   top, bottom, read from the snapshot) or strictly below all four, let
   `avg = floor((sum of the four + 2) / 4)`; the coefficient takes its
   runner-up if that has `E > 0` and frame `avg`.
8. **Denoise** (`wavelet.wgsl:denoise`, each level after its filters, when
   `denoise > 0`). With `t = denoise / 65025` (a 0–255 scale,
   `255² = 65025`, for luma in 0–1) and `e = |c|²` of the winning coefficient
   alone: `c ← c·(e − t)/e` if `e > t`, else `c ← 0`. Phase is kept. The
   coarsest band is not denoised.
9. **Inverse transform** (`mod.rs:encode_inverse`), coarse to fine from the
   averaged `LL`, vertical then horizontal (`inverse_vertical`,
   `inverse_horizontal`). One 1D synthesis step to length `2H` from halves
   `lo`, `hi` of length `H`:

   ```
   y[n] = Σ_{j = n mod 2, step 2, j < 6}
            lo[mirror(q)]·conj(LOWPASS[j]) + s(q)·hi[mirror(q)]·conj(HIGHPASS[j]),
          q = (n − j + 2) / 2
   ```

   with `s(q)` −1 for `q` outside `[0, H)` and 1 inside (`mirror_sign`): the
   adjoint of the analysis step, with the coefficients beyond the ends as
   the symmetric boundary makes them, so a frame alone comes back exactly.

   Vertically `(LL, LH)` give `A` and `(HL, HH)` give `B`; horizontally
   `(A, B)` give the next finer band. `extract_real` keeps the real part of
   level 0's output, cropped to the frame, as the merged target (r32float,
   not clamped or quantized).
10. **Color reassignment** (pass 2, `mod.rs:blend_frame`,
    `wavelet.wgsl:reassign`). `reassign_init` sets each pixel's key to
    `EMPTY` (1e30). For each frame, at each pixel: sample `rgb` as in step 2
    (source encoding), and

    ```
    key = |luma(rgb) − target| + (on_frame(pixel) ? 0 : OUTSIDE)
    ```

    with `OUTSIDE` = 1e6. The pixel keeps `(rgb, key)` and the frame's plane
    if `key` is lower, or equal with a lower plane. `finish` writes
    `clamp(rgb, 0, 1)` with alpha 1, and the planes as the frame map.
11. **Held and streamed.** Pass 1 holds one uploaded frame, the padded luma
    and transform scratch, each level's `best` (3 × 16 B per coefficient,
    doubled by `runner` with consistency on), the coarsest sums and the
    energy record. `end_pass_one` releases them all but the source texture.
    Pass 2 holds the merged target and 20 B per pixel (`pick`, `plane`).
    `finalize` refuses until every stacked frame has been through pass 2
    (`ledger.rs:SecondPass`). `gpu_bytes` gives the peak.
12. **Settings.** `denoise`: 0–10 in steps of 0.1, default 1.0
    (`DENOISE_RANGE`, `DENOISE_STEP`, `DEFAULT_DENOISE`). `consistency`:
    0–2, default 0 (`MAX_CONSISTENCY`).

## Properties and limits

- Every result pixel is one frame's warped, exposure-corrected sample at that
  pixel, clamped to [0, 1]. Each pixel's frame is chosen alone: nothing ties
  neighboring pixels to the same frame.
- Pass-2 order does not change the result. In pass 1, an exact energy tie
  goes to the earlier frame, and the neighbor filter compares accumulation
  indices as numbers.
- The consistency filters can only fall back to the runner-up.
- Frames can be left out only during pass 1, and every stacked frame must be
  read again in pass 2.
- Winners take about 16 B per padded pixel over all levels (12 B at the finest
  level, a quarter of that at each coarser one), 32 B with consistency on.

## References

- B. Forster, D. Van De Ville, J. Berent, D. Sage, M. Unser, "Complex
  Wavelets for Extended Depth-of-Field: A New Method for the Fusion of
  Multichannel Microscopy Images", Microscopy Research and Technique 65(1-2),
  33–42, 2004. doi:10.1002/jemt.20092.
  <https://bigwww.epfl.ch/publications/forster0404.html>. The paper's parts:
  the complex Daubechies transform (6/6 taps, after Lawton and Lina &
  Mayrand), the maximum-absolute-value selection, a subband (2-of-3) and a
  spatial consistency check, PCA gray conversion, the real part of the
  inverse, reassignment by closest gray value, and an optional wavelet
  denoise. Its spatial check is a 3×3 majority vote on the frame map; the
  neighbor filter here is a four-neighbor outlier rule.
- Petteri Aimonen, focus-stack, <https://github.com/PetteriAimonen/focus-stack>.
  MIT license; credited in
  `THIRD_PARTY_NOTICES.txt` and `licenses/focus-stack-LICENSE.md`.

## Decisions

- [ADR-111](../decisions/111-frames-streamed-memory-planned-gpu-failures-are-errors.md):
  why frames are streamed, which is why the wavelet keeps only the winners.
- [ADR-120](../decisions/120-the-stackers-are-tuned-by-eye-and-keep-their-own-kernels.md):
  what was tried for the window before 3×3 (4/2/1), and for the consistency
  filter; the boundary and the coarsest band's two classes.
