# Grid-search alignment

The grid aligner finds, for each frame, the similarity transform (x shift,
y shift, rotation and one magnification) that best lays it over the frame
before it. It scores every point of a small four-dimensional grid of
candidate transforms, moves to the best one, shrinks the grid and repeats,
working from a small copy of the frames up to a half-size one. It uses no
image gradients. It is the second aligner beside ECC (ADR-112): it copes
with exposure shifts and low texture, and its search limits can be widened
for large handheld jumps, but it fits only a similarity. The code is
`crates/deptheos_aligner/src/grid/`: the search in `mod.rs`, the scorer in
`score.rs`.

## Inputs and outputs

- **In:** the previous aligned frame and the new frame, each as a gray
  pyramid (step 1), the transform from frame 0 to the previous frame, and a
  `GridConfig`: the four search limits (`horizontal_pct` and
  `vertical_pct`, default 5 % of the longer side, 0–25; `rotation_deg`,
  1°, 0–10; `magnification_pct`, 3 %, 0–15; a limit of 0 turns its axis
  off), `sample_points` (10,000) and `apply_brightness` (off).
- **Out:** a `Step`: the previous frame → this frame transform as an affine
  matrix in full-resolution pixels, and a brightness factor. `Aligner`
  (`lib.rs`) composes the step onto the sequence, so every frame ends up
  with a transform from frame 0, the reference.
- **Or an error** (`AlignmentError::NoOverlap` or `NoMatch`) when the frame
  does not match the previous one (step 8). The stack leaves that frame
  out, and the next frame is aligned to the last frame that matched.

The transform is held as a `Similarity` about a pair of centers. It maps a
point `p` of the previous frame to the new frame:

```
F(p) = dst + m · R(−rotation) · (p − src)
```

`src` and `dst` start at the layer's center, `rotation` at 0° and the
magnification `m` at 1. The search moves `dst`, `rotation` and `m`; `src`
stays fixed.

## The algorithm

1. **Gray map and pyramid** (`GridAligner::prepare`). The frame is reduced
   to the equal-weight gray `(r + g + b) / 3` and resized with the Keys
   cubic to the map: `MAP_SCALE` = 0.5 of the frame, or less when the frame
   is wider than 12,000 px, so that the map is at most `MAP_MAX_DIMENSION`
   = 6000 px wide. A box pyramid is built on the map: each layer is the 2×2
   average of the one below, `ceil(dim / 2)` in size, until a side would
   drop below `MIN_LAYER_SIZE` = 32. Layer 0 is the map. A frame's pyramid
   depends on the frame alone, so it is built ahead of the search on any
   thread.

2. **Frame 0** (`GridAligner::first`). Frame 0 only becomes the first
   reference. If the standard deviation of its map is below
   `FEATURELESS_STD` = 1/512 it is an error, and the next frame is tried as
   frame 0.

3. **Where the samples are** (`detection_rect_through`, `sample_points`).
   The middle of frame 0's map (`DETECTION_RECT`, from 10 % to 90 % of each
   side) is carried through the sequence transform into the previous
   frame's coordinates and reduced to the axis-aligned rectangle inside it.
   On each layer the rectangle is scaled to the layer, and about
   `sample_points` (10,000) positions of the new frame are taken on a
   regular grid inside it:

   ```
   step_x = max(1, floor(sqrt(area / sample_points)))
   step_y = max(1, floor(area / sample_points / step_x))
   ```

   Every layer uses the same target, so a small layer is sampled at every
   pixel.

4. **Gain** (`brightness_ratio`). Before the search, the ratio of mean
   brightness, previous frame over new frame, is measured with the frames
   laid over each other unmoved, at about `BRIGHTNESS_SAMPLES` = 240,000
   positions of the rectangle. Positions that fall outside the previous
   frame are skipped on both sides. The ratio is limited to
   `MAX_BRIGHTNESS_RATIO` = 4 either way, and is 1 when either side is
   black. Every sample value of the new frame is multiplied by it.

