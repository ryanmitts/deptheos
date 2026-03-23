import Foundation
import ImageIO
import DeptheosKit
import os

/// Lightweight entry for displaying a project in the list without loading full textures.
struct ProjectEntry: Identifiable, Sendable {
    let id: URL
    let name: String
    let createdAt: Date
    let config: StackingConfig
    let imageCount: Int
    let thumbnail: CGImage?
}

/// Manages auto-saved projects in the app's sandbox container.
///
/// Projects are stored in `Application Support/Projects/` as `.deptheos` packages.
/// The sandbox scopes this to the app container automatically.
private nonisolated let projectSignposter = OSSignposter(subsystem: "com.deptheos", category: .pointsOfInterest)

@Observable
@MainActor
final class ProjectStore {
    private(set) var entries: [ProjectEntry] = []
    let projectsDirectory: URL

    init() {
        let appSupport = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first!
        projectsDirectory = appSupport
            .appendingPathComponent("Deptheos", isDirectory: true)
            .appendingPathComponent("Projects", isDirectory: true)
    }

    /// Ensure the projects directory exists.
    func ensureDirectory() throws {
        try FileManager.default.createDirectory(
            at: projectsDirectory,
            withIntermediateDirectories: true
        )
    }

    /// Scan the projects directory and rebuild the entries list.
    func scanProjects() async {
        do {
            try ensureDirectory()
            let dir = projectsDirectory
            let scanned = try await Self.scanEntries(in: dir)
            entries = scanned
        } catch {
            print("[ProjectStore] Scan failed: \(error)")
        }
    }

    @concurrent private static func scanEntries(in directory: URL) async throws -> [ProjectEntry] {
        let contents = try FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )

        var scanned: [ProjectEntry] = []
        for url in contents where url.pathExtension == "deptheos" {
            if let entry = loadEntry(from: url) {
                scanned.append(entry)
            }
        }

        return scanned.sorted { $0.createdAt > $1.createdAt }
    }

    /// Auto-save a project to the sandbox after stacking completes.
    @discardableResult
    @concurrent func autoSave(
        project: StackingProject,
        imageURLs: [URL],
        metalContext: MetalContext
    ) async throws -> URL {
        let state = projectSignposter.beginInterval("Project Save")
        let dir = projectsDirectory
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        let ksuid = KSUID.generate(timestamp: project.createdAt)
        let url = dir.appendingPathComponent("\(ksuid).deptheos", isDirectory: true)

        try await project.save(to: url, metalContext: metalContext)

        // Save bookmarks for source image access
        let bookmarks = BookmarkManager.createBookmarks(for: imageURLs)
        BookmarkManager.saveBookmarks(bookmarks, to: url)

        print("[ProjectStore] Auto-saved to \(url.lastPathComponent)")
        projectSignposter.endInterval("Project Save", state)
        return url
    }

    /// Load a full project from a store entry.
    @concurrent func loadProject(
        entry: ProjectEntry,
        metalContext: MetalContext
    ) async throws -> (StackingProject, [URL]) {
        let state = projectSignposter.beginInterval("Project Load", "\(entry.name, privacy: .public)")
        let project = try await StackingProject.load(from: entry.id, metalContext: metalContext)

        let bookmarks = BookmarkManager.loadBookmarks(from: entry.id)
        let resolvedURLs = BookmarkManager.resolveBookmarks(bookmarks)

        let imageURLs: [URL]
        if !resolvedURLs.isEmpty {
            imageURLs = resolvedURLs
        } else {
            imageURLs = project.sourceImagePaths.map { URL(fileURLWithPath: $0) }
        }

        projectSignposter.endInterval("Project Load", state)
        return (project, imageURLs)
    }

    /// Delete a project from disk and refresh the list.
    func deleteProject(_ entry: ProjectEntry) {
        do {
            try FileManager.default.removeItem(at: entry.id)
            entries.removeAll { $0.id == entry.id }
        } catch {
            print("[ProjectStore] Delete failed: \(error)")
        }
    }

    /// Delete all projects from disk and clear the list.
    func deleteAllProjects() {
        for entry in entries {
            do {
                try FileManager.default.removeItem(at: entry.id)
            } catch {
                print("[ProjectStore] Delete failed for \(entry.name): \(error)")
            }
        }
        entries.removeAll()
    }

    // MARK: - Private

    private nonisolated static func loadEntry(from url: URL) -> ProjectEntry? {
        let metadataURL = url.appendingPathComponent("metadata.json")
        guard let jsonData = try? Data(contentsOf: metadataURL),
              let metadata = try? JSONDecoder().decode(ProjectMetadata.self, from: jsonData)
        else { return nil }

        let jpgURL = url.appendingPathComponent("thumbnail.jpg")
        let heicURL = url.appendingPathComponent("thumbnail.heic")
        let thumbnailURL = FileManager.default.fileExists(atPath: jpgURL.path) ? jpgURL : heicURL
        let thumbnail: CGImage? = if FileManager.default.fileExists(atPath: thumbnailURL.path),
            let source = CGImageSourceCreateWithURL(thumbnailURL as CFURL, nil) {
            CGImageSourceCreateImageAtIndex(source, 0, nil)
        } else {
            nil
        }

        // Derive display name from source folder
        let displayName: String
        if let firstPath = metadata.sourceImagePaths.first {
            let folder = URL(fileURLWithPath: firstPath).deletingLastPathComponent().lastPathComponent
            if !folder.isEmpty && folder != "/" {
                displayName = folder
            } else {
                displayName = "Stack"
            }
        } else {
            displayName = "Stack"
        }

        return ProjectEntry(
            id: url,
            name: displayName,
            createdAt: metadata.createdAt,
            config: metadata.config,
            imageCount: metadata.sourceImagePaths.count,
            thumbnail: thumbnail
        )
    }
}

// MARK: - KSUID

/// K-Sortable Unique ID: 4-byte timestamp (seconds since 2014-05-13) + 16-byte random payload,
/// base62-encoded to a 27-character string. Lexicographically sortable by creation time.
nonisolated enum KSUID {
    private static let epoch: UInt32 = 1_400_000_000 // 2014-05-13T16:53:20Z

    static func generate(timestamp: Date = Date()) -> String {
        let ts = UInt32(clamping: Int64(timestamp.timeIntervalSince1970) - Int64(epoch))

        var bytes = [UInt8](repeating: 0, count: 20)
        bytes[0] = UInt8((ts >> 24) & 0xFF)
        bytes[1] = UInt8((ts >> 16) & 0xFF)
        bytes[2] = UInt8((ts >> 8) & 0xFF)
        bytes[3] = UInt8(ts & 0xFF)

        // 16 random bytes
        for i in 4..<20 {
            bytes[i] = UInt8.random(in: 0...255)
        }

        return base62Encode(bytes)
    }

    private static let base62Chars = Array("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")

    private static func base62Encode(_ bytes: [UInt8]) -> String {
        // Convert 20 bytes to a big integer, then repeatedly mod 62
        var value = bytes.reduce(into: [UInt8]()) { result, byte in
            // Multiply existing value by 256 and add new byte
            var carry = UInt(byte)
            for i in stride(from: result.count - 1, through: 0, by: -1) {
                let prod = UInt(result[i]) * 256 + carry
                result[i] = UInt8(prod % 62)
                carry = prod / 62
            }
            while carry > 0 {
                result.insert(UInt8(carry % 62), at: 0)
                carry /= 62
            }
        }

        // Pad to 27 characters
        while value.count < 27 {
            value.insert(0, at: 0)
        }

        return String(value.map { base62Chars[Int($0)] })
    }
}
