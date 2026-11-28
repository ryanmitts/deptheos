# ADR-114: An open render keeps its full-size source, a reduced copy and the local tone's planes, for one generation of its pixels, within a stated budget; large passes reuse textures from a pool

## Status

Accepted.

## Context

Adjust redraws one render many times a second while a slider moves. Each draw used to upload the stored pixels, make the local tone's planes from them and allocate its output, then let all of it go. At 45 megapixels the upload alone is 363 MB, the planes take seconds of CPU work, and a fresh 360 MB texture per pass costs about as much as the pass. lightcraft (MIT OR Apache-2.0) found the same: allocating fresh 100 to 300 MB buffers per pass cost as much as the passes (`docs/gpu-pipeline.md`).

Keeping things between draws needs a rule for when they go stale and a bound on what they hold. On a Mac and an iPhone the GPU's memory is the app's memory too. ADR-164 gives iOS no fixed number: what is left is read live.

## Decision

### What an open render keeps

While Adjust is open on a render, `GpuCopies` (`crates/deptheos_app/src/adjust/copies.rs`) holds these, made for one generation of the stored picture with its paint:

- **The full-size source**, the stored pixels with paint, uploaded as the draw takes them: Rgba16Unorm, or Rgba32Float for a table-based profile linearized on the CPU (ADR-135).
- **The reduced copy**: long side at most 4096, Rgba16Float, linear (`crates/deptheos_render/src/dng_render/reduce.rs`). Each texel is the mean of the area of the source it covers, with edge texels weighted by how much of them it covers, taken in values as they show and turned linear again (ADR-138), so a preview drawn from it keeps the level the viewer shows. A standard stack's codes are decoded through its curves first. Drawn with `Constants::reduced`, which says the values are already linear. A wider filter such as Lanczos rings. At most a 2.02 times reduction at 45 MP leaves little to alias. The reduced copy is never saved, and exports and thumbnails never read it.
- **The fit preview's copies** (ADR-138): the reduced copy at the size shown, and the local tone's values of the whole frame on a grid and at that size.
- **The local tone's planes** (ADR-137), kept by `KeptPlanes` (`crates/deptheos_render/src/dng_render/draw.rs`) with the key they were made under (`Constants::planes_key`). The key holds which planes are needed and every value the log brightness reads: the white balance, the camera matrix and its hue-saturation table, the baseline exposure, and a standard stack's decode. A drag of exposure, the curve or a local slider's amount keeps them; a move of the white makes them again.

### Generations

A render's generation (`crates/deptheos_app/src/engine/generation.rs`) is a number for its stored picture with paint. Paint raises it on every commit, undo, redo and reset. Copies are made for one generation; when the generation has moved, they are made again. Nothing else makes them again. Numbers are unique for the launch, so a paint document that reloads can't reuse one.

### The texture pool

Large textures made per pass come from a pool on the context (`crates/deptheos_gpu/src/context/pool.rs`): a redraw's output, the log brightness and the planes' values per strip, a neighbor stage's input. A texture is reused only at its exact size, format and use, since a draw's bounds come from its texture. A dropped texture waits in its thread's list until that thread next submits, because commands it recorded may still read it. Outside a draw (`GpuContext::drawing`) nothing is recorded, so it joins the pool at once. The pool keeps at most 768 MB (256 MB on iOS), oldest freed first, and a memory shed empties it (`engine::memory::shed`). This follows lightcraft 5bdb4e7a6 `crates/gpu/src/ctx.rs` (`Buf`, `RenderScope`, `Gpu::pool`), for textures.

### The budget

`RenderMemory` (`copies.rs`) counts one open render and logs it at debug when its copies are made. At 45 MP (8256 × 5504) on Rgba16Unorm:

| On the GPU | MB |
|---|---|
| Full-size source | 363 |
| Reduced copy (4096 × 2731) | 89 |
| The fit preview's copies (2048 × 1365): the copy and its values in 16-bit float, the grid in 32 | 89 |
| One full redraw's output | 363 |
| A redraw's strip with local tone: its planes' values and the log brightness around it | 338 |
| **At its height** | **1,244** |

A region tick past half zoom on a 2560 × 1600 screen (`VIEWPORT`), its output and its values, takes 393 MB; a redraw lets the region's values go first, so the count takes the larger of the two. `GPU_BUDGET_45MP` is 1,280 MiB (1,342 MB), and a test holds the count to it. The pool can keep up to 768 MB more, which a shed empties.

Measured on an M5 Max at 45 MP: the upload and the reduced copy take 58 ms (the reduction alone 7 ms). The planes for Highlights, Shadows and Clarity take about 470 ms to make, and a full redraw with them kept takes 490 ms, most of it the per-strip values on the CPU.

At the same moment the CPU holds the stack's pixels (272 MB, RGB u16), the frame map (91 MB), the display's levels (242 MB in 8-bit with their halves, two during a swap), the kept planes (22 MB with Highlights, Shadows and Clarity), a strip's values and log brightness as they are read back (338 MB), and the preview's two flat pictures (131 MB at the largest region). On a Mac or an iPhone these share one memory with the GPU's. On iOS Adjust checks the live reading (`engine::memory::left`, less its reserve) before it makes its copies or draws in full, not a fixed number (ADR-164). Under memory pressure the session lets go in two steps (ADR-153): a moderate shed frees the preview's copies at the sizes drawn and the region's values (about 150 MB at 45 MP), keeping the source, the reduced copy and the planes; a critical one frees every copy until memory is no longer short.

## Alternatives considered

- **Upload on every draw.** It is simple, but at 45 MP the upload, planes and output cost more than the draw.
- **A reduced copy averaged in linear light.** It keeps the physical level, but the viewer shows the mean of display values, so a drag would end with a jump on fine detail (ADR-138).
- **A reduced copy made by point sampling, or kept in 8 bits.** Point sampling shimmers and moves the level on fine detail. 8 bits bands once exposure lifts the shadows.
- **Pooled textures reused by best fit,** as lightcraft does for buffers. A texture's size is its draw's bounds, so only an exact fit is safe.
- **Keying the planes on the whole constants.** Then every exposure tick would rebuild them.

## Consequences

- A change to the pixels that doesn't go through paint's commit, undo, redo or reset must raise the render's generation itself, or Adjust shows stale copies.
- A new value the log brightness reads must join `Constants::planes_key`.
- A pass that records into an encoder of its own and drops a pooled texture before submitting must run inside `GpuContext::drawing`.

## Code

- `crates/deptheos_gpu/src/context/pool.rs`, `crates/deptheos_gpu/src/context.rs`.
- `crates/deptheos_render/src/dng_render/reduce.rs`, `crates/deptheos_render/src/dng_render/reduce.wgsl`, `crates/deptheos_render/src/dng_render/draw.rs`, `crates/deptheos_render/src/dng_render/mod.rs`.
- `crates/deptheos_app/src/adjust/copies.rs`, `crates/deptheos_app/src/engine/generation.rs`, `crates/deptheos_app/src/paint/worker.rs`, `crates/deptheos_app/src/engine/memory.rs`.
- `crates/deptheos_render/tests/integration/reduced.rs`, `crates/deptheos_render/tests/integration/local_tone.rs`.
