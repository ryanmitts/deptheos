import CoreGraphics
import Foundation
import SwiftUI
import os
@preconcurrency import DeptheosEngine

private let captureLog = AppLog(category: "capture")
private let pictureLog = AppLog(category: "pictures")
/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// A stack in progress, alone or a batch's, inside `Running`.
struct StackingState {
    /// The engine's run, canceled to stop it.
    let request: Request
    /// Stops the check of the frames' files before the engine starts.
    let fetch = FetchCancel()
    /// Its time in the background from iOS: its own, or its batch's.
    let continued: ContinuedWork
    var stage: Stage
    var liveMap: CGImage?
    /// The frames' scopes, held until the stack leaves `Running`.
    var lease: Lease = .none

    /// Where a stack in progress is. Until `made` is shown, the result on
    /// screen before holds no full-size pictures (`AppModel.wantsPictures`).
    ///
    /// Lifecycle: running (its progress, and why it was told to stop) or
    /// saving (the result made). Changed only by `AppModel.runStack`,
    /// `showFetching` and `stopBackgroundGPUWork`, through `updateStacking`.
    ///
    ///     running ──the engine made it──▶ saving ──kept──▶ (no stack)
    ///        └──canceled or failed──▶ (no stack)
    enum Stage {
        /// The engine runs, or a frame downloads first. `stop` is why it
        /// was told to stop; what that means is decided when it returns,
        /// since it may have finished first.
        case running(progress: Progress, stop: Stop?)
        /// Being written: the background doesn't stop it, and nothing is
        /// left to cancel.
        case saving(made: StackResult)
    }

    enum Progress {
        /// An iCloud frame downloading before the engine starts.
        case downloading(current: UInt32, total: UInt32, frames: UInt32)
        /// The engine's: its phase, the frames taken in (the rail's key to
        /// the live map) and the bar, which a two-pass stack fills once.
        case engine(StackProgress)

        /// Before the engine has said anything, for a stack of `frames`.
        static func starting(_ frames: UInt32) -> Progress {
            .engine(StackProgress(
                phase: .started, current: 0, total: frames, taken: 0, reversed: false, fraction: 0))
        }

        var frames: UInt32 {
            switch self {
            case .downloading(_, _, let frames): frames
            case .engine(let progress): progress.total
            }
        }

        /// The frames taken in and whether last first; none while
        /// downloading.
        var taken: (count: UInt32, reversed: Bool) {
            switch self {
            case .downloading: (0, false)
            case .engine(let progress): (progress.taken, progress.reversed)
            }
        }

        fileprivate var step: Step {
            switch self {
            case .downloading: .downloading
            case .engine(let progress): .engine(progress.phase)
            }
        }

        fileprivate var count: (current: UInt32, total: UInt32) {
            switch self {
            case .downloading(let current, let total, _): (current, total)
            case .engine(let progress): (progress.current, progress.total)
            }
        }
    }

    /// What a running stack is doing, as its label words it.
    fileprivate enum Step {
        case downloading
        case engine(StackPhase)

        /// "Aligning and stacking", when no count is known.
        var name: LocalizedStringResource {
            switch self {
            case .downloading:
                LocalizedStringResource(
                    "Downloading frame", comment: "Stack progress: a frame in iCloud is downloaded before the stack starts.")
            case .engine(.started):
                LocalizedStringResource("Starting", comment: "Stack progress: the stack is starting.")
            case .engine(.aligning):
                LocalizedStringResource(
                    "Aligning and stacking", comment: "Stack progress: frames are lined up and stacked.")
            case .engine(.finalizing):
                LocalizedStringResource("Finalizing", comment: "Stack progress: the result is being finished.")
            case .engine(.reversed):
                LocalizedStringResource(
                    "Restarting in reverse order",
                    comment: "Stack progress: the stack starts again from the last frame.")
            case .engine(.blending):
                LocalizedStringResource("Blending", comment: "Stack progress: the frames are being blended.")
            case .engine(.completed):
                LocalizedStringResource("Finishing", comment: "Stack progress: the stack is almost done.")
            }
        }

        /// "Aligning and stacking 12 of 66".
        func progress(_ current: UInt32, of total: UInt32) -> LocalizedStringResource {
            switch self {
            case .downloading:
                LocalizedStringResource(
                    "Downloading frame \(current) of \(total)",
                    comment: "Stack progress: frames in iCloud are downloaded before the stack starts. The placeholders are the frame's number and how many frames the stack has.")
            case .engine(.started):
                LocalizedStringResource(
                    "Starting \(current) of \(total)",
                    comment: "Stack progress: the stack is starting. The placeholders are the frame's number and how many frames there are.")
            case .engine(.aligning):
                LocalizedStringResource(
                    "Aligning and stacking \(current) of \(total)",
                    comment: "Stack progress: frames are lined up and stacked. The placeholders are the frame's number and how many frames there are.")
            case .engine(.finalizing):
                LocalizedStringResource(
                    "Finalizing \(current) of \(total)",
                    comment: "Stack progress: the result is being finished. The placeholders are the step and how many steps there are.")
            case .engine(.reversed):
                LocalizedStringResource(
                    "Restarting in reverse order \(current) of \(total)",
                    comment: "Stack progress: the stack runs again from the last frame. The placeholders are the frame's number and how many frames there are.")
            case .engine(.blending):
                LocalizedStringResource(
                    "Blending \(current) of \(total)",
                    comment: "Stack progress: the frames are being blended. The placeholders are the frame's number and how many frames there are.")
            case .engine(.completed):
                LocalizedStringResource(
                    "Finishing \(current) of \(total)",
                    comment: "Stack progress: the stack is almost done. The placeholders are the step and how many steps there are.")
            }
        }
    }

