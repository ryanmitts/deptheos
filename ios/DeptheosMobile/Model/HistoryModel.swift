import CoreGraphics
@preconcurrency import DeptheosEngine
import Foundation
import Observation

private let log = AppLog(category: "app")

/// The history as the app shows it: every render and failed batch stack,
/// oldest first, their thumbnails, and the engine calls that change them.
/// What changes the screen stays with `AppModel`, which owns this.
@Observable
final class HistoryModel {
    /// Whether the renders and failures here are the engine's.
    ///
    /// Lifecycle: unread or read. Changed only by `refresh`.
    ///
    ///     unread ──refresh, the engine listed them──▶ read
    ///     unread ──refresh, it could not──▶ unread
    ///
    /// Once read it stays read. Unread, they are empty, which is not
    /// "nothing is used": no copy is deleted then (`AppModel.unusedImports`).
    enum Read {
        case unread, read
    }
    private(set) var read = Read.unread

    var renders: [RenderInfo] = [] {
        didSet { revision &+= 1 }
    }
    /// Counts each change of `renders` and `failures`, for what is derived
    /// from them (`AppModel.showings`, `AppModel.library`).
    private(set) var revision = 0
    /// Bytes on disk per render, as `refresh` last measured; none for one
    /// saved since.
    private(set) var sizes: [String: UInt64] = [:]
    /// Batch stacks that failed, oldest first, to load and try again.
    var failures: [FailureInfo] = [] {
        didSet { revision &+= 1 }
    }
    /// The failure last loaded, until something replaces it.
    var shownFailure: String?
    /// Stacks with frames and no render, "Not stacked yet" in the Library;
    /// not the stack on screen.
    var unstacked: [UnstackedInfo] = []
    /// Recently Deleted, newest first (ADR-140): read with the history and
    /// after each delete, restore and Empty.
    private(set) var deleted: [DeletedInfo] = []
    /// Render thumbnails, as `ThumbnailPolicy` keeps them (ADR-144): only
    /// its `keep` adds and its `drop` removes.
    private(set) var thumbnails: [String: CGImage] = [:]
    /// One is 512 pixels long, about 0.7 MB decoded, so some 70 MB: more
    /// rows than any screen shows, whatever the Library's length.
    static let thumbnailsKept = 96

    /// Runs the engine calls, reporting their errors.
    @ObservationIgnored unowned let app: AppModel
    @ObservationIgnored private let thumbs = ThumbnailPolicy(kept: UInt32(thumbnailsKept))
    /// The rows in view showing a thumbnail, latest last; a render is in it
    /// once per row (a tile and Render Info).
    @ObservationIgnored private var inView: [String] = []

    init(app: AppModel) {
        self.app = app
    }

    /// "610 MB" in the user's locale; `nil` until every one is measured.
    func sizeText(_ ids: [String]) -> String? {
        sizeBytes(ids).map { AppModel.size(Int64(clamping: $0)) }
    }

    /// In bytes; `nil` until every one is measured.
    func sizeBytes(_ ids: [String]) -> UInt64? {
        var total: UInt64 = 0
        for id in ids {
            guard let bytes = sizes[id] else { return nil }
            total += bytes
        }
        return total
    }

    /// Delete Other Renders: all of `render`'s stack but it and the starred.
    func others(of render: RenderInfo) -> [String] {
        renders.filter { $0.stack == render.stack && $0.id != render.id && !$0.picked }.map(\.id)
    }

    func title(of id: String?) -> String? {
        renders.first { $0.id == id }?.shownTitle
    }

