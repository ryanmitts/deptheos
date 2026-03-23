import Foundation
import CoreGraphics
import ImageIO

/// Loads image thumbnails asynchronously with bounded concurrency.
actor ThumbnailLoader {
    private let maxPixelSize: Int
    private let maxConcurrent: Int

    init(maxPixelSize: Int = 80, maxConcurrent: Int = 4) {
        self.maxPixelSize = maxPixelSize
        self.maxConcurrent = maxConcurrent
    }

    /// Load thumbnails for a batch of URLs. Returns (URL, CGImage?) pairs.
    /// Throws `CancellationError` if the parent task is cancelled.
    func loadThumbnails(for urls: [URL]) async throws -> [(URL, CGImage?)] {
        try await withThrowingTaskGroup(of: (URL, CGImage?).self, returning: [(URL, CGImage?)].self) { group in
            var active = 0
            var urlIterator = urls.makeIterator()
            var results: [(URL, CGImage?)] = []
            results.reserveCapacity(urls.count)

            // Seed initial batch
            while active < maxConcurrent, let url = urlIterator.next() {
                let maxPx = maxPixelSize
                group.addTask { (url, Self.loadThumbnail(for: url, maxPixelSize: maxPx)) }
                active += 1
            }

            // Process completions, feeding new work
            for try await result in group {
                try Task.checkCancellation()
                results.append(result)
                active -= 1
                if let url = urlIterator.next() {
                    let maxPx = maxPixelSize
                    group.addTask { (url, Self.loadThumbnail(for: url, maxPixelSize: maxPx)) }
                    active += 1
                }
            }

            return results
        }
    }

    /// Load a single thumbnail. Returns nil on failure or cancellation.
    private static func loadThumbnail(for url: URL, maxPixelSize: Int) -> CGImage? {
        guard !Task.isCancelled else { return nil }
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }

        let options: [CFString: Any] = [
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceCreateThumbnailFromImageIfAbsent: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
        ]

        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }
}
