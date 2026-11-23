# ADR-164: iOS shows the full image through one tiled view, holds only what is on screen, sheds under pressure, and installs only on the App Store's performance tier

## Status

Accepted. The engine's side of the memory policy (the budgets, the refusal before a stack that doesn't fit) is ADR-111's.

## Context

Checking a stack's sharpness at 100% is what a focus-stacking tool is for. A flat picture at most 4096 px on its long side (`AppModel.displaySide`) shows a 45 MP result at under half its width and a 100 MP one at under a third. Every picture the viewer shows (the result, the depth map, the lit map, a frame or a render compared) is made by the engine as a tiled pyramid (`PresentedImage`, 510 px cores with a 1 px gutter, `crates/deptheos_app/src/engine/present.rs`), and Retouch's canvas draws such a pyramid by zoom at full size.

On iOS the app's CPU and GPU memory are one pool, and the system ends an app that takes too much. Stacking holds several full-size frames and the GPU's working set at once, so the app is made for the devices with the most memory.

`UIRequiredDeviceCapabilities` is how an app tells the App Store which devices it is for. It has no key for memory. The App Store lets a later version keep or relax the list, never add to it, so a requirement has to be in the first version submitted or not at all. App Review tests on whatever device it has, and a stack refused, or a process ended for memory, on its device reads as an app that doesn't work.

## Decision

### One picture, one tiled view

