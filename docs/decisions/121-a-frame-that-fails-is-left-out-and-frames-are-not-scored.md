# ADR-121: A frame that fails is left out, a few at most; a stack shows where each frame went, not a score for it

## Status

Accepted.

## Context

In a real focus bracket one frame can be corrupt, bumped, blank or too
featureless to align. Failing the whole stack on it wastes the rest of the
bracket, and stacking it unaligned ruins the result. A frame of another
pixel size than the rest is a different case: the user has almost certainly
mixed in a turned photo or a file from another shoot.

Once a stack is made, the apps can tell the user about its frames in several
ways: where each frame's pixels went into the result, how much of the
result's detail each frame gave, and, before stacking, where each frame is
sharp (focus peaking). Each costs GPU work and room on screen, and some
invite the wrong action.

## Decision

### A frame that cannot be read, aligned or uploaded is left out

`StackingSession` (`accumulate` in `crates/deptheos_app/src/runs/session.rs`)
leaves out a frame that fails to read, convert, align or upload. A frame
fails to align when ECC fails, or when Grid search's best match scores worse
than half of what two unrelated images score (`MAX_MATCH_RATIO`).

- The frame keeps its accumulation index and its depth plane as an empty
  slot (`Stacker::skip_frame`). In a Depth stack the frames either side
  blend across the gap.
- The first frame that stacks is the reference: a left-out first frame
  makes the next one frame 0. A frame Grid search rejects does not become
  the frame the next one aligns to; that one aligns to the last good frame.
  When the previous frame is itself featureless there is nothing to judge
  against, and the frame is kept with the search's result.
- At most `left_out_limit(n) = (n / 10).clamp(1, 3)` frames may be left out,
  and at least two must stack (one, in a one-frame stack). Past
  that the stack stops with `TooManyLeftOut`, naming the first left-out
  frame and why: the frames are most likely not one bracket. One in ten, at
  least one and at most three are a judgment, not a measurement.
- The render records each left-out frame and its reason. Reopening it gives
  the list back with those frames excluded, and a notice names them. A
  left-out frame is no change to the working version whether it is included
  or excluded (`Working::changes` in `crates/deptheos_app/src/library/stacks.rs`),
  so reopening does not mark it Edited.
- A stack replayed from its recorded geometry leaves out what it left out
  (ADR-147).
- A cancel or a lost GPU never leaves a frame out: both stop the stack,
  the lost device with its own error (`GpuError::DeviceLost`, ADR-111).

### Frames that don't belong stop the stack

- A frame of another pixel size than the reference stops the stack with an
  error naming the file (`check_frame_size`).
- A frame in another color space is converted into the first frame's
  (ADR-132).
- A frame that can't be stacked at all (CMYK, Lab, HDR, a TIFF layout the
  reader refuses) stops the stack at planning, before anything is decoded,
  naming the file (`CoreError::Refused`).

### A stack shows where each frame went, not a score for it

The apps keep **Contribution** (`C`, View menu) and the **depth map** (`M`,
View ▸ Show Depth Map), both read from the stack's frame map. The depth map
is a toggle over the result, not a view of its own, because it is a
diagnostic looked at now and then. What each looks like is ADR-149.
Contribution is exact for Depth and Wavelet; for Pyramid it is an
estimate, the best-scoring frame at the record level (level 1).

The engine computes and stores no per-frame score. The focus rail's ticks (ADR-149) show each frame's share of the result as their heights, counted from the frame map when a stack is made or read back and never stored, on a control for moving through the stack that has no include, exclude or trim.

The desktop has no focus peaking. Peaking is only in the iOS camera, where
it guides a burst as it is shot (ADR-162).

## Alternatives considered

### Fail the stack on any bad frame

Too brittle on long brackets, where one frame failing to decode or align
would waste the rest.

### Leave out any number of frames, with the notice

In the apps the notice names what was left out, so a limit adds little
there. But a batch or a command-line run saves its render with no one
looking, and a stack that lost a fifth of its frames has bands of missing
focus that a notice does not make usable. The limit stops those runs with an
error instead of a saved render.

### Leave out a frame of another size too

A size mismatch means a turned frame or a photo from another shoot or crop
mode, which the user should fix rather than have skipped without a word.

### Grid alignment that never fails

The search always finds a best candidate, even for a blank or unrelated
frame. Kept, that candidate is a large bogus step that every later frame
chained to it inherits, and it shrinks the auto-crop to a sliver. Real
stacks score at most 0.23 of an unrelated pair even 25 frames apart; black,
gray, noise and mirrored frames score 0.69 to 1.0. Half sits between.

### Per-frame score bars beside the list

They show the bell curve any focus sweep has, and invite unchecking a frame that is the only sharp one for a small feature. Computing them cost every stack a readback, a CPU pass and a second set of per-frame maps in each stacker. Contribution answers where a frame contributed.

### Focus peaking on the desktop

Contribution answers which frames made the finished stack directly, and
peaking before stacking does not match the depth stacker's metric, which
pools over blurs widened by `RADIUS_BLUR_SCALE` (ADR-120).

## Consequences

- Every stacker, Contribution, the depth feathering and every mean divisor
  account for empty slots through `FrameLedger` and `SecondPass`
  (`crates/deptheos_core/src/ledger.rs`).
- A batch or command-line run with many bad frames stops with an error
  rather than saving a render with gaps in its focus.
- A stack pays for no detail maps: a stacker's finished state holds only the
  result, its map and its frame map.

## Code

- `crates/deptheos_app/src/runs/session.rs`, `crates/deptheos_app/src/runs/session/stack.rs`, `crates/deptheos_app/src/runs/stacker.rs`.
- `crates/deptheos_app/src/library/stacks.rs`.
- `crates/deptheos_core/src/ledger.rs`, `crates/deptheos_core/src/error.rs`, `crates/deptheos_core/src/contribution.rs`.
- `crates/deptheos_aligner/src/grid/mod.rs`, `crates/deptheos_aligner/src/grid/score.rs`.
- `crates/deptheos_gpui/src/app/actions.rs`.
