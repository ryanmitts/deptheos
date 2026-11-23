@preconcurrency import DeptheosEngine
import Foundation

/// Whether a frame's file is there to read.
nonisolated enum FrameState: Equatable, Sendable {
    case missing
    /// In iCloud, not downloaded: it is when the stack needs it.
    case inCloud
    /// What it was picked on didn't open: a drive unplugged, a share away.
    case offline(Place?)

    /// Under the frame's name in the list.
    var label: LocalizedStringResource {
        switch self {
        case .missing:
            LocalizedStringResource("Missing", comment: "Under a frame's name in the list: its file isn't where the list says.")
        case .inCloud:
            LocalizedStringResource("In iCloud", comment: "Under a frame's name in the list: it is in iCloud and not downloaded yet.")
        case .offline:
            LocalizedStringResource(
                "Not connected", comment: "Under a frame's name in the list: the drive or server it is on isn't connected.")
        }
    }
}

/// Stops a pass over frames' files (a check, a copy): asked between files,
/// and it ends a download the pass waits on.
nonisolated final class FetchCancel: @unchecked Sendable {
    private let lock = NSLock()
    private var canceled = false
    private var coordinator: NSFileCoordinator?

    var isCanceled: Bool { lock.withLock { canceled } }

    func cancel() {
        let waiting = lock.withLock {
            canceled = true
            return coordinator
        }
        waiting?.cancel()
    }

    /// `false` when already canceled.
    fileprivate func begin(_ coordinator: NSFileCoordinator) -> Bool {
        lock.withLock {
            if canceled { return false }
            self.coordinator = coordinator
            return true
        }
    }

    fileprivate func end() {
        lock.withLock { coordinator = nil }
    }
}

/// Reading frames from wherever Files reaches. Each read is coordinated,
/// which makes a provider download a file not on the device; a plain read
/// can wait for ever. Everything here blocks: never on the main actor.
nonisolated enum FrameFiles {
    struct Canceled: Error {}

    /// Without downloading it.
    static func state(of path: String) -> FrameState? {
        let url = URL(fileURLWithPath: path)
        let values = try? url.resourceValues(forKeys: [.isUbiquitousItemKey, .ubiquitousItemDownloadingStatusKey])
        if let values, values.isUbiquitousItem == true {
            return values.ubiquitousItemDownloadingStatus == .notDownloaded ? .inCloud : nil
        }
        if FileManager.default.fileExists(atPath: path) { return nil }
        return FileManager.default.fileExists(atPath: placeholder(of: url).path) ? .inCloud : .missing
    }

    /// iCloud's stand-in for a file not downloaded, where the system keeps
    /// one: `.name.icloud` beside it.
    static func placeholder(of url: URL) -> URL {
        url.deletingLastPathComponent().appending(path: ".\(url.lastPathComponent).icloud")
    }

    static func placeholderTarget(_ url: URL) -> URL? {
        let name = url.lastPathComponent
        guard name.hasPrefix("."), name.hasSuffix(".icloud"), name.count > 8 else { return nil }
        return url.deletingLastPathComponent().appending(path: String(name.dropFirst().dropLast(7)))
    }

    /// `body` with the file, downloaded first if needed. The app's own
    /// files are read as they are.
    static func reading<T>(_ url: URL, cancel: FetchCancel, _ body: (URL) throws -> T) throws -> T {
        if Bookmarks.isOwned(url) { return try body(url) }
        let coordinator = NSFileCoordinator()
        guard cancel.begin(coordinator) else { throw Canceled() }
        defer { cancel.end() }
        var coordination: NSError?
        var outcome: Result<T, Error>?
        coordinator.coordinate(readingItemAt: url, options: .withoutChanges, error: &coordination) { actual in
            outcome = Result { try body(actual) }
        }
        if cancel.isCanceled { throw Canceled() }
        if let coordination { throw coordination }
        guard let outcome else { throw CocoaError(.fileReadUnknown) }
        return try outcome.get()
    }

    /// Why it can't be read, if so: opened, downloaded if needed, and its
    /// first byte read.
    static func unreadable(_ path: String, cancel: FetchCancel) throws -> String? {
        do {
            try reading(URL(fileURLWithPath: path), cancel: cancel) { url in
                let handle = try FileHandle(forReadingFrom: url)
                defer { try? handle.close() }
                _ = try handle.read(upToCount: 1)
            }
            return nil
        } catch is Canceled {
            throw Canceled()
        } catch {
            return error.localizedDescription
        }
    }

    /// The images directly in `folder`, by real name, undownloaded iCloud
    /// ones included. A folder just picked can list empty the first time,
    /// so an empty one is listed once more a moment later. Through
    /// `Blocking`.
    static func images(in folder: URL) async -> [URL] {
        let list = { @Sendable in try FileManager.default.contentsOfDirectory(atPath: folder.path) }
        var names = (try? await Blocking.run(.interactive, list)) ?? []
        if names.isEmpty {
            try? await Task.sleep(for: .milliseconds(300))
            names = (try? await Blocking.run(.interactive, list)) ?? []
        }
        var seen = Set<String>()
        var images: [URL] = []
        for name in names.sorted() {
            var url = folder.appending(path: name)
            if let target = placeholderTarget(url) {
                url = target
            } else if name.hasPrefix(".") {
                continue
            }
            guard isImage(url), seen.insert(url.lastPathComponent).inserted else { continue }
            images.append(url)
        }
        return images
    }

    /// The engine's own rule (camera RAW included), so what is listed and
    /// copied is what it stacks, and no GIF, WEBP or PSD.
    static func isImage(_ url: URL) -> Bool {
        isFrame(path: url.path)
    }
}
