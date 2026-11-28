# ADR-139: On iOS, Adjust's flat pictures are read where the engine holds them, lent to an image until it lets go

## Status

Accepted. The preview's tiers and budgets are ADR-138's; the session's bridge is ADR-153's.

## Context

A drag tick hands the app one flat picture (ADR-138): 11 MB of 8-bit BGRA at fit on an iPhone, 12.6 MB at 100%. Its budget is 16 ms from the set to the screen at fit and at 100% (33 ms with Texture or Sharpening set), on the slowest supported iPhone as on the desktop.

Crossing the bridge as bytes copies a picture about five times: the engine's clone, the bridge's own buffer, two in the generated Swift code, then `Data`. Timed in the Simulator with the debug build's probe (`DEPTHEOS_PROBE_ADJUST_TICK`, `crates/deptheos_app/src/runs/probe.rs`), at 44.7 MP, median and 95th percentile:

| Tick | Draw | Copy into Swift | Image drawn | Total |
|---|---|---|---|---|
| Fit, 2048 × 1365 | 5.6 / 6.7 | 20.4 / 22.1 | 0.9 / 1.0 | 27.0 / 29.1 |
| 100%, 2622 × 1206 | 6.1 / 7.3 | 23.4 / 24.2 | 1.0 / 1.3 | 30.7 / 31.7 |
| Fit, read in place | 5.6 | — | 0.3 | 5.9 / 7.0 |

The copy alone is past the budget, and it grows with the screen.

## Decision

- **The app reads a preview's pixels where the engine holds them.** `AdjustPreview` (`crates/deptheos_ffi/src/adjust.rs`) gives their address and length (`bgra_address`, `byte_count`) and no copy of them. The buffer is the `FlatPicture`'s, moved into the preview as the session hands it over. Nothing writes it while the preview lives: it goes back to the session's `FlatSlot` only in the preview's drop.
- **The image holds the preview.** `AdjustPreview.makeCGImage` (`ios/DeptheosMobile/Engine/EngineBridge.swift`) makes a `CGDataProvider` over the pixels whose info is the preview, retained, and whose release lets go of it. CoreGraphics and Core Animation call the release only once nothing reads the pixels, so the buffer is never reused under an image on screen, and is given back as soon as the next tick's picture replaces it.
- **A preview names its colors** (`ScreenSpace`: sRGB or Display P3), and its image is tagged from it.
- **Everything else still crosses as bytes.** A settled picture is one per drag, and the viewer's base and tiles are made off the main thread (ADR-164), so their copies are off the drag's path.

## Alternatives considered

### Copy fewer times

The bridge's buffer and Swift's lift are the generated code's; cutting the engine's clone and `Data`'s copy still leaves two full copies and the zero-fill before one, about 10 ms on the Simulator, more of the budget than the draw.

### An IOSurface written by the engine

The readback could land in a surface Core Animation shows directly. It saves the image's one draw too, but wgpu can't map into a surface the app made, so it needs a Metal path of the engine's own beside wgpu (ADR-100).

### A Swift-owned buffer filled by the engine

The app hands the engine a buffer to read back into. The same copies are avoided, but the buffer's life is then the app's to manage against the session's slot, in two places instead of one.

## Consequences

- The address is valid only while the app holds that `AdjustPreview`. Only `makeCGImage` reads it; nothing else in Swift takes the address.
- A preview held longer than the screen needs (in a test, a log) keeps its buffer from the slot, and the next tick allocates one.
- The bridge's tests check the length and that the address is set; that no buffer is reused under an image is the drop's order, not a test.

## Code

- `crates/deptheos_ffi/src/adjust.rs`, `crates/deptheos_ffi/src/adjust_tests.rs`.
- `ios/DeptheosMobile/Engine/EngineBridge.swift`.
- `crates/deptheos_app/src/adjust/preview.rs` (`FlatSlot`).
- The probe: `crates/deptheos_app/src/runs/probe.rs`, `crates/deptheos_ffi/src/bench.rs`, `ios/DeptheosMobile/Model/E2E.swift`.