    func refresh() async {
        // A render saved, starred or deleted during the read makes it
        // stale, so the history is read again.
        var listed: ([RenderInfo], [FailureInfo])?
        var before: ([RenderInfo], [FailureInfo])
        repeat {
            before = (renders, failures)
            listed = await app.ask { (try await $0.listRenders(request: $1), try await $0.listFailures(request: $1)) }
        } while listed != nil && before != (renders, failures)
        if let listed {
            (renders, failures) = listed
            if read == .unread {
                read = .read
                log.debug("history: read")
            }
        } else if read == .unread {
            log.error("the history could not be read: nothing is pruned or deleted by it")
        }
        if let measured = await app.ask({ try await $0.renderSizes(request: $1) }) { sizes = measured }
        await refreshUnstacked()
        await refreshDeleted()
    }

    func refreshDeleted() async {
        if let listed = await app.ask({ try await $0.deleted(request: $1) }) {
            deleted = listed
        }
    }

    /// In bytes.
    var deletedBytes: UInt64 {
        deleted.reduce(0) { $0 + $1.size }
    }

    /// They change whenever another stack goes on screen.
    func refreshUnstacked() async {
        if let listed = await app.ask({ try await $0.listUnstacked(request: $1) }) {
            unstacked = listed
        }
    }

    /// Delete whole stacks into Recently Deleted; frames' files are not
    /// touched. What went, or `nil` when nothing did; a part failure is
    /// said, and what went before it is returned for its Undo.
    func deleteStacks(_ stacks: [String]) async -> Deletion? {
        guard let engine = app.engine else { return nil }
        var went: [String] = []
        var records: [DeletedInfo] = []
        for stack in stacks {
            let renders = self.renders.filter { $0.stack == stack }.map(\.id)
            let failures = self.failures.filter { $0.stack == stack }.map(\.id)
            do {
                records += try await engine.deleteStack(stack: stack, renders: renders, failures: failures)
                went.append(stack)
            } catch {
                app.report(error)
                break
            }
        }
        guard !went.isEmpty else { return nil }
        let going = Set(went)
        let renderIDs = renders.filter { going.contains($0.stack) }.map(\.id)
        if let shown = shownFailure,
            failures.contains(where: { $0.id == shown && going.contains($0.stack) })
        {
            shownFailure = nil
        }
        apply(thumbs.deleted(ids: renderIDs))
        renders.removeAll { going.contains($0.stack) }
        failures.removeAll { going.contains($0.stack) }
        unstacked.removeAll { going.contains($0.stack) }
        await refreshDeleted()
        return Deletion(stacks: went, renders: renderIDs, records: records)
    }

    /// After `AppModel.deleteLibrary`: nothing here until `refresh` lists
    /// what stayed.
    func libraryDeleted() {
        renders = []
        failures = []
        unstacked = []
        deleted = []
        sizes = [:]
        shownFailure = nil
        apply(thumbs.cleared())
    }

    /// Keep Only Starred: what it would delete; none when nothing is
    /// starred (the engine's rule).
    func unstarred(of stack: String) -> [String] {
        unstarredOf(stack: stack, renders: renders.filter { $0.stack == stack })
    }

    /// Keep a failed batch stack in `stack`, not stacked yet, reworded and
    /// shown.
    func recordFailure(
        _ job: BatchJob, stack: String, settings: StackSettings, batch: Batch, error: String
    ) async {
        let (label, frames) = (job.label, job.frames)
        if let failure = await app.ask({ engine, _ in
            try await engine.saveFailure(
                label: label, frames: frames, settings: settings, stack: stack, batch: batch,
                error: error)
        }) {
            failures.removeAll { $0.id == failure.id }
            failures.append(failure)
        }
    }

    /// Keep each of a batch's `jobs` as it starts as a failed stack saying
    /// `why`, hidden until it fails or the run ends, so a run iOS ended
    /// leaves them to try again.
    func recordUnstacked(
        _ jobs: [BatchJob], stacks: [String], settings: StackSettings, batch: Batch, why: String
    ) async {
        _ = await app.ask({ engine, _ in
            try await engine.recordUnstacked(
                jobs: jobs, stacks: stacks, settings: settings, batch: batch, why: why)
        })
    }

