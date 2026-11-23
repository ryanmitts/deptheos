# ADR-131: Camera RAW decodes with LibRaw, Adobe DNG Converter or, on iOS, Apple's decoder; every DNG is read and written through the Adobe DNG SDK, with LibRaw's demosaic for mosaic DNGs

## Status

Accepted.

## Context

Camera RAW formats (CR3, NEF, ARW, RAF, …) need a decoder and a demosaic, and the color depends on the camera matrices and profile. Users who develop their files in Adobe's raw editors expect Adobe's color science. Adobe distributes Adobe DNG Converter and its DCP camera profiles free for macOS and Windows (not Linux), under terms that do not allow bundling them in a third-party installer.

DNG is both an input (camera DNGs, the converter's output) and the RAW export format (ADR-130). A DNG is either linear, already demosaiced (the converter's `-l` output, Apple ProRAW), or a mosaic, one color a pixel (cameras that shoot DNG natively, drones, Android phones, the iOS camera's Bayer bursts, ADR-162). Reading one accurately means applying its linearization tables, per-channel black and white levels, `OpcodeList2`, `DefaultCrop` and orientation while keeping its camera profile; writing one means a valid DNG that raw editors open with the reference frame's profile and metadata. The Adobe DNG SDK is the reference for both, and it reads a linear DNG better than anything else; its demosaic is a plain one, built for previews. Its stock Stage 1 reader and lossless JPEG decoder are single-threaded.

## Decision

### The decoders (`RawBackend`)

`RawBackend` (`crates/deptheos_app/src/frames/raw_backend.rs`) is `LibRaw`, `AdobeDng` or `Apple`. Each render records the one it used, so its frames always decode the way it was stacked, and compare and paint sources decode with it.

- **LibRaw**, always compiled into the engine on every platform (ADR-134), called through `crates/deptheos_raw/native/raw_c_api.cpp`. It decodes to 16-bit linear camera RGB (`configure_linear`: raw color, unit multipliers, gamma 1.0, no auto-brightening, `adjust_maximum_thr = 0`) with the PPG demosaic, faster than AHD.
- **Adobe DNG Converter**, run as an external process where it is installed (macOS, Windows). It converts camera RAW to linear DNGs (`-l`) in parallel into a temporary cache, a few frames ahead of the stacker (`crates/deptheos_app/src/frames/conversions/`), and those DNGs are read by the SDK. Uncompressed converter output is an opt-in setting (faster to decode, twice the disk).
- **Apple's decoder, iOS only** (`CIRAWFilter` in `AppleFrameDecoder.swift`), offered only where the host installed a decoder. It renders camera RAW and mosaic DNGs with its defaults into 16-bit Display P3 and hands over P3's profile (ADR-132); the stack is then of standard images and exports as TIFF, JPEG or PNG. JPEG, HEIC, PNG and TIFF always go to Apple's decoder on iOS.
- **Adobe camera profiles for LibRaw frames.** LibRaw's built-in matrices look worse than Adobe's, so a LibRaw stack applies an installed Adobe DCP profile (Adobe Standard by default, Camera Standard, or None; `crates/deptheos_core/src/dcp.rs`), searched in the user's profile folders and Adobe's standard installation folders. DNG and converter files carry their own embedded profile.

**Defaults.** The desktop's RAW decoder setting defaults to **Automatic**: Adobe DNG Converter when installed (Adobe's own demosaic and profile, and a faster decode per frame on 40+ MP files), LibRaw otherwise. On macOS and Windows the desktop shows a one-time notice the first time camera RAW is added without the converter, suggesting it. The CLI defaults to LibRaw, so it runs with no external dependency or temporary disk unless `--raw-backend` asks. On iOS, RAW Decoder in Stack Settings (shown while the list holds camera RAW or mosaic DNGs) is Apple by default, for imported files and the camera's own Bayer bursts alike; LibRaw makes the stack linear, with DNG export.

### DNGs

