import BackgroundTasks
import Foundation
import os

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Time asked of iOS for a stack or batch to go on using the GPU after the
/// app leaves the foreground (`BGContinuedProcessingTask`), its progress
/// shown by the system. Asked when the work starts, in front.
///
/// Lifecycle: `State` is requested, running, expiring (iOS took the time
/// back while a stack winds down) or over (why). Changed only by this
/// class's methods and iOS's launch and expiry, on the main actor.
///
///     requested ──iOS launched it──▶ running ──ended──▶ over(.ended)
///        │                             └──iOS expired it──▶ expiring ──settled or ended──▶ over(.expired)
///        │                                (nothing to stop: over(.expired) at once)
///        └──ended before the launch──▶ over(.ended) (the launch completes it)
///     (not asked: no GPU in the background, or refused) ──▶ over
///
/// An expired task is completed only once the GPU work has stopped.
final class ContinuedWork {
    enum State {
        /// Not launched yet. It runs at once or the submission fails
        /// (`.fail`), so this counts as granted.
        case requested
        case running(BGContinuedProcessingTask)
        /// iOS took the time back while a stack ran: completed once it has
        /// stopped (`settled`), so no GPU work outlasts it.
        case expiring(BGContinuedProcessingTask)
        case over(Over)
    }

    enum Over {
        case unsupported
        case refused
        /// iOS took the time back, or the person stopped it in the system's
        /// progress.
        case expired
        case ended
    }

    private(set) var state: State
    /// On expiry before the end: whether a stack is winding down, so the
    /// task waits for `settled`.
    private let onExpired: () -> Bool
    private var title: String
    private var subtitle: String

    /// Call it in the foreground.
    init(title: String, subtitle: String, onExpired: @escaping () -> Bool) {
        self.title = title
        self.subtitle = subtitle
        self.onExpired = onExpired
        guard BGTaskScheduler.supportedResources.contains(.gpu) else {
            state = .over(.unsupported)
            stateLog.debug("continued work: not asked, no background GPU on this device")
            return
        }
        guard let bundle = Bundle.main.bundleIdentifier else {
            state = .over(.refused)
            return
        }
        // An identifier registers only once, so each request has its own
        // under the Info.plist's wildcard.
        let identifier = "\(bundle).stacking.\(UUID().uuidString)"
        state = .requested
        let registered = BGTaskScheduler.shared.register(
            forTaskWithIdentifier: identifier, using: .main
        ) { [weak self] task in
            MainActor.assumeIsolated {
                guard let task = task as? BGContinuedProcessingTask else {
                    task.setTaskCompleted(success: false)
                    return
                }
                if let self {
                    self.launched(task)
                } else {
                    task.setTaskCompleted(success: false)
                }
            }
        }
        guard registered else {
            state = .over(.refused)
            stateLog.error("continued work: \(identifier, privacy: .public) is not permitted by the Info.plist")
            return
        }
        let request = BGContinuedProcessingTaskRequest(
            identifier: identifier, title: title, subtitle: subtitle)
        request.requiredResources = .gpu
        request.strategy = .fail
        do {
            try BGTaskScheduler.shared.submit(request)
            stateLog.debug("continued work: requested")
        } catch {
            state = .over(.refused)
            stateLog.info("continued work: refused: \(error.localizedDescription, privacy: .public)")
        }
    }

    var covers: Bool {
        switch state {
        case .requested, .running: true
        case .expiring, .over: false
        }
    }

    /// iOS took the time back or never gave it; the app may ask anew.
    var lapsed: Bool {
        switch state {
        case .requested, .running: false
        case .expiring: true
        case .over(let why): why != .ended && why != .unsupported
        }
    }

    /// 0 to 1, and the line under its title.
    func report(fraction: Double, subtitle: String) {
        let changed = subtitle != self.subtitle
        self.subtitle = subtitle
        guard case .running(let task) = state else { return }
        task.progress.completedUnitCount = Int64((min(max(fraction, 0), 1) * 1000).rounded())
        if changed { task.updateTitle(title, subtitle: subtitle) }
    }

    func end(success: Bool) {
        switch state {
        case .running(let task):
            task.progress.completedUnitCount = task.progress.totalUnitCount
            task.setTaskCompleted(success: success)
            state = .over(.ended)
            stateLog.debug("continued work: ended")
        case .expiring:
            settled()
        case .requested:
            // Its launch, if it comes, completes it at once.
            state = .over(.ended)
            stateLog.debug("continued work: ended before its launch")
        case .over:
            break
        }
    }

    private func launched(_ task: BGContinuedProcessingTask) {
        guard case .requested = state else {
            // Ended (or refused) before iOS launched it.
            task.setTaskCompleted(success: true)
            return
        }
        task.progress.totalUnitCount = 1000
        task.updateTitle(title, subtitle: subtitle)
        task.expirationHandler = { [weak self] in
            Task { @MainActor in self?.expired(task) }
        }
        state = .running(task)
        stateLog.debug("continued work: running")
    }

    /// The covered GPU work has stopped: taken-back time goes back now.
    func settled() {
        guard case .expiring(let task) = state else { return }
        state = .over(.expired)
        stateLog.debug("continued work: expiring → expired, its stack stopped")
        task.setTaskCompleted(success: false)
    }

    private func expired(_ task: BGContinuedProcessingTask) {
        guard case .running(let current) = state, current === task else { return }
        state = .expiring(task)
        stateLog.debug("continued work: running → expiring")
        // GPU work stops first, then the time goes back: at once, or once
        // the stack returns.
        if !onExpired() { settled() }
    }
}
