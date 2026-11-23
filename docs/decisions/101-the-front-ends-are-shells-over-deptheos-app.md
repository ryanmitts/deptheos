# ADR-101: The front ends are shells over `deptheos_app`: the desktop on GPUI, iOS in SwiftUI over UniFFI

## Status

Accepted.

## Context

Deptheos has three front ends: the desktop app (`crates/deptheos_gpui`), the iOS app (`ios/DeptheosMobile`, through `crates/deptheos_ffi`) and the command line (`crates/deptheos_cli`; ADR-103). When session setup, alignment orchestration, crop, the color model, history grouping, option names and batch state lived in each front end, the copies diverged and caused bugs particular to one front end.

The desktop app needs a native-feeling component set on macOS, Windows and Linux. The iPad and iPhone app needs a native SwiftUI interface, and Apple's frameworks (ImageIO, `CIRAWFilter`) already decode JPEG, HEIC and camera RAW there.

The desktop app keeps its Library, the working list (`list.json`), each stack's working version, its preferences and its window state as files it rewrites as they change; two copies of it running for one user would overwrite each other's. macOS runs one instance per bundle and delivers later opens to it (`on_open_urls`); on Windows and Linux every launch from a shortcut or Open With is a new process.

## Decision

### One application layer

`crates/deptheos_app` holds everything the front ends share, and each front end is a shell over it:

- **`Engine`** (`engine.rs`; ADR-100): the GPU context, the shared pipelines, RAW conversion and its cache, CPU display images, and the threads blocking work runs on (ADR-143). Every stack starts at `Engine::session`: the desktop's, iOS's, the CLI's, sequences and the bench.
- **Shared logic**: frames and their lists, stacks and the history with its strip by day, export and names, option keys and labels (`StackerKind`, `RawBackend`, `AlignMode` and others with `ALL`, `key()` and `label()`), paint, the viewer's view, preferences, sequences and the turntable fix, and the user's sentences (ADR-161).
- **`batch::Machine`** (`runs/batch/machine.rs`): one pure state machine for a batch run, events in and actions out, no I/O (ADR-144). The desktop (`crates/deptheos_gpui/src/app/batch_run.rs`), the CLI's batch and turntable, and iOS (the FFI's `BatchQueue`) run the same code.
- `deptheos_app` has no UI toolkit types, so every shell can use it. The CLI depends on `deptheos_app` and, for `bench --profile`, `deptheos_gpu`; `deptheos_ffi` on `deptheos_app` and, for the host decoder, `deptheos_core`.

### The desktop: GPUI and gpui-kit

- The desktop app uses GPUI, Zed's UI framework, through its crates.io snapshots (`gpui-pre`), with the `gpui-kit` component library for widgets, lists, menus, notifications and themes. GPUI draws with its own renderer (Metal on macOS, Direct3D on Windows, `wgpu` on Linux through gpui-kit); the engine keeps its own headless device (ADR-100).
- gpui-kit pins its GPUI snapshot exactly, so `gpui`, `gpui-kit` and the pair named in gpui-kit's own `Cargo.toml` are bumped together. gpui-kit enables `runtime_shaders`, so building on macOS needs no Xcode Metal toolchain.
- Native file dialogs come from `rfd` (`crates/deptheos_gpui/src/file_dialog.rs`), because GPUI's prompt has no file-type filter.
- Blocking engine work runs on the engine's threads, which have 8 MiB stacks, reached through `crates/deptheos_gpui/src/work.rs` (ADR-143), never on GPUI's background executor: on macOS that is GCD, whose 512 KB worker stacks native decoders such as the DNG SDK and image codecs can overrun.

### iOS: SwiftUI over UniFFI

- A native SwiftUI app, `ios/DeptheosMobile`, universal (its floor is in ADR-102).
- The bindings are `crates/deptheos_ffi`, generated with UniFFI from proc-macros only, built as a static library and packaged as an XCFramework by `ios/build-xcframework.sh`, consumed through the local Swift package `ios/DeptheosEngine`.
- **Coarse and `async`.** A few long calls, each `async`: its Rust side hands the work at once to the engine's threads (the pool's lane, the store thread, or a stack's own thread) and awaits a one-shot channel, so nothing blocks the Swift side and no async runtime is needed (`crates/deptheos_ffi/src/request.rs`; ADR-143). The quick calls the app makes as it draws stay synchronous. UniFFI doesn't carry a Swift task's cancellation into Rust, so each call that can be stopped takes a `Request`, which the app cancels with its task; a Library write takes none.
- Progress and the live map come back through the foreign trait `StackObserver` (`result.rs`), called from the stacking and readback threads; the app reads each observer as one `AsyncStream`.
- **Decoding.** Standard images go to the app's `FrameDecoder` foreign trait (`decode.rs`; ADR-132); DNG and camera RAW stay with the engine's decoders unless a stack chooses Apple's (ADR-131).
- **The engine's own types cross as they are**, exported with `#[uniffi::remote]` (`Source`, `Split`, `AlignMode`, `RawBackend`, `ExportFormat`, the history's `Batch` and others).
- Results come back as CPU pixels, 8-bit BGRA for Core Graphics, and as files written by the engine's savers (ADR-100).

