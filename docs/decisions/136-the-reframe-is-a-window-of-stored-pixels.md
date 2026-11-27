# ADR-136: The reframe is a window of stored pixels, flipped and turned, applied last

## Status

Accepted.

## Context

Adjust's Crop cuts a render to a rectangle and turns or mirrors it. Everything else about a render is in the stored picture's coordinates: its paint, its frame map, the depth map, compares and the line-up of two renders. A crop that resampled, or that moved those coordinates, would make every one of them map through it, and a paint stroke or a contribution lookup would land a pixel off after a rounding. Stacking already has a crop of its own, "Crop to the area every frame covers" (`StackGeometry`, `--no-crop`), which decides what the stack holds; this one only decides what is drawn.

## Decision

- **The reframe is a rectangle in stored pixels, then a flip, then a quarter-turn** (`Reframe` in `crates/deptheos_adjust/src/reframe.rs`). The rectangle is whole pixels, so cutting it is exact. `crop: None` keeps the whole picture.
- **The flip mirrors left to right, before the turn; the turn is clockwise.** A flip top to bottom is the mirror and a half turn, so the eight ways a rectangle can lie are each one value, and each has its EXIF and DNG Orientation (`Reframe::orientation`).
- **It is applied last, as an output window.** A look carries one (`Look::reframe`, `crates/deptheos_app/src/engine/look.rs`), and the export draws the whole picture and then copies the window out of it, flipped and turned (`Reframe::apply`, in `export`, `crates/deptheos_app/src/runs/session/stack.rs`). No pixel is resampled, so a reframed export's 16-bit pixels equal that window of the same render drawn whole (`tests/integration/reframe.rs`).
- **The vignette is centered on the rectangle** (ADR-137): the render function takes it with its constants, so the whole picture is drawn under the cropped picture's vignette and a crop's window draws as the crop alone would. A draw at another scale takes the rectangle in its own pixels (`Constants::scaled`, ADR-138); a region is drawn at frame coordinates and takes it unchanged.
- **The mapping between stored and output points is plain math in `deptheos_adjust`**, with no GPU: `to_stored` and `to_output` take continuous points (a pixel's center at its index plus a half), and `apply` agrees with them pixel for pixel. A pointer on the reframed picture finds its stored pixel through `to_stored`.
- **A rectangle that reaches outside the picture is refused** (`OutsidePicture`), never clipped, since the picture's size is known when it is set.
- **The command line names it `--reframe X,Y,W,H`**, with `--turn 90|180|270` and `--flip horizontal|vertical`, on `deptheos redraw` (ADR-103). The name keeps it apart from stacking's `--no-crop`. On screen the tool is Crop (`docs/terms-and-usage.md`).
- **A DNG keeps every pixel.** An export to DNG writes the reframe as tags a raw editor reads and can undo: the crop as its `DefaultUserCrop`, relative to the whole picture, and the flip and turn as its `Orientation` (`Reframe::orientation`), both read back by the SDK in `a_reframe_is_written_as_the_user_crop_and_orientation` (`crates/deptheos_dng/src/lib.rs`). The redraw command writes one from a camera RAW render with only the reframe and white balance, the tags a DNG has for them.
- **Turns and flips act on the picture as shown** (`Reframe::turned_right`, `turned_left`, `flipped_horizontally`, `flipped_vertically`): a mirror after a turn is the mirror before the opposite turn, so each is one reframe value again, and the crop, in stored pixels, is kept by each.
- **The crop tool works on the uncropped picture as shown** (`deptheos_app::adjust::crop`, pure, so both apps drag alike): the crop drawn over it (`CropFrame::area`, the whole picture when there is none), its eight handles and a drag inside (`Handle`), each drag kept on the picture, at least 16 pixels a side and of the aspect's shape anchored at the opposite corner or edge (`CropFrame::drag`), and stored back in whole pixels (`CropFrame::reframed`). The aspects are Original (the picture's own, as shown), Free, 1:1, 4:5, 3:2 and 16:9, each ratio with its turned pair; picking one makes the largest crop of its shape inside the one there, centered. A drag sets the reframe as it moves and commits once on release; a turn, a flip, a shape picked and Reset Crop (no crop, turn or flip) are one commit each, and Esc puts back the reframe the tool opened on, as one more. Paste Adjustments never carries the reframe (`rules::pasted`).
- **On the desktop** (`crates/deptheos_gpui/src/app/cropping.rs`, `viewer/input.rs`) C opens and closes the tool, ⌘[ and ⌘] turn, X swaps the shape's width and height, Return keeps the crop and Esc cancels; the panel lists them. The tool is `AdjustTool::Crop`, holding the reframe it opened on and the aspect.
- **An angle is a later field.** Straightening needs a resample; it would come before the rectangle, which is then measured on the straightened picture, and leave the flip and turn as they are.

