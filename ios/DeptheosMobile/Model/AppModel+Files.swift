import CoreGraphics
import Foundation
@preconcurrency import DeptheosEngine

private let log = AppLog(category: "files")
/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// A frame's thumbnail in the Frames list.
///
/// Lifecycle: none until wanted with its file there, loading (its task),
/// shown or failed (the file's `FileStamp`). Changed only by
/// `AppModel.setFrameThumb`.
///
///     (none) ──wanted, its file there──▶ loading ──read──▶ shown
///                                           └──unreadable──▶ failed
///     failed ──its file changed (`checkFiles`)──▶ (none)
///     any ──its row gone──▶ (none), a load canceled
enum FrameThumb {
    case loading(Task<Void, Never>)
    case shown(CGImage)
    /// `nil` when the file's stamp couldn't be read.
    case failed(FileStamp?)

    var name: String {
        switch self {
        case .loading: "loading"
        case .shown: "shown"
        case .failed: "failed"
        }
    }
}

/// What tells a file changed: its modification date and size.
nonisolated struct FileStamp: Equatable, Sendable {
    let modified: Date?
    let size: Int?

    /// `nil` if it can't be read.
    static func of(_ path: String) -> FileStamp? {
        let keys: Set<URLResourceKey> = [.contentModificationDateKey, .fileSizeKey]
        guard let values = try? URL(fileURLWithPath: path).resourceValues(forKeys: keys) else { return nil }
        return FileStamp(modified: values.contentModificationDate, size: values.fileSize)
    }
}

/// Reaching frames picked in Files (ADR-145): their roots' scopes, the
/// roots found again, and whether each frame is there.
extension AppModel {
    /// Hold open the scopes of the roots `files` are under until the lease
    /// is dropped. A root whose bookmark led elsewhere is found there
    /// (`lease(_:)`). Background work passes its lasting `request`, since
    /// one in front is refused there.
    func access(_ files: [String], under request: Request = Request()) async -> Lease {
        guard !files.isEmpty,
              let found = await ask(under: request, { try await $0.rootsOf(request: $1, paths: files) })
        else { return .none }
        var seen = Set<String>()
        return await lease(found.compactMap { $0 }.filter { seen.insert($0.root).inserted })
    }

    /// Hold open the scopes of `roots`. Where their bookmarks led goes to
    /// the engine, which finds roots that moved and remakes stale
    /// bookmarks; the app follows (`relinked`). A folder that only partly
    /// matches is never offered: its root stays missing until Locate….
    func lease(_ roots: [RootInfo]) async -> Lease {
        guard let engine, let bookmarks, !roots.isEmpty else { return .none }
        let (lease, resolutions) = await bookmarks.lease(roots)
        guard !resolutions.isEmpty else { return lease }
        do {
            let moved = try await engine.findAgain(resolutions: resolutions)
            await relinked(moved)
        } catch {
            log.error("the roots couldn't be looked for: \(Self.message(error).string, privacy: .public)")
        }
        return lease
    }

    /// At launch, once the Library is read: every root's bookmark is
    /// resolved and its scope opened, so moved roots are found. The scopes
    /// stay open until the store needs room.
    func findRootsAgain() async {
        guard let roots = await ask({ try await $0.bookmarkedRoots(request: $1) }) else { return }
        // How many, never which: a path names the user's folders.
        log.info("folders read where they are: \(roots.count)")
        _ = await lease(roots)
    }

    /// Keep `picked` readable now and after a restart (ADR-145): its
    /// bookmark goes with its root in `sources.json`. Missing roots on its
    /// drive or share are looked for under it first, so picking a drive's
    /// top folder brings back every root on it. `false` when it couldn't
    /// be kept; the caller leaves it out and says so (`accessNotKept`).
    func keep(_ picked: Picked) async -> Bool {
        guard let engine else { return false }
        guard picked.scoped, !Bookmarks.isOwned(picked.url) else { return true }
        guard let bookmark = picked.bookmark else { return false }
        let path = picked.path
        // Open while the engine looks under it.
        let scope = PickedScopes.opening([picked.url])
        do {
            let moved = try await engine.keepFolder(folder: path, bookmark: bookmark)
            withExtendedLifetime(scope) {}
            await relinked(moved)
            return true
        } catch {
            log.error("a picked folder could not be kept: \(Self.message(error).string, privacy: .public)")
            return false
        }
    }

