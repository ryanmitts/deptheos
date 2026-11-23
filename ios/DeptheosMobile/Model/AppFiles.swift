import CoreTransferable
import Foundation
import UniformTypeIdentifiers

/// The app's own folders: the history in Application Support, captures
/// and imports in Documents (On My iPhone › Deptheos), with new names that
/// never replace a file.
enum AppFiles {
    static func historyDirectory() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil,
            create: true)
        let directory = support.appending(path: "Deptheos/History", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    /// Holds Documents and the history. The engine names a frame in it from
    /// the container, so a container moved on reinstall moves nothing.
    static func containerDirectory() throws -> URL {
        let documents = try FileManager.default.url(
            for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        return documents.deletingLastPathComponent().standardizedFileURL
    }

    /// Named for the time it is made.
    static func newFolder(in parent: String, named prefix: String) throws -> URL {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "'\(prefix)' yyyy-MM-dd HH.mm.ss"
        let name = formatter.string(from: .now)
        let root = try ownedDirectory(parent)
        let folder = firstFree { n in
            root.appending(path: n.map { "\(name) \($0)" } ?? name, directoryHint: .isDirectory)
        }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    /// Called `name`, numbered if taken: a copy of a folder picked in
    /// Files, naming its stack as the original would.
    nonisolated static func newImportFolder(named name: String) throws -> URL {
        let root = try ownedDirectory("Imports")
        let folder = firstFree { n in
            root.appending(path: n.map { "\(name) \($0)" } ?? name, directoryHint: .isDirectory)
        }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        notBackedUp(folder)
        return folder
    }

    /// Made if missing.
    nonisolated static func importsFolder() throws -> URL {
        try ownedDirectory("Imports")
    }

    /// The copies from Photos and Files.
    nonisolated static func importFolders() -> [URL] {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let imports = documents.appending(path: "Imports", directoryHint: .isDirectory)
        let found = (try? FileManager.default.contentsOfDirectory(
            at: imports, includingPropertiesForKeys: [.isDirectoryKey], options: .skipsHiddenFiles)) ?? []
        return found.filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }
    }

    nonisolated static func size(of folder: URL) -> Int64 {
        guard let files = FileManager.default.enumerator(
            at: folder, includingPropertiesForKeys: [.totalFileAllocatedSizeKey])
        else { return 0 }
        var total: Int64 = 0
        for case let file as URL in files {
            total += Int64((try? file.resourceValues(forKeys: [.totalFileAllocatedSizeKey]).totalFileAllocatedSize) ?? 0)
        }
        return total
    }

    /// Left out of backups: a copy of frames in Photos or Files too. A
    /// camera burst is the only copy, and is backed up.
    nonisolated static func notBackedUp(_ folder: URL) {
        var folder = folder
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? folder.setResourceValues(values)
    }

    /// The Library is backed up but for Recently Deleted (ADR-170). Called
    /// once the engine has opened it, so its format has passed; the folder
    /// stays, so what a delete moves in later is left out too.
    nonisolated static func leaveRecentlyDeletedOutOfBackup(_ library: URL) {
        let deleted = library.appending(path: "Deleted", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: deleted, withIntermediateDirectories: true)
        notBackedUp(deleted)
    }

    /// As the system counts it for what the user asked for.
    nonisolated static func freeSpace() -> Int64? {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        return try? documents.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
            .volumeAvailableCapacityForImportantUsage
    }

    /// `prefix-<uuid>`, for a file handed to or copied out of another app.
    nonisolated static func newTemporaryFolder(prefix: String) throws -> URL {
        let folder = FileManager.default.temporaryDirectory
            .appending(path: "\(prefix)-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    /// Delete an earlier launch's temporary folders: exports handed off and
    /// renders dragged out (nothing says when they're done), diagnostics,
    /// and photos copied out of Photos. Nothing uses them at launch.
    nonisolated static func sweepTemporaryFolders() {
        let fileManager = FileManager.default
        let temporary = fileManager.temporaryDirectory
        guard let names = try? fileManager.contentsOfDirectory(atPath: temporary.path) else { return }
        let prefixes = ["Export-", "Drag-", "Photo-", "Diagnostics-"]
        for name in names where prefixes.contains(where: name.hasPrefix) {
            try? fileManager.removeItem(at: temporary.appending(path: name))
        }
    }

    /// Numbered when taken (two cameras' DSC_0001.JPG).
    nonisolated static func unusedName(_ name: String, in folder: URL) -> URL {
        let (stem, ext) = ((name as NSString).deletingPathExtension, (name as NSString).pathExtension)
        return firstFree { n in
            guard let n else { return folder.appending(path: name) }
            return folder.appending(path: ext.isEmpty ? "\(stem) \(n)" : "\(stem) \(n).\(ext)")
        }
    }

    /// The first of `candidate(nil)`, `candidate(2)`, … nothing is at.
    private nonisolated static func firstFree(_ candidate: (Int?) -> URL) -> URL {
        var url = candidate(nil)
        var n = 2
        while FileManager.default.fileExists(atPath: url.path) {
            url = candidate(n)
            n += 1
        }
        return url
    }

    private nonisolated static func ownedDirectory(_ name: String) throws -> URL {
        let documents = try FileManager.default.url(
            for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let directory = documents.appending(path: name, directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }
}

/// A photo from the Photos library, as its original file.
struct PhotoFile: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .image) { received in
            // The received file lives only for this call: copy it out.
            let copy = try AppFiles.newTemporaryFolder(prefix: "Photo")
            let target = copy.appending(path: received.file.lastPathComponent)
            try FileManager.default.copyItem(at: received.file, to: target)
            return PhotoFile(url: target)
        }
    }
}