### The slider numbers beside it

`deptheos_adjust` also holds each adjustment slider's numbers (`slider.rs`: range, step, default and track) and the relative temperature scale of standard stacks (`temperature.rs`), so both apps and the command line read one definition. A value that isn't a finite number reads as the slider's default. The Kelvin track is even from 2000 to 7000 K on its left half and logarithmic above; the relative scale runs from -100 to 100 around 6500 K in equal mireds, 0.8 a step. Both, and the ranges, are lightcraft's (MIT OR Apache-2.0: `crates/develop/src/controls.rs` and `crates/ui-egui/src/panels/edit/sliders.rs`). What a value does to the picture is the process version's, not these numbers'.

The panel both apps draw is `deptheos_app::adjust::sliders`: each slider (`Slider`) with its numbers, how it reads and sets an `Adjustments` value, how its value is written ("+0.35 EV", "5200 K", "1.0 px", "+25") and read back when typed, and its name and help, messages in `deptheos_app.ftl` (`adjust-*`, ADR-161), in five groups in order: White Balance (Temperature, Tint), Tone (Exposure, Contrast, Highlights, Shadows, Whites, Blacks), Presence (Clarity, Texture, Vibrance, Saturation), Sharpening (Amount, Radius, Detail, Masking) and Vignette (Amount, Midpoint). `Panel::of` a stack says which show: a stack with no color (`CompletedStack::has_color`) has no white balance, Vibrance or Saturation. A camera RAW stack's white balance sliders rest at its as-shot white, and its white balance is as shot again once both are back there; a slider set or reset leaves the process version alone (ADR-152).

## Alternatives considered

### Crop by resampling into a new picture

Paint, the frame map and compares would all need the crop's mapping, and every stroke and lookup would round through it. A window keeps them in stored pixels.

### Crop the stored pixels before drawing

It gives the same picture while every stage is per pixel, but a stage that reads neighbors or a vignette centered on the frame would see the cut edge. Drawing whole and windowing after leaves them their whole input.

### A turn and a flip in either order, or two flips

Two flips and four turns describe each orientation twice. One mirror before the turn gives each of the eight exactly once, as the Orientation tag does.

### Clip a rectangle to the picture

A typo in a flag would draw a different crop than asked for, silently.

## Consequences

- An export of a reframed render draws the whole picture first; the window costs a copy of its pixels.
- 8-bit exports are dithered by position in the output, so they are not compared with a window of the whole.
- A thumbnail is drawn under its render's reframe, the window cut from a small draw and flipped and turned on the CPU, its vignette on the window in the draw's pixels (ADR-138). Each view draws a pane of the render through `view::Framing` (ADR-149): the result reframed, every other view flipped and turned but uncropped, with pointers mapped back to stored pixels.

## Code

- `crates/deptheos_adjust/src/reframe.rs`, `crates/deptheos_adjust/src/slider.rs`, `crates/deptheos_adjust/src/temperature.rs`.
- `crates/deptheos_app/src/adjust/crop.rs`, `crates/deptheos_gpui/src/app/cropping.rs`, `crates/deptheos_gpui/src/viewer/input.rs`.
- `crates/deptheos_app/src/adjust/sliders.rs`, `crates/deptheos_app/i18n/en-US/deptheos_app.ftl`.
- `crates/deptheos_app/src/engine/look.rs`, `crates/deptheos_app/src/runs/session/stack.rs`, `crates/deptheos_app/tests/integration/reframe.rs`.
- `crates/deptheos_cli/src/redraw.rs`.
- `crates/deptheos_dng/src/lib.rs` (`DngTags::user_crop`, `DngTags::orientation`), `crates/deptheos_dng/native/dng_io.cpp`.
