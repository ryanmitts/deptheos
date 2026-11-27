# ADR-100: The engine is Rust on one headless wgpu device; front ends get CPU pixels

## Status

Accepted.

## Context

Deptheos runs on macOS, Windows, Linux and iOS. Swift and Metal serve only Apple platforms, so the engine needed a cross-platform GPU and systems stack.

Each front end draws with a device of its own: the desktop's GPUI with its native renderer (Metal on macOS, Direct3D on Windows, wgpu on Linux; ADR-101), the iOS app with SwiftUI and Core Graphics. Neither can take a `wgpu::Texture` from the engine.

During a stack, the prefetch decode threads allocate and free several frame-sized buffers per frame (decoded pixels, the aligner's gray levels). The system allocator on Windows returns each to the OS, and the next frame faults it back in zero-filled, page by page.

## Decision

### Rust and wgpu

- The engine is a Rust workspace (`crates/`). Its GPU work is WGSL compute shaders run through `wgpu`, composed with `naga_oil` and handed to `wgpu` as naga IR (the `naga-ir` feature in `Cargo.toml`).
- `deptheos_gpu` is the only crate that creates adapters and devices (`crates/deptheos_gpu/src/context.rs`, `crates/deptheos_gpu/src/context/adapter.rs`).
- **FXC on DX12.** The engine works in `f32` throughout and doesn't ask for `SHADER_F16` (ADR-110), so no shader needs shader model 6.2 or DXC. DX12 shaders compile with `wgpu`'s default, FXC, which Windows ships; DXC linked in would be one more library to carry notices for. FXC is older and slower, so shaders are run on Windows machines before every release (the device table in `docs/releasing.md`).
- **Limits are negotiated from the adapter** (`build_limits` in `adapter.rs`), never a fixed request, so a device starts on smaller adapters.
- **Bounded dispatch time.** Windows resets a GPU device after about two seconds of continuous work. The depth map's grid is capped at `MAX_MAP_SIDE`, 6,000 px (`crates/deptheos_core/src/depth/curves.rs`), so its serial smoothing sweeps stay inside the driver's watchdog.
- Which GPU the engine uses is a `DeviceChoice` (`adapter.rs`): automatic, one named adapter, or the software renderer.

### One headless device, CPU pixels out

- `deptheos_app::Engine` holds one headless `GpuContext`, separate from any UI toolkit's device (`crates/deptheos_app/src/engine.rs`). What crosses back to a front end is CPU data only (`PresentedImage`, `CpuImage`, `CompletedStack`); a shell needs no GPU types.
- **At most one readback per result.** Pixels cross once per stack or preview, not per displayed frame; pan and zoom are layout in the front end. A stack in sRGB needs no readback for display, because its display image is built from the pixels the CPU already holds. An adjusted render is the exception: it draws on the GPU, so each drag preview and each full redraw is read back once, from the copies the render keeps on the GPU while Adjust is open (ADR-114).
- **The GPU makes pixels; the CPU does what only the screen needs** (`crates/deptheos_app/src/engine/present.rs`): 8-bit conversion, the display's channel order (BGRA, which GPUI and Core Graphics both read), the depth map's colors, color management to sRGB, and the half-size display levels. GPUI samples images bilinearly with no mipmaps and limits its atlas size, so each image comes with its own levels, each cut into tiles of `TILE_CORE` px with a 1 px gutter; the desktop's atlas of those tiles is `crates/deptheos_gpui/src/images.rs`.

### mimalloc in the binaries

`mimalloc` is the `#[global_allocator]` of the two shipped binaries, `deptheos-gui` (`crates/deptheos_gpui/src/main.rs`) and `deptheos` (`crates/deptheos_cli/src/main.rs`), and of the `mimalloc_shed` example. It keeps freed pages to reuse, so the next frame's buffers don't fault in again.

## Alternatives considered

### An engine in Swift and Metal

It can't run on Windows or Linux.

### Another GPU API than wgpu

`wgpu` gives one compute pipeline across Metal (macOS, iOS), Direct3D 12 and Vulkan (Windows, Linux) from one set of WGSL shaders.

### Share the toolkit's device (zero-copy display)

Not possible across GPUI's native renderers and SwiftUI with Core Graphics. Sharing a device also ties the engine's `wgpu` version to the toolkit's.

### Build the display levels on the GPU

Blitting and reading back every level, with four queue drains per image, took 65–90 ms for the display and 37–50 ms for the depth map per stack. One readback followed by halving on the CPU is simpler and uses less GPU memory.

### The system allocator

On a 16-core Windows machine stacking 46 × 45 MP JPEGs with Pyramid, `mimalloc` took wall time from 3.10 s to 2.85 s (about 8%).

### A pool for every CPU buffer

GPU upload staging does use an explicit `StagingPool` (`crates/deptheos_core/src/upload.rs`; 50–80 ms of staging per frame down to 25–35 ms). The CPU allocations made inside third-party decoders (`image`, LibRaw, the DNG SDK) and the aligner can't be pooled from outside, and `mimalloc` keeps their pages warm without any change to them.

## Consequences

- One engine runs on macOS (Metal), Windows (DX12 or Vulkan), Linux (Vulkan) and iOS (Metal, through `deptheos_ffi`; ADR-101).
- The `wgpu` pin is the engine's to move, since no toolkit shares its device, but `naga_oil` and `wgpu-profiler` move with it, and gpui-kit's own `wgpu` (its Linux renderer) should match to keep one copy in the build (`Cargo.toml`).
- Unified memory on Apple hardware hides readback and texture-lifetime costs that discrete GPUs expose; GPU paths are measured on discrete GPUs as well.
- The UI's and the engine's GPUs are independent, and each can fall back alone. On a Linux VM with Mesa's Venus driver (which computes but can't present), the window draws on lavapipe while stacks run on the GPU (`draw_beside_venus` in `crates/deptheos_gpui/src/main.rs`). A Windows VM with only WARP stacks on the software renderer.
- A full-size result is held in CPU memory as display levels as well as in the render; new views keep display copies in mind.
- Every new on-screen view of GPU output needs a readback path in `present`, not a texture handed to the front end.
- `mimalloc` keeps freed pages, so the footprint after a stack can stay higher than with the system allocator. A display pyramid made on a worker and dropped on the main thread left it 360 MB up at 45 MP and 675 MB at 100 MP (`crates/deptheos_app/examples/mimalloc_shed.rs`). The desktop's viewer has `mimalloc` give it back (`mi_collect`, `give_back_memory` in `crates/deptheos_gpui/src/main.rs`) once it has let go of a picture and the atlas's drops have settled; it takes 12 to 16 ms on the main thread.
- A new binary or benchmark meant to match the shipped allocation behavior sets `mimalloc::MiMalloc` as its `#[global_allocator]`.

## Code

- `crates/deptheos_gpu/src/context.rs`, `crates/deptheos_gpu/src/context/adapter.rs`.
- `Cargo.toml`: the `wgpu`, `naga_oil`, `wgpu-profiler` and `mimalloc` pins, with the FXC and allocator notes.
- `crates/deptheos_core/src/depth/curves.rs`.
- `crates/deptheos_app/src/engine.rs`, `crates/deptheos_app/src/engine/present.rs`.
- `crates/deptheos_gpui/src/images.rs`, `crates/deptheos_gpui/src/main.rs`.
- `crates/deptheos_cli/src/main.rs`, `crates/deptheos_app/examples/mimalloc_shed.rs`.