5. **The score** (`difference_scores`, `score.rs`). A candidate transform is scored by
   the standard deviation of the difference between the frames at the
   sample points. Each sample position `q` of the new frame is mapped back
   to the previous frame, `F⁻¹(q)`, and read there bilinearly:

   ```
   d_i   = ratio · new(q_i) − previous(F⁻¹(q_i))
   score = sqrt(mean(d²) − mean(d)²)
   ```

   Lower is better. Samples that map outside the previous frame are left
   out of the sums; a candidate with no sample left scores `f32::MAX`.
   Subtracting the mean makes the score ignore a constant brightness
   offset, and the gain of step 4 removes a brightness scale.

6. **The search** (`search`, `align_one_layer`). The search holds one step
   size per axis. The step sizes start at the search limits: the horizontal
   and vertical shift limits as fractions of the layer's longer side, the
   magnification limit as a fraction, and the rotation limit in degrees.
   The layers are searched from the coarsest to layer 0. The point found on
   one layer, with its centers scaled, starts the next, and the step sizes
   carry over as they are.

   Each layer has a finest spacing per axis, `unit`. One unit moves a pixel
   at the edge of the layer by `granularity` pixels, where `granularity` is
   `COARSE_LAYER_GRANULARITY` = 0.5 on every layer but layer 0 and
   `FINEST_LAYER_GRANULARITY` = 0.2 on layer 0. With `r` half the layer's
   longer side:

   ```
   unit(shift)         = granularity / (2r)      fraction of the longer side
   unit(magnification) = granularity / r
   unit(rotation)      = granularity · asin(1 / r)   in degrees
   ```

   A layer is searched in rounds (`Grid`):

   1. **Build the grid.** Each axis gets
      `n = min(2 · trunc(step / unit) + 1, MAX_STEPS_PER_AXIS)` positions
      (`MAX_STEPS_PER_AXIS` = 7), evenly spaced `2 · step / (n − 1)` apart
      over `±step` around the current point. `n` is odd, so the current
      point is the center of the grid. An axis whose step is smaller than
      its unit has one position and is not searched. When every axis has
      one position, the layer is finished.
   2. **Score** every combination, up to 7⁴ = 2401 candidates, in parallel.
   3. **Take the lowest score.** A candidate has to score strictly lower
      than the center to replace it, so an exact tie keeps the current
      point (`best_from`). If no candidate could be scored, the layer ends.
   4. **Refine** (`Grid::refine`). On each axis where the winner has a
      neighbor on both sides, a parabola is fitted through the winner's
      score and its two neighbors' scores, and the point moves to the
      parabola's minimum:

      ```
      frac = clamp(0.5 · (s_lo − s_hi) / (s_lo − 2·s_mid + s_hi), −0.5, 0.5)
      ```

      in units of the grid spacing, applied only when the denominator is
      positive. Each axis is fitted separately, and the refined point is
      not scored again.
   5. **Contract** (`Grid::contract`). On each axis where the winner was
      inside the grid, the step becomes `min(step / 2, spacing)`: half the
      step when the axis had 3 or 5 positions, a third of it when it had 7.
      On an axis where the winner was at either end of the grid, the step
      is kept, so the next round searches the same distance again around
      the new point.

   A layer runs at most `MAX_ROUNDS_PER_LAYER` = 64 rounds.

7. **The step** (`step_between`). The point found on layer 0 is turned into
   an affine matrix and rescaled from map pixels to full-resolution pixels
   by `1 / map_scale`.

8. **Does the frame match?** (`match_quality`, `Match::rejection`). The
   search always returns some best candidate, so the result is judged
   afterwards on layer 0. With the variances taken over the samples that
   land in the previous frame:

   ```
   unrelated = sqrt(var(new) + var(previous))
   reject when score > MAX_MATCH_RATIO · unrelated      (MAX_MATCH_RATIO = 0.5)
   ```

   `unrelated` is what two uncorrelated images would score. A previous
   frame with a standard deviation below `FEATURELESS_STD` gives no
   verdict, and the frame is kept. A frame with no sample inside the
   previous frame is an error.