    enum Stop {
        /// The app left the foreground, where iOS stops GPU work, without
        /// time to go on there.
        case background
        /// iOS took back its background time.
        case expired

        var message: LocalizedStringResource {
            switch self {
            case .background:
                LocalizedStringResource(
                    "The stack stopped when you left Deptheos, and iOS gave it no time to finish in the background.",
                    comment: "Alert text: a stack stopped because the app went to the background.")
            case .expired:
                LocalizedStringResource(
                    "iOS stopped the stack in the background before it finished.",
                    comment: "Alert text: the time iOS gave a stack in the background ran out.")
            }
        }

        var exportMessage: LocalizedStringResource {
            switch self {
            case .background:
                LocalizedStringResource(
                    "The export stopped when you left Deptheos, and iOS gave it no time to finish in the background.",
                    comment: "Alert text: an export stopped because the app went to the background.")
            case .expired:
                LocalizedStringResource(
                    "iOS stopped the export in the background before it finished.",
                    comment: "Alert text: the time iOS gave an export in the background ran out.")
            }
        }
    }

    /// Stops the engine and the check before it.
    func cancel() {
        fetch.cancel()
        request.cancel()
    }

    /// The rail's key to the live map: each frame's legend color once
    /// taken in, `nil` before, and how many are in. Saving, the result's
    /// own colors (`nil` for a frame left out).
    var railKey: (colors: [LegendColor?], taken: Int) {
        switch stage {
        case .running(let progress, _):
            let taken = progress.taken
            let colors = runningLegend(total: progress.frames, taken: taken.count, reversed: taken.reversed)
            return (colors, Int(taken.count))
        case .saving(let made):
            let colors = made.frameColors()
            return (colors, colors.count)
        }
    }

    var isSaving: Bool {
        if case .saving = stage { true } else { false }
    }

    /// 0 to 1: the engine's, or the downloads'.
    var fraction: Double {
        switch stage {
        case .running(.downloading(let current, let total, _), _):
            total == 0 ? 0 : Double(current) / Double(total)
        case .running(.engine(let progress), _): progress.fraction
        case .saving: 1
        }
    }

    /// "Aligning and stacking 12 of 66", then "Saving".
    var label: LocalizedStringResource {
        switch stage {
        case .running(let progress, _):
            let (current, total) = progress.count
            return total > 0 && current > 0 ? progress.step.progress(current, of: total) : progress.step.name
        case .saving:
            return LocalizedStringResource("Saving", comment: "Stack progress: the result is made and being saved.")
        }
    }
}

extension AppModel {
    /// How long the Library's writes are waited for as the app leaves,
    /// inside iOS's half minute or so for a background task.
    static let writtenTimeoutMs: UInt32 = 20_000

    /// Stack the burst just taken with the algorithm the camera chose.
    func stackCapture(algorithm: AlgorithmKind) async {
        stackSettings.algorithm = algorithm
        let started = Date.now
        let memory = memoryReading()
        setPanel(.closed)
        guard await stack() else { return }
        let took = Date.now.timeIntervalSince(started)
        captureLog.info("capture stacked (\(algorithm.rawValue, privacy: .public)) in \(took, format: .fixed(precision: 1)) s; \(memory, privacy: .public) at the start")
    }