- **One engine object for a picture that can be shown: `Picture`** (`crates/deptheos_ffi/src/picture.rs`), holding the `PresentedImage`'s `Arc`: its size, its levels (`PictureLevel`), a flat `base(max_side)`, and its tiles with their gutter, copied on the pool's interactive lane for a request (`Picture::tiles`). Every call that gives the viewer a picture returns a `Picture`. A frame alone gets one from the engine's own decode (`Engine::frame_picture`), RAW developed as the desktop shows it.
- **One tiled view in the app, `TiledPictureView`**, Retouch's tile canvas (ADR-148) made the app's one: sized one point per full-size pixel, the base stretched under it, the tiles in view above it. It reads a `TileSource`, which `Picture` and Retouch's paint session both provide. What it draws is one enum (`TiledDrawing`: `empty`, `base`, `tiling` a level over it), changed only by its `redraw` and logged at debug.
- **The base first, detail only where needed.** At fit and down to the base's own 1:1 the base alone shows; only finer levels are tiled, fetched off the main thread, newest first, each applied only if its picture and level are still the ones wanted. A tile not yet in shows the coarser picture under it, so nothing goes blank.
- **Each thing shown carries its own `Picture` beside its flat image**, in the state that holds it: `ShownResult`, `CompareImage` (loaded as a `CompareLoad`), `LitMap` (through `MapLighter`), `FramePreview` and `FrameFull`, each loaded through a `LatestLoad` (`Model/LatestLoad.swift`), so a pane never guesses which picture is its own and a load overtaken shows as its own picture. A retouched render's is the paint worker's own pyramid, handed over with its flat picture (`PaintObserver::presented`, `source_shown`); nothing more is computed.
- **Every pane draws `Picture`s**: the result, the depth map and its light, a frame alone, a frame or render compared in every layout, the overlay of Split and Flicker as a second tiled view under the same mask, and Retouch's source pane. Zoom is held as fractions of the image (`ViewerZoom`, `ZoomSync`), so synced panes and kept zooms carry over.
- **Zoom limits** (`Views/ZoomToFit.swift`). A double tap goes from fit to 100% (one image pixel per screen pixel) at the point tapped, and back; from under fit it goes back to fit. A pinch goes in to 800% or 16 times fit, whichever is further, and out to a quarter of fit, centered. 100% counts the screen's physical pixels (`nativeScale`).
- **A frame alone keeps the file's fast preview while the rail is dragged**, and loads its `Picture` once it settles (`FrameFull`, changed only by `setFullFrame`), on a load of its own so the next preview never waits behind a full decode. It is decoded as stacking on iOS decodes it (the host decoder for HEIC, JPEG and camera RAW, the DNG SDK for DNG), so its colors at 100% are the stack's. Only the frame selected has one.
- **Flat pictures stay where they serve.** The 4096 px base is kept, and what uses it keeps it: stand-ins (a Library thumbnail while a stack opens, Retouch's canvas before its tiles), the Library's thumbnails and the Opening's landing. The live picture while a stack runs is flat, and so is a frame that couldn't be lined up for a compare, which shows the file's preview.
- **The pyramid's shape.** The engine's pyramid has at most four levels and stops at about 1024 px on its long side, so above the 4096 px base there are one or two levels to tile. The depth map's finest level is half size; a level is placed by the picture's size over the level's, as each halving rounds up. Retouch's tiles come off the main thread a few at a time, so a stroke's tiles land a moment after the stroke.

### Memory

- **Only what is on screen is held**: the result, one compare side (and the one it replaces until its replacement lands), one lit map, one frame alone. A `Picture` lives as long as a view holds it; `release_displays` drops only the stack's own references. Saving a render takes its thumbnail, then lets go of the map's levels before the encode; the display, shown right after, stays.
- **In the background every full-size picture is let go of and the bases stay**; back in front, what is on screen is made again (the result's and the map's from the stack's pixels, the rest through their own loads).
- **Memory pressure is heard off the main thread** by a dispatch source (`Model/MemoryPressure.swift`; `ios/build-xcframework.sh` allows a global queue only there, in `Blocking` and in the bookmark resolver), which sheds the engine at once (`shed_memory`, over `deptheos_app::engine::memory::shed`) and then the app on the main actor (`AppModel.shed`). A moderate shed (a warning, or going to the background) trims the thumbnails off screen; a critical one, or any shed while a stack runs, also lets go of the full-size pictures not shown (`fitPictures`), made again when wanted.
- **Every shed and state change logs the footprint and what is left** (`memory_reading`, from `phys_footprint` and `os_proc_available_memory`), so a run on a small device at 45 and 100 MP can be read from the log it leaves.

What a picture holds, computed: 8-bit BGRA, the full pyramid about 4/3 of its full level (the map, half size, about a quarter of that):

| Stack | Result pyramid | Map pyramid | A compared frame | The stack's own pixels |
|---|---|---|---|---|
| 24 MP | 128 MB | 32 MB | 128 MB | 144 MB |
| 45 MP | 240 MB | 60 MB | 240 MB | 270 MB |
| 61 MP | 325 MB | 81 MB | 325 MB | 366 MB |
| 100 MP | 533 MB | 133 MB | 533 MB | 600 MB |

### The device floor

- **`UIRequiredDeviceCapabilities` is `arm64` and `iphone-performance-gaming-tier`** (`ios/DeptheosMobile-Info.plist`). Apple defines the tier as the graphics performance of an iPhone 15 Pro or better, which in practice is the iPhones with 8 GB or more and the iPads with an M-series or A17 Pro chip.
- **The refusal stays.** The tier is the floor for installing; a stack too big for the memory the device gives is still refused before it starts, with its sentence (ADR-111).
- **The website names the tier's devices**, not a narrower list, and never says the app installs on any iPhone.

## Alternatives considered

### A bigger flat picture

Raising `displaySide` to the image's size is one line. It decodes and uploads the whole image for every pane, every swap and every scrub, at 4 bytes a pixel and again as a texture, and Split or Flicker holds two: on a 100 MP stack, 800 MB before anything else. It also slows the open that the 4096 px picture keeps quick.

### `CATiledLayer`

The system's tiled layer fetches tiles by zoom on its own threads. It fades tiles in, draws blank until each lands, asks for levels by powers of two of its own choosing rather than the pyramid's, and gives no say over which request is current, which the app's rule for async results needs (CLAUDE.md). Retouch's canvas already did what it would, with none of that.

### A detail patch over the visible part

Render the visible rectangle at full resolution once a zoom or pan settles. The patch is wrong the moment the view moves, so panning at 100% shows the soft picture and sharpens late on every stop, which is the moment the user is judging; two panes in Split need two patches kept in step.

### The full-size level made on demand

Hold only the coarser levels and render level-0 tiles into a cache, as Retouch's `FullTiles` does. It saves the most memory and is the most to get right: the result rendered per tile for RAW, a compared frame from its warped pixels.

### No install requirement, the refusal alone

It lets someone with an older iPhone stack a small bracket from its own camera, but puts the app on 4 GB and 6 GB devices where a camera burst at full size may not fit, in front of buyers and reviewers alike, and can't be taken back later: the list can only be relaxed.

### Starting stricter

No key stricter than this tier follows memory, and none names a model year. Starting here and relaxing later, if smaller devices prove to stack well, is the only order the App Store allows.

## Consequences

- A 45 MP result compared with a frame holds about 0.8 GB of pictures beside the stack itself, on a device of 8 GB at the least (an iPhone 15 Pro).
- Retouch and the viewer share one tile view, so a fix to either is a fix to both.
- A requirement can't be added to `UIRequiredDeviceCapabilities` after the first submission; only relaxed.

## Code

- `crates/deptheos_ffi/src/picture.rs`, `crates/deptheos_ffi/src/paint.rs`, `crates/deptheos_ffi/src/settings.rs`, `crates/deptheos_app/src/engine/present.rs`, `crates/deptheos_app/src/engine/memory.rs`.
- `ios/DeptheosMobile/Views/TiledPictureView.swift`, `Views/TiledZoomView.swift`, `Views/PaintCanvas.swift`, `Views/ZoomToFit.swift`, `Model/ViewTypes.swift`, `Model/AppModel+Pictures.swift`, `Model/LatestLoad.swift`, `Model/MemoryPressure.swift`, `Model/AppModel+Lifecycle.swift`.
- `ios/DeptheosMobile-Info.plist`.
