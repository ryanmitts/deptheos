import Foundation
@preconcurrency import DeptheosEngine

private nonisolated let log = AppLog(category: "files")

/// A file or folder as Files' picker handed it over. Only that URL carries
/// the security scope (one remade from it opens nothing), so a bookmark is
/// made only of a `Picked` (`AppModel.keep`).
nonisolated struct Picked: Sendable {
    /// As Files gave it, never remade.
    let url: URL
    /// Only automation's paths have no scope.
    let scoped: Bool

    init(fromFiles url: URL) {
        self.url = url
        scoped = true
    }

    #if DEBUG
    /// A plain path standing in for a pick (`-PickPath`, on the simulator):
    /// no scope, readable as is, no bookmark.
    init(automation path: String) {
        url = URL(fileURLWithPath: path)
        scoped = false
    }
    #endif

    /// As the list and the engine name it.
    var path: String { url.standardizedFileURL.path }
    var name: String { url.lastPathComponent }

    /// Made with its scope open, for the engine to keep with its root in
    /// `sources.json` (ADR-145); `nil` when the scope didn't open or iOS
    /// made none.
    var bookmark: Data? {
        guard url.startAccessingSecurityScopedResource() else {
            log.error("a picked item's scope did not open")
            return nil
        }
        defer { url.stopAccessingSecurityScopedResource() }
        do {
            return try url.bookmarkData()
        } catch {
            log.error("a picked item could not be bookmarked: \(error.localizedDescription, privacy: .public)")
            return nil
        }
    }
}

/// The security scopes of the Library's roots (ADR-145). Bookmarks live in
/// `sources.json` and come with each `RootInfo`; none is kept here.
///
/// A scope opens when something reads under it, through a `Lease`
/// (`AppModel.access`), and stays open so going back to a stack is free.
/// Only past `openLimit` are the least recently used closed, never one a
/// lease holds: iOS allows a few hundred scopes and then fails every one
/// until restart. Only scopes started here are stopped here.
final class Bookmarks {
    /// A root's scope.
    ///
    /// Lifecycle: closed, open (through its URL, last used when) or
    /// unreachable. Held in `scopes`, changed only by `lease` and `evict`
    /// (through `setAccess`).
    ///
    ///     closed ──leased, its bookmark resolved and opened──▶ open ──evicted (over the limit, no lease holds it)──▶ closed
    ///     closed ──leased, not resolved in time, or not opened──▶ unreachable ──leased──▶ open, or unreachable
    enum Access {
        case closed
        /// Through the bookmark's own URL (a plain file URL opens nothing),
        /// last needed at `used`.
        case open(URL, used: UInt64)
        /// Not resolved or not opened: a drive unplugged, a share offline,
        /// access revoked. Tried again by the next lease.
        case unreachable

        var name: String {
            switch self {
            case .closed: "closed"
            case .open: "open"
            case .unreachable: "unreachable"
            }
        }
    }

    private enum Outcome: Sendable {
        case open(root: String, url: URL, fresh: Data?)
        case failed(root: String, reason: String)

        var root: String {
            switch self {
            case .open(let root, _, _), .failed(let root, _): root
            }
        }
    }

    /// The system's limit is undocumented (a few hundred on an iPhone); a
    /// stack needs one, a batch one per root.
    static let openLimit = 32
    /// After this, bookmarks not back are given up, so a silent share holds
    /// up nothing.
    static let resolveWithin = Duration.seconds(5)

    private var scopes: [String: Access] = [:]
    /// Counts leases, to order scopes by last need.
    private var clock: UInt64 = 0
    /// Roots held open, by how many leases hold each.
    private var held: [String: Int] = [:]

    /// Open the scopes of `roots` and hold them until the lease drops.
    /// Returns it with where each bookmark opened now led (a stale one
    /// remade), for `Engine.findAgain`. Resolved off the main actor within
    /// `resolveWithin`, never mounting a volume or asking anything.
    func lease(_ roots: [RootInfo]) async -> (Lease, [BookmarkResolution]) {
        clock += 1
        let ids = Set(roots.map(\.root))
        for id in ids { held[id, default: 0] += 1 }
        var wanted: [(root: String, data: Data)] = []
        var asked = Set<String>()
        for root in roots where asked.insert(root.root).inserted {
            if case .open? = scopes[root.root] {
                touch(root.root)
            } else if let data = root.bookmark {
                wanted.append((root.root, data))
            }
        }
        var resolutions: [BookmarkResolution] = []
        if !wanted.isEmpty {
            let outcomes = await Self.resolve(wanted, within: Self.resolveWithin)
            for outcome in outcomes {
                switch outcome {
                case .failed(let root, let reason):
                    log.info("a bookmark did not open: \(reason, privacy: .public)")
                    if !isOpen(root) { setAccess(root, .unreachable) }
                case .open(let root, let url, let fresh):
                    // Opened by another lease meanwhile: this one is extra.
                    guard !isOpen(root) else {
                        url.stopAccessingSecurityScopedResource()
                        continue
                    }
                    setAccess(root, .open(url, used: clock))
                    resolutions.append(BookmarkResolution(root: root, path: url.path, fresh: fresh))
                }
            }
            let answered = Set(outcomes.map(\.root))
            for (root, _) in wanted where !answered.contains(root) && !isOpen(root) {
                log.info("a bookmark was not resolved in time")
                setAccess(root, .unreachable)
            }
        }
        evict()
        let open = Set(ids.filter { isOpen($0) })
        return (Lease(roots: Array(ids), open: open, store: self), resolutions)
    }

