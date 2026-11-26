# ADR-136: The reframe is a window of stored pixels, flipped and turned, applied last

## Status

Accepted.

## Context

Adjust's Crop cuts a render to a rectangle and turns or mirrors it. Everything else about a render is in the stored picture's coordinates: its paint, its frame map, the depth map, compares and the line-up of two renders. A crop that resampled, or that moved those coordinates, would make every one of them map through it, and a paint stroke or a contribution lookup would land a pixel off after a rounding. Stacking already has a crop of its own, "Crop to the area every frame covers" (`StackGeometry`, `--no-crop`), which decides what the stack holds; this one only decides what is drawn.

## Decision

- **The reframe is a rectangle in stored pixels, then a flip, then a quarter-turn** (`Reframe` in `crates/deptheos_adjust/src/reframe.rs`). The rectangle is whole pixels, so cutting it is exact. `crop: None` keeps the whole picture.
- **The flip mirrors left to right, before the turn; the turn is clockwise.** A flip top to bottom is the mirror and a half turn, so the eight ways a rectangle can lie are each one value, and each has its EXIF and DNG Orientation (`Reframe::orientation`).
- **It is applied last, as an output window.** A look carries one (`Look::reframe`, `crates/deptheos_app/src/engine/look.rs`), and the export draws the whole picture and then copies the window out of it, flipped and turned (`Reframe::apply`, in `export`, `crates/deptheos_app/src/runs/session/stack.rs`). No pixel is resampled, so a reframed export's 16-bit pixels equal that window of the same render drawn whole (`tests/integration/reframe.rs`).
- **The mapping between stored and output points is plain math in `deptheos_adjust`**, with no GPU: `to_stored` and `to_output` take continuous points (a pixel's center at its index plus a half), and `apply` agrees with them pixel for pixel. A pointer on the reframed picture finds its stored pixel through `to_stored`.
- **A rectangle that reaches outside the picture is refused** (`OutsidePicture`), never clipped, since the picture's size is known when it is set.
- **The command line names it `--reframe X,Y,W,H`**, with `--turn 90|180|270` and `--flip horizontal|vertical`, on `deptheos redraw` (ADR-103). The name keeps it apart from stacking's `--no-crop`. On screen the tool is Crop (`docs/terms-and-usage.md`).
- **A DNG keeps the whole picture.** An export to DNG with a reframe is refused by the engine; the redraw command doesn't write DNG.
- **An angle is a later field.** Straightening needs a resample; it would come before the rectangle, which is then measured on the straightened picture, and leave the flip and turn as they are.

### The slider numbers beside it

`deptheos_adjust` also holds each adjustment slider's numbers (`slider.rs`: range, step, default and track) and the relative temperature scale of standard stacks (`temperature.rs`), so both apps and the command line read one definition. A value that isn't a finite number reads as the slider's default. The Kelvin track is even from 2000 to 7000 K on its left half and logarithmic above; the relative scale runs from -100 to 100 around 6500 K in equal mireds, 0.8 a step. Both, and the ranges, are lightcraft's (MIT OR Apache-2.0: `crates/develop/src/controls.rs` and `crates/ui-egui/src/panels/edit/sliders.rs`). What a value does to the picture is the process version's, not these numbers'.

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
- Thumbnails, the viewer and DNG's user crop are not reframed until renders store their reframe.

## Code

- `crates/deptheos_adjust/src/reframe.rs`, `crates/deptheos_adjust/src/slider.rs`, `crates/deptheos_adjust/src/temperature.rs`.
- `crates/deptheos_app/src/engine/look.rs`, `crates/deptheos_app/src/runs/session/stack.rs`, `crates/deptheos_app/tests/integration/reframe.rs`.
- `crates/deptheos_cli/src/redraw.rs`.
