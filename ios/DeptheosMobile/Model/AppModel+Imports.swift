import Foundation
@preconcurrency import DeptheosEngine

private let log = AppLog(category: "files")

/// Unused copies in Imports, and frames used in place copied in.
extension AppModel {
    /// The Imports folders the Library uses nothing of, with their size.
    /// In use is the engine's `foldersInUse` plus the frames being read in.
    /// None while the history is unread or the engine can't say: a wrong
    /// delete makes frames unreadable. Copies under ten minutes old are
    /// left: one may be on its way into the list.
    func unusedImports() async -> (folders: [URL], bytes: Int64) {
        guard history.read == .read,
              let inUse = await ask({ try await $0.foldersInUse(request: $1) }) ?? nil
        else {
            log.info("unused copies: not counted, the Library's use of them isn't known")
            return ([], 0)
        }
        var reading: [String] = []
        if case .reading(let request) = adding {
            reading = request.paths.map { ($0 as NSString).deletingLastPathComponent }
        }
        return await Self.imports(unusedBy: inUse + reading)
    }

    /// Imports folders that no `used` folder is, is in or holds, measured
    /// through `Blocking`.
    private static func imports(unusedBy used: [String]) async -> (folders: [URL], bytes: Int64) {
        (try? await Blocking.run(.background) {
            // The engine names the container `/var/…`, a listing
            // `/private/var/…`.
            let used = Set(used.map { URL(fileURLWithPath: $0).resolvingSymlinksInPath().path })
            let recent = Date.now.addingTimeInterval(-600)
            var folders: [URL] = []
            var bytes: Int64 = 0
            for folder in AppFiles.importFolders() {
                let path = folder.resolvingSymlinksInPath().path
                let made = (try? folder.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .now
                let isUsed = used.contains {
                    $0 == path || $0.hasPrefix(path + "/") || path.hasPrefix($0 + "/")
                }
                guard made < recent, !isUsed else { continue }
                folders.append(folder)
                bytes += AppFiles.size(of: folder)
            }
            return (folders, bytes)
        }) ?? ([], 0)
    }

    /// Counted again now, off the main actor. How many went.
    func deleteUnusedImports() async -> Int {
        guard !busy, !adding.isActive else { return 0 }
        let unused = await unusedImports().folders
        guard !busy, !adding.isActive else { return 0 }
        // Removed through `Blocking`: a large copy takes a while.
        let deleted = (try? await Blocking.run(.background) {
            unused.count(where: { (try? FileManager.default.removeItem(at: $0)) != nil })
        }) ?? 0
        log.info("deleted \(deleted) unused import folders")
        return deleted
    }

    /// The list's frames' roots outside Deptheos, as last checked: what
    /// Copy Frames into Deptheos copies.
    var outsideFolders: [String] { listRoots.map(\.folder) }

    /// Copy Frames into Deptheos (ADR-145): each of `folders` is copied
    /// whole or not at all. Every frame the Library names under it goes, at
    /// the same path, into a staging folder in Imports; the engine checks
    /// each copy by size and fingerprint and only then repoints the folder
    /// in one relink (`relinked`). A frame that can't be copied leaves the
    /// folder where it is, drops its staging, and is named.
    func copyFramesIn(_ folders: [String]) async {
        guard !busy, !adding.isActive, let engine, history.read == .read else { return }
        guard !folders.isEmpty else { return }
        setAdding(.planning(folders.map { ($0 as NSString).lastPathComponent }, .empty))
        var moved: [FolderMove] = []
        var failure: UserText?
        for folder in folders {
            guard let frames = await ask({ try await $0.framesToCopy(request: $1, folder: folder) }), !frames.isEmpty
            else { continue }
            let source = URL(fileURLWithPath: folder)
            // Read until the engine has checked the copy against them.
            let lease = await access(frames.map { source.appending(path: $0).path })
            let staging: URL
            do {
                staging = try AppFiles.newImportFolder(named: source.lastPathComponent)
            } catch {
                failure = Self.message(error)
                continue
            }
            // Followed in Settings however quick: no sheet flashes there.
            let cancel = FetchCancel()
            setAdding(.fetching(FetchProgress(done: 0, total: frames.count, downloadOnly: false), cancel, .empty))
            let reason = (try? await Blocking.run(.background) { [weak self] in
                Self.copy(frames, from: source, to: staging, cancel: cancel) { done in
                    Task { @MainActor in self?.fetched(done) }
                }
            }) ?? nil
            guard let reason else {
                // Canceled: nothing is copied.
                try? FileManager.default.removeItem(at: staging)
                break
            }
            do {
                let path = staging.path
                moved += try await engine.takeCopy(folder: folder, copy: path)
                withExtendedLifetime(lease) {}
                log.info("copied \(frames.count) frames into Deptheos")
            } catch {
                // Whole, or not at all: the missing frames are named, with
                // the system's reason for the first.
                try? FileManager.default.removeItem(at: staging)
                let said = Self.message(error).string
                failure = .verbatim(reason.first.map { said + " " + $0 } ?? said)
            }
        }
        setAdding(.none)
        await relinked(moved)
        if let failure {
            errorMessage = ErrorMessage(.app(LocalizedStringResource(
                "A folder couldn't be copied into Deptheos, so its frames are still used where they are. \(failure.string)",
                comment: "Alert text after Copy into Deptheos in the Library. The placeholder is a sentence saying why, the system's or the app's.")))
        }
    }

    /// Copy each of `frames` from `source` to the same path under
    /// `staging`, downloading first if needed, reporting after each. `nil`
    /// when canceled; else the system's reason for the first failure, if
    /// any (the engine's check names the frames).
    nonisolated static func copy(
        _ frames: [String], from source: URL, to staging: URL, cancel: FetchCancel,
        progress: @Sendable (Int) -> Void
    ) -> [String]? {
        var reasons: [String] = []
        for (index, frame) in frames.enumerated() {
            if cancel.isCanceled { return nil }
            let target = staging.appending(path: frame)
            do {
                try FileManager.default.createDirectory(
                    at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                try FrameFiles.reading(source.appending(path: frame), cancel: cancel) { actual in
                    // On the device this is a clone: no space, no time.
                    try FileManager.default.copyItem(at: actual, to: target)
                }
            } catch is FrameFiles.Canceled {
                return nil
            } catch {
                if reasons.isEmpty { reasons.append(error.localizedDescription) }
            }
            progress(index + 1)
        }
        return reasons
    }
}