    /// Stack the included images (with `selected`, those among them);
    /// `true` when a render was made and kept.
    @discardableResult
    func stack(selected: Set<String>? = nil) async -> Bool {
        guard !busy else { return false }
        // Always the working version, whatever render is on screen.
        let consider = { selected.map { s in self.rows.filter { s.contains($0.file) } } ?? self.rows }
        let considered = consider()
        let stacked = considered.filter(\.included)
        let problem = await stackProblem(considered)
        // Another stack started meanwhile, or the list changed: this tap is
        // spent.
        guard !busy, consider() == considered else { return false }
        if let problem {
            errorMessage = ErrorMessage(
                title: LocalizedStringResource(
                    "The stack can't start", comment: "Alert title when Stack is tapped and the frames can't be stacked."),
                text: problem)
            return false
        }
        notice = nil
        addedToExisting = nil
        saveList()
        // Both files of a pair, so the render's list brings the pair back.
        let listed = considered.flatMap(\.files)
        switch await stack(paths: stacked.map(\.file), listed: listed, settings: settings) {
        case .saved: return true
        case .canceled: return false
        case .failed(let message):
            // The error is over the photo: its controls come back.
            showControls()
            errorMessage = ErrorMessage(
                title: LocalizedStringResource(
                    "The stack didn't finish", comment: "Alert title when a stack fails; the text says why."),
                text: message)
            return false
        }
    }

    enum StackOutcome {
        case saved
        case canceled
        case failed(UserText)
    }

    /// Stack `paths` with `settings` and keep the render as the stack on
    /// screen's next, `listed` as its list. A batch keeps its own
    /// (`driveBatch`).
    func stack(paths: [String], listed: [String], settings: StackSettings) async -> StackOutcome {
        let name = stackName ?? String(localized: Self.untitledStack)
        let continued = continuedWork(
            title: String(localized: LocalizedStringResource(
                "Stacking \(name)",
                comment: "Title of the system's progress for a stack running in the background. The placeholder is the stack's name.")))
        let outcome: StackOutcome
        // The render it starts from, whose adjustments it inherits.
        let from = base
        switch await runStack(paths: paths, settings: settings, continued: continued) {
        case .made(let made, let generation):
            if let error = await keep(
                made, listed: listed, into: stackID, made: .stacked(base: from),
                generation: generation)
            {
                outcome = .failed(error)
            } else {
                outcome = .saved
            }
        case .canceled: outcome = .canceled
        case .failed(let message): outcome = .failed(message)
        }
        if case .saved = outcome { continued.end(success: true) } else { continued.end(success: false) }
        // Felt, for a stack waited on with the eyes elsewhere; none when
        // the user stopped it.
        switch outcome {
        case .saved: UINotificationFeedbackGenerator().notificationOccurred(.success)
        case .failed: UINotificationFeedbackGenerator().notificationOccurred(.error)
        case .canceled: break
        }
        return outcome
    }

    /// Ask iOS for background time for a stack or batch starting now.
    func continuedWork(title: String) -> ContinuedWork {
        ContinuedWork(title: title, subtitle: String(localized: StackingState.Step.engine(.started).name)) { [weak self] in
            self?.continuedWorkExpired() ?? false
        }
    }

    /// The app is in front, or iOS gave it time.
    func mayUseGPU(_ continued: ContinuedWork) -> Bool {
        UIApplication.shared.applicationState != .background || continued.covers
    }

    enum StackRun {
        /// Shown as "Saving" until [`keep`] keeps it.
        case made(StackResult, generation: Int)
        case canceled
        case failed(UserText)
    }

