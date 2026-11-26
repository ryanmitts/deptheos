# ADR-137: The adjustment stages: one order on both kinds of stack, each slider's law in the resolve step

## Status

Accepted.

## Context

Adjust's sliders change how a render's stored pixels are drawn, never the pixels (ADR-135). A camera RAW stack is drawn through the DNG SDK's stages on the GPU (ADR-130), a standard stack through its profile's curves (`standard.wgsl`). The sliders have to sit between those stages so that RAW keeps matching the SDK where they are at rest, both kinds respond alike where they share a stage, and the first step from neutral never jumps.

## Decision

### The order

Every adjusted draw runs, in linear ProPhoto between the input and the output space:

1. The stored pixels to linear ProPhoto. RAW: the profile's black level, white clip and camera matrix, clamped to [0, 1], then its hue-saturation map. Standard: the profile's curves undone, then its matrix.
2. Exposure: for RAW the SDK's ramp, then a linear gain, then the shoulder; for a standard stack the gain and the shoulder.
3. RAW only: the profile's look table and tone curve.
4. The output space, clamped per channel, and its encoding.

The stages are functions in `crates/deptheos_render/src/dng_render/adjust.wgsl`, imported by both shaders, each behind a flag in its kind's uniform. With every flag off a draw is the unadjusted one: a neutral look takes the unadjusted path (ADR-135) and draws bit for bit as before.

### Where the clamps sit

- **Only the exposure ramp's clamp at white moves**, and only while exposure is set. What the gain lifts past white goes to the shoulder, which brings it back under white before the look table. The clamps after the camera matrix, at the tone curve's input and at the output stay, so a saturated color at camera white clips as the SDK clips it.
- **A clamp moves only where a stage needs the values it removes**, and every stage approaches the unadjusted picture as its slider approaches neutral.

### Each slider's law is in the resolve step

`deptheos_render::resolve` (`crates/deptheos_render/src/resolve.rs`) turns the sliders into the numbers the shaders take (`Stages`, `crates/deptheos_render/src/dng_render/stages.rs`). The shaders hold no slider math, so the laws are table-tested with no GPU, and the f64 reference (`dng_render/reference.rs`) takes the same numbers and checks the shaders.

- **Exposure is a law on the total**, the slider's stops plus a RAW stack's baseline. The SDK's ramp and its darkening curve take the total as far as they can, never less than the baseline's own darkening; a linear gain takes the rest. So a raised exposure renders as the SDK's exposure does, at any baseline, and a lowered one darkens white too, which the SDK's negative exposure does not. A standard stack has no baseline: its exposure is the gain.
- **The shoulder** is lightcraft's roll-off into white in stops (MIT OR Apache-2.0: craft-libs `crates/develop/src/tone.rs`, `rolloff_past`), joined at slope 1 at its knee. The knee fades in from a clamp at white as exposure raises the picture past what was white, toward `SHOULDER_STOPS` below white.

### The shaders' constants come from Rust

The constants the shaders share with Rust (`standard.rs`'s `CODES` among them) are written into a WGSL module, `deptheos::render_constants`, from their Rust values when the pipelines are built (`constants_module` in `stages.rs`), so the two cannot drift.

### The checks

- **The f64 reference** follows both shaders, a linear stack's for profiles without HSV tables, and the shaders stay within 1 code (sRGB) and 2 codes (a linear stack) of it under every slider value tested.
- **A first step from neutral** moves a picture by at most twice what the next step does, on a gradient with values past white (`tests/integration/adjust.rs`): a stage that clipped or unclipped a range of values at its first step would jump.
- **Each slider's direction** is a test: exposure raised brightens, lowered darkens.

## Alternatives considered

### Exposure through the SDK's exposure alone

The SDK's negative exposure keeps white at white, so lowering exposure would not darken the brightest tones. A gain after the ramp does, and the SDK's ramp still renders a raised exposure.

### A clamp after the gain

Raising exposure would clip every highlight within the raise of white. The shoulder keeps them apart.

### Constants written by hand in each shader

Two copies of a number drift. The module written from Rust is the only copy the shaders see.

## Consequences

- With exposure set, a RAW stack's values above white reach the shoulder rather than the clamp, so an exposure raised from neutral never matches the SDK in the highlights; with the shoulder off it does (`Constants::without_shoulder`).
- How far the shoulder reaches is the process version's, chosen by eye.

## Code

- `crates/deptheos_render/src/resolve.rs`, `crates/deptheos_render/src/dng_render/stages.rs`, `crates/deptheos_render/src/dng_render/adjust.wgsl`, `crates/deptheos_render/src/dng_render/dng_render.wgsl`, `crates/deptheos_render/src/dng_render/standard.wgsl`.
- `crates/deptheos_render/src/dng_render/reference.rs`, `crates/deptheos_render/tests/integration/adjust.rs`.
- `crates/deptheos_adjust/src/lib.rs`, `crates/deptheos_cli/src/redraw.rs`.
