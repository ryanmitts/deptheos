# ADR-113: Golden references hold each stacker's output within a GPU's rounding, and the Rust tests run on the developer's GPU and on CI's software GPUs, one integration-test binary per crate

## Status

Accepted.

## Context

Synthetic property tests check behavior (sharpness recovery, footprint masking), but a refactor of the accumulators, shader composition or a stacker's states must be shown to leave the output as it was, on any GPU the tests run on: Metal compiles with fast math, and other backends and drivers round and fuse differently.

Nearly all of the engine's and the desktop app's tests need a GPU adapter: the three stackers and their goldens, the GPU DNG color render (ADR-130), the Lanczos and Keys warps, `StackingSession`'s end-to-end tests and `deptheos_gpui`'s headless window tests (`crates/deptheos_gpui/src/test_support.rs`). With no adapter, `GpuContext::new_or_skip` (`crates/deptheos_gpu/src/context.rs`) prints "skipping: no GPU adapter available" and the test returns, so `cargo test` on a hosted CI runner without a GPU would pass while skipping most of the suite. Software rendering (`DEPTHEOS_SOFTWARE_GPU=1`) takes minutes for a full-size stack (`docs/windows.md`), but the tests' frames are small.

Locally the suite has to rebuild quickly, and every integration-test binary relinks `wgpu`, the shader compiler and the native C++ libraries.

## Decision

### Golden references

- `pyramid_output_is_pinned`, `depth_output_is_pinned` and `wavelet_output_is_pinned` (`crates/deptheos_core/tests/integration/`) stack a fixed four-frame synthetic stack (one frame aligned with an exposure correction, one left out; 203×150 for the pyramid and the wavelet, 289×257 for depth, odd at its three blend levels) at two or three settings each, and compare the result and the frame or depth map with a 16-bit PNG reference in `crates/deptheos_core/tests/goldens/` (about 1.6 MB in all). The frames are in `common.rs`, the comparison in `golden.rs`.
- An output passes when at most 0.1% of its pixels are off by more than one 8-bit step in a channel (winners a near-tie flips) and the rest are at least 75 dB PSNR from the reference. A rerun on the same GPU gives about 110 dB, the reference's own rounding; one frame's exposure off by 0.1% or its warp by 0.002 px gives 70 to 74 dB, and a 1% change to a depth setting changes over 0.5% of pixels.
- `just bless-goldens` (`DEPTHEOS_BLESS_GOLDENS=1`) writes the references from the outputs of the machine it runs on, for an intended change, in the commit that makes it. Each test prints every output's distance.
- What reaches the stackers is pinned exactly at the input stage (ADR-132), on the CPU: `opaque_srgb_frames_load_bit_identically` (`tests/integration/decode.rs`) hashes opaque sRGB frames as loaded, and `deliberately_changed_inputs_are_pinned` (`crates/deptheos_core/src/io/input.rs`) hashes each input the stage changes on purpose.
- The tests skip when there is no adapter, unless `DEPTHEOS_REQUIRE_GPU=1` (below).

### Where the tests run

- **The Rust suite runs locally on the developer's GPU**, through the `justfile`'s recipes (CLAUDE.md, "Quick checks"), kept fast by the per-user native library cache (ADR-134).
- **One integration-test binary per crate.** `tests/integration/main.rs` includes each test module in `deptheos_core` and `deptheos_app`. Only tests that need a process of their own are separate binaries:
  - `crates/deptheos_core/tests/gpu_memory.rs`: every `wgpu` device on macOS shares one Metal device, so concurrent tests would skew its peak allocation.
  - `crates/deptheos_app/tests/logs.rs` and `crates/deptheos_ffi/tests/logging.rs`: the global logger installs once a process.
  - `crates/deptheos_app/tests/rayon_pool.rs`: rayon's global pool is built once a process.
  - `crates/deptheos_core/tests/host_decoder.rs` and `crates/deptheos_app/tests/batch_kinds.rs`: the host decoder is the process's.
- **End to end, real frames through the apps** (`crates/deptheos_e2e`, cases in `e2e.toml`). Each case stacks a set of real frames as a user drives the app, the desktop's through its test (`just e2e`) and iOS's in the Simulator through its UI test (`just e2e-ios`). A case may set adjustments, a reframe and an export space: the desktop's test types each slider's value in Adjust, sets the reframe as the crop tool does, and picks the space in the Export sheet; iOS, which has no Adjust, refuses such a case. Every export is checked: format and bit depth, the tags kept and the private ones gone (ADR-133), the frames' color space and color, and the picture a person signed off. Whether a picture is a good stack is judged by a person, who looks through a run's exports (`just e2e-review`) and signs them off with `just bless`, which writes a quarter-scale reference picture; a case without one fails. Later runs are held to it within a PSNR the GPU's rounding stays above, so the cases pass on any Mac and fail when the picture changes, each difference reported by size and PSNR. The frames and references are kept outside the repository (`~/DeptheosE2E`), since they are large camera files.
- **GitHub Actions:**
  - `.github/workflows/tests.yml` runs the whole workspace's tests on macOS, Windows and Linux under cargo-nextest's `ci` profile (`.config/nextest.toml`), which kills a test that hangs, and checks the website's committed parameters (`deptheos_webexport --check`). The GPU tests run on the runners' software GPUs (`DEPTHEOS_SOFTWARE_GPU=1`): WARP on Windows, Mesa's lavapipe on Linux. Those jobs set `DEPTHEOS_REQUIRE_GPU=1`, so a test that would skip for want of a GPU (`GpuContext::skip_test`) fails instead. A macOS runner is a virtual machine whose Metal device is not assured, so there a GPU test may skip.
  - `.github/workflows/web.yml` checks the third-party notices (`script/notices.sh --check`) and runs the website's TypeScript typecheck, unit tests and production Vite build, compiling no Rust.
  - `.github/workflows/release.yml` checks version tags, the notices, the design tokens (`node script/tokens.mjs --check`) and the website's parameters (`deptheos_webexport --check`), then builds and packages the whole workspace natively on macOS, Windows and Linux (x86_64 and arm64).
