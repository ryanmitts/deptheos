# ADR-135: Every draw of a stack takes a look, resolved once and drawn by one render function

## Status

Accepted.

## Context

A stack's stored pixels become colors on screen, in compares, Retouch, exports, aligned frames and sequences, and pick the profile an export carries. Each site picked its own route: a linear stack through the GPU color render (ADR-130), a standard one through a CPU conversion, an export passing its values through. Adjustments on stacked results have to apply the same way at every site, or the screen and the export disagree. And a slider that reads neighbors must draw a region as it draws the whole picture, or rectangles and tiles show seams.

## Decision

### One value says how to draw

- **A `Look` is the stack, its adjustments and an output space** (`crates/deptheos_app/src/engine/look.rs`). Every function that turns stored pixels into color takes one, so a site that has not been routed does not build. It also carries a reframe, applied last (ADR-136). There are four ways to make one:
  - `shown`: the render on screen, under adjustments, in the display's space.
  - `stored`: the stored picture on screen, with no adjustments. Source frames, the depth map and Retouch use it.
  - `exported`: the render in a file. A linear stack exports in sRGB, a standard one in its own profile. Aligned frames export with no adjustments.
  - `redrawn`: the render in a file through the adjusted path even when neutral, with a reframe: the redraw command's (ADR-103).
- **`Adjustments` lives in `deptheos_adjust`**, a crate with no Deptheos dependency, and carries its process version. A neutral value draws exactly as none: the tests check this bit for bit at every site.
- **`OutputSpace` is Srgb or StackProfile.** The screen is sRGB while Color Management is on, else the stack's own profile, which the look resolves as it draws. StackProfile passes an unadjusted standard stack's values through, and the export carries the profile (`Look::icc`).

### The resolve step

- **`deptheos_render::resolve`** takes the stack's color (linear render parameters, or a standard stack's profile), the adjustments and the output space. It returns a `Draw`: the GPU's `Constants`, or the stored values to convert on the CPU. It is pure and needs no GPU, so table tests cover it.
- **Each slider's law is here** (ADR-137): the adjustments become the numbers the shaders take, so the shaders hold no slider math.
- **Equal constants draw equal pictures.** Paint's rectangles reuse a `PreparedRender` by comparing `Constants`.

### Adjusted standard stacks draw on the GPU

- **The stored 16-bit values upload as they are** and the shader undoes the profile's curves from a table of every code (`standard.wgsl`): exact, where a 16-bit float texture loses highlight precision and a 32-bit one doubles the memory. The pixels go to linear ProPhoto, then to the output space, encoded through sRGB's curve or back through the profile's own to the nearest code.
- **A table-based profile is linearized by moxcms on the CPU** into a float texture, and draws in sRGB, since its space has no curves to encode back through.
- **Unadjusted, nothing changes.** Neutral, the adjusted path lands within a few codes of the CPU display (`tests/integration/standard.rs`); an f64 reference checks the shader.

### The color render is its own crate

- **`deptheos_render` holds the color render, the resolve step and the render function.** `deptheos_core` does not depend on it, so a stacker cannot depend on how a picture is shown.

### One render function

- **`DngRenderPipeline::draw` takes a `Source` and a `Region`** (`crates/deptheos_render/src/dng_render/draw.rs`). The source is a whole texture or a part of the frame, such as paint's rectangle; the region is in frame pixels. It draws in tiles of 2048 pixels, a dispatch and a submit each, so no GPU command runs long.
- **`draw_with` adds a stage that reads neighbors.** The per-pixel pass draws the region grown by the stage's margin, kept within what the source covers. The stage then writes the region, reading a whole-frame input at frame coordinates. A 3×3 median test stage proves that a region drawn this way equals that region of the whole picture, bit for bit, across tile edges.
- **The shader's stages are functions**, run as one fused pass with f32 values between them.

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

A file's own preview (`Engine::load_preview`) is not a stack and has no look; it resolves its color with no adjustments, in the display's space.

## Alternatives considered

### An adjustments argument beside today's parameters

Each site would work out the output space again, and one that forgot the argument would still build.

### Slider math at each site

Every front end's path would carry it, and its tests would drive the GPU. A pure resolve step is tested with tables.

### Draw the whole picture as one dispatch

A long dispatch on a large picture risks the operating system's GPU watchdog, and a stage that reads neighbors needs regions and margins anyway.

## Consequences

- Unadjusted standard stacks keep the CPU display and the export pass-through.
- The front ends pass neutral adjustments until renders store their own.
- A stage that reads neighbors must keep its reach within its margin. A rectangle source's margin must lie inside the rectangle.
- `just handoff-check` compares every draw site's pixels with a capture written before the change, bit for bit.

## Code

- `crates/deptheos_adjust/src/lib.rs`.
- `crates/deptheos_render/src/resolve.rs`, `crates/deptheos_render/src/dng_render/mod.rs`, `crates/deptheos_render/src/dng_render/draw.rs`, `crates/deptheos_render/src/dng_render/standard.rs`, `crates/deptheos_render/src/dng_render/standard.wgsl`.
- `crates/deptheos_app/src/engine/look.rs`, `crates/deptheos_app/src/engine.rs`, `crates/deptheos_app/src/runs/session/stack.rs`, `crates/deptheos_app/src/paint/render.rs`.
- `crates/deptheos_render/tests/integration/regions.rs`, `crates/deptheos_render/tests/integration/standard.rs`, `crates/deptheos_render/src/dng_render/reference.rs`, `crates/deptheos_app/src/engine/handoff_tests.rs`.
