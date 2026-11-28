import Foundation
@preconcurrency import DeptheosEngine

// The Library's model: the engine works out the tiles a filter and search
// let through (`libraryTiles`, ADR-144); here they are matched with their
// renders and failures.

extension LibraryFilter: @retroactive CaseIterable, @retroactive Identifiable {
    public static var allCases: [LibraryFilter] { [.all, .starred, .failed] }
    public var id: Self { self }

    /// In the iPhone's filter picker.
    var name: LocalizedStringResource {
        switch self {
        case .all:
            LocalizedStringResource("All Stacks", comment: "Library filter (iPhone picker): every stack.")
        case .starred:
            LocalizedStringResource("Starred", comment: "Library filter (iPhone picker): stacks with a starred render.")
        case .failed:
            LocalizedStringResource("Failed", comment: "Library filter (iPhone picker): stacks whose batch stack didn't finish.")
        }
    }
}

/// The key of `AppModel.libraryMemo`: the filter, the query, the history's
/// revision, the stacks not stacked yet, the day (Today and Yesterday move
/// at midnight) and the count line's `size`.
struct LibraryKey: Equatable {
    let history: Int
    let unstacked: [UnstackedInfo]
    let filter: LibraryFilter
    let query: String
    let size: String?
    let today: Date
}

/// The engine's tiles, each with its renders or failure.
struct LibraryShown {
    let unstacked: [UnstackedInfo]
    let days: [LibraryDaySection]
    /// Jump To's spans that have stacks.
    let spans: [LibraryJump]
    let failed: [String]
    /// For the counts and the count line.
    let tiles: LibraryTiles

    /// With no search.
    func count(_ filter: LibraryFilter) -> Int {
        switch filter {
        case .all: Int(tiles.all)
        case .starred: Int(tiles.starred)
        case .failed: Int(tiles.failedStacks)
        }
    }
}

/// A span of Jump To that has stacks, with the first day shown in it.
struct LibraryJump: Identifiable {
    let count: LibrarySpanCount
    let day: Date
    var id: LibrarySpan { count.span }
}

extension AppModel {
    /// A stack with neither a name nor a folder.
    nonisolated static let untitledStack = LocalizedStringResource(
        "Untitled stack", comment: "The name of a stack that has no name and no folder to be named after.")

    /// `name`, or "Untitled stack" when the engine found none.
    nonisolated static func named(_ name: String?) -> String {
        name ?? String(localized: untitledStack)
    }

    /// What `filter` and `query` let through, worked out again only when
    /// its key changes (`libraryMemo`): the view asks several times a
    /// redraw, and each would send every render across.
    func library(_ filter: LibraryFilter, query: String) -> LibraryShown {
        let key = LibraryKey(
            history: history.revision, unstacked: libraryUnstacked, filter: filter, query: query,
            size: history.renders.isEmpty ? nil : history.sizeText(history.renders.map(\.id)),
            today: Calendar.current.startOfDay(for: .now))
        return libraryMemo.answer(for: key) { key in libraryShown(key) }
    }

