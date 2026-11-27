# ADR-137: The adjustment stages: one order on both kinds of stack, each slider's law in the resolve step

## Status

Accepted.

## Context

Adjust's sliders change how a render's stored pixels are drawn, never the pixels (ADR-135). A camera RAW stack is drawn through the DNG SDK's stages on the GPU (ADR-130), a standard stack through its profile's curves (`standard.wgsl`). The sliders have to sit between those stages so that RAW keeps matching the SDK where they are at rest, both kinds respond alike where they share a stage, and the first step from neutral never jumps.

## Decision

### The order

Every adjusted draw runs, in linear ProPhoto between the input and the output space:

1. The stored pixels to linear ProPhoto. RAW: the profile's black level, white clip and camera matrix at the chosen white, clamped to [0, 1], then its hue-saturation map. Standard: the profile's curves undone, then its matrix, then white balance, which joins the matrix since no clamp comes between.
2. Exposure: for RAW the SDK's ramp, then a linear gain, then the shoulder; for a standard stack the gain and the shoulder.
3. RAW only: the profile's look table and tone curve.
4. Contrast, Whites and Blacks, one curve on linear ProPhoto.
5. Vibrance, then Saturation, scaling OkLab's chroma.
6. The output space, clamped per channel, and its encoding.

The stages are functions in `crates/deptheos_render/src/dng_render/adjust.wgsl`, imported by both shaders, each behind a flag in its kind's uniform. With every flag off a draw is the unadjusted one: a neutral look takes the unadjusted path (ADR-135) and draws bit for bit as before.

### Where the clamps sit

- **Only the exposure ramp's clamp at white moves**, and only while exposure is set. What the gain lifts past white goes to the shoulder, which brings it back under white before the look table. The clamps after the camera matrix, at the tone curve's input and at the output stay, so a saturated color at camera white clips as the SDK clips it.
- **A clamp moves only where a stage needs the values it removes**, and every stage approaches the unadjusted picture as its slider approaches neutral.

### Each slider's law is in the resolve step

`deptheos_render::resolve` (`crates/deptheos_render/src/resolve.rs`) turns the sliders into the numbers the shaders take (`Stages`, `crates/deptheos_render/src/dng_render/stages.rs`). The shaders hold no slider math, so the laws are table-tested with no GPU, and the f64 reference (`dng_render/reference.rs`) takes the same numbers and checks the shaders.

- **Exposure is a law on the total**, the slider's stops plus a RAW stack's baseline. The SDK's ramp and its darkening curve take the total as far as they can, never less than the baseline's own darkening; a linear gain takes the rest. So a raised exposure renders as the SDK's exposure does, at any baseline, and a lowered one darkens white too, which the SDK's negative exposure does not. A standard stack has no baseline: its exposure is the gain.
- **A camera RAW stack's white balance is the DNG SDK's white point.** Its temperature and tint are the SDK's (`dng_temperature`), starting at the render's as-shot values (`CompletedStack::as_shot`). `Look` (`crates/deptheos_app/src/engine/look.rs`) asks the SDK for the color science at that white (`DngImage::render_params_at`, about 0.6 ms) before the resolve step, which stays pure; the shaders change nothing for it. Within the matrices' range it renders as the SDK does at that white, and a DNG export writes it as the file's as-shot white, its pixels untouched.
- **A standard stack's white balance is relative**, since its scene's white is unknown: the scale's 0 is 6500 K, each step 0.8 mired (`deptheos_adjust::temperature`), and its tint is lightcraft's, positive toward magenta. One Bradford adaptation in linear ProPhoto takes the white the sliders name to the 6500 K reference, scaled so white's largest channel stays 1: white takes a tint but never clips to one. Where a white's cone response is not above 0 it adapts nothing. The locus, tint and checked adaptation are lightcraft's as they stand (MIT OR Apache-2.0: craft-libs b2d2a1e94 `crates/color-transform/src/cct.rs`, `crates/cms/src/math.rs`), in `crates/deptheos_render/src/white.rs`.
- **Contrast, Whites and Blacks are one curve**, lightcraft's operators as they stand (MIT OR Apache-2.0; `crates/deptheos_render/src/curve.rs`). A tone map on luminance in a gamma-2.2 domain, identity at neutral, scales the pixel by its ratio and pulls it toward its luminance as far as a channel passes white (craft-libs b2d2a1e94 `crates/develop/src/tone.rs`). A standard stack takes all three there. A camera RAW stack takes Whites there, after its profile's tone curve, then Contrast and Blacks as lightcraft's fitted operators on sRGB-encoded ProPhoto, Contrast per channel and Blacks mixed with luminance (craft-libs b2d2a1e94 `crates/develop/src/light_v3.rs`, constants from `light_v3_params.rs` in `curve_fits.rs`). The tables are made in the resolve step and bound to the shaders as one buffer; `curve.wgsl` reads them.
- **The tone map's shoulder into white is as wide as Whites lifts white**, up to lightcraft's (0.95 to 1.05). lightcraft's own pulls white to 0.988 at any edit of the curve, which would make the first step of Contrast or Blacks from neutral jump; sized to the lift, Contrast and Blacks keep white at 1 and a small lift gets a small shoulder.
- **Vibrance and Saturation scale OkLab's chroma**, lightcraft's as they stand (MIT OR Apache-2.0: craft-libs b2d2a1e94 `crates/develop/src/colorops.rs`; `crates/deptheos_render/src/chroma.rs`). Vibrance raises low chroma most, below 0.22, and when raised acts less near skin's hue (sRGB's 25°); Saturation scales every chroma, to gray at -100. OkLab is Ottosson's, from linear ProPhoto through XYZ adapted to D65 (craft-libs b2d2a1e94 `crates/color-transform/src/oklab.rs`), its cone matrix scaled so white lands exactly on the neutral axis, so gray keeps no chroma for Vibrance to raise. A stack with no color, a camera RAW stack without a color profile or a standard stack whose channels are all equal (`CompletedStack::has_color`), takes no color slider.
- **The shoulder** is lightcraft's roll-off into white in stops (MIT OR Apache-2.0: craft-libs b2d2a1e94 `crates/develop/src/tone.rs`, `rolloff_past`), joined at slope 1 at its knee. The knee fades in from a clamp at white as exposure raises the picture past what was white, toward `SHOULDER_STOPS` below white.