- **The DNG SDK reads every linear DNG, on every platform, whatever the stack's decoder.** The engine takes its Stage 2 image: linear, black-subtracted, 16-bit camera RGB, cropped to `DefaultCrop` and rotated upright. `OpcodeList2` is applied there and dropped from exported metadata.
- **LibRaw demosaics every mosaic DNG, never the SDK.** The SDK reads a mosaic DNG as metadata only, the file's own color science, which the stack keeps (`LinearColor::Dng`). LibRaw supplies the pixels, cropped to the DNG's `DefaultCrop` through its `cropbox` (`crop_to_dng_default`) before the demosaic and the rotation. A mosaic DNG decodes to the size its header gives, and to the same pixels on the desktop, the CLI and iOS under LibRaw.
- **Mosaic or linear is read from the DNG's TIFF structure** (`deptheos_dng::is_mosaic`: the main image's PhotometricInterpretation, a few hundred bytes, no SDK), remembered per file by its length and modification time (`io::is_mosaic_dng`, whose lock is held only for the lookup, not the read), since the apps ask while they draw. A DNG that can't be read this way counts as linear, so it goes to the SDK, which says why it can't be read.
- **A DNG the SDK can't read is left out.** The frame fails with the SDK's error ("couldn't be read as a DNG"), its diagnostics in the log, and is left out as any unreadable frame, so a stack never takes one frame in another decoder's color and crop.
- **A DNG read takes at most 2^29 pixels** (`MAX_READ_PIXELS`) in its image or its default crop, far past any camera's. A larger header is refused before the SDK allocates for it, with the bridge's `DT_ERROR_TOO_LARGE` as `Cause::TooLarge`; the user reads that the file is too large to read and that a DNG can have at most 536 MP.
- **`OpcodeList3` (lens warp) is never applied while stacking, and goes into a DNG only where it still applies** (`frame_opcode_list3`, `crates/deptheos_dng/native/dng_io.cpp`). Its coordinates are the source's Stage 3 image's, so a read keeps it only when the image it hands out is that whole Stage 3 (the default crop is the active area, upright as stored, a mosaic at full size), and an export writes it only when the result is that size. Otherwise it is left out, so no reader warps a picture framed differently from the one it was made for. Metadata-only reads, which give a stack its export metadata, read the opcode lists too. The Library's color reference (ADR-130) carries the list whatever its own size, with the size it applies to as its `OriginalDefaultCropSize` (`DngWriteKind::ColorReference` in `dng_bridge.h`), which a read takes back as that size.
- **The DNG SDK writes every DNG**, from the reference frame's metadata held by Rust as an opaque C++ handle (`dt_dng_write`; the tags an export adds and drops are ADR-133).

### The SDK is always built

The DNG SDK, with the XMP toolkit, libjpeg and libjxl beside it, is compiled into the engine on every platform it builds for (macOS, iOS, Windows and Linux), as LibRaw is (ADR-134). `crates/deptheos_dng/build.rs` fails the build on any other target.

### Fast read paths

Custom read paths serve the common layouts and fall back to the stock SDK reader for anything else (`crates/deptheos_dng/native/dng_fast_read.cpp`, `dng_lossless_jpeg_fast.cpp`). Every fast path is tested for bit-exact parity with the stock path (`crates/deptheos_dng/src/fast_read_tests.rs`).

- Pixels copy straight into the Rust buffer, in multi-threaded row bands.
- Uncompressed Stage 1 reads across up to 8 threads with positional reads, the black subtraction fused into the copy-out (a 45 MP frame from 187 ms to about 43 ms).
- A fast lossless JPEG decoder gives exactly the SDK decoder's samples for the tiles it accepts: one SOF3 frame of 1 to 4 components, predictors 0 to 7, any point transform, no restart interval. It declines the rest. It cuts compressed DNG stacks by about 25% on an M5 Max.
- On Windows, a pass whose DNGs would take more than three quarters of the memory free for caching reads around the OS file cache (`FileCache::Bypass`, `FILE_FLAG_NO_BUFFERING`; `file_cache_for` in `crates/deptheos_app/src/runs/session.rs`). Past that the cache fills and every read waits for evictions (200 frames of 270 MB on a 64 GB PC: 29 s through the cache, 16 s around it). A stack that fits reads through it, so a second pass reads from memory. Elsewhere every read goes through the cache.

### The C boundary

Every C++ entry point in `crates/deptheos_dng/native/dng_c_api.cpp` runs through one `guard.h` barrier that catches C++ exceptions and turns them into error codes with a cause. The Rust FFI structs are declared by hand (no `bindgen`), with compile-time size and field-offset assertions on both sides.

## Alternatives considered

### Converter output without `-l`, demosaiced on the GPU

Mosaic output demosaiced on the GPU with Malvar-He-Cutler cut the conversion of 200 CR3s from about 110 s to 24 s on an M5 Max, but gave a different demosaic from Adobe's. The converter backend exists to give Adobe's exact demosaic and rendering, so it converts with `-l`.

