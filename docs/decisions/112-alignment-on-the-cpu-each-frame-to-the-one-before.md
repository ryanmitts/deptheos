# ADR-112: Alignment runs on the CPU, in SIMD Rust on a bounded pool, with two aligners that each align a frame to the one before

## Status

Accepted.

## Context

Deptheos aligns frames on every platform (macOS, Windows, Linux and iOS). Focus breathing changes the magnification from frame to frame, and a rail or a handheld stack adds shift and rotation. No one alignment model fits every stack: affine correlation can struggle on low-texture or exposure-shifted frames, while a similarity search cannot express the shear or anisotropic scale of a deep scene with rail parallax. ECC is iterative, and each Gauss-Newton iteration ends in a 6×6 linear solve on sums reduced over the image.

## Decision

### Two aligners, chained

`deptheos_aligner::Aligner` (`crates/deptheos_aligner/src/lib.rs`) owns the sequence and runs one of two steps, chosen by `AlignMode`:

- **ECC** (`ecc/`, the default): a full affine transform, coarse to fine from about 256 px to 2,048 px, by Gauss-Newton steps over the pixels both frames cover.
- **Grid** (`grid/`): a coarse-to-fine pattern search for a similarity transform (shift, rotation, one magnification) over about 10,000 samples from the middle 80% of a half-size grayscale image, with a brightness ratio as a by-product.

Both align each frame to the previous good frame and multiply the steps, so frame 0 is the reference and a failed frame changes nothing. A **reverse check** restarts alignment with the frames reversed when the second frame is magnified relative to the first, so the most magnified frame sets the output and nothing is scaled down. It trips only past a magnification of 1.001 (`REVERSE_DEADBAND`, `crates/deptheos_app/src/runs/session.rs`), above the aligners' wavering around 1, and it is always on: neither app nor the CLI can turn it off, and no setting carries it. `render.json` keeps a `reverse_check` key, always `true`, ignored on read.

A frame that doesn't align is left out (ADR-121) rather than chained through, so later frames never inherit a bad step. ECC rejects on correlation (below 0.6), overlap (below 25%) and the step's plausibility (5% shift, 5% scale or shear, 5° rotation). Grid rejects a frame whose best score is above half of what two unrelated images score (`MAX_MATCH_RATIO`), and gives no verdict when the previous frame is featureless.

### The crop

With Crop on, the result is cut to the rectangle every aligned frame covers, so no edge shows a frame's empty border (`Aligner::crop`). It crops only while that rectangle is at least a quarter of the frame: under that the frames moved too far for a crop to keep the picture, so the result is left uncropped and a notice says its edges may show smear (`note-uncropped`). A render replayed from its recorded geometry keeps its recorded crop (ADR-147).

### On the CPU

`crates/deptheos_aligner` is CPU-only, with no `wgpu` dependency.

