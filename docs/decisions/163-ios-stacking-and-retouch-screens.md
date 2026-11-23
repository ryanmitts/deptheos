# ADR-163: iOS Stacking and Retouch: the photo full bleed under the system's bars, the sources behind one Frames | Renders switch, one sheet at a time that covers the photo, and Retouch's one deck with the source beside the canvas

## Status

Accepted. The deck along the photo, the focus rail, the view menu, the viewer's view (the engine's) and Retouch's source with its lit depth map are ADR-149's; the change line and Revert are ADR-142's; Copy Frames into Deptheos is ADR-145's; the paint session's scrubs and opens are ADR-148's.

## Context

The iOS Stacking screen shows one stack: its photo, its frames and renders, its Stack Settings, and Retouch over a render. It runs on four shapes: an iPhone, an iPad (in a window that may be narrower than the 1100 pt that shows both columns), and the iPhone Duo, whose outer display is short and has a vertical bar of side buttons, and whose inner display is a regular-width screen with a fold that can divide it when the device is partly open. The height under the photo is the scarcest room on a phone and a tablet, and every control has to fit the smallest shape without colliding.

Retouch paints a render back from one of its frames, from another render of the stack, or from the original. Choosing what to paint from, seeing whether it lines up with the painted result, and finding the frame that a part of the result came from are its main tasks.

## Decision

### The photo and the system's bars

- **The photo runs full bleed**: under the navigation bar and the deck on every shape, under the Duo's side bar, and across the screen at compact width; at regular width it stops at a column. The controls over it stay inside the safe area, and zoomed in, the photo can be dragged clear of them (`ZoomToFit`'s margins).
- **The bars are the system's**, fitted by visibility priority rather than width thresholds. The stack's name is the navigation title, with the native title menu; its subtitle names the render shown and its state ("#4 · Edited"). Nothing is drawn over the photo for a render: showing one changes only the picture (ADR-142). The app draws no ellipsis menu of its own.
- **Stacking is dark in its own views, not the window's**, so the Library under it stays as it is.
- **A tap on the photo hides every control over it** (`Panel.photoOnly`), with the status bar and the home indicator, as the system's photo viewer does; a second tap shows them. View ▸ Hide Controls does the same from a keyboard; VoiceOver keeps them shown.
- **iOS has no Focus Mode and no renders strip under the photo**, a deliberate difference from the desktop: the height under the photo is the scarcest room on screen, the stack's history is looked for with its frames, and tap to hide already gives the photo the whole screen.

### The sources

- **A stack's sources are one view with a Frames | Renders switch at its head** (`SourcesSidebar`), the same in Stacking's column at regular width, the iPhone's Frames sheet and Retouch's column. Renders lists the stack's renders as rows the column's width, with room for the method, its settings and the change count. In Retouch the same switch picks a frame or another render to paint from.
- **The Frames head says where the frames are** (`StorageBadge`): "On T7 SSD", "On iCloud Drive", or "T7 SSD Not Connected" with Locate…, and offers Copy Frames into Deptheos for frames used in place. Frames in the app or elsewhere on the device get no badge.

### Sheets and columns