- **iOS.** `crates/deptheos_ffi` is tested from Rust (`cargo test -p deptheos_ffi` and the `smoke` example). The app has a UI test target, `ios/DeptheosMobileUITests`, and no unit tests; it is driven in the Simulator through debug launch arguments (`ios/DeptheosMobile/Views/Automation.swift`).
- **The website's browser checks** (`web/scripts/smoke.ts`, `web/scripts/touch-check.ts`, `just web-smoke`) run locally against a live Vite server in headless Chrome through `puppeteer-core`.

## Alternatives considered

### Visual checks or property tests only

They miss a small change to a stacker's output that a refactor was meant not to make.

### Hashes of the output

They catch a one-bit change, but only on the GPU and driver that made them: any other backend fails them with nothing wrong, so they cannot run on a CI runner's software GPU or another developer's machine. What they would catch and the tolerance lets pass is a change below it, such as one rounding step.

### References at full float precision or 8 bits

Raw `f32` files are twice the size of 16-bit PNGs, whose rounding is far below the tolerance. 8-bit references round by up to half the step that counts a pixel as changed, which leaves no room for a GPU's own differences.

### Larger frames

The noise in the frames does not compress, so the references grow with their pixels: 5.8 MB with depth's and the wavelet's at 517×389, every intended change adding as much to the repository's history.

### GPU tests only on the developer's GPU

A change that breaks Windows or Linux would wait for a release's testing, and a test that skipped would pass unseen. Software adapters are orders of magnitude slower than a GPU (about 277 s for an 8-frame 45 MP pyramid stack on a 32-thread CPU), but the tests' frames are under a megapixel, and nextest's `ci` profile allows a GPU test five minutes.

### Exact references for the end-to-end cases

Real frames go through the aligner and the GPU, so an exact comparison fails on any machine but the one that blessed it. The oracles judge the picture itself, and the reference only reports how far it moved.

### One test binary per file

Twenty-five binaries relinked after every edit. One binary for `deptheos_core`'s integration tests took the relinks after a one-line edit from 25 to 9 (23 s to 13 s on a 32-core machine) and the run from 27 s to 17 s, the modules running in parallel.

## Consequences

- A refactor meant to keep output is checked by the golden tests; an intentional algorithm change re-blesses the references in the same commit.
- A failure says by how much each output moved, not where; the references open as pictures, and `deptheos bench --save-raw` and `--compare-raw` or a visual diff finds where.
- The goldens pass on any GPU whose rounding stays within the tolerance; a change smaller than it goes unseen.
- A new integration test goes in its crate's `tests/integration/` as a module unless it needs a process of its own.
- Before pushing a change to the engine or the desktop app, a developer runs the tests on a machine with a GPU and checks that `new_or_skip` did not skip them; CI runs them again on Windows's and Linux's software GPUs.
- A test that needs a GPU skips through `GpuContext::skip_test`, so `DEPTHEOS_REQUIRE_GPU` reaches it.
- Speed, memory and hardware drivers on Windows and Linux are checked by testing on those machines before a release.

## Code

- `crates/deptheos_core/tests/integration/` (`common.rs`, `golden.rs`, `pyramid.rs`, `depth.rs`, `wavelet.rs`, `decode.rs`, `main.rs`), `crates/deptheos_core/tests/goldens/`, `crates/deptheos_core/src/io/input.rs`.
- `crates/deptheos_gpu/src/context.rs`.
- `crates/deptheos_core/tests/gpu_memory.rs`, `crates/deptheos_core/tests/host_decoder.rs`, `crates/deptheos_app/tests/logs.rs`, `crates/deptheos_app/tests/rayon_pool.rs`, `crates/deptheos_app/tests/batch_kinds.rs`, `crates/deptheos_ffi/tests/logging.rs`.
- `crates/deptheos_e2e/`, `e2e.toml`, `crates/deptheos_gpui/src/app/tests/e2e.rs`, `script/e2e-ios.sh`.
- `.github/workflows/web.yml`, `.github/workflows/tests.yml`, `.github/workflows/release.yml`, `.config/nextest.toml`.
- `crates/deptheos_gpui/src/test_support.rs`, `ios/DeptheosMobile/Views/Automation.swift`, `justfile`.