- **Parallelism and SIMD.** Row work runs on `rayon`; the inner loops (the warp's interior, gray and resize, blur, the Keys cubic pass and grid scoring) run on `rten-simd`'s runtime-dispatched instruction sets (SSE2, AVX2 and AVX-512 on x86, NEON on ARM). One ECC iteration is one fused pass (warp, masked statistics, Hessian, projections and correlation per row block), with `f32` lane sums and an `f64` merge and solve.
- **The caller owns the threads.** The crate makes no global pool. `StackingSession` builds a bounded pool (`SessionOptions::align_threads`: a third of the hardware threads, 2 to 8) and installs it around alignment preparation, on the prefetch threads, and the step, on the session's thread. It is the stack's share of one CPU budget (ADR-143): each frame's decode gets the cores the aligner leaves, divided by the read-ahead window.
- **Few dependencies.** A separable 7-tap blur and the crate's own `Plane` keep it small; `nalgebra` carries the transforms.
- **Bit-identical across instruction sets.** Loops are vectorized across pixels, samples or candidates, never by reassociating one value's sum, and no `algebraic_*` float operations are used. ECC's sums are the exception: they use `rten-simd`'s `mul_add`, a fused multiply-add where the CPU has one (NEON, AVX2 and up), so an ECC transform can differ in its last bits between instruction sets, and is the same from run to run on one. The grid aligner's `sin`, `cos` and `atan2` come from `libm`, so its transforms are bit-identical on every platform.

## Alternatives considered

### One aligner only

ECC and the grid search complement each other: ECC fits full affine warps with no user parameters, while the grid search handles exposure shifts (with optional exposure compensation when stacking) and lets the user widen its search limits for large handheld jumps.

### Aligning every frame against frame 0

In a deep stack frames 0 and 45 share almost no in-focus detail, so matching against frame 0 has no fine-scale geometric signal (±0.5% scale noise, ±0.1° rotation noise and 10–60 px corner jitter on a 46-frame 45 MP stack). Adjacent frames are the most alike: chaining adjacent steps and masking ECC's statistics to the pixels both cover took 46-frame forward-and-back loop-closure corner drift from 35.9 px to 0.63 px.

### Gradient-weighted grid samples

Weighting samples by squared gradient made 46-frame loop closure worse (10.9 px at the worst corner against 5.9 px with uniform weights).

### ECC in WGSL on the GPU

ECC needs a GPU-to-CPU readback every iteration for the 6×6 solve: free on unified memory, expensive on discrete GPUs through `wgpu`. On the CPU, preparation runs on the prefetch threads beside decoding, and the step (4.6–6.3 ms per 45 MP frame on an Apple M5 Max across 6 threads) overlaps the GPU stacking the previous frame.

### Gray and downscale preparation on the GPU

A GPU gray and 4× downscale matched the CPU's plane to 3e-7 and saved about 0.1–0.2 s on a 46-frame 45 MP JPEG stack (Ryzen 9 9950X3D and RTX 5090), not worth an extra device copy and readback buffer for every staged frame in flight.

### Rayon's global pool, or a smaller pool

On the global pool every prefetch thread fanned out over all cores (eight prefetch workers and the session thread), starving the single-threaded decoders and slowing the step through contention; the bounded pool aligns as fast and leaves cores for decoding. At a quarter of the threads, preparation became the limit once 8 frames decoded at once (from 50 to 90 ms per 45 MP frame, waiting for the pool). A pool also keeps preparation parallel when the window is small (2 frames), where running it on each prefetch thread in turn would serialize it.

### Plain loops in place of explicit SIMD

Measured on an Apple M5 Max, the gradients are faster as a plain auto-vectorized loop and stay plain Rust. The other kernels (ECC's sums, the warp interior, the blend, grid scoring) stay explicit because the compiler doesn't vectorize them as well: ECC's sums take 51 ms as a plain loop against 17.7 ms with `rten-simd`.

### Crop to the covered area however small it is

A handheld stack with one large jump would come out a sliver of the picture. Uncropped, with the notice, keeps the picture and says why its edges smear.

## Consequences

- Chaining accumulates small errors along the stack; the loop-closure tests (`compare_aligners`, `LOOP_CLOSURE=1`) hold the drift to bounds.
- The grid aligner cannot express shear or anisotropic scale.
- Alignment competes with decoding for cores and memory bandwidth, so the aligner's pool and the read-ahead window are tuned together (`SessionOptions`, ADR-111).
- A SIMD change keeps the scalar path's order of operations, so results stay bit-identical across instruction sets.
- A frame with binary alpha brings its data area (ADR-132): `PreparedFrame::with_data_area` bounds what the aligner counts as covered by its largest opaque rectangle, so a pre-aligned frame's transparent border is cropped like the empty border of a moved frame.

## Code

- `crates/deptheos_aligner/src/lib.rs`, `crates/deptheos_aligner/src/ecc/`, `crates/deptheos_aligner/src/grid/`.
- `crates/deptheos_app/src/runs/session.rs`.
- `docs/algorithms/ecc.md`, `docs/algorithms/grid-search.md`.
