# ADR-167: stackrig drives the camera with raw PTP through ImageCaptureCore and the turntable over BLE, in a dependency-free Swift tool

## Status

Accepted

## Context

Shooting a multi-angle focus-stacked turntable sequence (ADR-147, for example 50–100 angles × 200 frames per angle) requires an automated capture loop: drive the camera lens to its near limit, trigger an in-camera focus bracket, wait for all frames to write, and advance the Bluetooth turntable to the next angle. `tools/stackrig` is the standalone macOS developer utility that drives a Canon EOS R5 Mark II over USB and a Revopoint dual-axis turntable over Bluetooth LE.

## Decision

`tools/stackrig` is a dependency-free macOS Swift package (`Sources/stackrig/Camera.swift`, `Turntable.swift`, `main.swift`):

- **Camera control via raw PTP over `ImageCaptureCore`:** Sends standard PTP opcodes and Canon vendor property codes through macOS's built-in `ImageCaptureCore` framework (`requestSendPTPCommand`). It enables remote event mode, polls the camera's event stream to count written bracket files, configures the focus-bracketing property, steps the lens to the near stop under Live View (comparing live-view frames every 10 steps until focus stops changing), and optionally downloads each file to the Mac.
- **Turntable control via `CoreBluetooth`:** Sends text commands over a BLE GATT characteristic and polls the turntable angle until motion settles.
- **Safety and manifest:** `stackrig run` writes a CSV log of each angle's timestamps, file count, and folder, and refuses to start if a target download folder already contains files so a rerun can never overwrite captured shots.

## Alternatives considered

### Canon EDSDK or external `libgphoto2` C dependency

Using `ImageCaptureCore`'s raw PTP passthrough requires zero third-party C libraries or proprietary SDK downloads on macOS.

## Consequences

- `stackrig` is a macOS-only hardware tool tailored to the Canon EOS R5 Mark II's PTP event codes (`0xC1B6`) and focus-bracketing property layout.
- Other PTP tethering apps that drain the camera's event queue must be closed while `stackrig` runs (`tools/stackrig/README.md`).

## Code

- `tools/stackrig/Sources/stackrig/Camera.swift`, `Turntable.swift`, `main.swift`.
- `tools/stackrig/README.md`.
