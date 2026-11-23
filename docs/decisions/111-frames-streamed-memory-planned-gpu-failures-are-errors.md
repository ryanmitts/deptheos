# ADR-111: Stackers take frames one at a time, a stack's memory is planned before its first decode, and GPU failures are errors

## Status

Accepted.

## Context

A focus stack is tens to hundreds of high-resolution frames (46 CR3 frames are about 12 GB decoded on the host and about 32 GB as `rgba32f`), which do not fit in RAM or VRAM at once. The pyramid stacker needs each frame once; the depth stacker, which needs a finished depth map before it blends, and the wavelet stacker, which reassigns color after merging coefficients, see every frame twice.

Large stacks can also exceed the memory of discrete GPUs, iOS devices and software renderers. D3D12 and Vulkan drivers page overcommitted GPU memory to system RAM until the driver stalls or resets, and on iOS the system ends an app that goes past its memory. `wgpu`'s default error handler panics on any validation or out-of-memory error, and a failed call whose error is suppressed leaves an invalid object behind, so later work fails silently and the render comes out corrupt.

## Decision

### Frames, one at a time

- **Streaming stackers.** `PyramidStacker`, `DepthStacker` and `WaveletStacker` (`crates/deptheos_core/src/{pyramid,depth,wavelet}/`) take one uploaded frame a call and keep only running GPU state. Each has its own `State` enum (the pyramid: uninitialized, accumulating, finalized; the depth stacker adds blending and failed; the wavelet stacker reassigning and failed), with no shared trait; `crates/deptheos_app/src/runs/stacker.rs` dispatches across them by `match`.
- **The session reads frames, not the stacker.** `StackingSession` (`crates/deptheos_app/src/runs/session.rs`) decodes frames ahead on prefetch threads, runs each frame's alignment preparation beside its copy into a staging buffer, and on its own thread takes the alignment step, uploads and calls the stacker. For a second pass it decodes the frames again by accumulation index, reusing pass 1's transforms.
- **One record of stacked and left-out frames.** `FrameLedger` (`crates/deptheos_core/src/ledger.rs`) gives every frame an index in pass 1, stacked or left out (ADR-121). `SecondPass`, inside the second-pass state, refuses a duplicate frame and makes `finalize` wait until every stacked frame has been supplied once.
- **Read-ahead by source type and byte budget** (`runs/session/reading.rs`). DNGs, and frames a converter makes, which decode as DNG, read 1 ahead: the DNG SDK's decode is multi-threaded, and a window of 1 stacks DNGs about twice as fast as 2 with up to about 2 GB less peak memory. Camera RAW (LibRaw's, or the system's on iOS) reads 1 per 8 hardware threads, at most 4. Standard images read as many as a byte budget allows, from 2 up to half the hardware threads. On the desktop Settings sets that budget ("Frame read-ahead (MB)"); on iOS it is a sixth of what the app has left, within bounds, read as the stack starts (`engine/memory.rs`).
- **The window is a speed choice only** (the same pixels at 1, 2, 4 and 9 in flight), so a stack that would not fit in memory beside it reads fewer frames ahead, down to one; both passes and the reverse restart (ADR-112) keep the window a run chose.

### Memory, planned before the first decode

- **One memory policy.** The limit, the reserve, the floor and a stack's budget are read from one module, `deptheos_app::engine::memory` (`crates/deptheos_app/src/engine/memory.rs`). `GpuContext::memory_budget` (`crates/deptheos_gpu/src/context/memory.rs`) knows only the GPU's: Metal's recommended working set, for the GPU's allocations alone (`MemoryPool::Gpu`); none where the device reports none (D3D12, Vulkan, and the Simulator's working set of 0).
- **What a stack needs is estimated before anything is allocated** (`memory_need`, `MemoryNeed`): the stacker's `gpu_bytes` at the frames' depth (ADR-110), the frame-size buffers (the window's, the one being fed, the one the GPU is reading) and the uploader's own buffer where staged frames are copied. Each stacker's `gpu_bytes` is one figure a pixel for what it holds besides its source (`BYTES_PER_PIXEL`: the pyramid 56, the depth map 50, the wavelets 45, or 61 with a runner-up, a pixel of the padded frame), Metal's padding included, plus the source. `crates/deptheos_core/tests/gpu_memory.rs` holds each figure to Metal's measured allocation at the peak: at or above it, by less than 15%. Completing a stack holds less than its peak (the frame buffers and decodes are gone, and every stacker drops its accumulators at finalize), and the same test checks that a finished stacker and its readbacks come to less than the estimate, so they are not counted separately.
- **On iOS the CPU's memory counts too, less a reserve.** The app's CPU and GPU memory are one pool, and the budget is what the app has left (`os_proc_available_memory`) less a reserve for the rest of the app, taken in one place (`engine/memory.rs`) so the check and the refusal agree. The need then adds what the CPU holds while stacking: each decode in flight at its peak, with the decoder's own working memory as measured of the app's decoder (`script/check-bitmap-decode.sh`), and, when aligning, each of those frames' preparation with the one being aligned and the aligner's reference. Pass 2 holds no preparations, so pass 1 counts for both. Where the app has no limit (the Simulator), the budget is the GPU's alone, and there that is none too, so a stack there is checked only against the size and binding limits.
- **On macOS the working set is a paging threshold, not a limit.** The CPU's memory isn't counted against it, nothing is reserved, and what the engine's device already holds (a Retouch session's sources) isn't subtracted.
- **On D3D12 and Vulkan the driver guards.** The `wgpu` instance refuses an allocation past 95% of the memory budget the driver reports (`memory_budget_thresholds`, `crates/deptheos_gpu/src/context/adapter.rs`), an out-of-memory error that `GpuContext::check` returns, instead of overcommitting. The guard is off for the CPU renderer (WARP) and integrated GPUs, whose memory is the system's RAM, where the reported budget can be far below what is free: D3D12's for WARP in a 12 GB VM refused a 5 GB stack, and Vulkan's for an AMD integrated GPU refused a 179 MB texture where the same GPU took 7 GB unguarded.
- **Size and binding limits are checked upfront** (`check_accum_fits`, `GpuContext::check_frame_fits`), with `GpuError::FrameTooLarge`.
- **Decided once, before the first decode.** The budget is read once, after a blocking poll so what the app let go of is counted as free, before any frame decodes (on iOS a later reading would already hold the decodes), and every check in the run compares with that figure. The frames' depth is settled first (ADR-110). From the first frame's header the session reads as many frames ahead as fit, up to its source type's window, and refuses (`GpuError::OverBudget`) only if the stack doesn't fit beside one, saying what it needs then. Every source has a header that gives the size it decodes to: a DNG's metadata, camera RAW's from LibRaw's identify without unpacking (`deptheos_raw::read_size`). A stack whose first header can't be read, or whose frames are conversions not made yet, reads one frame ahead and is checked once a frame says its size (`ensure_fits`, which skips a shape already checked), so it never needs to start again. The log line before a stack gives each part of `MemoryNeed` (GPU, staged frames, frames on the CPU).
- **Watched while it runs, on iOS.** The estimate leaves out what it can't count well (the rest of the app, a decode on a device that differs from the Mac's), so before each frame is fed the session reads what the app has left, after a non-blocking poll so `wgpu` has freed what finished work let go (`Headroom`). Under a floor of half the reserve, twice in a row (one low reading may be memory freed and not yet returned), the stack stops with `GpuError::LowMemory` (worded by `gpu-low-memory`) rather than the system ending the app without a word. Each reading is logged at debug.

