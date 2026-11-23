import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers
import os
@preconcurrency import DeptheosEngine

/// A running stack's progress and live map as one stream, `news`, read on
/// the main actor. Only the newest waits (`bufferingNewest(1)`), so a
/// lagging main actor skips to it. A run's end is its call's return.
/// Letting go of the stream cancels the run's request.
nonisolated final class StackObserverBridge: StackObserver, @unchecked Sendable {
    struct News: Sendable {
        /// After the run's newest event, which sums those before it.
        var progress: StackProgress?
        var liveMap: CGImage?
    }

    let news: AsyncStream<News>
    private let continuation: AsyncStream<News>.Continuation
    private let newest = OSAllocatedUnfairLock(initialState: News())

    init(request: Request) {
        (news, continuation) = AsyncStream.makeStream(of: News.self, bufferingPolicy: .bufferingNewest(1))
        continuation.onTermination = { ended in
            if case .cancelled = ended { request.cancel() }
        }
    }

    func progress(progress: StackProgress) {
        send { $0.progress = progress }
    }

    func liveMap(image: DisplayImage) {
        guard let cgImage = image.makeCGImage() else { return }
        send { $0.liveMap = cgImage }
    }

    /// The run returned: nothing more comes.
    func finish() { continuation.finish() }

    private func send(_ change: @Sendable (inout News) -> Void) {
        continuation.yield(newest.withLock { change(&$0); return $0 })
    }
}

extension DisplayImage {
    /// 8-bit BGRA as an sRGB image, without a copy.
    nonisolated func makeCGImage() -> CGImage? {
        guard width > 0, height > 0,
              let provider = CGDataProvider(data: bgra as CFData),
              let space = CGColorSpace(name: CGColorSpace.sRGB)
        else { return nil }
        let info = CGBitmapInfo(
            rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue
                | CGBitmapInfo.byteOrder32Little.rawValue)
        return CGImage(
            width: Int(width), height: Int(height),
            bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: Int(width) * 4,
            space: space, bitmapInfo: info, provider: provider,
            decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
}

/// ImageIO previews (thumbnails, source previews). For camera RAW and DNG
/// the file's stored preview when large enough and whole, else ImageIO's
/// RAW decode; other files are decoded.
///
/// Made through `Blocking`, never Swift's pool: ImageIO's RAW decode blocks
/// its thread, and a burst of DNG thumbnails would deadlock the pool.
nonisolated enum ImagePreview {
    /// `urgent` goes ahead of queued thumbnails. `nil` once canceled.
    static func load(_ url: URL, maxPixelSize: Int, urgent: Bool = false) async -> CGImage? {
        (try? await Blocking.run(urgent: urgent) { load(url, maxPixelSize: maxPixelSize) }) ?? nil
    }

    static func load(_ url: URL, maxPixelSize: Int) -> CGImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        if isRaw(source), let stored = storedPreview(source, maxPixelSize: maxPixelSize) {
            return stored
        }
        let options: [CFString: Any] = [
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }

    /// For a picture at least this size, a stored preview smaller than this
    /// is visibly soft full screen; a smaller picture takes one at least its
    /// own size.
    private static let storedMinimum = 1024

    /// Camera RAW, DNG included, whose full decode develops the sensor data.
    private static func isRaw(_ source: CGImageSource) -> Bool {
        guard let type = CGImageSourceGetType(source).flatMap({ UTType($0 as String) }) else { return false }
        return type.conforms(to: .rawImage)
    }

    /// The file's stored preview, upright, at most `maxPixelSize`: only if
    /// its long edge reaches `storedMinimum` (or `maxPixelSize` if smaller)
    /// and its shape is the image's within 1% (it can be a letterboxed 4:3
    /// in a 3:2 frame). Decodes nothing, so the caller decodes once.
    private static func storedPreview(_ source: CGImageSource, maxPixelSize: Int) -> CGImage? {
        guard let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = (properties[kCGImagePropertyPixelWidth] as? NSNumber)?.doubleValue,
              let height = (properties[kCGImagePropertyPixelHeight] as? NSNumber)?.doubleValue,
              width > 0, height > 0
        else { return nil }
        // Orientations 5 to 8 turn the image a quarter.
        let orientation = (properties[kCGImagePropertyOrientation] as? NSNumber)?.intValue ?? 1
        let aspect = (5...8).contains(orientation) ? height / width : width / height
        let options: [CFString: Any] = [
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceCreateThumbnailFromImageIfAbsent: false,
            kCGImageSourceCreateThumbnailWithTransform: true,
        ]
        guard let preview = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary),
              max(preview.width, preview.height) >= min(storedMinimum, maxPixelSize),
              abs(Double(preview.width) / Double(preview.height) / aspect - 1) <= 0.01
        else { return nil }
        return preview
    }
}
