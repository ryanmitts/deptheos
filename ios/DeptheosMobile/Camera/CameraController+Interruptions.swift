@preconcurrency import AVFoundation
import Foundation
import os

private nonisolated let log = AppLog(category: "camera")

/// A call, another app, screen sharing (no camera while multitasking),
/// heat or the app sent away pauses the session; a runtime error can stop
/// it. Either way a running burst would wait forever, so it ends there,
/// kept if it has enough frames.
nonisolated extension CameraController {
    /// Seconds a burst ended early waits for its photos still in flight.
    static let drainTimeout: TimeInterval = 2
    /// Long enough for any photo the pipeline will deliver, so only one that
    /// never comes is gone without.
    static let doneDrainTimeout: TimeInterval = 10

    func observeSession() {
        let center = NotificationCenter.default
        observers = [
            center.addObserver(
                forName: AVCaptureSession.wasInterruptedNotification, object: session, queue: nil
            ) { [weak self] note in
                let reason = (note.userInfo?[AVCaptureSessionInterruptionReasonKey] as? Int)
                    .flatMap(AVCaptureSession.InterruptionReason.init(rawValue:))
                guard let self else { return }
                queue.async { self.paused(Self.message(for: reason)) }
            },
            center.addObserver(
                forName: AVCaptureSession.interruptionEndedNotification, object: session, queue: nil
            ) { [weak self] _ in
                guard let self else { return }
                queue.async { self.resumed() }
            },
            center.addObserver(
                forName: AVCaptureSession.runtimeErrorNotification, object: session, queue: nil
            ) { [weak self] note in
                let error = note.userInfo?[AVCaptureSessionErrorKey] as? AVError
                guard let self else { return }
                queue.async { self.failed(error) }
            },
        ]
    }

    static func message(
        for reason: AVCaptureSession.InterruptionReason?
    ) -> LocalizedStringResource {
        switch reason {
        case .videoDeviceInUseByAnotherClient, .audioDeviceInUseByAnotherClient:
            LocalizedStringResource(
                "Another app is using the camera.",
                comment: "Hint on the camera screen while another app has the camera.")
        case .videoDeviceNotAvailableWithMultipleForegroundApps:
            // Only where the camera can't be shared while multitasking.
            LocalizedStringResource(
                "The camera pauses in Split View and Slide Over. Make Deptheos full screen to shoot.",
                comment: "Hint on the camera screen while Deptheos shares the screen with another app.")
        case .videoDeviceNotAvailableDueToSystemPressure:
            LocalizedStringResource(
                "The camera paused to let the device cool down.",
                comment: "Hint on the camera screen while the device is too hot for the camera.")
        default:
            LocalizedStringResource(
                "The camera is paused.",
                comment: "Hint on the camera screen while the camera is interrupted for another reason.")
        }
    }

    func paused(_ message: LocalizedStringResource) {
        // A call or another app: expected, not an error.
        log.info("camera interrupted: \(message.key, privacy: .public)")
        end(.cutShort(BurstInterrupted()))
        // A session stopped on purpose has nothing to resume.
        guard sessionState != .stopped else { return }
        become(.interrupted(message))
    }

    private func resumed() {
        guard case .interrupted = sessionState else { return }
        log.info("camera resumed")
        become(session.isRunning ? .running : .failed)
    }

    func failed(_ error: AVError?) {
        log.error("camera runtime error: \(String(describing: error), privacy: .public)")
        end(.cutShort(BurstInterrupted()))
        guard sessionState != .stopped else { return }
        // After a media services reset the session must restart, but only a
        // few times close together, so a recurring error leaves it failed.
        if error?.code == .mediaServicesWereReset || !session.isRunning {
            let now = Date.now
            restarts.removeAll { now.timeIntervalSince($0) > Self.restartWindow }
            if restarts.count < Self.maxRestarts {
                restarts.append(now)
                log.info("restarting the camera (\(self.restarts.count) of \(Self.maxRestarts) within \(Int(Self.restartWindow)) s)")
                session.startRunning()
            } else {
                log.error("camera not restarted: \(Self.maxRestarts) runtime errors within \(Int(Self.restartWindow)) s")
                if session.isRunning { session.stopRunning() }
                become(.failed)
                return
            }
        }
        // An interruption's message stays until it ends.
        if case .interrupted = sessionState { return }
        become(session.isRunning ? .running : .failed)
    }

    /// On `queue` only; the main queue keeps the messages in order.
    func become(_ state: SessionState) {
        dispatchPrecondition(condition: .onQueue(queue))
        if state != sessionState {
            log.debug("session: \(self.sessionState.label, privacy: .public) → \(state.label, privacy: .public)")
        }
        sessionState = state
        guard let onSessionChange else { return }
        DispatchQueue.main.async { MainActor.assumeIsolated { onSessionChange(state) } }
    }
}
