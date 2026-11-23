import CoreGraphics
import Foundation
import ImageIO

/// A standard image (JPEG, HEIC, PNG, TIFF) opened once by ImageIO and
/// drawn by Core Graphics as stored, in its own color space, into a buffer
/// allocated elsewhere, for the engine's input stage to convert (ADR-132).
///
/// `open` reads what the engine plans by from the same source `draw` then
/// fills from, so the buffer is sized from the state that fills it.
///
/// On the CPU, one decode per call. Core Image's render would hold GPU
/// textures and caches beside each of the engine's concurrent decodes,
/// which its memory estimate can't count.
///
/// Free of the engine's types, so a script on a Mac can check it
/// (`script/check-bitmap-decode.sh`).
nonisolated enum BitmapDecode {
    enum Failure: Error, CustomStringConvertible {
        case unreadable
        case length(Int, Int)
        case context

        var description: String {
            switch self {
            case .unreadable: "could not be read"
            case let .length(given, needed): "a buffer of \(given) bytes for \(needed)"
            case .context: "no bitmap context for the destination"
            }
        }
    }

    /// What the samples are stored as.
    enum Depth { case eight, sixteen, float }

    /// How the samples of a pixel are laid out in the buffer.
    enum Layout {
        /// One sample.
        case gray
        /// Gray, then alpha; gray premultiplied by alpha.
        case grayAlpha
        /// Red, green, blue and a fourth sample to skip.
        case rgbx
        /// Red, green, blue premultiplied by alpha, then alpha.
        case rgba

        var channels: Int {
            switch self {
            case .gray: 1
            case .grayAlpha: 2
            case .rgbx, .rgba: 4
            }
        }
    }

    /// As the engine takes it.
    enum Color {
        /// Drawn in `space`.
        case space(CGColorSpace)
        /// The engine refuses these by name, before drawing.
        case cmyk, lab, hdr
    }

    struct Opened: @unchecked Sendable {
        let image: CGImage
        /// As stored, not turned.
        let width: Int
        let height: Int
        let depth: Depth
        let layout: Layout
        let color: Color
        /// The EXIF orientation, 1 to 8.
        let orientation: UInt32

        var bytesPerSample: Int {
            switch depth {
            case .eight: 1
            case .sixteen: 2
            case .float: 4
            }
        }

        /// In bytes.
        var length: Int { width * height * layout.channels * bytesPerSample }

        /// For the engine; `nil` for a space without one (taken as sRGB)
        /// and a refused color.
        var icc: Data? {
            guard case let .space(space) = color else { return nil }
            return space.copyICCData() as Data?
        }

        /// Rows tightly packed, samples native-endian.
        func draw(into pixels: UnsafeMutableRawPointer, length given: Int) throws {
            guard given == length else { throw Failure.length(given, length) }
            guard case let .space(space) = color else { throw Failure.context }
            let rect = CGRect(x: 0, y: 0, width: width, height: height)
            if layout != .grayAlpha {
                let context = try Self.context(
                    pixels, width: width, height: height, depth: depth, layout: layout, space: space)
                context.interpolationQuality = .none
                context.draw(image, in: rect)
                return
            }
            // A bitmap context has no gray with alpha: gray drawn over the
            // zeroed buffer is premultiplied, the alpha drawn apart (an
            // 8-bit RGBA drawing's fourth sample), then the two interleaved.
            let samples = width * height
            let gray = UnsafeMutableRawPointer.allocate(byteCount: samples * bytesPerSample, alignment: 16)
            defer { gray.deallocate() }
            gray.initializeMemory(as: UInt8.self, repeating: 0, count: samples * bytesPerSample)
            let alpha = UnsafeMutablePointer<UInt8>.allocate(capacity: samples * 4)
            defer { alpha.deallocate() }
            alpha.initialize(repeating: 0, count: samples * 4)
            let grayContext = try Self.context(
                gray, width: width, height: height, depth: depth, layout: .gray, space: space)
            grayContext.interpolationQuality = .none
            grayContext.draw(image, in: rect)
            let alphaContext = try Self.context(
                alpha, width: width, height: height, depth: .eight, layout: .rgba,
                space: CGColorSpaceCreateDeviceRGB())
            alphaContext.interpolationQuality = .none
            alphaContext.draw(image, in: rect)
            switch depth {
            case .eight:
                interleave(gray, alpha, into: pixels, samples) { UInt8($0) }
            case .sixteen:
                interleave(gray, alpha, into: pixels, samples) { UInt16($0) * 257 }
            case .float:
                interleave(gray, alpha, into: pixels, samples) { Float($0) / 255 }
            }
        }

        private func interleave<T>(
            _ gray: UnsafeMutableRawPointer, _ alpha: UnsafeMutablePointer<UInt8>,
            into pixels: UnsafeMutableRawPointer, _ samples: Int, _ widen: (UInt8) -> T
        ) {
            let g = gray.assumingMemoryBound(to: T.self)
            let out = pixels.assumingMemoryBound(to: T.self)
            for i in 0..<samples {
                out[i * 2] = g[i]
                out[i * 2 + 1] = widen(alpha[i * 4 + 3])
            }
        }

        /// `pixels` `nil`: memory of its own, for a check.
        static func context(
            _ pixels: UnsafeMutableRawPointer?, width: Int, height: Int, depth: Depth,
            layout: Layout, space: CGColorSpace
        ) throws -> CGContext {
            let order: CGBitmapInfo = switch depth {
            case .eight: []
            case .sixteen: .byteOrder16Little
            case .float: [.byteOrder32Little, .floatComponents]
            }
            let alpha: CGImageAlphaInfo = switch layout {
            case .gray, .grayAlpha: .none
            case .rgbx: .noneSkipLast
            case .rgba: .premultipliedLast
            }
            let channels = layout == .grayAlpha ? 1 : layout.channels
            let bytes = switch depth {
            case .eight: 1
            case .sixteen: 2
            case .float: 4
            }
            guard let context = CGContext(
                data: pixels, width: width, height: height, bitsPerComponent: bytes * 8,
                bytesPerRow: width * channels * bytes, space: space,
                bitmapInfo: alpha.rawValue | order.rawValue)
            else { throw Failure.context }
            return context
        }
    }

    /// An iPhone HEIC opens as its standard-range image, its gain map not
    /// applied.
    static func open(_ url: URL) throws -> Opened {
        // Not cached: the decoded picture goes as soon as it is drawn.
        let options = [
            kCGImageSourceShouldCache: false,
            kCGImageSourceDecodeRequest: kCGImageSourceDecodeToSDR,
        ] as CFDictionary
        guard let source = CGImageSourceCreateWithURL(url as CFURL, options),
              let image = CGImageSourceCreateImageAtIndex(source, 0, options),
              let stored = image.colorSpace
        else { throw Failure.unreadable }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        let orientation = (properties?[kCGImagePropertyOrientation] as? UInt32)
            .flatMap { (1...8).contains($0) ? $0 : nil } ?? 1
        let depth: Depth = image.bitmapInfo.contains(.floatComponents)
            ? .float : image.bitsPerComponent > 8 ? .sixteen : .eight
        let alpha = switch image.alphaInfo {
        case .first, .last, .premultipliedFirst, .premultipliedLast: true
        default: false
        }
        let space = stored.model == .indexed ? stored.baseColorSpace ?? stored : stored
        let opened = { (layout: Layout, color: Color) in
            Opened(
                image: image, width: image.width, height: image.height, depth: depth,
                layout: layout, color: color, orientation: orientation)
        }
        let rgb: Layout = alpha ? .rgba : .rgbx
        switch space.model {
        case .cmyk: return opened(rgb, .cmyk)
        case .lab: return opened(rgb, .lab)
        default: break
        }
        if CGColorSpaceUsesITUR_2100TF(space) { return opened(rgb, .hdr) }
        let layout: Layout? = switch space.model {
        case .monochrome: alpha ? .grayAlpha : .gray
        case .rgb: rgb
        default: nil
        }
        // Its own space where a bitmap context can use it, else Display P3.
        if let layout, (try? Opened.context(
            nil, width: 1, height: 1, depth: depth, layout: layout, space: space)) != nil
        {
            return opened(layout, .space(space))
        }
        guard let p3 = CGColorSpace(name: CGColorSpace.displayP3) else { throw Failure.context }
        return opened(rgb, .space(p3))
    }
}
