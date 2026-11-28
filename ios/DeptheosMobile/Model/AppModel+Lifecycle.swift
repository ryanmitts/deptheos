import Foundation
import SwiftUI
@preconcurrency import DeptheosEngine

private let captureLog = AppLog(category: "capture")
/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Time asked of iOS to finish work after leaving the foreground. It ends
/// once, at the work's end or the time's: a task still open when time runs
/// out gets the app killed rather than suspended.
final class BackgroundTask {
    private var id = UIBackgroundTaskIdentifier.invalid

    init(_ name: String) {
        id = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            self?.end()
        }
    }

    func end() {
        guard id != .invalid else { return }
        UIApplication.shared.endBackgroundTask(id)
        id = .invalid
    }
}

/// Leaving the foreground, coming back, and memory running short.
extension AppModel {
    /// Leaving the foreground, where iOS stops GPU work without
    /// `ContinuedWork`: a running stack stops cleanly and a batch pauses,
    /// to go on when back. Everything sent to the Library is on disk inside
    /// iOS's time: the retouching committed and closed, then the store's
    /// queue awaited. Every other request is canceled at one switch
    /// (`leaveForeground`) and refused until back (`enteredForeground`).
    func enteredBackground() {
        leaveForeground()
        adjust.away()
        saveList()
        let background = BackgroundTask("Save on leaving")
        let engine = engine
        Task {
            // Committed as each change ends; whatever is left now.
            await paint.save()
            let written = (try? await engine?.written(timeoutMs: Self.writtenTimeoutMs)) ?? true
            if !written { captureLog.error("leaving: the Library's writes didn't finish in time") }
            background.end()
        }
        stopBackgroundGPUWork(.background)
        // Full-size pictures go, flat ones stay. In the background the app
        // is ended under its own limit, so shed as on a warning too.
        fitPictures(background: true)
        shedMemory(level: .moderate)
        shed(.moderate)
    }

    /// Memory is short (`MemoryPressure` told the engine already), or the
    /// app went to the background (moderate): keep only the thumbnails in
    /// view. At critical, or at either while a stack or batch runs, the
    /// pictures are fitted again at once, so none off screen is held. If
    /// memory keeps falling, the engine stops the stack before the system
    /// ends the app (ADR-111).
    func shed(_ level: MemoryShed) {
        let stackRuns = switch running {
        case .idle: false
        case .batch: true
        case .stacking(let stack): if case .running = stack.stage { true } else { false }
        }
        stateLog.info("memory \(String(describing: level), privacy: .public)\(stackRuns ? " while stacking" : ""): \(memoryReading(), privacy: .public); dropping thumbnails out of view")
        history.shedThumbnails()
        if stackRuns || level == .critical { fitPictures() }
        adjust.shed(level)
    }

    /// The memory pressure is over: Adjust's copies are made again if a
    /// critical shed took them.
    func memoryNormal() {
        stateLog.info("memory normal: \(memoryReading(), privacy: .public)")
        if UIApplication.shared.applicationState != .background { adjust.back(engine) }
    }

    /// iOS took back the background time: in the background, GPU work
    /// stops as if it had none. Whether a stack is winding down; the time
    /// goes back once it has (`ContinuedWork.settled`).
    func continuedWorkExpired() -> Bool {
        guard UIApplication.shared.applicationState == .background else { return false }
        return stopBackgroundGPUWork(.expired)
    }

    /// In the background, stop GPU work iOS gave no time for: an export
    /// stops, a batch pauses, a stack stops. Whether a stack was told to
    /// stop.
    @discardableResult
    private func stopBackgroundGPUWork(_ why: StackingState.Stop) -> Bool {
        stopExports(why)
        switch running {
        case .idle:
            return false
        case .stacking(let stack):
            guard !stack.continued.covers else { return false }
        case .batch(let run, _):
            // A batch's stacks run under the batch's time: they pause
            // together.
            guard !run.continued.covers else { return false }
            perform(run.queue.pause())
        }
        // A result being saved finishes under a background task.
        guard let stacking, case .running(let progress, _) = stacking.stage else { return false }
        updateStacking { $0.stage = .running(progress: progress, stop: why) }
        stateLog.debug("stack: told to stop in the background (\(String(describing: why), privacy: .public))")
        stacking.cancel()
        return true
    }

    /// Back in front: the engine takes requests again, and what leaving
    /// stopped is asked again. A paused batch goes on, once its stack has
    /// wound down; one whose background time ran out asks anew.
    func enteredForeground() {
        enterForeground()
        stateLog.info("foreground: \(memoryReading(), privacy: .public)")
        fitPictures(background: false)
        askStoppedAgain()
        mapLighter.askAgain()
        paint.askStoppedAgain()
        adjust.back(engine)
        requestFullFrame()
        if history.read == .unread, engine != nil { Task { await history.refresh() } }
        if let run = batchRun {
            if run.continued.lapsed {
                // A stack still winding down gives the time back now: in
                // front the GPU is the app's.
                run.continued.settled()
                updateBatch { $0.continued = continuedWork(title: run.title) }
                reportProgress()
            }
            perform(run.queue.resume())
        }
    }
}
