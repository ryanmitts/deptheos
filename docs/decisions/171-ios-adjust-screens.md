# ADR-171: iOS Adjust is a mode beside Retouch, over Stacking, with the sliders under the photo or in a column, and the crop drawn as a window

## Status

Accepted. The session, its machine and its budgets are ADR-153's and ADR-138's; the preview read in place is ADR-139's; the viewer's view and panes are ADR-149's; Stacking's screens, its sheets and Retouch are ADR-163's; the crop tool's geometry is ADR-136's.

## Context

Adjust on iOS shows the engine's session (ADR-153): sliders, a crop with its turn and flip, a drag's preview over the settled picture, Before held, the histogram and the eyedropper. It runs on an iPhone, where the height under the photo is the scarcest room, and on an iPad, where a window can change width while Adjust is open. Retouch already takes over Stacking the same way, and the two must never be open together.

The engine hands every picture over turned and flipped; the crop is left to the app (ADR-149). Before this, iOS drew no crop at all.

## Decision

- **One mode for what is done to the render shown** (`RenderMode` in `ios/DeptheosMobile/Model/AppModel+Mode.swift`): viewing, retouching or adjusting, each case holding its own data. It is changed only by `AppModel.setMode`, and inside a case only through `whileRetouching` and `whileAdjusting`, called by `PaintModel` and `AdjustModel`. `Panel` stays as it is: the mode is not a sheet.
- **Adjust is a screen over Stacking**, as Retouch is (`AdjustSessionView`): the photo full bleed, Undo and Redo, a More menu (Zoom to Fit, Export, Copy and Paste Adjustments, Reset All) and Done in the system's bars.
- **The deck follows the width.** At compact width it floats under the photo and shows one heading's sliders, picked from a row of headings. At regular width it is a column beside the photo with every heading. The heading picked and the tool live above both layouts (the tool in the engine's view), so a size-class change keeps them.
- **The model only shows and sends.** `AdjustModel` holds what the session last said and sends each change; the sliders' positions, values and words, the crop's handles and drags (`CropFrame`, `crates/deptheos_ffi/src/crop.rs`) and the turns and flips (`reframe_turned`, `reframe_flipped`) are the engine's. No slider or crop rule is in Swift.
- **A slider commits when let go.** Its thumb follows the finger while dragging, not the session's status; a double tap on its name resets it. VoiceOver reads its value as the engine words it and steps it one step at a time, each step a change.
- **The crop is a window of the picture.** The zoom view zooms a clipping container (`TiledZoomView.windowView`) holding the picture, offset so only the window shows. The window is the crop where the view's pane is the result, so the viewer and Adjust both show a cropped render cropped; in the crop tool and Before the whole picture shows. Every conversion stays in the picture's pixels, and a pointer maps to stored pixels through `PaneFraming`.
- **The crop tool is drawn on the photo** (`CropMarkView`): dimmed outside, with corner and side handles. A press on a handle drags it, anywhere else pans the photo; a pointer shows a cursor over a handle. Turn and flip are buttons, with ⌘[ and ⌘] as on the desktop.
- **Before is the render as stored with its paint**, drawn by the engine (`AdjustSession.before`) as Adjust opens and on each turn or flip, through a `LatestLoad`.
- **The keys are the desktop's**: A opens and closes Adjust, ⇧⌘C and ⇧⌘V copy and paste adjustments, ⌘Z and ⇧⌘Z undo and redo, W the eyedropper, Esc leaves.
- **Leaving Adjust re-reads the render** when its reframe changed or the screen isn't showing its saved value (the session's `redraw`), so its display and map come under the reframe kept.

## Alternatives considered

### Adjust as a sheet

A sheet of sliders over the viewer would reuse `Panel`. It covers the photo the sliders change, and on an iPad it would be a second sheet beside the sources; a mode keeps the photo whole and Adjust from ever being open with Retouch.

### The crop drawn by the engine

The engine could hand over the cropped picture. Every crop drag would then draw the whole picture again, and the crop tool would need the uncropped one anyway; a window costs nothing to move.

## Consequences

- A turned render's display and map are laid out at their turned size.
- Retouch still paints in stored pixels over a picture shown turned: on a turned render its canvas is the stored picture.
- The compare panes show the result uncropped.

## Code

- `ios/DeptheosMobile/Model/AppModel+Mode.swift`, `Model/AppModel.swift` (`setMode`).
- `ios/DeptheosMobile/Model/AdjustModel.swift`, `Model/AppModel+Adjust.swift`.
- `ios/DeptheosMobile/Views/AdjustSessionView.swift`, `Views/AdjustCropDeck.swift`, `Views/CropOverlay.swift`, `Views/TiledZoomView.swift`.
- `crates/deptheos_ffi/src/crop.rs`, `crates/deptheos_ffi/src/adjust.rs` (`before`), `crates/deptheos_app/src/adjust/session.rs` (`before`).