    private func libraryShown(_ key: LibraryKey) -> LibraryShown {
        let (filter, query) = (key.filter, key.query)
        let library = LibraryHistory(
            renders: history.renders.map {
                LibraryRender(
                    id: $0.id, stack: $0.stack, stackName: $0.stackName, name: $0.name, picked: $0.picked,
                    createdMs: $0.createdMs, batch: $0.batch)
            },
            failures: history.failures.map {
                LibraryFailure(id: $0.id, stack: $0.stack, label: $0.label, createdMs: $0.createdMs, batch: $0.batch)
            },
            unstacked: key.unstacked)
        let tiles = libraryTiles(
            history: library, filter: filter, query: query, untitled: String(localized: Self.untitledStack),
            size: key.size, words: AppWords.words)
        let renders = Dictionary(history.renders.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let failures = Dictionary(history.failures.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let days = tiles.days.map { LibraryDaySection($0, renders: renders, failures: failures) }
        let shown = LibraryShown(
            unstacked: tiles.unstacked, days: days,
            spans: tiles.spans.compactMap { span in span.first.map { LibraryJump(count: span, day: days[Int($0)].id) } },
            failed: tiles.failed, tiles: tiles)
        return shown
    }

    /// Stacks with frames and no render, the one on screen among them (the
    /// engine lists the others).
    var libraryUnstacked: [UnstackedInfo] {
        var stacks = history.unstacked.filter { $0.stack != stackID }
        if !list.files.isEmpty, !history.renders.contains(where: { $0.stack == stackID }) {
            stacks.append(UnstackedInfo(stack: stackID, name: stackName, frames: UInt32(rows.count)))
        }
        return stacks
    }

    /// Nothing to show in the Library at all.
    var libraryIsEmpty: Bool {
        history.renders.isEmpty && history.failures.isEmpty && libraryUnstacked.isEmpty
    }

    /// Moved into Recently Deleted at once, the notice offering Undo
    /// (ADR-140).
    func deleteRenders(_ ids: [String]) async {
        guard !ids.isEmpty, !busy, let (deletion, baseNow) = await history.delete(ids, base: base) else { return }
        let renderIDs = deletion.renders
        paint.remove(renderIDs)
        if let adjusted = adjust.renderID, renderIDs.contains(adjusted) { stopAdjusting() }
        // The base was deleted: the newest left in its stack is, so Edited
        // and Revert still have one.
        if baseNow != base {
            base = baseNow
            saveList()
        }
        // The deleted picture goes at once; no result is shown until the
        // base's lands.
        if let shown = result?.renderID, ids.contains(shown) {
            setResult(nil)
            await showOwnResult()
        }
        offerUndo(deletion.records)
    }

    /// Delete whole stacks, everything of each into Recently Deleted, the
    /// notice offering Undo. The stack on screen among them gives way to an
    /// empty new stack.
    func deleteStacks(_ stacks: [String]) async {
        guard !stacks.isEmpty, !busy else { return }
        let fresh = newWorking(settings: working.settings)
        guard let deletion = await history.deleteStacks(stacks) else { return }
        paint.remove(deletion.renders)
        if deletion.stacks.contains(stackID) {
            leaveMode()
            viewGeneration += 1
            setResult(nil)
            setPreviewLoad(.none)
            showPlainFrame()
            edit(fresh)
        }
        offerUndo(deletion.records)
    }
}

/// "12 stacks".
nonisolated func stacksCount(_ count: Int) -> LocalizedStringResource {
    LocalizedStringResource(
        "\(count) stacks",
        comment: "A count of stacks: beside a day's heading, over Not Stacked Yet, on a batch card, under a folder in Settings ▸ Storage ▸ Folders. The placeholder is how many.")
}

/// "68 renders".
nonisolated func rendersCount(_ count: Int) -> LocalizedStringResource {
    LocalizedStringResource(
        "\(count) renders",
        comment: "A count of renders: over the strip of the stack's renders, in a Library tile's accessibility label. The placeholder is how many; zero reads 'No renders'.")
}

// MARK: Tiles

/// A Library tile: a stack with the renders the filter and search let
/// through, or a render-less stack's failed run.
struct LibraryStack: Identifiable {
    let stack: String
    let content: Content
    /// All the stack's renders.
    let total: Int
    /// "4", or "2 of 4" when the filter or search lets two through.
    let badge: String?

    enum Content {
        /// Newest first (at least one), and the cover: the newest starred,
        /// else the newest.
        case renders([RenderInfo], cover: RenderInfo)
        case failure(FailureInfo)
    }

    var id: String { stack }

    var failed: Bool {
        if case .failure = content { true } else { false }
    }

    /// `nil` when its render or failure has gone meanwhile.
    init?(_ tile: LibraryTile, renders: [String: RenderInfo], failures: [String: FailureInfo]) {
        switch tile.kind {
        case .renders(let cover, let shown, let total):
            guard let cover = renders[cover] else { return nil }
            content = .renders(shown.compactMap { renders[$0] }, cover: cover)
            self.total = Int(total)
        case .failed(let failure):
            guard let failure = failures[failure] else { return nil }
            content = .failure(failure)
            total = 0
        }
        stack = tile.stack
        badge = tile.badge
    }
}

/// The stacks a batch made, in the order it made them.
struct LibraryBatchRun: Identifiable {
    let id: String
    let batch: Batch
    let entries: [LibraryStack]
    /// When the oldest shown was made.
    let started: Date
    let failed: Int
}

/// A day's tiles, newest first.
struct LibraryDaySection: Identifiable {
    let id: Date
    /// "Today", "Yesterday", or the date.
    let title: UserText
    let stackCount: Int
    /// Loose stacks next to each other share a grid; a batch is a card.
    let groups: [Group]

    enum Group: Identifiable {
        case loose(String, [LibraryStack])
        case batch(LibraryBatchRun)

        var id: String {
            switch self {
            case .loose(let id, _): "loose-\(id)"
            case .batch(let run): run.id
            }
        }
    }

    /// "7 stacks".
    var countText: LocalizedStringResource { stacksCount(stackCount) }

    init(_ day: LibraryDay, renders: [String: RenderInfo], failures: [String: FailureInfo]) {
        let date = Date(timeIntervalSince1970: Double(day.day))
        id = date
        title = Self.title(of: date)
        stackCount = Int(day.stacks)
        let entries = { (tiles: [LibraryTile]) in
            tiles.compactMap { LibraryStack($0, renders: renders, failures: failures) }
        }
        groups = day.runs.map { run in
            switch run {
            case .loose(let tiles):
                .loose(tiles.first?.stack ?? "", entries(tiles))
            case .batch(let batch, let tiles, let startedMs, let failed):
                .batch(LibraryBatchRun(
                    id: batch.id, batch: batch, entries: entries(tiles),
                    started: Date(timeIntervalSince1970: Double(startedMs) / 1000), failed: Int(failed)))
            }
        }
    }

    private static func title(of day: Date) -> UserText {
        let calendar = Calendar.current
        if calendar.isDateInToday(day) {
            return .app(LocalizedStringResource("Today", comment: "Library, the heading over the stacks made today."))
        }
        if calendar.isDateInYesterday(day) {
            return .app(LocalizedStringResource(
                "Yesterday", comment: "Library, the heading over the stacks made yesterday."))
        }
        // Formatted in the user's locale already.
        return .verbatim(day.formatted(.dateTime.weekday(.wide).day().month(.wide)))
    }
}

extension RenderInfo {
    /// Its name, else its folder's, else "Untitled stack".
    nonisolated var shownStackName: String { AppModel.named(stackName) }

    /// Its name, else its folder's, else "Untitled stack".
    nonisolated var shownTitle: String { AppModel.named(title) }
}

extension UnstackedInfo {
    /// Its name, else its folder's, else "Untitled stack".
    nonisolated var shownName: String { AppModel.named(name) }
}

extension Batch {
    /// "Auto sort · 9 frames skipped", in the app's language.
    var shownLabel: String { AppWords.words.batchLabel(batch: self) }
}

// MARK: Delete Library

private let deleteLog = AppLog(category: "app")

/// Whether Delete Library… can run now, and if not why; derived, never
/// stored.
enum LibraryDeletable: Equatable {
    /// The history hasn't been read: what it holds isn't known.
    case unread
    /// Nothing is in it.
    case empty
    /// A stack, batch, open or Retouch holds the app.
    case busy
    case ready(stacks: Int, renders: Int)
}

extension AppModel {
    /// Not while anything runs or opens, in Retouch, or when empty.
    var libraryDeletable: LibraryDeletable {
        guard history.read == .read else { return .unread }
        if libraryIsEmpty { return .empty }
        if busy || !viewing { return .busy }
        return .ready(stacks: library(.all, query: "").count(.all), renders: history.renders.count)
    }

    /// Delete the whole Library (Settings asks first): every stack and the
    /// working version, which an empty stack replaces; settings stay. The
    /// frames stay; Imports copies nothing uses become Storage's unused
    /// copies. Rechecks `libraryDeletable`, since a stack may have started.
    /// What went wrong, if not all of it went.
    func deleteLibrary() async -> ErrorMessage? {
        guard case .ready = libraryDeletable, let engine else { return nil }
        let ids = history.renders.map(\.id)
        let fresh = newWorking(settings: working.settings)
        deleteLog.info("deleting the Library: \(ids.count) renders, \(self.history.failures.count) failed stacks")
        let failure: ErrorMessage?
        do {
            try await engine.deleteLibrary()
            failure = nil
        } catch {
            var message = ErrorMessage(Self.message(error))
            message.title = .verbatim(AppWords.words.libraryDeleteFailed())
            failure = message
        }
        // As at a first launch, whatever stayed: what did is listed again.
        paint.remove(ids)
        leaveMode()
        viewGeneration += 1
        setResult(nil)
        setPreviewLoad(.none)
        showPlainFrame()
        history.libraryDeleted()
        // Recently Deleted went with it: nothing is left to undo.
        setDeleteUndo(.none)
        edit(fresh)
        await history.refresh()
        return failure
    }
}
