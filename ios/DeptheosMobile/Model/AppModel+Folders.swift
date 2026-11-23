import Foundation
@preconcurrency import DeptheosEngine

/// A folder outside Deptheos read in place, as Settings ▸ Storage ▸ Folders
/// lists it: a Library root (ADR-145), listed while a stack or render uses
/// a frame under it.
nonisolated struct KeptFolder: Identifiable, Sendable, Equatable {
    let path: String
    /// A drive, a share, iCloud Drive, when known.
    let place: Place?
    /// Its drive or share may be disconnected, or it moved.
    let reach: Reach
    /// Stacks with a frame under it in their list, a render or a failed run.
    let stacks: Int

    var id: String { path }
    var name: String { (path as NSString).lastPathComponent }
}

extension AppModel {
    /// The roots kept with a bookmark.
    var keptFolderCount: Int { Int(engine?.bookmarkedCount() ?? 0) }

    /// The folders read in place, with the stacks using each as the engine
    /// counts. Their scopes are leased first to tell which can't be
    /// reached; one found where its bookmark led is followed. `nil` when
    /// the Library can't be read.
    func keptFolders() async -> [KeptFolder]? {
        guard let roots = await ask({ try await $0.readFolderRoots(request: $1) }) else { return nil }
        let lease = await lease(roots)
        defer { withExtendedLifetime(lease) {} }
        guard let folders = await ask({ try await $0.readFolders(request: $1) }) else { return nil }
        return folders
            .map { folder in
                KeptFolder(
                    path: folder.root.folder, place: Place.of(folder.root), reach: folder.reach,
                    stacks: Int(folder.stacks))
            }
            .sorted {
                let order = $0.name.localizedStandardCompare($1.name)
                return order == .orderedSame ? $0.path < $1.path : order == .orderedAscending
            }
    }

    /// Copy into Deptheos for a listed folder (`copyFramesIn`).
    func copyIn(_ folder: KeptFolder) async {
        await copyFramesIn([folder.path])
    }

    /// Locate… for an unreachable listed folder (`relocate`). A drive's top
    /// folder, picked once after a disconnect, brings back every folder on
    /// it.
    func locate(_ folder: KeptFolder, at new: Picked) async {
        guard !busy, !adding.isActive else { return }
        await relocate(old: folder.path, to: new)
    }
}
