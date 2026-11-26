# ADR-135: Every draw of a stack takes a look, resolved once and drawn by one render function

## Status

Accepted.

## Context

A stack's stored pixels become colors in many places: the result on screen, compares, Retouch's sources and live rectangles, exports, aligned frames, sequences, and the ICC profile an export carries. Each of these picked its own route: a linear stack through the GPU color render (ADR-130), a standard one through a CPU conversion from its profile, an export passing its values through. Adjustments on stacked results (exposure, white balance and the sliders after them) have to apply the same way at every one of those sites, or the screen and the export disagree. And a slider that reads neighbors must draw a region as it draws the whole picture, or paint rectangles and tiles show seams.

## Decision

### One value says how to draw

- **A `Look` is the stack, its adjustments and an output space** (`crates/deptheos_app/src/engine/look.rs`). Every function that turns stored pixels into color takes one, so a site that has not been routed does not build. There are three ways to make one:
  - `shown`: the render on screen, under adjustments, in the display's space.
  - `stored`: the stored picture on screen, with no adjustments. Source frames, the depth map and Retouch use it.
  - `exported`: the render in a file. A linear stack exports in sRGB, a standard one in its own profile. Aligned frames export with no adjustments.
- **`Adjustments` lives in `deptheos_adjust`**, a crate with no Deptheos dependency, and carries its process version. A neutral value draws exactly as none: the tests check this bit for bit at every site.
- **`OutputSpace` is Display, Srgb or StackProfile.** Display follows the Color Management setting. StackProfile passes a standard stack's values through and tags the export with its profile (`Look::icc`).

### The resolve step

- **`deptheos_render::resolve`** takes the stack's color (linear render parameters, or a standard stack's profile), the adjustments and the output space. It returns a `Draw`: either the color render's `Constants` (the uniform, the composed tone curve and the profile's tables), or the stored values to convert on the CPU. It is pure and needs no GPU, so table tests cover it.
- **Equal constants draw equal pictures.** `Constants` is compared to reuse a `PreparedRender` (paint's rectangles), not the render parameters it came from.

### The color render is its own crate

- **`deptheos_render` holds the color render, the resolve step and the render function**, out of the stackers' reach. `deptheos_core` does not depend on it, so a stacker cannot draw in a look.

### One render function

- **`DngRenderPipeline::draw` takes a `Source` and a `Region`** (`crates/deptheos_render/src/dng_render/draw.rs`). The source is a whole texture, or a part of the frame such as paint's rectangle. The region is in frame pixels. The function draws in tiles of 2048 pixels, one dispatch and one submit each, so no single GPU command runs long on a large picture.
- **`draw_with` adds a stage that reads neighbors.** The per-pixel pass draws the region grown by the stage's margin, kept within what the source covers. The stage then writes the region, reading a whole-frame input at frame coordinates. A 3×3 median test stage proves that a region drawn this way equals that region of the whole picture, bit for bit, across tile edges.
- **The shader's stages are functions** in `dng_render.wgsl`, run as one fused pass with f32 values between them, so stages can be added between them without a second pass.

### The draw sites

On screen, in a `shown` look:

- the stacked result as a run ends (`Engine::stack`, `LinearDisplay::render`)
- a render opened from the Library (`Engine::present_stack`)
- a display redrawn from the pixels (`Engine::present_pixels`)
- the other render in a render compare (`Engine::load_render_compare`)

On screen, in a `stored` look:

- a source frame compared, with its contribution if asked (`Engine::load_compare`, `compare_rgb`)
- Retouch's sources and its painted displays, whole and by rectangle (`Engine::paint_source`, paint's `RectRenderer`)

In a file, in an `exported` look:

- a save, a retouched save and a flattened save (`Engine::save`, `save_pixels`, `save_flattened`)
- an aligned frame, with no adjustments
- the frames of grow and turntable sequences (the sequence writer's `finish`)
- the ICC profile an export carries (`StackTags::read`, `aligned_frame_metadata`)

A file's own preview (`Engine::load_preview`) is not a stack and has no look. It resolves its own color with no adjustments, in the display's space.

## Alternatives considered

### An adjustments argument beside today's parameters

Each site would have to work out the output space and profile again, and a site that forgot the argument would still build. One value that every draw takes makes a missed site a build error.

### Resolve inside the shader's caller at each site

That puts slider math in every front end's path, and the tests would have to drive the GPU. A pure resolve step is tested with tables, and the f64 reference takes its output.

### Leave the color render in `deptheos_core`

The stackers and the color render would share a crate, and nothing would keep a stacker from depending on how a picture is shown.

### Draw the whole picture as one dispatch

That is simpler, but a long dispatch on a large picture risks the operating system's GPU watchdog, and a stage that reads neighbors needs regions and margins anyway.

## Consequences

- Unadjusted standard stacks keep the CPU display and the export pass-through. Only a linear stack, or an adjusted one, draws on the GPU.
- The front ends pass neutral adjustments until renders store their own.
- A stage that reads neighbors must keep its reach within its margin. A rectangle source's margin must lie inside the rectangle.
- `just handoff-check` compares every draw site's pixels with a capture written before the change, bit for bit.

## Code

- `crates/deptheos_adjust/src/lib.rs`.
- `crates/deptheos_render/src/resolve.rs`, `crates/deptheos_render/src/dng_render/mod.rs`, `crates/deptheos_render/src/dng_render/draw.rs`, `crates/deptheos_render/src/dng_render/dng_render.wgsl`.
- `crates/deptheos_app/src/engine/look.rs`, `crates/deptheos_app/src/engine.rs`, `crates/deptheos_app/src/runs/session/stack.rs`, `crates/deptheos_app/src/paint/render.rs`.
- `crates/deptheos_render/tests/integration/regions.rs`, `crates/deptheos_app/src/engine/handoff_tests.rs`.
