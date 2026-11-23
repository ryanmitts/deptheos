# ADR-162: The iOS camera shoots stacks itself: focus-locked bursts by sweep or bracket, laid out from the screen's shape, with focus peaking of its own

## Status

Accepted. A stack of Bayer RAW bursts is decoded as ADR-131 says.

## Context

A focus bracket needs focus to step through the subject while exposure, white balance, framing, orientation and the physical lens stay fixed. Ordinary camera apps vary exposure and white balance between shots, or switch between the wide and ultra-wide lenses mid-burst.

The camera runs on an iPhone upright and on its side, an iPad, and the iPhone Duo: its outer display on its side, its near-square inner display, and partly open, with the fold across the screen. The Duo's back camera can face the user, folded or open, as a front camera does, and a camera facing the user is expected to show a mirror image.

The photographer needs to see where the focal plane is while shooting. Focus peaking marks in-focus detail on the live preview; it must stay quiet where nothing is in focus, and line up with whatever the preview shows.

## Decision

### Bursts

- **Two modes.** **Sweep** locks focus at closest focus while the shutter is held, and the photographer moves the phone smoothly toward the subject. **Bracket** steps the lens itself, waiting for it to settle before each shot.
- **What a burst locks.** Exposure, white balance and the first frame's orientation hold for the whole burst. The controller binds to one physical back camera (ultra-wide, wide or telephoto), never the virtual multi-camera device, so iOS never switches lenses mid-burst.
- **The lens.** It is the automatic choice unless the photographer chooses: the ultra-wide where it has adjustable focus and focuses closer than the wide, else the wide (non-Pro iPhones' ultra-wide is fixed-focus). On a camera with more than one, the status pill is the lens menu; each lens is labeled by its magnification against the wide's, read from the device.
- **A bracket runs from a first frame's focus, by a step, for a number of frames** (5, 10, 15 or 20; the step 0.0025 to 0.05 of the lens's travel, 0.02 to start; the first focus 0 to 0.9). In Bracket the lens rests at the first frame's focus (`setRestLens`), so the picture composed on is the first frame's; while the step's slider is dragged the lens shows the last frame's focus.
- **A bracket can wait** after the shutter, Off, 2 or 5 seconds (2 to start), for the shake of the touch to die, and gives no haptic at its first frame. **The shutter cancels** a bracket counting down or under way (`cancelBurst`); its frames are deleted.
- **A dropped bracket step is taken again**, twice at most (`maxRetakes`), and the lens moves only once the step's photo has finished, so the files stay numbered near to far (ADR-146).
- **Zero shutter lag is off for brackets and on for sweeps**: it can hand back a frame from just before the request, which in a bracket may be before the lens settled. The log checks this by comparing both times on the session's clock.
- **Exposure is the photographer's to place.** A tap on the picture meters there (`meter(at:)`); an Exposure menu sets compensation, which holds through the burst's lock; the shutter time and ISO are shown, in yellow for a sweep slower than 1/30 s. A burst waits up to half a second for exposure to settle before locking it (`BurstState.metering`). The torch has a brightness (25, 50 or 100%), is off each time the camera opens, and stays as it is through a burst.
- **Formats.** HEIC at 12 or 48 MP, or 12 MP Bayer RAW as a DNG: the sensor's linear data with none of Apple's per-frame tone mapping, noise reduction or sharpening. Bayer RAW is the format unless chosen otherwise, where the camera has it.
- **Pace.** At most 3 photos in flight (`maxInFlight`): asking faster than the pipeline finishes backs it up until it refuses every request (`AVCapturePhotoOutput` error -11830). A sweep is capped at 5, 10 or 15 fps (10 to start) or as fast as the camera can, and is at most 120 frames (`maxFrames`). Under serious system pressure (heat, a battery that can't give the power) the video feed, and so an unlimited sweep, is held to 20 fps, as the camera's own guidance advises.
- **Room.** A burst is not begun without room for it (a bracket's frames, a few seconds of a sweep), and one that leaves less than `spaceReserve` (500 MB) free is cut short with room to stack it. Five dropped frames in a row cut a burst short (`maxFailedInRow`).
- **How a burst ends** (`BurstOutcome`). A completed burst saves into `Documents/Captures` (visible in Files) and opens as a new stack, which waits for Stack; one with fewer than 2 frames is deleted, with an alert, and not made a stack. A bracket without all its steps, or a burst cut short (by an interruption, a step given up on or the limits above) with 2 frames or more, ends `partial`, and the camera asks, "Bracket Cut Short" or "Sweep Cut Short": Stack Them, or Discard. A sweep let go before its second frame is deleted without an alert; the hint says to hold. A call, another app taking the camera, Split View, heat or the app sent away pauses the session (`CameraController+Interruptions.swift`), and a running burst ends there rather than wait for photos that never come: it waits 2 seconds for its photos in flight (`drainTimeout`), and a burst that ended as it should waits 10 (`doneDrainTimeout`).
- **A failed burst leaves the camera open.** A full device, a frame not saved or no room is an alert over the camera, which can shoot again; only a camera that can't start is the unavailable screen, with its session stopped.
- **Feedback.** A sweep's picture does not blink at each frame (up to 15 a second); its count on the shutter climbs. A bracket's picture blinks at each.
- **A burst taken while the app is busy** with a stack or a batch is left in Captures, with a notice to add it from Files. Captures is not among Settings' unused copies.
- **Two lifecycles on the camera queue**: `SessionState` and `BurstState` (with `Ending`, changed through `withBurst`) in `CameraController.swift`; the screen's is `Phase` in `CameraView.swift`, changed only by `setPhase`.

### Layout

- **The layout is worked out from the screen each time it changes, and never stored** (`CameraView.ScreenLayout` in `Camera/CameraLayout.swift`, from the measured size, the safe area, the fold through iOS 27.1's reserved regions, and the picture's measured shape). Each change of case is logged.
- **The picture's shape is measured.** The preview reports the picture's width over its height as it shows it (`PreviewView.placeOverlay`, from the rectangle the peaking overlay is placed by) and logs it with the angle it is turned by; until it has, the picture's long side is taken to run along the screen's.
- **The picture is fitted first, and the controls take the room it leaves:**

  ```text
  overlaid  the viewfinder across the screen, the bars over it: centered on the whole
            screen where the picture ends above the shutter (an iPhone upright); on any
            other phone-shaped screen held upright (the Duo's outer display, a short
            iPhone), at the top of the safe area at the picture's height
  below     the viewfinder at the top with the top bar over it, the controls under it
            (an iPad upright)
  beside    the viewfinder at the leading edge, every control in the column the picture
            leaves at the trailing edge (the Duo's inner display, an iPad or an iPhone
            on its side); on a short screen the shutter goes beside the mode switch
  book      the viewfinder on the fold's leading side, the controls on its trailing side
  table     the viewfinder above a fold across the screen, the controls on the half below
  ```

  Of `below` and `beside`, the one that gives the bigger picture wins. The column is what the picture leaves, never less than a least width. `below` keeps one height for the controls, so the picture is the same size in Sweep and Bracket.
- **The fold.** Partly open, the fold always makes a book or a table. Flat, it does only where the picture on one side has at least four fifths of the area of the biggest picture the screen could hold (`creaseShare`), so folding partly moves nothing; a picture that would lose more crosses the crease. Where no flat fold is reported, the inner display gets `beside` or `below`.
- **Every layout has the same controls in the same order**; only their place changes. The one preview is placed over the viewfinder's measured place, so a change of layout moves it rather than building it again.
- **The shutter's touch is watched from the screen**, not from the shutter's place in a layout: a turn of the phone mid-sweep makes the shutter anew, and the touch that ends with it lets go, ending the sweep.
- **When the back camera faces the user, the preview and the peaking feed mirror together** (`AVCaptureDeviceDirectionCoordinator`, iOS 27.1, in `CameraPreview`, `watchDirection`; `setPreviewMirrored` for the feed), so the picture reads as a front camera's and the peaking stays on it. **The photos are never mirrored**: a burst's frames are what the lens saw. Before iOS 27.1, and until the coordinator first reports, nothing is mirrored.

### Focus peaking

- **Focus peaking is in the iOS camera only.** The desktop has none; after a stack it has the contribution overlay (ADR-121).
- **It has its own measure, separate from the stacker's** (`Camera/FocusPeaking.swift`, `FocusPeakingShader.swift`, `Peaking.swift`), on Metal Performance Shaders and Metal kernels of its own, with no CPU readback:
  - a guided filter denoises the viewfinder frame;
  - each pixel's one-pixel step is compared against its slope over several pixels (a coarse step of 4);
  - a pixel enters one of three tiers (cyan, yellow, red) only when its score clears both an absolute floor, which keeps a wholly defocused frame quiet, and a threshold from the frame's own mean and spread, which adapts to iPhones whose video feeds sharpen differently;
  - a band of at least 8 px at the frame's edge (`borderMargin`) is never marked and is left out of the statistics, since reads past the edge are clamped.

## Alternatives considered

### Apple ProRAW

ProRAW applies multi-frame demosaicing and local tone mapping per shot, which can vary across a bracket. Bayer RAW gives the stacker linear sensor measurements in which only focus changes.

### The bars over a whole-screen viewfinder on every shape

On the Duo's inner display and an iPad the picture runs under the shutter and the mode switch, and partly open, the fold crosses it.

### The viewfinder fitted between the bars on every shape

On an upright iPhone it made the viewfinder smaller, and not the same size in Sweep and Bracket.

### Fill the screen and mark the frame with a crop guide

On the inner display an upright picture filled across the width shows about half its height, and a stack is framed on the whole picture.

### A squarer capture

It fills more of the inner display and throws away a quarter of the sensor.

### The picture centered, the controls split to both sides

It straddles the crease when flat, jumps sideways when the device is partly opened, and the bracket's step control fits neither side.

### Mirror the photos too

The frames would be mirror images of the subject: lettering and asymmetric parts backwards in the result and its exports.

### Mirror the preview only

Peaking is drawn from its own feed, so it would land on the opposite side from the edges it marks.

### The stacker's focus metric for peaking

It is tuned for winner-take-all selection across frames, with wide pooling (32 times wider in Depth, ADR-120), not crisp edge marks on one viewfinder frame.

### A Sobel edge outline with a percentile threshold

It marks high-contrast out-of-focus edges when something is in focus, and draws sensor noise when nothing is.

## Consequences

- A Bayer RAW frame is about 24 MB, always 12 MP, and its stack exports as DNG.
- A new camera control goes in every layout, in the same order.
- Defocused viewfinder frames stay quiet, since every tier must clear the absolute floor.

## Code

- `ios/DeptheosMobile/Camera/CameraController.swift`, `CameraController+Burst.swift`, `CameraController+Lenses.swift`, `CameraController+Exposure.swift`, `CameraController+Setup.swift`, `CameraController+Interruptions.swift`.
- `ios/DeptheosMobile/Camera/CameraView.swift`, `CameraBars.swift`, `CameraShutter.swift`, `CameraLayout.swift`, `CameraPreview.swift`.
- `ios/DeptheosMobile/Camera/FocusPeaking.swift`, `FocusPeakingShader.swift`, `Peaking.swift`.
- `ios/DeptheosMobile/Model/AppModel+Capture.swift`.
