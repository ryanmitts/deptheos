# ADR-172: Photos show in the screen's colors: iOS tags each picture with its space, and the desktop window follows its screen

## Status

Accepted. The engine's half, the screen as an output space and every display image naming its space, is ADR-135's.

## Context

Every picture the engine handed an app was 8-bit sRGB, so a wide-gamut screen showed photos clipped to sRGB. Apple's RAW decoder on iOS hands over Display P3 (ADR-132), so the most common iOS stack held colors its own screen could show and never did. Saturation and white balance are judged by eye in Adjust (ADR-137), and an export in Display P3, Adobe RGB or ProPhoto (ADR-133) held colors the screen had cut off.

Two rules bound the change. A screen that is exactly sRGB must show exactly what it did, bit for bit. And a picture must never shift color as it settles: Adjust's preview, its settled picture, Before and the compare panes must be in the same space.

The engine can draw a picture in Display P3 (ADR-135). How an app gets it onto the screen differs: iOS tags each image, and Core Animation converts it per screen; a GPUI window on macOS is one surface whose bytes are read in the window's one color space.

## Decision

### iOS

- **The engine is told the window's screen** (`ScreenGamutReader`, `ios/DeptheosMobile/Views/ScreenGamut.swift`): Display P3 when the trait collection's display gamut is P3, else sRGB, as the window first appears and whenever its gamut changes (an external display). It is a setting, not a request: pictures shown before keep their tags, which the system converts, and the next picture is drawn in the new space.
- **Every picture is tagged from its own space.** `DisplayImage` carries `space` across the bridge, and `DisplayImage.makeCGImage` and `AdjustPreview.makeCGImage` (`ios/DeptheosMobile/Engine/EngineBridge.swift`) tag the image with it, so the settled picture, the preview read in place (ADR-139), Before, the compare panes, tiles and Retouch's canvas are each tagged with what they are. Thumbnails and the live map are sRGB and say so.

### macOS

- **The window follows its screen** (`crates/deptheos_gpui/src/color.rs`): Display P3 when the main window's screen can show it (`canRepresentDisplayGamut:`), else sRGB, read as the window opens, moves, changes screen or comes to the front. That sets the engine's screen and tags every window's Metal layer with the space, which macOS converts per screen. It follows the screen rather than drawing P3 everywhere, for the round trip measured below.
- **The theme and brand colors are generated in both spaces** (`script/tokens.mjs`: `themes/deptheos-p3.json` and `brand.rs`'s `DISPLAY_P3`), so the chrome keeps its colors in a P3 window. True grays come out the same; the faintly blue ones move a code. A change of space reinstalls the theme.
- **Pictures are converted at hand-off, as each tile is painted** (`crates/deptheos_gpui/src/images.rs`): a tile in another space than the window's, such as a thumbnail, the depth map or a picture kept across a change of screen, is converted as it reaches GPUI's atlas, so a change of screen converts only what is seen. The rail's legend, Adjust's flat preview and its histogram's colors are converted too. In the same space nothing is converted.
- **Color Management off is unchanged:** windows untagged, the sRGB theme, the engine passing values through, nothing converted.
- **Windows and Linux stay sRGB.**

## Alternatives considered

### Draw in sRGB and let the system convert

That is what clipped the colors: they are gone before any conversion.

### Always Display P3

On an sRGB screen every picture would be quantized to 8-bit P3 and converted back. Measured over the sRGB cube, about half of all colors come back changed, by up to 8 codes in dark saturated colors, breaking the rule that an sRGB screen shows exactly what it did. iOS supports iPads whose screens are sRGB, and a Mac may drive an sRGB display.

### Extended-range or 16-bit pictures

Half-float pictures would hold any gamut and let the system convert, but double the memory of every display, tile and preview against the budgets of ADR-114 and the copy budget of ADR-139.

## Consequences

- On an sRGB screen nothing changes: the engine draws sRGB, which `just handoff-check` proves bit for bit.
- A picture shown on one screen and kept while the window moves to another is converted by the system, clipped if the new screen is narrower, until it is drawn again.
- On a Mac with a P3 screen and an sRGB one, a window moved between them converts what it shows as it next paints; thumbnails and the Library's pictures are converted at once, and disk caches stay sRGB.
- The Simulator's screenshots are always sRGB; the debug build's `-DumpShown <path>` writes the result's picture as handed to the screen, with its own color space.

## Code

- `ios/DeptheosMobile/Views/ScreenGamut.swift`, `ios/DeptheosMobile/Views/ContentView.swift`.
- `ios/DeptheosMobile/Engine/EngineBridge.swift`.
- `crates/deptheos_ffi/src/images.rs`.
- `ios/DeptheosMobile/Views/Automation.swift` (`-DumpShown`).
- `crates/deptheos_gpui/src/color.rs`, `crates/deptheos_gpui/src/images.rs`, `crates/deptheos_gpui/src/brand.rs`, `script/tokens.mjs`.
- `crates/deptheos_app/src/engine/present.rs`.