### LibRaw for linear DNGs

LibRaw takes about 2 s to read a 44 MP linear DNG, against 43–60 ms through the DNG SDK, and cannot write DNG.

### A pure-Rust DNG reader and writer

Reimplementing Stage 1 and 2 processing, opcode lists, DCP profiles and JPEG XL and lossless JPEG DNG writing would take weeks and risk subtle color differences from Adobe's reference.

### The DNG SDK's demosaic for mosaic DNGs on iOS

It avoids building LibRaw for iOS and matches the SDK's own crop. Its demosaic is visibly softer than LibRaw's at 100%, a focus stack is judged at 100%, and the platforms would disagree.

### Crop LibRaw's output after the decode

It needs the SDK's crop passed across to the decode and a full-size copy per frame. LibRaw crops before its demosaic when asked, at no cost.

### Mosaic DNGs always through LibRaw on iOS

It would keep the iOS camera's Bayer bursts on the linear path ADR-162 shoots them for. But a photographer who wants Apple's rendering of their RAW files should get it for every RAW file, a DNG straight from a sensor as much as a CR3. The linear path is one switch away.

### The decoder choice for linear DNGs too

A linear DNG through `CIRAWFilter` would be developed again by Apple from data the SDK reads exactly; nothing is gained.

## Consequences

- LibRaw and Adobe DNG Converter crop and demosaic differently, so frames decoded by different backends do not line up pixel for pixel; hence the backend recorded in each render.
- The converter is a child process: canceling a stack kills the conversions in flight. The cache keeps at most 8 GB of conversions nobody holds. Depth and Wavelet hold every frame's conversion until pass 2 has read it, so once the first conversion says how large they are, the session checks the disk and stops with `Error::NoConversionSpace` if the rest would leave less than 1 GB free.
- DCP matching needs the exact camera model ("Canon EOS R5" never matches "Canon EOS R50"). Without the preferred profile it falls back to the other standard profile, then any profile for that camera, then LibRaw's matrices.
- Linux uses LibRaw, with any DCP folders the user adds. LibRaw on iOS has no Adobe profiles to find: camera RAW it decodes there is colored by LibRaw's matrices unless a DCP is found.
- Neither the SDK nor LibRaw applies a mosaic DNG's `OpcodeList1` and `OpcodeList2` (bad pixels, lens shading gain maps): a phone's or drone's gain map is not applied to its mosaic DNGs.
- An X-Trans mosaic DNG's crop origin is rounded down to its 6×6 pattern by LibRaw; its size is still the `DefaultCrop`'s.
- On iOS, a stack of the camera's Bayer bursts is a standard-image stack by default; RAW Decoder set to LibRaw makes it linear, with DNG export. The iOS app is a few megabytes larger for LibRaw.
- `dng_fast_read.cpp` asserts the SDK version it was written against (`dngVersion_1_7_1_0`), a tripwire for the internal SDK invariants it relies on, to be checked whenever the vendored SDK is updated.

## Code

- `crates/deptheos_raw/native/raw_c_api.cpp`, `crates/deptheos_core/src/dcp.rs`, `crates/deptheos_core/src/io/load.rs`.
- `crates/deptheos_dng/src/lib.rs`, `crates/deptheos_dng/src/mosaic.rs`; `crates/deptheos_dng/native/` (`dng_c_api.cpp`, `dng_io.cpp`, `dng_write.cpp`, `dng_bridge.h`, `dng_fast_read.cpp`, `dng_lossless_jpeg_fast.cpp`, `guard.h`).
- `crates/deptheos_app/src/frames/raw_backend.rs`, `crates/deptheos_app/src/frames/conversions/`, `crates/deptheos_app/src/runs/session.rs`, `crates/deptheos_app/src/library/stacks.rs`.
- `crates/deptheos_gpui/src/prefs.rs`; `crates/deptheos_ffi/src/settings.rs`, `crates/deptheos_ffi/src/list.rs`.
- `ios/DeptheosMobile/Engine/AppleFrameDecoder.swift`, `ios/DeptheosMobile/Model/StackSettingsModel.swift`, `ios/DeptheosMobile/Views/InspectorView.swift`.
- Tests: `crates/deptheos_core/tests/host_decoder.rs`, `crates/deptheos_core/tests/integration/decode.rs`, `crates/deptheos_dng/src/fast_read_tests.rs`.
