# ADR-149: The viewer: one view reducer for both apps; a view bar of the render shown, a focus rail through its frames and direct lenses; the depth map is the frame map, the rail its key, and Contribution one light on two canvases

## Status

Accepted.

## Context

Judging a stack means moving through 30 to 150 frames to the one sharp at a spot, comparing it or another render with the result, and seeing where the result took each frame. A chip naming what the result is compared with, with a menu of file names and arrows either side, would control the whole view with Compare off and one pane with it on; stepping would be a press per frame or a menu that closes on every pick; and on a phone such a bar doesn't fit beside Stack. The depth map and Contribution are looked at often while judging a stack, so they can't be two taps away.

Each stacker could show a picture of its own: Depth's smoothed frame index, Pyramid's best sharpness at the finest level, Wavelet's merged gray target. Only the first says which frame each spot came from, which is what the apps and `docs/terms-and-usage.md` call the depth map; the other two name no frame, so the view would mean a different thing by method under one name. Every render keeps a 16-bit frame map (`frame_map.jxl`, ADR-140): the depth map itself for Depth, the frame each pixel's color came from for Wavelet, Pyramid's winner record.

A tint over the frame where the result came from it, and the depth map lit for a frame, are one fact (both read the frame map through the same ramp); drawn two ways they would need two looks and two sets of state, and a colored tint covers the pixels a person turns Contribution on to judge.

A rail of 66 frames in about 100 points gives each frame 1.5 points: ticks drawn off whole pixels look uneven, and a finger rolling as it lifts changes the frame by itself.

