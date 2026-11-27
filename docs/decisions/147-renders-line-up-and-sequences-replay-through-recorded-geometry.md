# ADR-147: Renders and frames line up through each render's recorded transforms; grow and turntable sequences are made in the app, from pyramid snapshots, as numbered image sequences

## Status

Accepted.

## Context

Several features lay one image over another's pixel grid: a source frame beside the result, two renders compared, pixels painted from a frame or another render, a render replayed for a grow sequence. Two renders of one bracket can differ in the frames included, in which frame became the reference (the reverse magnification check, ADR-112) and in their crop.

Deptheos also makes two kinds of focus-stacking animation (`crates/deptheos_app/src/runs/sequence/`):

- **Grow:** frame k is the stack of the first k frames, the subject coming into focus.
- **Turntable:** one stack per angle of a subject on a turntable: **Full** (a finished stack per angle), **Grow across stacks** (focus grows while it turns), or **Both**.

Made outside the app, a grow sequence of N frames means running the command line N times from scratch, O(N²) frames of stacking, and writing tens of gigabytes of aligned intermediate TIFFs a stack.

## Decision

### Line-up from recorded transforms

- **Finished results are never aligned to each other again.** Every render records where each source frame landed, and every mapping between renders and frames is composed from those records.
- **`StackGeometry`** (`crates/deptheos_app/src/runs/session/stack.rs`), stored in `render.json`, records each input frame's affine transform to the reference frame and its brightness factor, the render's crop, whether the order was reversed, and which frames were left out. It is a permanent part of the render's format (ADR-141).
- **A source frame the render stacked** is warped by its recorded transform and the render's crop offset in one Lanczos-3 GPU pass (`crates/deptheos_core/src/warp.rs`), resampled once from the original frame.
- **Another render** (`CompletedStack::alignment_to`): through any frame c both stacked, `T_other(c)⁻¹ · T_self(c)` and the other render's crop origin, with exposure matched by `b_self(c) / b_other(c)`. Renders of different frame subsets, reference frames and crops line up this way, their shared pixels to 0.000 mean error.
- **A frame the render didn't stack** (`frame_alignment`) goes through a sibling render in the history that stacked both that frame and a frame shared with this render.
- **Replays reuse `StackGeometry`** with no alignment: grow sequences, the command line's depth parameter sweeps (ADR-103) and `StackingSession::with_alignment`.
- **Refused:** `alignment_to` finds no alignment for renders whose recorded RAW backends differ (`RawBackend`: LibRaw, Adobe DNG Converter and Apple's decoder crop and demosaic differently, ADR-131), or that share no frame recorded the same way (aligned in both, or unaligned in both); a frame the render didn't stack goes only through siblings of its backend. Comparing or painting from another render is refused when one is linear (RAW or DNG frames) and the other is not (`Error::MixedRenders`, in `Engine::warp_render`), and a source frame of the other kind than the stack is refused before it is decoded (`Engine::upload_frame`).

### Sequences

- **In-place pyramid snapshots.** `PyramidStacker::collapse()` collapses the running pyramid accumulators into an output frame without ending the accumulation. The Pyramid stacker's soft winner-take-all weights each frame independently of the order frames arrive in, so a snapshot after k frames equals a k-frame stack up to rounding, and a grow sequence's last frame equals the finished render.
- **A grow sequence comes from a finished Pyramid render**, replaying its stacked frames through their recorded transforms and crop, with no alignment, and taking a snapshot after each frame (back to front by default). Depth and Wavelet are two-pass methods whose state after k frames is not a finished stack, so grow needs a Pyramid render; a Full turntable works with any method.
- **The turntable is part of Batch**, under "After stacking" in the Batch dialog, and `deptheos sequence turntable` on the command line. One run stacks every angle's folder, checks each stack for a stray leading frame and stacks it again without one (`sequence/check.rs`), takes the crop common to every stack kept, and writes the sequence.
- **The output is a numbered image sequence, and nothing else**, on every platform: `<name>_0001.tif` … with a `manifest.json`, written by `SequenceWriter` (`sequence/writer.rs`) and closed by `close(self)` (ADR-143). Every editor the frames are for imports them as one clip and encodes a movie itself.
- **The frame order** (Name or Capture Time, `crates/deptheos_app/src/frames/order.rs`) is recorded in `render.json`, so a replay feeds frames in the exact order they were aligned.

## Alternatives considered

### Align finished results to each other

Slower, can fail or drift on the differences different stacking methods make, and unneeded when both renders already know their exact transform to a shared source frame.

### Stack 1..k from scratch for each frame of a grow sequence

O(N²) accumulations instead of O(N) snapshots in one pass, and gigabytes of intermediates.

### Encode H.264 or HEVC video on every platform

Hardware encoders often reject full camera resolutions (8192×5464, say), and Windows has no built-in ProRes encoder. Full-resolution numbered TIFF, PNG or JPEG frames work at any size on every system and import directly into editors such as DaVinci Resolve.

### A ProRes movie beside the frames on macOS

A ProRes 422 HQ movie written beside the frames: one platform's feature, with color tagging of its own to get right (sRGB values tagged BT.709), for a movie every editor the frames are for makes itself when it imports them as one clip. The command line has no `--prores` or `--fps` option (`crates/deptheos_cli/src/args.rs` tests that both are refused).

## Consequences

- Lining up a compared render or a paint source is one GPU warp with no alignment search.
- Two renders that share no stacked frame can't be compared or painted from each other.
- A grow sequence replays the unretouched pyramid stack and has none of paint mode's strokes; the Export Grow Sequence dialog says so when a render has paint.
- A grow sequence draws every frame under the final render's saved sliders (`History::adjustments_to_export`), not its reframe: every frame keeps the stack's size.

## Code

- `crates/deptheos_app/src/runs/session/stack.rs`, `crates/deptheos_app/src/runs/session.rs`.
- `crates/deptheos_app/src/engine.rs`.
- `crates/deptheos_core/src/warp.rs`, `crates/deptheos_core/src/pyramid/mod.rs`.
- `crates/deptheos_app/src/runs/sequence/`, `crates/deptheos_app/src/frames/order.rs`.
- `crates/deptheos_gpui/src/batch_dialog.rs`, `crates/deptheos_gpui/src/app/sequence.rs`, `crates/deptheos_cli/src/sequence/`.