    /// Alert text when a folder picked in Files can't be kept readable.
    nonisolated static func accessNotKept(_ name: String) -> LocalizedStringResource {
        LocalizedStringResource(
            "Deptheos couldn't keep access to \"\(name)\", so it wasn't used. Pick it in Files again.",
            comment: "Alert text after a folder or file is picked in Files: iOS did not let Deptheos keep reading it. The placeholder is its name.")
    }

    // MARK: The list's frames

    /// Look again at which frames of the list are there, their scopes held
    /// open (`listLease`), then load thumbnails. Off the main actor: a
    /// share or drive can be slow. A frame whose drive or share is absent
    /// is not connected rather than missing (`Engine.reach`).
    func checkFiles() {
        let check = fileChecks.next()
        let files = rows.map(\.file)
        let failed = frameThumbs.compactMap { file, thumb -> (String, FileStamp?)? in
            if case .failed(let stamp) = thumb { (file, stamp) } else { nil }
        }
        Task {
            let lease = await access(files)
            guard fileChecks.isCurrent(check) else { return }
            // Held until the list is looked at again.
            listLease = lease
            guard let (found, changed) = try? await Blocking.run(.interactive, {
                      let states = Dictionary(uniqueKeysWithValues: files.compactMap { file in
                          FrameFiles.state(of: file).map { (file, $0) }
                      })
                      // The thumbnails that failed, of files changed since.
                      let changed = failed.filter { FileStamp.of($0.0) != $0.1 }.map(\.0)
                      return (states, changed)
                  }),
                  let (roots, reach) = await ask({ engine, request in
                      (try await engine.rootsOf(request: request, paths: files),
                       try await engine.reach(request: request, paths: files))
                  }),
                  fileChecks.isCurrent(check)
            else { return }
            var states = found
            for (index, reach) in reach.enumerated() where reach == .notConnected {
                states[files[index]] = .offline(roots[index].flatMap(Place.of))
            }
            let fresh = states
            var seen = Set<String>()
            listRoots = roots.compactMap { $0 }.filter { seen.insert($0.root).inserted }
            unavailable = fresh
            for file in changed {
                if case .failed? = frameThumbs[file] { setFrameThumb(file, nil) }
            }
            loadThumbnails()
        }
    }

    /// Load each missing thumbnail unless its file isn't here: reading an
    /// iCloud one would download every frame at once. A failed one waits
    /// for its file to change.
    func loadThumbnails() {
        for row in rows where frameThumbs[row.file] == nil && unavailable[row.file] == nil {
            let file = row.file
            let task = Task {
                let read = try? await Blocking.run(urgent: false) {
                    let image = ImagePreview.load(URL(fileURLWithPath: file), maxPixelSize: 120)
                    return (image, image == nil ? FileStamp.of(file) : nil)
                }
                // Its row gone meanwhile: canceled, and nothing applied.
                guard !Task.isCancelled, case .loading? = frameThumbs[file], let (image, stamp) = read
                else { return }
                setFrameThumb(file, image.map(FrameThumb.shown) ?? .failed(stamp))
            }
            setFrameThumb(file, .loading(task))
        }
    }

    func frameThumbnail(_ file: String) -> CGImage? {
        if case .shown(let image)? = frameThumbs[file] { image } else { nil }
    }

    /// The only place `frameThumbs` changes; a load left is canceled.
    func setFrameThumb(_ file: String, _ thumb: FrameThumb?) {
        let before = frameThumbs[file]
        if case .loading(let task)? = before { task.cancel() }
        stateLog.debug("frame thumbnail: \(before?.name ?? "none", privacy: .public) -> \(thumb?.name ?? "none", privacy: .public)")
        frameThumbs[file] = thumb
    }

    /// Where the list's frames are when used in place on a drive, a server
    /// or iCloud Drive ("On T7 SSD").
    var framesPlace: Place? {
        listRoots.first.flatMap(Place.of)
    }

    /// What the unreachable frames are on, when known.
    var offlinePlace: Place? {
        for case .offline(let place?) in unavailable.values { return place }
        return nil
    }