    /// The batch ended: its unstacked stacks join the failures, read on the
    /// store thread after the run's writes, so in the background too.
    func batchEnded() async {
        if let listed = await app.ask({ engine, _ in try await engine.batchEnded() }) {
            failures = listed
        }
    }

    /// Delete renders and failures into Recently Deleted (one engine call).
    /// What went, with the working version's `base` now (its stack's newest
    /// left if `base` went), or `nil` when the engine could not.
    func delete(_ ids: [String], base: String?) async -> (Deletion, base: String?)? {
        guard let deleted = await app.ask({ engine, _ in try await engine.deleteRenders(ids: ids, base: base) })
        else { return nil }
        let failed = Set(failures.map(\.id))
        let renderIDs = ids.filter { !failed.contains($0) }
        failures.removeAll { ids.contains($0.id) }
        if let shown = shownFailure, ids.contains(shown) { shownFailure = nil }
        renders.removeAll { ids.contains($0.id) }
        apply(thumbs.deleted(ids: renderIDs))
        await refreshDeleted()
        return (Deletion(stacks: [], renders: renderIDs, records: deleted.records), deleted.base)
    }

    /// A blank name goes back to its folder's.
    func rename(_ render: RenderInfo, to name: String) async {
        let id = render.id
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        guard await app.ask({ engine, _ in try await engine.renameRender(id: id, name: trimmed.isEmpty ? nil : trimmed) })
            != nil
        else { return }
        await refresh()
    }

    func togglePicked(_ render: RenderInfo) async {
        let (id, picked) = (render.id, !render.picked)
        guard await app.ask({ engine, _ in try await engine.setRenderPicked(id: id, picked: picked) }) != nil
        else { return }
        if let index = renders.firstIndex(where: { $0.id == id }) {
            renders[index].picked = picked
        }
    }

    func thumbnailAppeared(_ id: String) {
        inView.append(id)
        wantInView()
    }

    func thumbnailDisappeared(_ id: String) {
        if let index = inView.lastIndex(of: id) { inView.remove(at: index) }
        wantInView()
    }

    /// Memory is short: keep only the thumbnails in view.
    func shedThumbnails() {
        apply(thumbs.shed())
    }

    /// Latest shown first.
    private func wantInView() {
        var seen: Set<String> = []
        apply(thumbs.wanted(ids: inView.reversed().filter { seen.insert($0).inserted }))
    }

    /// `read` is the thumbnail just read, for a `keep` of it.
    private func apply(_ actions: [ThumbnailAction], read: (id: String, image: CGImage)? = nil) {
        for action in actions {
            switch action {
            case .read(let token, let ids):
                startRead(token, ids)
            case .keep(let id):
                if let read, read.id == id { thumbnails[id] = read.image }
            case .drop(let id):
                thumbnails[id] = nil
            }
        }
    }

    /// Read `ids`' thumbnails in turn, telling the policy as each lands; an
    /// unreadable one leaves the placeholder.
    private func startRead(_ token: UInt64, _ ids: [String]) {
        guard let engine = app.engine else {
            apply(thumbs.finished(token: token))
            return
        }
        Task {
            var stopped = false
            for id in ids {
                let image: CGImage?
                do {
                    image = try await Request().run { try await engine.renderThumbnail(request: $0, id: id) }
                        .makeCGImage()
                } catch EngineError.Canceled {
                    // Stopped as the app left: not failed; those in view
                    // are asked again once back.
                    stopped = true
                    break
                } catch {
                    image = nil
                }
                apply(thumbs.read(token: token, id: id, ok: image != nil), read: image.map { (id, $0) })
            }
            apply(thumbs.finished(token: token))
            if stopped { app.stopped { self.wantInView() } }
        }
    }
}

/// What a delete took, for the notice and its Undo.
struct Deletion {
    let stacks: [String]
    let renders: [String]
    let records: [DeletedInfo]
}
