# ADR-169: On iOS, a measured value never sizes or moves the view it was measured from

## Status

Accepted

## Context

The viewer floats its controls over a full-bleed photo and keeps them clear of the bars, a fold and the inner camera, which SwiftUI reports only through geometry (`onGeometryChange`, `GeometryProxy.reservedRegions`). When the controls were ordinary children of the container being measured, padding set from a reading could grow that container; its parent re-centered it, the reading moved, and the padding changed again. On an iPad Pro this never settled and the watchdog ended the app (an iPad Pro in landscape reports its camera as a strip down one side). Rounding and caps slow such a loop; they don't end it.

## Decision

- **Measure only a view whose size its parent alone decides,** and place what the reading moves in an `.overlay` of it, which is offered its size and can't change it. The viewer's backdrop and photo are that view (`ViewerView.swift`); its controls, Retouch's (`PaintOverlay`), the hold label and every pane's label are overlays. The camera's controls take exactly the screen (`CameraView.swift`).
- **A reserved region is a rectangle, not an inset.** `ReservedAreas.occludedInsets(within:)` gives each edge the occlusions lying along it (longer than deep against it) that start within a band of it, capped at the band; controls on that edge move in by it.
- **The photo's scroll view reads its own safe area** (`TiledZoomView.margins`, from `safeAreaInsets` and `safeAreaInsetsDidChange`), so a zoomed photo's insets follow the bars inside UIKit. SwiftUI sends it only what UIKit can't know: the bottom row's height, or a low sheet's depth (`Below`).
- **Readings are whole points** (`ReservedAreas.read`, `CameraView.Shape.rounded`), so a reading a fraction off isn't a change.

## Alternatives considered

### Keep measuring into children, damped by rounding and caps

That was the state that froze. Each fix closed one trigger; the next device shape or text size finds another.

### A custom `Layout`

It places children in one pass without state, but it can't read reserved regions, so the fold and camera would still be measured; the overlay already gives a measured container a fixed size.

## Consequences

- Controls that don't fit their room overflow it visibly rather than enlarging the viewer.
- A new reading must name a container its result can't resize; a reading inside the controls (the bottom row's height) may feed only the photo's UIKit insets and overlays.

## Code

- `ios/DeptheosMobile/Views/ViewerView.swift`, `PaintControls.swift`, `RetouchPanes.swift`, `ReservedAreas.swift`.
- `ios/DeptheosMobile/Views/TiledZoomView.swift`, `ZoomToFit.swift`, `PaintCanvas.swift`.
- `ios/DeptheosMobile/Camera/CameraView.swift`, `CameraLayout.swift`.
