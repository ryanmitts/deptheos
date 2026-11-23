# ADR-130: RAW stacks stay linear camera RGB, and their color is rendered on the GPU from the DNG SDK's parameters

## Status

Accepted.

## Context

Photographers who stack camera RAW or DNG brackets usually develop the result afterwards in a raw editor. Rendering RAW frames to sRGB before stacking would bake in white balance, exposure and the profile's tone curve, and throw away scene-referred headroom.

A linear result still has to be shown and exported as JPEG, PNG or TIFF. That goes through the camera's color pipeline: white balance, the camera profile's matrices and tables (`HueSatMap`, `LookTable`), baseline exposure and the tone curve. It runs for every source preview, compare view, stacked result, dirty rectangle while painting, and non-DNG export. The Adobe DNG SDK's reference renderer (`dng_render`) runs on the CPU, takes about 160 ms for a 44 MP linear DNG, and needs a whole `dng_negative` rebuilt in RAM.

## Decision

### Frames and results stay linear

- **Every RAW and DNG frame decodes to 16-bit demosaiced linear camera RGB**, with no white balance, tone curve or gamma (`deptheos_core::io::decode_linear`; the decoders are ADR-131). LibRaw's `adjust_maximum_thr` is 0, so every frame in a bracket keeps the camera's fixed white level without per-frame gain flicker.
- **Sharpness is measured in sRGB; the output is linear.** Focus metrics and the Wavelet luma target use sRGB-encoded copies. The Pyramid stacker blends in sRGB and converts its output back to linear, because a linear Laplacian pyramid over-weights highlights and darkens the image. The Depth blend mixes linear camera RGB directly.
- **A result exports as a linear DNG** carrying the reference frame's color matrices, camera profile, white balance, EXIF, and `OpcodeList3` where it still applies (ADR-131). For a LibRaw stack, the DNG metadata is built from LibRaw's matrices plus the chosen Adobe camera profile (`dt_dng_from_raw_metadata`). JPEG, PNG and TIFF exports render the linear result through the GPU pipeline below.
- **A render keeps its color metadata.** Each saved render of a linear stack, camera RAW or DNG frames alike, keeps a 16×16 `color.dng` in the Library carrying the reference frame's DNG metadata, so it can be shown, retouched or exported as DNG after its source frames have been moved or deleted. It holds no GPS position or serial numbers (ADR-133).
- **A stack is all RAW and DNG, or all standard images.** `StackingSession` refuses a mixed list (`mixed_kinds`) before reading any frame, since linear camera RGB and gamma-encoded images cannot be blended in one color pipeline. On iOS, a camera RAW file or mosaic DNG decoded by Apple counts as a standard image (ADR-131).

### The color render runs on the GPU; its parameters come from the SDK

- `deptheos_compute_render_params` (`crates/deptheos_dng/native/dng_metadata.cpp`) rebuilds a negative from the frame's metadata and lets the DNG SDK compute the camera-to-ProPhoto matrix (white balance included), `HueSatMap`, `LookTable`, exposure offset and a 257-point tone curve. Profile selection and dual-illuminant interpolation stay Adobe's code.
- `crates/deptheos_core/src/dng_render/dng_render.wgsl` applies those steps in the SDK renderer's order. `PreparedRender` caches the GPU buffers for a parameter set, and `render_into` renders any sub-rectangle (a live brush stroke) identically to that rectangle of a full-frame render.
- Parity with the SDK's CPU renderer is held at more than 60 dB PSNR (65.9–78.6 dB on the test DNGs and profiles) by `crates/deptheos_core/tests/integration/dng_render.rs` and `dcp.rs`.
- The SDK's CPU renderer serves only as the tests' reference (`deptheos_dng::sdk_render_file_srgb`), the end-to-end checker's render of a linear DNG, frame or export, from its own negative (`open_raw` in `crates/deptheos_e2e/src/read.rs`; camera RAW and mosaic DNGs it decodes as the app does), and the last-resort thumbnail of a DNG that has no embedded preview and that LibRaw cannot read (`sdk_render`, `crates/deptheos_app/src/frames/source_thumbnail.rs`).

## Alternatives considered

### Decode RAW to sRGB unless exporting a DNG

A render is stacked once and exported to any format afterwards. Stacking in linear camera RGB gives the same result whether the user later exports a DNG, TIFF, PNG or JPEG.

### Let the first frame's format pick the pipeline for a mixed list

Blending linear camera RGB with sRGB JPEGs breaks the colors of whichever format differs from frame 0.

### The DNG SDK's CPU renderer for display and exports

Too slow for paint rectangles, and stack results and warped compare views are already GPU textures: each display update would need a full-frame readback and a CPU render.

### LibRaw's sRGB output for non-DNG exports

The viewer (through the DNG pipeline) and the exported file then showed different colors. With every view and non-DNG export through `dng_render.wgsl`, previews, compare views, paint strokes and exported files match.

## Consequences

- Exported DNGs are demosaiced (linear) DNGs: a raw editor gets the full highlight and shadow latitude, white balance and profile choice; its mosaic-only features do not apply.
- `dng_render.wgsl` must keep parity with the SDK's renderer; synthetic parameter tests and file tests in `dng_render.rs` guard against regressions such as a dropped profile tone curve or a half-sample LUT offset.
- `RGBTables`, `ProfileGainTableMap` (used by Apple ProRAW) and overrange HDR profiles are not implemented in `dng_render.wgsl`. None of the 4,418 standard Adobe camera profiles uses `RGBTables` or `ProfileGainTableMap`.
- On iOS, camera RAW and mosaic DNGs decoded by Apple's `CIRAWFilter` (the default) arrive as 16-bit Display P3 (ADR-132), so only linear DNGs take the linear path and offer DNG export; a stack set to LibRaw takes it with all of them (ADR-131).

## Code

- `crates/deptheos_core/src/io/load.rs`, `crates/deptheos_core/src/pyramid/pyramid.wgsl`.
- `crates/deptheos_core/src/dng_render/mod.rs`, `crates/deptheos_core/src/dng_render/dng_render.wgsl`.
- `crates/deptheos_dng/native/dng_metadata.cpp`, `crates/deptheos_dng/native/dng_c_api.cpp`.
- `crates/deptheos_app/src/runs/session.rs`, `crates/deptheos_app/src/library/render_file.rs`.
- `crates/deptheos_core/tests/integration/dng_render.rs`, `dcp.rs`.