    fileprivate func release(_ roots: [String]) {
        for root in roots {
            guard let count = held[root] else { continue }
            held[root] = count > 1 ? count - 1 : nil
        }
        evict()
    }

    var openCount: Int { scopes.keys.count(where: { isOpen($0) }) }

    /// The app's own Documents need no scope.
    nonisolated static func isOwned(_ url: URL) -> Bool {
        guard let documents = try? FileManager.default.url(
            for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: false)
        else { return false }
        let path = url.resolvingSymlinksInPath().standardizedFileURL.path
        let root = documents.resolvingSymlinksInPath().standardizedFileURL.path
        return path == root || path.hasPrefix(root + "/")
    }

    // MARK: Inside

    private func isOpen(_ root: String) -> Bool {
        if case .open? = scopes[root] { true } else { false }
    }

    private func touch(_ root: String) {
        if case .open(let url, _)? = scopes[root] { scopes[root] = .open(url, used: clock) }
    }

    private func setAccess(_ root: String, _ access: Access) {
        scopes[root] = access
        log.debug("scope: \(access.name, privacy: .public)")
    }

    /// Close the least recently needed past the limit, none a lease holds.
    private func evict() {
        let over = openCount - Self.openLimit
        guard over > 0 else { return }
        var open: [(root: String, url: URL, used: UInt64)] = scopes.compactMap { root, access in
            if case .open(let url, let used) = access, held[root] == nil { (root, url, used) } else { nil }
        }
        open.sort { $0.used < $1.used }
        for (root, url, _) in open.prefix(over) {
            url.stopAccessingSecurityScopedResource()
            setAccess(root, .closed)
        }
    }

    /// Resolve each of `wanted` and open its scope, each on its own thread
    /// so a hung share holds up no other, all within `within`. One back too
    /// late is closed again.
    ///
    /// Waited for in its own task, out of the caller's cancel: a canceled
    /// wait would leave scopes open with no one to close them.
    private nonisolated static func resolve(
        _ wanted: [(root: String, data: Data)], within: Duration
    ) async -> [Outcome] {
        await Task.detached(priority: .userInitiated) {
            await collect(wanted, within: within)
        }.value
    }

    private nonisolated static func collect(
        _ wanted: [(root: String, data: Data)], within: Duration
    ) async -> [Outcome] {
        let (stream, continuation) = AsyncStream.makeStream(of: Outcome.self)
        for (root, data) in wanted {
            DispatchQueue.global(qos: .userInitiated).async {
                let outcome = resolveOne(root: root, data: data)
                if case .terminated = continuation.yield(outcome), case .open(_, let url, _) = outcome {
                    url.stopAccessingSecurityScopedResource()
                }
            }
        }
        let timer = Task {
            try? await Task.sleep(for: within)
            continuation.finish()
        }
        var outcomes: [Outcome] = []
        for await outcome in stream {
            outcomes.append(outcome)
            if outcomes.count == wanted.count { break }
        }
        timer.cancel()
        continuation.finish()
        return outcomes
    }

    /// Never mounting a volume or asking: an absent share is unreachable,
    /// not waited for. A stale bookmark is remade where it led.
    private nonisolated static func resolveOne(root: String, data: Data) -> Outcome {
        var stale = false
        let url: URL
        do {
            url = try URL(
                resolvingBookmarkData: data, options: [.withoutUI, .withoutMounting],
                relativeTo: nil, bookmarkDataIsStale: &stale)
        } catch {
            return .failed(root: root, reason: error.localizedDescription)
        }
        guard url.startAccessingSecurityScopedResource() else {
            return .failed(root: root, reason: "iOS didn't give Deptheos access.")
        }
        return .open(root: root, url: url, fresh: stale ? try? url.bookmarkData() : nil)
    }
}

/// Some frames' roots held open from `AppModel.access` until dropped
/// (ADR-145); readers hold one while they read. Dropped, the scopes stay
/// open until the store needs room (`Bookmarks.evict`).
nonisolated final class Lease: Sendable {
    /// For the app's own frames, or before the engine.
    static let none = Lease(roots: [], open: [], store: nil)

    private let roots: [String]
    private let open: Set<String>
    private let store: Bookmarks?

    fileprivate init(roots: [String], open: Set<String>, store: Bookmarks?) {
        self.roots = roots
        self.open = open
        self.store = store
    }

    func opened(_ root: String) -> Bool { open.contains(root) }

    deinit {
        guard let store, !roots.isEmpty else { return }
        let roots = roots
        Task { @MainActor in store.release(roots) }
    }
}