    /// Stack `paths`, its progress on screen and, under `continued`, in
    /// the system's.
    func runStack(paths: [String], settings: StackSettings, continued: ContinuedWork) async
        -> StackRun
    {
        guard let engine, stacking == nil else {
            return .failed(.app(LocalizedStringResource(
                "A stack is already running.", comment: "Alert text: a second stack was asked for while one runs.")))
        }
        // Time iOS took back is given back only once GPU work has stopped.
        defer { continued.settled() }
        // iOS stops GPU work in the background, so a stack asked for there
        // without time stops before it starts, as a running one would.
        if !mayUseGPU(continued) {
            stateLog.debug("stack: not started, the app is in the background")
            if batchRun == nil {
                errorMessage = ErrorMessage(.app(StackingState.Stop.background.message))
            }
            return .canceled
        }
        viewGeneration += 1
        let generation = viewGeneration
        // Lasting: it goes on in the background under `continued`, or
        // `stopBackgroundGPUWork` stops it.
        let request = Request.lasting()
        let liveSide = Self.liveSide
        startStacking(StackingState(
            request: request, continued: continued,
            stage: .running(progress: .starting(UInt32(paths.count)), stop: nil)))
        stateLog.debug("stack: running")
        // So the result Retouch closes on isn't given its full-size
        // picture again (`wantsPictures`).
        leaveMode()
        // No auto-lock while stacking: the background may stop the GPU.
        keepAwake(.stack, true)
        defer { keepAwake(.stack, false) }
        // The live map shows in place of all but a frame alone.
        backToResult()
        // The result it may replace, and what that is compared with, let go
        // of their full-size pictures before the engine reads free memory
        // (`wantsPictures`). The engine's line logs after.
        pictureLog.info("stack: \(memoryReading(), privacy: .public) before the result's full-size pictures are let go of")
        fitPictures()
        // Under the lasting request, so a background run gets them too.
        let lease = await access(paths, under: request)
        updateStacking { $0.lease = lease }
        if let fetch = stacking?.fetch, let problem = await preflight(paths, request: request, cancel: fetch) {
            stackEnded()
            return .failed(problem)
        }
        // Progress of this run only, and none once it is saving.
        let observer = StackObserverBridge(request: request)
        let listening = Task { [weak self] in
            for await news in observer.news {
                guard let self, self.stacking?.request === request else { continue }
                if let progress = news.progress,
                   case .running(let before, let stop)? = self.stacking?.stage
                {
                    // Its turns, not each frame: restarted reversed, all in.
                    let (was, whole) = (before.taken, progress.taken == progress.total)
                    if progress.reversed != was.reversed || (whole && was.count != before.frames) {
                        stateLog.debug("stack: \("\(progress.taken) of \(progress.total)", privacy: .public) frames taken in\(progress.reversed ? ", reversed" : "", privacy: .public)")
                    }
                    self.updateStacking { $0.stage = .running(progress: .engine(progress), stop: stop) }
                    self.reportProgress()
                }
                if let map = news.liveMap, self.stacking?.liveMap !== map {
                    self.updateStacking { $0.liveMap = map }
                }
            }
        }
        // The stream ends with the run, however it ends.
        defer { observer.finish() }
        // Grid search's limits: the preferences, as they are when it starts.
        let alignment = preferences.alignment
        do {
            let stack = try await request.run {
                try await engine.stack(
                    paths: paths, settings: settings, alignment: alignment,
                    liveSide: liveSide, request: $0, observer: observer)
            }
            // What it sent before it returned lands first.
            observer.finish()
            await listening.value
            // Made, even if it was told to stop on the way: it is kept.
            updateStacking { $0.stage = .saving(made: stack) }
            stateLog.debug("stack: saving")
            reportProgress()
            return .made(stack, generation: generation)
        } catch EngineError.Canceled {
            // Said once it did stop, so a stack that finished first isn't
            // reported as stopped.
            if case .running(_, let stop?)? = stacking?.stage, batchRun == nil {
                errorMessage = ErrorMessage(.app(stop.message))
            }
            stackEnded()
            return .canceled
        } catch {
            stackEnded()
            return .failed(Self.message(error))
        }
    }

    /// Keep `stack` as a render in stack `into`, made as `made`, and show
    /// it; why not, if it couldn't be kept.
    func keep(
        _ stack: StackResult, listed: [String], into: String, made: RenderMade,
        generation: Int
    ) async -> UserText? {
        guard let engine else {
            stackEnded()
            return engineDown
        }
        // Writing needs no GPU: let it finish if the app is sent away.
        let background = BackgroundTask("Save render")
        defer { background.end() }
        let render: RenderInfo
        let batch: Batch? = if case .batch(let batch) = made { batch } else { nil }
        do {
            render = try await engine.saveRender(frames: listed, result: stack, stack: into, made: made)
        } catch {
            stackEnded()
            return Self.message(error)
        }
        history.renders.append(render)
        // A failed run tried again: its stack has a render now.
        history.failures.removeAll { $0.stack == render.stack }
        if batch == nil, render.stack == stackID {
            base = render.id
            saveList()
        }
        history.shownFailure = nil
        await show(stack, renderID: render.id, generation: generation)
        stateLog.debug("stack: ended, kept")
        endStacking()
        // Shown with its pictures, or if something else was shown, that
        // gets its pictures back.
        fitPictures()
        requestFullFrame()
        // Frames left out, said now; a batch sums up at its end.
        if batch == nil { notice = stack.notice().map(UserText.verbatim) }
        return nil
    }

    /// Ended without a result: the viewer goes back to what it showed.
    private func stackEnded() {
        stateLog.debug("stack: ended")
        endStacking()
        if result == nil { showPlainFrame() } else { backToResult() }
        // The result it would have replaced gets its pictures back, and
        // those of what it is compared with (a compare stays, so no
        // `setView`), and a frame alone's full size.
        fitPictures()
        requestFullFrame()
    }

    /// Stop the stack, and its batch: it closes once what it made is kept.
    func cancelStack() {
        if let run = batchRun {
            perform(run.queue.cancel())
        } else {
            stacking?.cancel()
        }
    }

    /// The system's progress, under the stack's or batch's title.
    func reportProgress() {
        switch running {
        case .idle:
            break
        case .stacking(let stack):
            stack.continued.report(fraction: stack.fraction, subtitle: String(localized: stack.label))
        case .batch(let run, let stack):
            run.continued.report(
                fraction: run.queue.fraction(current: stack?.fraction ?? 0), subtitle: run.status.progress)
        }
    }
}
