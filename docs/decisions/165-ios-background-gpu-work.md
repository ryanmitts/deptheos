# ADR-165: iOS stacks and batches go on in the background under BGContinuedProcessingTask, and stop cleanly where iOS gives no GPU time

## Status

Accepted.

## Context

Focus stacking is GPU-bound and can run for several minutes on a large bracket or batch. iOS ends an app's GPU command submissions once it is in the background unless the app holds background GPU time, so switching apps or taking a call would otherwise crash the stack or cancel it at once.

## Decision

- **Background GPU time is asked for as a run starts** (`ContinuedWork` in `ios/DeptheosMobile/Model/ContinuedWork.swift`): a stack or batch started in the foreground requests a `BGContinuedProcessingTask` with the `.gpu` resource and reports its progress to the system's progress UI. While it is granted, leaving the app lets the run go on.
- **Where there is no such time, or it ends, the run stops cleanly** (`stopBackgroundGPUWork` in `Model/AppModel+Lifecycle.swift`):
  - Where `BGTaskScheduler.supportedResources` lacks `.gpu`, or iOS denies the request, leaving the foreground stops a single stack and pauses a batch (`Stop::Pause` in `batch::Machine`) until the app is back.
  - When iOS takes the time back, or the user cancels from the system's progress UI, the stack stops with "iOS stopped the stack in the background before it finished." The task is completed only once the stack has returned (`ContinuedWork.State.expiring`, ended by `settled()`), so no GPU work goes on past it.
  - A render being written once its GPU work is done finishes under a `UIApplication` background task (`BackgroundTask`), ended once, on completion or expiration, so iOS suspends the app rather than kills it.
  - A stack asked for while the app is already in the background stops before it starts.
- **An export is held the same way** (`ExportRun` in `AppModel.exports`, `runExport` in `Model/AppModel+Export.swift`): its lasting request asks for its own background GPU time, and where there is none, or it ends, the export is canceled and the user told why.
- **Adjust lets go of its GPU copies as the app leaves** (`AdjustSession::away`, and again under memory pressure), keeping its values and their writes, and makes them again on `back` (ADR-153).
- **Everything else stops at one foreground token** (`leave_foreground` in `crates/deptheos_ffi/src/request.rs`, ADR-143): every request but the lasting ones is canceled as the app leaves, and refused until it is back, so no load, picture or thumbnail reaches the GPU in the background. A load still wanted is asked again once the app is back (`AppModel.stopped`, `MapLighter.askAgain`).

## Alternatives considered

### Cancel every stack when the app leaves the foreground

Simple, and safe on every device, but a long stack or a batch would need the app on screen for minutes. It is kept only where iOS gives no background GPU time.

## Consequences

- `BGContinuedProcessingTask` must be requested while the app is in the foreground, so `ContinuedWork` starts with every stack or batch, not as the app leaves.
- `ContinuedWork.State` is one lifecycle enum for the background time and its expiry (ADR-143).

## Code

- `ios/DeptheosMobile/Model/ContinuedWork.swift`, `ios/DeptheosMobile/Model/AppModel+Stacking.swift`, `ios/DeptheosMobile/Model/AppModel+Lifecycle.swift`, `ios/DeptheosMobile/Model/AppModel+Export.swift`.
- `crates/deptheos_ffi/src/request.rs`, `crates/deptheos_app/src/runs/batch/machine.rs`.