    /// Before a stack, every frame is opened and its first byte read, a
    /// cloud one downloaded first, so an unreadable frame stops the stack
    /// now rather than leaving it out of the render. `nil` when all can be
    /// read or the stack was canceled. Through `Blocking`, one at a time.
    func preflight(_ paths: [String], request: Request, cancel: FetchCancel) async -> UserText? {
        let total = UInt32(paths.count)
        var failures: [(path: String, reason: String)] = []
        for (index, path) in paths.enumerated() {
            do {
                // Only a download is worth showing.
                if try await Blocking.run(.interactive, { FrameFiles.state(of: path) == .inCloud }) {
                    showFetching(UInt32(index + 1), of: total, request: request)
                }
                if let reason = try await Blocking.run(.interactive, { try FrameFiles.unreadable(path, cancel: cancel) }) {
                    failures.append((path, reason))
                }
            } catch {
                return nil
            }
        }
        guard let first = failures.first else { return nil }
        log.info("stack not started: \(failures.count) of \(paths.count) frames can't be read: \(first.reason, privacy: .public)")
        // They are here now, or known not to be.
        checkFiles()
        let unread = failures.map(\.path)
        let reach = await ask { try await $0.reach(request: $1, paths: unread) } ?? []
        if let away = reach.firstIndex(of: .notConnected) {
            let count = failures.count
            let root = await ask { try await $0.rootsOf(request: $1, paths: [unread[away]]) }?.first ?? nil
            if let place = root.flatMap(Place.of) { return .app(place.cantBeReached(count)) }
            return .app(LocalizedStringResource(
                "\(count) frames can't be reached. Connect the drive or server, or use Locate… if the folder has moved.",
                comment: "Alert text when Stack is tapped: frames are on a drive or server that isn't connected, its name unknown. The placeholder is how many frames."))
        }
        // The reason is the system's own sentence.
        let name = (first.path as NSString).lastPathComponent
        if failures.count == 1 {
            return .app(LocalizedStringResource(
                "\"\(name)\" can't be read. \(first.reason)",
                comment: "Alert text when Stack is tapped: a frame's file can't be read. The placeholders are the file's name and the system's sentence saying why."))
        }
        return .app(LocalizedStringResource(
            "\(failures.count) frames can't be read, starting with \"\(name)\". \(first.reason)",
            comment: "Alert text when Stack is tapped: frames' files can't be read. The placeholders are how many, the first one's file name and the system's sentence saying why."))
    }

    // MARK: Relinking

    /// Locate…: the folder `old` is now at `picked`. Its bookmark is made
    /// first, and nothing moves when none can be. The engine matches the
    /// frames under `old` there and points the folder at it, saying what it
    /// found ("Found 12 of 14 frames in "Bee""); the app follows
    /// (`relinked`). A drive's top folder brings back every root on it.
    /// Whether it was pointed there.
    @discardableResult
    func relocate(old: String, to picked: Picked) async -> Bool {
        guard let engine else { return false }
        var bookmark: Data?
        if picked.scoped, !Bookmarks.isOwned(picked.url) {
            guard let made = picked.bookmark else {
                errorMessage = ErrorMessage(.app(Self.accessNotKept(picked.name)))
                return false
            }
            bookmark = made
        }
        let new = picked.path
        // Open while the engine matches the frames under it.
        let scope = PickedScopes.opening([picked.url])
        let located: LocatedInfo
        do {
            located = try await engine.locate(old: old, new: new, bookmark: bookmark)
            withExtendedLifetime(scope) {}
        } catch {
            errorMessage = ErrorMessage(.app(LocalizedStringResource(
                "The renders couldn't be pointed at the new folder. \(Self.message(error).string)",
                comment: "Alert text after Locate…: the renders couldn't follow the folder picked. The placeholder is the engine's sentence saying why.")))
            return false
        }
        log.info("located \(located.found) of \(located.of) frames")
        await relinked(located.moved)
        sayLocated(located, in: picked.name)
        return true
    }