- **At regular width** the sources are a split-view column and Stack Settings a popover anchored to its bar button, with Stack in its bar.
- **At compact width each bar button has its own sheet, and they are one presentation** (`Panel`: closed, photoOnly, sources with its tab, settings; each low or tall). A button toggles its own sheet; another button changes what the sheet holds at the same height, so nothing animates down and up again. New sheet content is a new `Panel` case.
- **The size class carries a sheet across**: turning regular makes an open sheet its column; turning compact makes the open column its sheet.
- **No sheet is presented over a sheet at compact width** (`Presented`, one at a time). Render Info and the questions after a pick in Files push onto the sheet; Export and the camera close it first. The system's own pickers and the share sheet still present over it.
- **An error is shown by the topmost presentation** (`errorAlert`, `Views/ErrorAlert.swift`): the window's root and each sheet's carry it, and only the one last presented shows it, so an error raised in a sheet is seen there, once.
- **A sheet covers the photo and the deck, and the photo never rescales for it.** At the low height the photo above the sheet stays in use, and a zoomed-in photo can be dragged clear of the sheet (`lowSheetDepth`, derived from `Panel`'s height each time). Notices sit just above a low sheet so one that comes then is seen.

### Stack

- **Stack has one place for each shape of screen**: the end of the deck (`StackCorner`, with its progress and Cancel while a stack runs), or the system's vertical bar on the Duo's outer display. Where a fold divides the viewer top to bottom, Stack is on the fold's trailing side.
- **The Stack Settings sheet has Stack in its bar**, in the confirmation place opposite Close, since the deck's Stack is under the sheet; it closes the sheet to show the run. What Stack stacks ("62 of 66 frames") is the sheet's subtitle.
- **The deck's home control in Stacking is never lit.** Being on the render shown is the default view, not a mode; lit glass in a deck means a mode that isn't the default.

### The fold

- **The fold and the inner camera are read as reserved regions** (`ReservedAreas`, iOS 27.1). The deck keeps to the fold's leading side and Stack to its trailing; compare layouts divide their panes at the fold; controls along the top move clear of the inner camera; the Library's grid has an even number of columns, so the fold never runs through a tile. Before iOS 27.1 the reserved regions read as none.
- **Folding and unfolding keeps the place**: Retouch's zoom, the Library's folded batches and scroll position, and any question asked are held above the size-class branch.

### Opening a stack

- **Opening a stack is one request from the tap until its picture lands** (`Opening`, ADR-143). The stack is switched before Stacking comes in, so nothing of the stack left shows, and the controls turn ready once, with the picture. Nothing is inserted or reflowed as the picture lands: the deck is laid out from the first frame wherever there is a render to land on.
- **Stacking fades in over the Library, on every device**, starting on the render's Library thumbnail when it is in memory, so the picture lands with no jump. Nothing is loaded for the stand-in.

### Retouch

- **Retouch has a deck in the place of Stacking's, the same on every shape** (`RetouchDeck`), each control in the slot its Stacking counterpart has: Paint From joined to the focus rail; the view menu; the eye, where Stacking's Flip is; and at the trailing end, Sharpest Frame, then Brush and Eraser as two segments of one capsule. It is one row where that fits and two tiers where not. Each control owns its glass, lit while on.
- **Paint From is always one menu**, joined to the rail's leading end so the two read as one control: the menu picks the kind of source (Frames, another render, Original, which is the Eraser), the rail which frame. Its label is always the source.
- **The focus rail runs through the render's frames** (ADR-149): a scrub paints from the frame it rests on.
- **The source can be seen beside the canvas** (`RetouchPhoto`, placed by one `PaneArrangement`): Painted, Split, Side by Side, and Top and Bottom, or in the canvas's place while the eye is on. The source is the aligned, exposure-matched picture the paint worker delivers, read-only. The two panes zoom and pan together, and painting is on the canvas only.
- **Depth Map covers the canvas** with the render's map, lit for the frame painted from (ADR-149); a touch there moves the view, never paints.
- **Sharpest Frame picks the frame the result took a point from.** The button turns the next tap into a pick that starts no stroke; a two-finger tap picks too. For a render with no frame map the button is off.
- **Keys and the Pencil follow the desktop's**: ← and → step the source, `[` `]` the brush's size, `{` `}` its hardness, `\` shows the source; a squeeze of Apple Pencil Pro set to show the source toggles it.
- **Paint mode is `PaintModel.Mode`** (closed, open), and inside it `PaintModel.Open`, changed only by `PaintModel`'s methods. What the brush paints from, the scrub's pace and an open's failure are the shared `PaintSession`'s (ADR-148), read as its `PaintStatus`.
- **The canvas opens on the render's picture** already on screen, stretched to full size; strokes don't start until it is ready. A failed open shows a card over the canvas with the engine's reason, Retry and Done, not an alert, which a sheet being dismissed can keep from showing.
- **Entering Retouch clears the view under it to the result alone**, without starting a load; leaving it shows the result alone, as the desktop does.

## Alternatives considered

### The photo in a box between the bars

Simpler to lay out, but on the Duo's outer display and an upright iPhone it left the photo a third of the screen. Full bleed is what the system's own photo apps do.

### The desktop's Renders strip and Focus Mode on iPad, or a tab bar on the iPhone

The apps would be alike, at the cost of a fixed band under the photo where height matters most, and of a mode that duplicates tap to hide. A tab bar takes a row on every screen for a choice the bar's buttons make.

### Other sheet arrangements

One sheet with a switch of sides would need a second switch for Frames | Renders; one button for both sheets takes two taps where one does; two sheets that replace each other animate down and up, and at the tall height cover the bar; sheets over the Frames sheet stack two Close buttons and cover the photo and the bar on the Duo's outer display.

### Refit the photo above a low sheet

It would keep the photo whole above the sheet, but the photo would jump on every open and close and shrink a landscape photo to a strip. A photo half under a low sheet can be zoomed and dragged clear instead.

### Stack as the last row of the Stack Settings form, or not in the sheet

The last row is out of sight until the form is scrolled to its end; without it, stacking costs a tap to close the sheet after every change of settings.

### Stacking moving in over the Library

On the Duo the system stands Stacking's bar on its side, and that bar flashes through a move transition; a fade does not.

### A row of Retouch controls per shape

Two sets of controls for one task, a menu of sources on one shape and arrows on another, neither able to show the source beside the canvas.

### The source only in the canvas's place, or Sharpest Frame only as a two-finger tap

The source in the canvas's place answers whether it is sharp at one spot, not whether it lines up with the painted result around it, which a pane beside it shows while the brush is in hand. A two-finger tap alone leaves nothing on screen to say the pick exists, and a pinch that barely moves would pick.

## Consequences

- Anything new over the photo keeps inside the safe area and adds its depth to the viewer's margins. Anything that must be seen with a low sheet open places itself against the sheet's depth, as the notices do.
- Anything placed across the viewer or the Library reads `ReservedAreas`; anything that would present over a compact sheet pushes onto it instead.
- iOS and the desktop differ by design in the renders' place and Focus Mode; iOS Retouch adds Top and Bottom, Sharpest Frame as a button and a two-finger tap, and the Pencil's squeeze.
- A new control in Retouch goes in the one deck, in the slot of its Stacking counterpart; a new layout is a `PaintLayout` case placed by `PaneArrangement`.

## Code

- Stacking: `ios/DeptheosMobile/Views/StackingView.swift`, `Views/StackingColumns.swift`, `Views/ViewerView.swift`, `Views/ViewerDeck.swift`, `Views/ReservedAreas.swift`, `Views/ContentView.swift`.
- Sources: `ios/DeptheosMobile/Views/SourcesSidebar.swift`, `Views/RendersList.swift`, `Views/FramesList.swift`, `Views/InspectorView.swift`, `Model/FileSource.swift`.
- State: `ios/DeptheosMobile/Model/Panel.swift`, `Model/AppModel.swift`, `Model/AppModel+Opening.swift`, `Model/PaintModel.swift`, `Model/AppModel+Retouch.swift`.
- Retouch: `ios/DeptheosMobile/Views/RetouchDeck.swift`, `Views/RetouchPanes.swift`, `Views/PaintControls.swift`, `Views/PaintCanvas.swift`; the bridge in `crates/deptheos_ffi/src/paint.rs`.