What the viewer shows, written once per app, differs in a dozen cases (Flicker's first press, what a comparison remembers, what a failed lit picture does), and a view that holds load status changes at every load that lands.

A strip of bars giving each frame's share of the in-focus detail (ADR-121) mostly draws the bell curve of any sweep, and invites users to exclude frames that are the only sharp ones for a small feature.

## Decision

### One view, one reducer

- **The view is one pure reducer in the engine**, `deptheos_app::view::View`, pulled on the UI thread: `on(event) -> Answer { actions, now }`. Its state is the `Mode` (the result alone; a frame alone with its overlay; a comparison with its layout, sides and target; the depth map with its `Back` and its `Light`), the comparison last shown and, while Retouch or Adjust is open, that place's own state. It holds no time, no pixels and no load status: ids only. The frame on screen is the app's selection; the view says when that changes (`Action::Show`).
- **Load status is beside the view, each app's** (the desktop's `Loads`, iOS's `LatestLoad`s), which read the view to know what is wanted. A load landing never changes the view, so a late one can't reopen a comparison left. The lit map's loads carry a token, and a failure applies only while the map still waits for that token.
- **Both apps drive it**: the desktop through `Viewer::on`, Retouch included; iOS through the bridge's `ViewControl`, kept as `viewState` and changed only by `view(_:)`. iOS's Retouch keeps its own `PaintView` (ADR-163).
- **The cases it decides are its table tests** (`view.rs`), the same on both apps. Three carry a reason worth stating:
  - **Over a frame that supplied nothing, Contribution stays chosen and shows nothing**, its toggle disabled; it comes back on the next frame that supplied part. Turning it off would lose a choice the user made for the frames that matter.
  - **A lit picture that fails puts the light out**: the map stays, whole, and its label and the rail no longer name the frame. Leaving the map would take the user somewhere they didn't ask for; staying marked would say what the picture doesn't show.
  - **A render traded for the one compared swaps the sides**, so neither picture moves under the eye.

### The view bar

- **Three parts, left to right: the home control, the focus rail, the lenses.** The desktop keeps zoom and Focus Mode after them; iOS ends with Stack (ADR-163). Each new control takes room from the rail, which has a least width on each app.
- **The home control is the render shown** ("#4 ★"; "Result" before the result is saved). Its body shows the result alone; its menu lists the stack's renders, never its frames. It is never lit: the render shown alone is the resting view, and the bar lights only a mode turned on.
- **The lenses, on the desktop, are direct buttons**: Compare as a split button (its body toggles comparing in the layout used last; its menu the layouts and Flicker), Contribution, Depth Map, and Swap while a render is compared. **On iOS they are one view menu**, whose button shows the symbol of what is on and is lit; its rows are toggles, so Flicker picked again flips. A toggle with a chevron beside it gave no sign of being on.
- **No frame picker in the bar.** The menu bar's View menu stays on both apps, with every view command and its key.
- **A pane's label names what the pane shows and nothing else**; the picture itself shows Contribution, dimmed where the result took nothing.
- **The bar keeps its parts by room, not by device.** The desktop drops Compare's word and then the readout as the window narrows (`app/chrome.rs`), and the rail flexes into what is left; iOS's bar is one row where the parts fit beside Stack and two tiers where they don't.

### The focus rail

- **One tick per frame of the render shown, in the stack's order.** A tick is as tall as the frame's share of the result, scaled to the largest; every tick is the same height when the render has no frame map. The frame on screen is marked; frames the stack left out are amber; in Retouch a dot is over each frame painted from.
- **On the desktop** a press shows the frame under it and a drag scrubs; the wheel and ← → step; Option-drag (Alt on Windows and Linux) moves a frame per few points at any density. A label names the tick under the pointer ("Frame 18 · IMG_0418.DNG · 14%"), and a readout says where the frame on screen is ("18/46").
- **On iOS** a touch scrubs from where it lands, so ticks narrower than a finger need no aim, with a haptic per frame and a loupe naming it. On lift the frame under the finger a moment before is kept, so a roll as the finger lifts doesn't change it. Held still where frames are narrow, the rail magnifies about the frame under the finger. For VoiceOver the rail is one adjustable element. The timings are in `Views/FocusRail.swift`.
- **What a frame scrubbed to does follows the view**: with the result alone it is shown alone; while comparing it is compared with; over the depth map it is lit; in Retouch it is painted from. A pick in the Frames list does what a press on the rail does.
- **Loads never queue.** Each app keeps only the newest asked for, and the picture on screen stays until the new one lands; a load overtaken but newer than the picture on screen is shown as it lands, so a drag shows the frames it passes.
- **The rail is drawn on the display's pixels, and nothing is merged away.** Each frame's start is snapped to a whole pixel; where frames are narrower than a few pixels, one column per pixel stands for every frame in it, as tall as its tallest frame (never a mean, which would flatten the one sharp frame). Left-out frames are an amber lane under the ticks, so none hides a sharp neighbor.
- **The rail shows how much each frame supplied, and says nothing about keeping it.** It has no include, exclude or trim, and no text near it calls a frame weak (ADR-121): a frame with a short tick can still be the only sharp one for a small feature, which Contribution on that frame shows.
- **The shares are counted from the frame map, not stored** (`CompletedStack::frame_contributions()`): each frame's blend weight summed over the map's pixels, exact for Depth and Wavelet, an estimate for Pyramid. A histogram of the map's 65,536 values takes about 3 ms at 45 MP, off the thread that shows it. It is `None` without a frame map, never a list of zeros.
- **While a stack runs, the rail shows, read-only, as the live picture's key.** A frame's tick takes its legend color once the stack has taken it in, so the filled ticks are pass 1's progress. A frame's color is fixed from the start, so the rail doesn't change color when the result lands.

### The depth map is the frame map

- **A finished render's depth map is its frame map, for every method** (`CompletedStack::present_map`), each value drawn in a legend color, plasma from the first frame accumulated to the last. It is made when shown; nothing is saved for it (ADR-140). A render without a frame map has no depth map.
- **Its half-size levels pick a pixel; they don't average.** Pyramid and Wavelet maps name one frame a pixel, and the mean of two frames would name a third that supplied nothing there. Where no frame is clearly the source the map is speckled, which is what the stack did.
- **The picture while a stack runs is the frame map as it forms, one look for every method** (`Stacker::render_live`): Pyramid its winner record, Wavelet's pass 1 the frame each finest coefficient's winner came from, Depth the sharpest frame so far and then the smoothed map. A finished stack keeps its result and frame map alone, so no stacker keeps a focus buffer for a picture (about 180 MB at 45 MP).
- **The rail is the map's key.** While the map is shown, each stacked frame's tick is drawn in its legend color; away from it the ticks are plain, since color beside a photo being judged would say nothing.
- **The map points back at the rail.** On the desktop the pointer over the map marks the frame under it, and a click lights it; on iOS a tap lights the frame the result took that point from, and a tap on the lit part shows the map whole.

### Contribution: one light on two canvases

- **One rule for both canvases.** What a frame supplied is shown as it is, and the rest goes gray and dim (`UNLIT`), in proportion where the result blended the frame with a neighbor: over the frame, the frame's own pixels; on the map, its legend color. No color is laid over a photo. Both read one share (`PlaneRamp::share`), so the two pictures agree pixel for pixel but for the map's half size.
- **The names.** The frame's canvas is Contribution (C); the map's is lit.
- **A frame scrubbed to or picked over the map is lit, and the map stays.** The lit frame is data of the map's state (`view::Light`). A frame left out lights nothing.
- **The light is kept across the two.** Depth Map turned off while a frame is lit shows that frame with Contribution on; turned on from a frame with Contribution on, the map opens lit for it. Only Depth Map itself carries the light.
- **Contribution over the frame is not in Retouch.** Dimming the source would hide the part painted from, and the lit map already answers where the frame went.

### Retouch

- **Retouch's map is lit for the frame painted from, and follows it.** The original or another render shows the map whole. A click or tap on the map that isn't a pan paints from the frame under it. Strokes aren't in the frame map: Retouch's lit map describes the render as stacked.
- **The desktop's Retouch bar is built as Stacking's**, with Paint From in the home control's place, joined to the rail so the two read as one control: the pop-up picks the kind of source, the rail which frame. Compare's layouts there are Painted, Split, Side by Side and Top and Bottom, with no Flicker or Swap, and Retouch's layout memory is its own. Retouch's layouts have no keys, since 1 to 0 and `\` set the brush's opacity and hold the source in view.
- **The desktop draws its Renders strip only outside Retouch and Focus Mode** (`AppView::strip_shown`): in Retouch a render is listed once, on the Renders side of Paint From, and a click on it always means paint from it. Outside Retouch the desktop keeps the strip, since a desktop window doesn't lack the height a phone does.

### Adjust and the reframe

- **Adjust is a place of the view, as Retouch is** (`view::Adjusting`): opened on the result shown, never over Retouch or with no result, and it shows the result alone, so a layout, a frame or the map picked there does nothing. It opens with the sliders and the settled picture each time; nothing of it is kept between opens.
- **A drag's preview is one flat picture over the settled one** (`Picture::Preview`), so the viewer builds no tile pyramid per tick; the settled picture replaces it as it lands (`Event::Settled`). The view says which shows; the pictures are each app's, from the Adjust session (ADR-153).
- **Holding a key or button shows the render without its adjustments** (`Hold::Before`) in the picture's place, at the same framing, and letting go shows whatever landed meanwhile.
- **Each view draws one pane of the render** (`view::Pane`, `view/framing.rs`): the result alone, reframed; a comparison's sides and Adjust's crop tool, adjusted and uncropped with the crop marked; Retouch's canvas, the stored picture with paint, uncropped, the crop marked; a frame and the depth map, the stored picture. Every pane is flipped and turned as the result is, at the hand-off; only the crop is the result's alone.
- **Pointers map back to stored pixels through the pane's `Framing`**, so Sharpest Frame, the frame map and paint stay in stored coordinates whatever the turn. Switching between a reframed and an uncropped pane keeps the stored point under the pointer at the same zoom (`Placement::keeping`), where the viewer would otherwise reset to fit as the picture's size changes.

## Alternatives considered

### Keep the chip, its menu and its arrows, or controls on each pane

The chip's scope changed with Compare, and a menu of frames that closes on every pick is the slowest part of judging a stack. Pickers on each pane leave stepping a menu or an arrow a frame, change the bar's shape when Compare turns on, and leave nowhere for a rail.

### A rail of even ticks, or shares stored in `render.json`

Even ticks avoid ADR-121's risk, but the heights tell a scrub where the subject is sharp before it starts, at a few milliseconds a render. Stored shares would be a field to version and keep in step with the frame map, and older renders would be counted anyway.

### For a rail with more frames than room: every second tick, ticks that scroll, a rail that widens

Every second tick hides frames. Scrolling the ticks loses the whole sweep at a glance, and a drag can't both scroll and scrub. A rail that widens as a scrub starts moves the frame under the finger.

### Turn Depth Map off for Pyramid and Wavelet, keep their sharpness pictures, or smooth their maps

Turning it off leaves two methods without a view of where the result came from when the data is already kept. A sharpness scale has no frames on it, so the rail couldn't be its key. A majority filter would calm the speckle, and show areas as one frame's that the result didn't take from one frame.

### Remove Contribution and keep only the lit map, or tint the frame in its legend color

The map is half size and shows colors, not the frame: whether a frame was sharp where the result used it can only be judged on its own pixels. A tint's dark end disappears over dark subjects and its bright end over bright ones.

### Show the lit map during a drag and the frame on release

A plain press would flash the map and then the photo, and a drag would no longer show the frames going past, which is what scrubbing is for. The map is one key away.

### Light the frame on the CPU, as the map is, or keep load status in the view

Lighting on the CPU would need the lit picture built into every display level and a copy of the aligned frame kept to light; the GPU pass is small. Load status, or the frame on screen, in the view would change it at every load and make the shared reducer know each app's loads and selection.

## Consequences

- Per-frame shares are on screen. A Pyramid render's ticks, map and Contribution are estimates, and the view doesn't say so. Wavelet's map is exact and noisy, so its Contribution over a dark subject is scattered. Where two frames hand off, both are half lit, which is where halos are.
- Every method's live picture is one flat color after the first frame, the subject appearing as later frames win.
- A reversed stack's ticks run from the last color to the first, since colors follow the order frames were accumulated in and the rail the order given.
- The website's depth map figure has its own palette. The command line's `--contributions` writes the same look.
- The apps differ in small ways where it was built, not decided: the desktop's ← and → step along the rail and iOS's the list; iOS's rail shows before a first render.

## Code

- Engine: `crates/deptheos_app/src/view.rs`, `crates/deptheos_app/src/view/framing.rs`, `crates/deptheos_app/src/runs/session/contributions.rs`, `crates/deptheos_app/src/runs/session/stack.rs`, `crates/deptheos_app/src/engine/present.rs`.
- Stackers and shaders: `crates/deptheos_core/src/map.rs`, `crates/deptheos_core/src/plane.rs`, `crates/deptheos_core/src/contribution.rs`, `crates/deptheos_core/src/wavelet/wavelet.wgsl`.
- Bridge: `crates/deptheos_ffi/src/view.rs`, `crates/deptheos_ffi/src/result.rs`, `crates/deptheos_ffi/src/picture.rs`.
- Desktop: `crates/deptheos_gpui/src/viewer/`, `crates/deptheos_gpui/src/app/view_bar.rs`, `crates/deptheos_gpui/src/app/scrubber.rs`, `crates/deptheos_gpui/src/app/chrome.rs`, `crates/deptheos_gpui/src/paint.rs`.
- iOS: `ios/DeptheosMobile/Views/ViewerDeck.swift`, `Views/FocusRail.swift`, `Views/ViewerView.swift`, `Model/AppModel+View.swift`, `Model/LatestLoad.swift`.
