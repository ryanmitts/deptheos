# ADR-152: A render's adjustments are a file of their own beside it, naming their process version; Library format 3

## Status

Accepted.

## Context

A render's adjustment sliders and its reframe (ADR-136, ADR-137) change while the user works, many times a minute, long after the render was saved. Everything else in a render's folder is written once, as the stack ends (ADR-140): `render.json` is read by every listing and its type, `CompletedStack`, is used across the engine, so it can't change with each slider. Paint, the other thing that changes a render after it is made, already keeps its own file (`paint.sqlite`, ADR-148).

The values only mean something under the math that draws them. A slider's law can change between releases (a better tone curve, a sign fixed), and a saved value drawn under another law than it was set under changes a user's picture without their asking.

## Decision

- **Each render's adjustments and reframe are `adjustments.json` in its folder** (`crates/deptheos_app/src/library/adjustments.rs`). It holds every slider from the start, as numbers on each slider's own scale (`deptheos_adjust::slider`): exposure, white balance (as shot, kelvins and tint, or the relative scale), contrast, highlights, shadows, whites, blacks, clarity, texture, sharpening with its radius, detail and masking, vibrance, saturation, the vignette's amount and midpoint, and the reframe (a rectangle in stored pixels, a flip and a quarter-turn). It is written through mirror types, as `render.json` is (ADR-141), so the engine's types can change without changing the file.
- **No file is neutral with no reframe.** Every render made before format 3 has none, so an upgrade moves no render.
- **Every saved value names its process version** (`process`), the version of the math it is drawn under (`deptheos_adjust::ProcessVersion`). A missing or unknown version is damage. These rules follow lightcraft's `docs/process-versions.md` (lightcraft, MIT OR Apache-2.0):
  - Version 1 never changes once it ships. A change of look is a new version; a bug fix (NaN, black output, a crash) lands in place, but a fix that changes how a saved value looks, such as a slider's sign, is a new version.
  - A new version changes what saved values mean, so it bumps `SOURCES_SCHEMA` and an older build refuses the Library whole (ADR-141). A release adds at most one version.
  - A new render that inherits its base render's sliders takes that render's version with them, so it looks the same; a render that starts neutral takes the latest. Copy and Paste never carry the version: the target keeps its own. Reset of everything moves to the latest; reset of one slider leaves the version alone.
- **A damaged or unreadable file is not read as neutral.** The reader answers the adjustments, or `Unread::Damaged` (not this schema, a field missing, an unknown version, a crop outside the picture) or `Unread::Unreachable` (an I/O error), as paint's reader does (ADR-148). An export of such a render fails rather than come out unadjusted. A damaged file stays until the next write, which sets a file not of this schema aside (`fsutil::set_aside`) and replaces one that is.
- **Written on the store thread, one write per finished change** (ADR-143): a drag ended, a typed value, a crop, a reset, a paste, an undo; never per tick. Each write is numbered by the render's `Writes`; one that a later write overtook while queued is skipped (`Write::save`), as paint's snapshots are. Writes are atomic (ADR-141). Values that aren't finite numbers are written as their sliders' defaults.
- **The file moves with its folder**, to Recently Deleted and back (ADR-140).
- **Library format 3 is the file's format**, the only bump the adjustments make: every slider is in the file from version 1, so a slider added later (noise reduction) is another bump with its own upgrade. Formats 1 and 2 move to 3 on open by writing the number.

## Alternatives considered

### Fields in `render.json`

`render.json` is read by every listing and written once; an edit per finished drag would rewrite the manifest that a render is listed by, and a damaged write would lose the render, not its adjustments.

### A table in `paint.sqlite`

The adjustments would share paint's file and its lifecycle (open, roll back, spill), which knows nothing of them, and a render with no paint would gain a database to hold a few numbers.

### Unversioned values, drawn under the current math

Simplest, but a release that changes a law changes every saved picture, which a photographer can't undo.

## Consequences

- The iOS app's Library moves to format 3 with the first build that has this file, before iOS has Adjust, since the format number is shared code.
- An older build refuses a Library once this build has opened it.
- A process version is a Library format: changing a law is expensive by design.

## Code

- `crates/deptheos_app/src/library/adjustments.rs`, `crates/deptheos_app/src/library/history.rs`, `crates/deptheos_app/src/frames/sources/mod.rs`, `crates/deptheos_app/src/fsutil.rs`.
- `crates/deptheos_adjust/src/lib.rs`, `crates/deptheos_adjust/src/slider.rs`.
- `crates/deptheos_app/tests/integration/formats.rs`, `crates/deptheos_app/tests/fixtures/adjustments-v1`.