### One running desktop app per user

On macOS, AppKit's single instance does it. On Windows and Linux, `crates/deptheos_gpui/src/instance.rs` does, claimed once at the start of `main`: the first launch, a hand-off, or alone.

- The first launch holds an OS lock, `instance.lock`, in the app's config folder until it exits. A later launch hands its command-line paths to it and exits; the running app comes to the front and adds them through the channel macOS's `on_open_urls` feeds.
- The hand-off is one JSON line over loopback TCP, carrying only paths and a random launch token kept in a `0600` file beside the lock, so a process that can't read the user's config folder can't hand files over.
- It fails open: if the folder can't be locked, the listener can't bind or the holder doesn't answer, the launch runs alone and the Library's own lock (ADR-140) is the guard.

## Alternatives considered

### Each front end keeps its own orchestration

Rejected: the copies of session setup diverged in alignment, crop, color handling and Adobe DNG Converter sequences.

### A shared run-state struct that each shell drives

Shells deciding the transitions of a shared struct leave state-machine bugs. With the transitions in `batch::Machine`, every one is testable without a UI.

### Slint for the desktop

Slint drew the stacker's texture by sharing the engine's `wgpu` device, which tied the engine's `wgpu` version to Slint's unstable wgpu interop and needed renderer workarounds. GPUI with gpui-kit gives a more polished desktop component set and leaves the engine's `wgpu` version its own.

### Hand-written FFI mirror types

The two sides drift. Exporting `deptheos_app`'s own types with `#[uniffi::remote]` keeps one definition.

### Platform IPC for the hand-off (named pipes, Unix domain sockets)

Two implementations, one of which can't be exercised on macOS during development, or one more crate. A standard-library loopback stream is one portable path, tested on macOS, Windows and Linux.

### Lock only, and refuse a second launch

Open With Deptheos on a folder or photo in Explorer or a Linux file manager while the app is open would silently do nothing, which looks broken.

## Consequences

- A behavior fixed in `deptheos_app` is fixed on every front end, and `batch::Machine`'s table tests (`cargo test -p deptheos_app --lib batch`) cover all three shells.
- Behavior shared by more than one front end belongs in `deptheos_app`; a front end keeps presentation and platform work (dialogs, file access, decoding on iOS). `batch::Machine` stays free of I/O.
- iOS stacks with the same stackers, aligner, history and batch machine as the desktop; decoding and UI differ. Camera RAW decoded by Apple arrives as 16-bit Display P3 and stacks as a standard image; tests that depend on the backend assert both.
- After a Rust change, `ios/build-xcframework.sh` (`just ios-engine`) runs before Xcode builds the app.
- The desktop follows Zed's release cadence through snapshot crates; an upgrade moves three pins at once.
- What GPUI lacks is added around it: `applicationShouldTerminate:` is added to GPUI's macOS app delegate so a quit from the Dock waits for writes in flight (`crates/deptheos_gpui/src/app/quit.rs`).
- Desktop tests run headless through GPUI's harness (`#[gpui::test]`), on the real engine.
- A second launch on Windows or Linux exits right after its hand-off. On a Wayland compositor that wants an activation token, the files are added even if the window stays behind until clicked.

## Code

- `crates/deptheos_app/src/lib.rs`, `crates/deptheos_app/src/engine.rs`, `crates/deptheos_app/src/runs/batch/machine.rs`.
- `crates/deptheos_gpui/src/work.rs`, `crates/deptheos_gpui/src/file_dialog.rs`, `crates/deptheos_gpui/src/app/quit.rs`, `crates/deptheos_gpui/src/app/batch_run.rs`.
- `crates/deptheos_gpui/src/instance.rs`, `crates/deptheos_gpui/src/main.rs`.
- `crates/deptheos_ffi/src/`, `crates/deptheos_ffi/Cargo.toml`, `crates/deptheos_core/src/io/load.rs`.
- `ios/build-xcframework.sh`, `ios/DeptheosEngine/`, `ios/DeptheosMobile/`.
- `Cargo.toml`: the `gpui`, `gpui-kit` and `rfd` pins.
