import Foundation
import UniformTypeIdentifiers
import DeptheosKit

// MARK: - UTType

extension UTType {
    static let deptheosProject = UTType(exportedAs: "com.deptheos.project")
}

// MARK: - Bookmark Helpers

/// Security-scoped bookmark utilities for sandboxed file access.
nonisolated enum BookmarkManager {

    /// Create security-scoped bookmarks for the given source URLs.
    static func createBookmarks(for urls: [URL]) -> [Data] {
        urls.compactMap { url in
            #if os(macOS)
            let options: URL.BookmarkCreationOptions = .withSecurityScope
            #else
            let options: URL.BookmarkCreationOptions = .minimalBookmark
            #endif
            return try? url.bookmarkData(
                options: options,
                includingResourceValuesForKeys: nil,
                relativeTo: nil
            )
        }
    }

    /// Resolve security-scoped bookmarks to URLs and start accessing them.
    ///
    /// Each returned URL has had `startAccessingSecurityScopedResource()` called.
    /// The caller must eventually call `stopAccessingSecurityScopedURLs(_:)` when done.
    static func resolveBookmarks(_ bookmarks: [Data]) -> [URL] {
        var urls: [URL] = []
        for data in bookmarks {
            var isStale = false
            #if os(macOS)
            let options: URL.BookmarkResolutionOptions = .withSecurityScope
            #else
            let options: URL.BookmarkResolutionOptions = .init()
            #endif
            guard let url = try? URL(
                resolvingBookmarkData: data,
                options: options,
                relativeTo: nil,
                bookmarkDataIsStale: &isStale
            ) else { continue }

            if url.startAccessingSecurityScopedResource() {
                urls.append(url)
            }
        }
        return urls
    }

    /// Stop accessing security-scoped URLs previously resolved from bookmarks.
    static func stopAccessingSecurityScopedURLs(_ urls: [URL]) {
        for url in urls {
            url.stopAccessingSecurityScopedResource()
        }
    }

    /// Save bookmarks into a .deptheos package directory.
    static func saveBookmarks(_ bookmarks: [Data], to projectURL: URL) {
        guard !bookmarks.isEmpty else { return }
        let url = projectURL.appendingPathComponent("bookmarks.plist")
        if let data = try? PropertyListEncoder().encode(bookmarks) {
            try? data.write(to: url)
        }
    }

    /// Load bookmarks from a .deptheos package directory.
    static func loadBookmarks(from projectURL: URL) -> [Data] {
        let url = projectURL.appendingPathComponent("bookmarks.plist")
        guard let data = try? Data(contentsOf: url),
              let bookmarks = try? PropertyListDecoder().decode([Data].self, from: data) else {
            return []
        }
        return bookmarks
    }
}
