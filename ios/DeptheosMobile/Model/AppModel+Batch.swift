import Foundation
import UIKit
@preconcurrency import DeptheosEngine

/// A batch run: the stacks still to make, in turn, with the settings as at
/// Start, each kept under the run's header. Inside `Running.batch`.
struct BatchRun {
    let batch: Batch
    let settings: StackSettings
    /// Each job's render's stack, by place in the run, as its unstacked
    /// record names it (`BatchAction.recordUnstacked`).
    let stacks: [String]
    /// The engine's state machine, as on the desktop: it says what to do.
    let queue: BatchQueue
    /// As of its last change, for the views.
    var status: BatchStatus
    /// Asked again when back in front after it ran out (`enteredForeground`).
    var continued: ContinuedWork

    /// Its title in the system's progress ("Auto sort"), over "Batch 2 of
    /// 14".
    var title: String { batch.shownLabel }

    /// "Batch 2 of 14 · P1010217–219", "Batch 2 of 14 · paused".
    var progress: String { status.progress }
}

/// Batch…: one stack per folder, or each folder auto sorted, planned by
/// the engine as on the desktop.
extension AppModel {
    /// With the settings as they are now.
    func startBatch(_ plan: BatchPlan) async {
        guard !busy, engine != nil else { return }
        guard !plan.jobs.isEmpty else {
            notice = .app(LocalizedStringResource(
                "Nothing to stack: \(plan.summary)",
                comment: "Notice after Start in the Batch sheet when the plan has no stacks. The placeholder is the engine's summary of the plan."))
            return
        }
        notice = nil
        let queue = BatchQueue(jobs: plan.jobs, words: AppWords.words)
        let batch = newBatch(plan: plan)
        setRunning(.batch(
            BatchRun(
                batch: batch, settings: settings, stacks: plan.jobs.map { _ in newStackId() },
                queue: queue, status: queue.status(),
                continued: continuedWork(title: batch.shownLabel)),
            stack: nil))
        await driveBatch(queue.start())
    }

    /// Do what the queue said, from outside its driver (Cancel, the app
    /// leaving and back). The queue starts a stack only when none runs, so
    /// this never makes a second driver.
    func perform(_ actions: [BatchAction]) {
        refreshBatchStatus()
        guard !actions.isEmpty else { return }
        Task { await driveBatch(actions) }
    }

    /// Do each thing the queue says and report back, until it says nothing
    /// more. The pause decides what a stack stopped in the background comes
    /// to, not the stack's error.
    func driveBatch(_ actions: [BatchAction]) async {
        var pending = actions[...]
        while let action = pending.popFirst() {
            guard let run = batchRun else { return }
            switch action {
            case .stack(_, let frames):
                // Canceled while the last render saved, or in the background
                // without GPU time (it runs once back): not started.
                if run.queue.status().canceled || !mayUseGPU(run.continued) {
                    pending += run.queue.stopped()
                    break
                }
                refreshBatchStatus()
                switch await runStack(
                    paths: frames, settings: run.settings, continued: run.continued)
                {
                case .made(let result, let generation):
                    // Only ever the stack just made.
                    let made = run.queue.stacked()
                    var saved: [BatchAction] = []
                    if let save = made.save {
                        // Into its job's record's stack; the save removes
                        // the record.
                        if let error = await keep(
                            result, listed: save.listed, into: run.stacks[Int(save.job)],
                            made: .batch(batch: run.batch), generation: generation)
                        {
                            saved = run.queue.saveFailed(stack: save.stack, error: error.string)
                        } else {
                            saved = run.queue.saved(stack: save.stack)
                        }
                    }
                    pending += made.then + saved
                case .canceled:
                    pending += run.queue.stopped()
                case .failed(let message):
                    pending += run.queue.failed(error: message.string)
                }
            case .cancelStack:
                stacking?.cancel()
            case .recordUnstacked(let jobs, let why):
                // Every job hidden as unstacked until it fails or the run
                // ends, so a run iOS ends keeps what is left.
                await history.recordUnstacked(
                    jobs, stacks: run.stacks, settings: run.settings, batch: run.batch, why: why)
            case .recordFailure(let no, let job, let error):
                // Kept under the run, to find and try again.
                await history.recordFailure(
                    job, stack: run.stacks[Int(no)], settings: run.settings, batch: run.batch,
                    error: error)
            case .finish(let summary):
                notice = .verbatim(summary)
                run.continued.end(success: true)
                setRunning(.idle)
                // The stacks it didn't make are listed, to try again.
                await history.batchEnded()
                setScreen(.library)
                await showOwnResult()
                // What is shown gets its full-size pictures back if what
                // landed didn't bring them.
                fitPictures()
                return
            }
            refreshBatchStatus()
        }
    }

    private func refreshBatchStatus() {
        updateBatch { $0.status = $0.queue.status() }
        reportProgress()
    }

    /// The viewer holds a result not the stack on screen's (a batch's last,
    /// another stack's render) or one just deleted: show this stack's base,
    /// else a frame, so Retouch and Export act on its own. `continuing`:
    /// the open request this lands under.
    func showOwnResult(continuing request: Int? = nil) async {
        if let own = base, history.renders.contains(where: { $0.id == own }) {
            await showResult(of: own, continuing: request)
        } else {
            // An open under way ends here with nothing to land; anything
            // else on its way drops.
            if let request { endOpening(request) } else { viewGeneration += 1 }
            setResult(nil)
            showPlainFrame()
            await loadSourcePreview()
        }
    }
}
