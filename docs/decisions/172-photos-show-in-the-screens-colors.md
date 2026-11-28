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

## Alternatives considered

### Draw in sRGB and let the system convert

That is what clipped the colors: they are gone before any conversion.

### Always Display P3

On an sRGB screen every picture would be quantized to 8-bit P3 and converted back. Measured over the sRGB cube, about half of all colors come back changed, by up to 8 codes in dark saturated colors, breaking the rule that an sRGB screen shows exactly what it did. iOS supports iPads whose screens are sRGB.

### Extended-range or 16-bit pictures

Half-float pictures would hold any gamut and let the system convert, but double the memory of every display, tile and preview against the budgets of ADR-114 and the copy budget of ADR-139.

## Consequences

- On an sRGB screen nothing changes: the engine draws sRGB, which `just handoff-check` proves bit for bit.
- A picture shown on one screen and kept while the window moves to another is converted by the system, clipped if the new screen is narrower, until it is drawn again.
- The Simulator's screenshots are always sRGB; the debug build's `-DumpShown <path>` writes the result's picture as handed to the screen, with its own color space.

## Code

- `ios/DeptheosMobile/Views/ScreenGamut.swift`, `ios/DeptheosMobile/Views/ContentView.swift`.
- `ios/DeptheosMobile/Engine/EngineBridge.swift`.
- `crates/deptheos_ffi/src/images.rs`.
- `ios/DeptheosMobile/Views/Automation.swift` (`-DumpShown`).