9. **Brightness factor.** With `apply_brightness` on, the ratio of step 4
   is measured again with the transform just found and returned as the
   frame's brightness factor, which the stackers multiply the frame by.
   With it off (the default) the factor is 1; the ratio of step 4 is still
   used for scoring.

When all four limits are 0 the search is off: every step is the identity
with a factor of 1, and no frame is judged.

## Properties and limits

- **The limits bound the first step, not the result.** A winner at the end
  of an axis keeps that axis's step and the next round searches from the
  new point, so the search can travel past a limit. Nothing clamps the
  final transform to the limits.
- **No guarantee of the best match.** Each round takes the best of a grid
  whose spacing may be wider than the score's features, mostly on the
  coarse layers. The edge rule of round step 5 exists so that a mistake on
  a coarse layer can still be corrected on a finer one: if every axis
  contracted every round, the finer layers could move an axis by less than
  the step that made the mistake.
- **The precision is finer than the grid.** The last spacing on layer 0 is
  about 0.2 px at the map's edge, 0.4 px of the frame. The parabola fit of
  round step 4 recovers the position between grid points. Without it, a
  slow magnification change over a long stack is under-read by about one
  spacing per frame.
- **The same on every platform.** The sines and cosines come from the
  `libm` crate, and the brightness sums are added in row order, so the
  search takes the same path on every CPU.
- **Similarity only.** Shear and separate x and y scales are not searched;
  the ECC aligner fits those (`ecc.md`).
- **One gain for the whole frame.** Brightness that changes unevenly
  between frames (vignetting, a moving shadow) is not modeled and raises
  the score of the right transform along with the wrong ones.
- **Cost.** A round is at most 2401 candidates × 10,000 bilinear reads. It
  runs on the CPU with rayon, one task per candidate, or per four in a
  large round, and reads the samples a SIMD vector at a time. Each
  candidate's sums are still added sample by sample in order, so the
  scores are the same on every instruction set.

The stacking session's reverse check reads the first step of whichever
aligner is in use: if frame 1 comes out magnified relative to frame 0
(magnification above 1 + `REVERSE_DEADBAND`, 0.1 %), alignment restarts with the frames in reverse
order (`deptheos_app/src/runs/session.rs`).

## Related work

No paper below describes the whole method. Each is cited for the one part
it covers.

- **The step rule.** J. R. Jain and A. K. Jain, "Displacement Measurement
  and Its Application in Interframe Image Coding", IEEE Transactions on
  Communications, vol. COM-29, no. 12, pp. 1799–1808, December 1981. Its
  2-D logarithmic search (section II) scores five positions, moves to the
  best while keeping the step, and halves the step only when the center
  wins. The rule of round step 5 is the same idea applied to each axis
  separately: an axis keeps its step when its best value is at the end of
  its grid and contracts otherwise. This code differs in searching four
  parameters jointly, with up to 7 positions per axis, and in contracting
  an axis whenever its winner is inside the grid, not only at the center.
- **Coarse-to-fine search, the fitted minimum and the score.** R. Szeliski,
  "Image Alignment and Stitching: A Tutorial", Foundations and Trends in
  Computer Graphics and Vision, vol. 2, no. 1, pp. 1–104, 2006,
  doi:10.1561/0600000009 (read as the technical report MSR-TR-2004-92).
  Section 3.2 describes hierarchical motion estimation: a search over a
  few discrete positions on a coarse level starts a smaller search on the
  next finer one. Section 3.4 describes evaluating discrete values around
  the best found and interpolating the matching score to a minimum, as
  round step 4 does. Section 3.1 gives the bias-and-gain model of
  brightness; the score here fixes the gain from the mean brightness and
  removes the bias by taking a standard deviation.

## Decisions

- [ADR-112](../decisions/112-alignment-on-the-cpu-each-frame-to-the-one-before.md):
  alignment runs on the CPU, with two aligners, ECC and grid, each frame
  aligned to the one before.
- [ADR-121](../decisions/121-a-frame-that-fails-is-left-out-and-frames-are-not-scored.md):
  a frame that does not match is left out.