### Failures

- **Errors are recorded and checked** (`crates/deptheos_gpu/src/context.rs`). Each `GpuContext` records the first error a thread causes on it (later ones before a check are logged only) and records device loss across all threads; `GpuContext::check` returns it, once. Faults are kept per context and thread, so a paint error cannot fail a concurrent stack, and one context's error cannot fail another on the same thread. `StackingSession` checks after allocating, after every frame and pass, and before trusting a readback.
- **A lost device stops the stack with its own error.** Every check fails with `GpuError::DeviceLost` once the device is lost; a frame that fails in pass 1 checks the device before being left out, so a lost device is never reported as a run of bad frames; staging and readback check the device before reporting a buffer mapping failure.
- **Buffers are filled through the queue** (`GpuContext::create_buffer_with_data`), and upload staging failures return a `Result`.
- **The engine's device is kept to the end of the process.** `Engine::keep_until_exit` (`crates/deptheos_app/src/engine.rs`) clears the converted temp frames and leaves the `wgpu` device for the OS to tear down, because destroying it aborts inside Mesa's Venus driver on Linux QEMU/UTM VMs.

## Alternatives considered

### Keep pass 1's frames in memory, or spill them to disk, for pass 2

Two hundred frames do not fit in RAM, and decoding the original files again measured faster than writing uncompressed temporary TIFFs in pass 1 and reading them back.

### A shared stacker trait

The stackers have different pass counts and state machines, so separate `State` enums with small shared helpers (`FrameLedger`, `accum.rs`, `check_binding_fits`) are simpler than a trait.

### A fixed frame-count read-ahead

Four frames held about 4.8 GB at the start of a RAW stack (four decoded frames plus four LibRaw decodes in flight at about 1.2 GB each). Sizing the window by source type and bytes bounds memory across formats.

### Refuse a stack that doesn't fit beside the chosen window

The window sets only how fast frames arrive, so refusing turned away stacks that run. Reading fewer frames ahead is slower but finishes.

### A per-level tally of each stacker's memory

Every term grows with the stack's pixels, so a tally was no more accurate than one figure a pixel.

### Panics (`wgpu`'s default), or `create_buffer_init`

`wgpu`'s default handler panics on every validation and out-of-memory error, where the session can report the error and stop. `create_buffer_init` panics when it maps a failed allocation on software renderers such as WARP.

### Replay on a new device after device loss

If the GPU resets or runs out of memory the session stops and reports it rather than retrying on unstable hardware; Try Again restacks.

## Consequences

- A depth or wavelet stack decodes every frame twice, and pass 2 must find the same files with the same content.
- Every stacker change must keep `FrameLedger`'s rule: a left-out frame still takes an index, so later frames keep their plane numbers in the frame map.
- Each pass decodes the original, so every frame is resampled once with its composed transform. The stackers warp at full frame size and `pack.wgsl` copies the crop out at integer offsets, with no second resampling.
- A new GPU path calls `GpuContext::check` where a failure must stop work, and `clear_fault` where new work starts after a handled error.
- A new stacker or GPU resource is counted in `gpu_bytes`, which `gpu_memory.rs` checks against Metal's measured peak.
- Process exit, not `Drop`, releases the engine's device; cleanup that must run on quit goes in `Engine::keep_until_exit`.

## Code

- `crates/deptheos_app/src/runs/session.rs`, `crates/deptheos_app/src/runs/session/reading.rs`, `crates/deptheos_app/src/runs/stacker.rs`.
- `crates/deptheos_app/src/engine/memory.rs`, `crates/deptheos_app/src/engine.rs`.
- `crates/deptheos_core/src/ledger.rs`, `crates/deptheos_core/src/pack.rs`, `crates/deptheos_core/tests/gpu_memory.rs`.
- `crates/deptheos_gpu/src/context.rs`, `crates/deptheos_gpu/src/context/memory.rs`, `crates/deptheos_gpu/src/context/adapter.rs`.