### Following lightcraft

The sliders take lightcraft's laws and numbers as they stand, from lightcraft 5bdb4e7a6 and the craft-libs revision it pins, b2d2a1e94; each citation names its revision. `just upstream` (`script/upstream.sh`) shows what lightcraft has changed since in the files cited, and what craft-libs has at the revision lightcraft now pins. Taking a change of look is a new process version. Where Deptheos departs:

- Every stage runs on linear ProPhoto, where the DNG SDK applies a camera profile's look table and tone curve; lightcraft works on linear Rec. 2020. OkLab's chroma is the same from either.
- The tone map's shoulder is sized to the Whites lift.
- The exposure shoulder is a stage of its own, after the gain, fading in from a clamp as exposure rises.

### The shaders' constants come from Rust

The constants the shaders share with Rust (`standard.rs`'s `CODES` among them) are written into a WGSL module, `deptheos::render_constants`, from their Rust values when the pipelines are built (`constants_module` in `stages.rs`), so the two cannot drift.

### The checks

- **The f64 reference** follows both shaders, a linear stack's for profiles without HSV tables, and the shaders stay within 1 code (sRGB) and 2 codes (a linear stack) of it under every slider value tested.
- **A dark gradient under Blacks**, raised and lowered, climbs with no flat run and no jump (`a_dark_gradient_under_blacks_shows_no_banding`): the operators' tables are over sRGB-encoded values, fine in the shadows. **Whites, Vibrance and Saturation draw alike on both kinds** for the same linear values (`the_shared_stages_draw_alike_on_both_kinds`); the kinds' Contrast and Blacks differ by design.
- **A first step from neutral** moves a picture by at most twice what the next step does, on a gradient with values past white (`tests/integration/adjust.rs`): a stage that clipped or unclipped a range of values at its first step would jump.
- **The DNG SDK's own render** at a white and a raised exposure, with the shoulder off, is matched above 60 dB, with no sample more than 1% off (`a_white_and_a_raised_exposure_render_like_the_sdk`), on synthetic color science and a real file's.
- **Each slider's direction** is a test: exposure raised brightens, lowered darkens; Contrast darkens shadows and lightens highlights, Whites and Blacks move their tones each way (`contrast_whites_and_blacks_move_their_tones`); Vibrance and Saturation raise and lower color (`vibrance_and_saturation_move_color`); a higher kelvin is warmer and a positive tint more magenta (`a_render_at_a_white_moves_as_the_sdk_says`), on the relative scale too (`white_balance_warms_and_tints_a_standard_stack`).

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

- `crates/deptheos_render/src/resolve.rs`, `crates/deptheos_render/src/white.rs`, `crates/deptheos_render/src/curve.rs`, `crates/deptheos_render/src/curve_fits.rs`, `crates/deptheos_render/src/chroma.rs`, `crates/deptheos_render/src/dng_render/curve.wgsl`, `crates/deptheos_render/src/dng_render/stages.rs`, `crates/deptheos_render/src/dng_render/adjust.wgsl`, `crates/deptheos_render/src/dng_render/dng_render.wgsl`, `crates/deptheos_render/src/dng_render/standard.wgsl`.
- `crates/deptheos_render/src/dng_render/reference.rs`, `crates/deptheos_render/tests/integration/adjust.rs`.
- `crates/deptheos_adjust/src/lib.rs`, `crates/deptheos_app/src/engine/look.rs`, `crates/deptheos_cli/src/redraw.rs`.
- `crates/deptheos_dng/src/lib.rs`: `render_params_at`, `Temperature`, `render_linear_to_srgb_at`, `DngTags::white`.
