# ADR-110: The GPU's data is 32-bit float in storage buffers, rounded once to 16 bits, from WGSL modules composed by naga_oil

## Status

Accepted.

## Context

A stack sums weighted contributions from tens or hundreds of frames, and its output can be 16-bit linear DNG. The three stackers, frame upload, warp, the DNG render and the focus maps share shader code: samplers, sRGB transfer curves, focus metric constants, the warp matrix and the accumulator layout. Standard WGSL has no `#import` and no conditional compilation, and some shaders need a different texture format on GPUs that lack an optional storage format (`Rgba16Unorm`, `R8Unorm`).

`wgpu` validates storage texture formats strictly per backend. Read-write storage on `rg` formats does not exist on Metal through `wgpu`; `Rgba16Unorm` storage is write-only and needs an optional capability; portable WebGPU guarantees read-write texture storage only for `r32float`, and Metal offers even that only from read-write texture tier 1. The iOS Simulator's device has no read-write storage texture of any format (it reports Apple GPU family 2). Its other limits fit the engine: workgroups of at most 256 invocations, at most 8.8 KB of workgroup memory, and the storage bindings per stage.

## Decision

### Shaders

- **Composition.** `GpuContext::create_shader_module_composed` (`crates/deptheos_gpu/src/context/shaders.rs`) builds each pipeline's shader with `naga_oil` from its source and the shared modules it imports, evaluates `#ifdef` blocks against the adapter's format support (`SOURCE_RGBA16UNORM`, `MAP_R8`), and hands `wgpu` the naga module (`ShaderSource::Naga`, through the `naga-ir` feature).
- **Where shaders live.** `shaders/` holds only the modules other shaders import by name, listed in `crates/deptheos_core/src/shaders/README.md`. A shader that is its own pipeline sits next to the Rust file that builds it, named after it (`depth/pipeline.rs` → `depth/depth.wgsl`).
- **Helpers shared between pipelines have no bindings.** Shared math and transfer curves live in binding-free modules (`deptheos::metric`, `deptheos::color`), so importing a constant never pulls unused bindings into a pipeline. A module imported by one pipeline only (`deptheos::focus`, `deptheos::quantize`) keeps its bindings.
- **One declaration per uniform struct, checked against Rust.** Uniforms are laid out with `encase` in Rust and declared once in WGSL. `shader_uniforms_match_their_rust_structs` and `shader_warp_params_matches_the_rust_struct` (`crates/deptheos_core/src/transform.rs`) parse both and require the same fields, order and types, and forbid a shader its own copy of `WarpParams`.
- **No loop guards.** Modules are made with `force_loop_bounding: false`: naga's infinite-loop guards kept Metal's compiler from optimizing shader loops (with tiled kernels, the depth stacker's GPU time on 46 JPEG frames went from 7.45 s to 3.33 s, bit-identical). Every shader loop is a counted `for` whose bound is a constant, a uniform or a texture size (the `SAFETY` comment in `context/shaders.rs`).

### Data

- **f32 everywhere in between.** Every GPU intermediate is 32-bit float (`R32Float`, `Rg32Float`, `Rgba32Float`); no shader uses `f16`.
- **Frames in.** Bytes upload through a storage buffer and a compute unpack (`crates/deptheos_core/src/upload.rs`, `upload.wgsl`) into the stacker's source, whose format follows the frames' depth (`FrameDepth`): `Rgba8Unorm` for 8-bit frames, `Rgba16Unorm` for 16-bit ones, `Rgba32Float` where the adapter lacks 16-bit-norm storage. Each holds its frames exactly, so a stack comes out the same through any of them, and 8-bit frames take 4 bytes a pixel instead of 8.
- **The depth is decided before any frame decodes.** It is the deepest frame's, read from each frame's name or header (`plan_frames` in `crates/deptheos_app/src/runs/session/reading.rs`, through `io::input_header`). RAW is 16-bit by name, and a header that can't be read counts as 16-bit, so the memory check (ADR-111) counts the source as it will be. A frame deeper than the stack was sized for (a file changed meanwhile) is an error, never narrowed (`Source::check`).
- **Staged frames are read in place on Apple GPUs.** There, sharing memory with the CPU, the unpack reads a frame where the decoding thread staged it (`StagedFrames::InPlace`, through `MAPPABLE_PRIMARY_BUFFERS`, on a Metal integrated GPU that offers it). Elsewhere it is copied into a storage buffer first (`StagedFrames::Copied`): a mappable buffer on a GPU with memory of its own lives in system memory, slow for a shader to read.
- **Accumulators are `vec4f` storage buffers**, row-major at each level's size (`deptheos::accum`, `crates/deptheos_core/src/accum.rs`), in all three stackers.
- **A scalar map a pass updates in place is an `f32` storage buffer**, a `ScalarMap` (`crates/deptheos_core/src/scalar.rs`), row-major at the map's size and indexed like the accumulators beside it: the pyramid's running best focus keys and the depth stacker's running best and opacity sum.
- **No pass reads and writes a storage texture.** A map a pass only writes is a write-only texture binding (the depth WTA's map), and one that later passes only read stays an `R32Float` texture. Every map the map kernel draws (`map.rs`, `map.wgsl`) is a texture.
- **The depth finalization writes out of place**, into textures already free once the last frame is in, so it allocates nothing (`depth/mod.rs`).
- **A fill kernel starts the maps whose start is not zero** (`FillPipeline`, `scalar.wgsl`: −2 for the pyramid's keys, −1 for the depth WTA's best), since `clear_buffer` writes only zeros and a buffer has no render-pass clear. The others are cleared with `clear_buffer`.
- **No device is refused for a missing feature.** Every method stacks on any adapter, the iOS Simulator's included, up to that device's size limits.
- **Results out.** `U16Packer` (`pack.rs`, `pack.wgsl`, `shaders/quantize.wgsl`) crops the `f32` result and rounds it to `Rgba16Unorm` through a 65,536-entry threshold table built from the CPU's own `quantize_u16`, so the GPU's rounding is exact even under fast math. 8-bit output rounds from that. Where the adapter lacks 16-bit-norm storage there is no packer: the `f32` result is read back and rounded on the CPU by the same `quantize_u16`. Every method's stack comes out within a code of 65,535 of the 16-bit path's on that float path (`the_float_path_stacks_as_the_16_bit_path_does` in `crates/deptheos_app/tests/integration/session.rs`, on a context whose format support is set as such an adapter's).

## Alternatives considered

### A copy of each struct in every shader that uses it

`naga_oil` rejects exported structs whose member names end in a digit (naga renames them during round-trip validation). Naming the matrix columns `x_axis`, `y_axis`, `z_axis` (as `glam` does) rather than `col0`…`col2` allows one shared `WarpParams` in `deptheos::warp`.

### f16 for scratch and running maxima

Summing the depth stacker's opacity in `f16` on an Apple M5 put about 1e-2 of absolute error into the depth confidence, shifting depth maps by 4–15/255 on 17–30% of pixels, while peak memory and wall time stayed within run-to-run noise. `f32` throughout loses nothing at no measured cost.

### Rgba16Float accumulators

Summing tens of frames in `rgba16f` quantizes highlights to about 10 bits.

### Read-write Rgba32Float textures, or four-layer R32Float textures

Read-write `Rgba32Float` needs `texture-formats-tier2`, and naga's Metal backend does not fence a thread's read of its own storage-texture write. Four-layer `R32Float` textures are portable, but updating a pixel takes four loads and four stores where a `vec4f` buffer takes one of each (a 66-frame 44.8 MP JPEG wavelet stack went from 2.49 s to 2.34 s on an M5 Max, the pyramid and depth bit-identical).

### Read-write R32Float textures for the running bests

They leave out every Metal device below read-write tier 1, the Simulator's among them.

### Ping-pong textures for the running bests

Each read-modify-write would read one texture and write another, swapping every frame. That doubles every running best (about 5.3 bytes a pixel more for the pyramid: 128 MB at 24 MP, 320 MB at 60 MP) and copies every pixel of each best on every frame, where a buffer is written only where a frame beats it.

### Read-write textures where offered, buffers only where not

Two code paths: the one the golden tests hold (ADR-113) on the developer's GPU, and another that would run only in the Simulator, where iOS has no unit tests. Nothing is gained: the same kernels already read and write a `vec4f` buffer a pixel, and a buffer avoids the unfenced read above.

### Splitting stacks into tiles to lift the Simulator's size limits

The Simulator binds at most 256 MiB a buffer and makes textures at most 8192 px a side; at 16 bytes a pixel a level-0 accumulator caps a stack there at about 16.7 MP. Tiling every stacker would change the code real devices run, for the Simulator alone. Larger frames get `GpuError::FrameTooLarge` from `check_accum_fits` and `GpuContext::check_frame_fits`.

### Let the texture store round the result

Storing the `f32` result straight into `Rgba16Unorm` leaves the rounding to each driver, and `x * 65535 + 0.5` in the shader is no better: Metal compiles with fast math and may fuse the multiply-add, landing a step from the CPU on values within an ulp of a tie. Adapters without 16-bit-norm storage round on the CPU (`quantize_u16`), so a result would differ by adapter. The table costs one 256 KiB buffer and a comparison a channel, and comparisons are exact in any math mode.

## Consequences

- A new shared helper goes in `shaders/`, with no bindings, registered where its users' pipelines are built; a new uniform struct needs an entry in the mirror test in `transform.rs`. Struct member names in shared modules must not end in a digit.
- A composition error panics at pipeline creation (`compose_module`): the shader sources are compiled into the binary, so it is a bug, not a bad input.
- `naga_oil` pins the `naga` version, and so `wgpu`'s (`Cargo.toml`).
- Memory is 16 bytes a pixel per accumulator and 4 per scalar map, with no row padding. A level is one binding, so a frame whose level-0 buffer exceeds the device's storage binding limit is refused before anything is allocated (`check_accum_fits`: 4 GiB, about 268 MP, on Metal; 2 GiB, about 134 MP, on D3D12; the driver's `maxStorageBufferRange` on Vulkan).
- The Simulator's output passes the golden tests, but it says nothing about speed or memory on a device.
- The wavelet stacker's picture while it runs (ADR-149) is an `r32float` texture, made from the winners each time it is asked for.

## Code

- `crates/deptheos_gpu/src/context/shaders.rs`, `crates/deptheos_gpu/src/context.rs`.
- `crates/deptheos_core/src/shaders/README.md`, `crates/deptheos_core/src/pipelines.rs`, `crates/deptheos_core/src/transform.rs` (the uniform mirror tests).
- `crates/deptheos_core/src/accum.rs`, `crates/deptheos_core/src/shaders/accum.wgsl`, `crates/deptheos_core/src/scalar.rs`, `crates/deptheos_core/src/scalar.wgsl`.
- `crates/deptheos_core/src/upload.rs`, `crates/deptheos_core/src/upload.wgsl`.
- `crates/deptheos_core/src/pyramid/pyramid.wgsl`, `crates/deptheos_core/src/depth/depth.wgsl`, `crates/deptheos_core/src/depth/mod.rs`.
- `crates/deptheos_core/src/pack.rs`, `crates/deptheos_core/src/pack.wgsl`, `crates/deptheos_core/src/shaders/quantize.wgsl`.