    /// All found is said in passing; some or none, until OK.
    private func sayLocated(_ located: LocatedInfo, in name: String) {
        let (found, of) = (Int(located.found), Int(located.of))
        guard of > 0 else { return }
        if found == of {
            showBriefNotice(.app(of == 1
                ? LocalizedStringResource(
                    "Found the frame in \"\(name)\".",
                    comment: "Notice after Locate…: the one frame looked for is in the folder picked. The placeholder is the folder's name.")
                : LocalizedStringResource(
                    "Found all \(of) frames in \"\(name)\".",
                    comment: "Notice after Locate…: every frame looked for is in the folder picked. The placeholders are how many and the folder's name.")))
        } else if found == 0 {
            errorMessage = ErrorMessage(.app(of == 1
                ? LocalizedStringResource(
                    "The frame wasn't found in \"\(name)\" as it was. It may have been moved, renamed or changed.",
                    comment: "Alert text after Locate…: the one frame looked for isn't in the folder picked as it was (by name, size and contents). The placeholder is the folder's name.")
                : LocalizedStringResource(
                    "None of the \(of) frames were found in \"\(name)\" as they were. They may have been moved, renamed or changed.",
                    comment: "Alert text after Locate…: none of the frames looked for are in the folder picked as they were (by name, size and contents). The placeholders are how many and the folder's name.")))
        } else {
            errorMessage = ErrorMessage(.app(LocalizedStringResource(
                "Found \(found) of \(of) frames in \"\(name)\". The rest may have been moved, renamed or changed.",
                comment: "Alert text after Locate…: some of the frames looked for are in the folder picked as they were (by name, size and contents). The placeholders are how many were found, how many were looked for and the folder's name.")))
        }
    }

    /// The one place the app follows a relink (ADR-145), whatever made it:
    /// `relocate`, `lease`, `answer`, `keep` or `copyFramesIn`. The engine
    /// has already repointed the Library's roots. Here the list and every
    /// render's paint follow `moved`, the history is read again, and the
    /// render shown is reopened (`reopenShown`), since what is open holds
    /// its frames' old paths.
    func relinked(_ moved: [FolderMove]) async {
        guard !moved.isEmpty else { return }
        log.info("following \(moved.count) folders moved")
        followList(moved)
        paint.relink(moved)
        await history.refresh()
        await reopenShown()
    }

    /// The selected frame, if it moved with its folder, stays selected.
    private func followList(_ moved: [FolderMove]) {
        guard let engine else { return }
        let selected = selectedFile
        setList(listFollow(list: list, moved: moved))
        if selectedFile == nil, let selected, let now = engine.pathsNow(paths: [selected]).first,
           rows.contains(where: { $0.file == now }) {
            selectedFile = now
            requestSourcePreview()
        }
    }

    /// Whether any of `frames`, read at `relinks`, is elsewhere now.
    func moved(_ frames: [String], since relinks: UInt64) -> Bool {
        guard let engine, engine.relinks() != relinks else { return false }
        return engine.pathsNow(paths: frames) != frames
    }

    /// After the relinks since it was read.
    func pathNow(_ path: String) -> String {
        engine?.pathsNow(paths: [path]).first ?? path
    }

    /// Reopen the render shown after its frames' folders moved: the open
    /// `StackResult` keeps the old paths, so compare would match nothing and
    /// Retouch couldn't paint. Its own request (`Opening.Target.result`), so
    /// a tap replaces it; the view stays (`retargetView`). Retouch reopens
    /// on it in the same layout, its undo kept by the worker. Nothing while
    /// another open or a stack runs: what lands then is read after the move.
    private func reopenShown() async {
        guard let engine, let shown = result, opening == nil, stacking == nil else { return }
        let frames = Array(shown.stacked.union(shown.leftOut.keys))
        guard engine.pathsNow(paths: frames) != frames else { return }
        let id = shown.renderID
        let retouching = paint.renderID == id
        let frame = paint.sourceFrame
        log.info("reopening the render shown: its frames moved")
        let generation = beginOpening(.result(id))
        defer { endOpening(generation) }
        let stack: StackResult
        do {
            stack = try await Request().run { try await engine.loadRender(request: $0, id: id) }
        } catch {
            if viewGeneration == generation { report(error) }
            return
        }
        await show(stack, renderID: id, generation: generation, keepsView: { true })
        guard retouching, paint.renderID == id, let now = result, now.stack === stack else { return }
        let from = frame.map { now.stacked.contains($0) ? $0 : pathNow($0) }
        paint.open(now.stack, renderID: id, frame: from, standIn: now.display, lease: now.lease)
    }

    private func showFetching(_ current: UInt32, of total: UInt32, request: Request) {
        guard stacking?.request === request, case .running(let progress, let stop)? = stacking?.stage
        else { return }
        let frames = progress.frames
        updateStacking {
            $0.stage = .running(progress: .downloading(current: current, total: total, frames: frames), stop: stop)
        }
        reportProgress()
    }
}
