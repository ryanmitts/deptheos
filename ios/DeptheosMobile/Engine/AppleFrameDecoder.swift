import CoreImage
import Foundation
import ImageIO
import UniformTypeIdentifiers
@preconcurrency import DeptheosEngine

/// The engine's decoder for JPEG, HEIC, PNG and TIFF through ImageIO and
/// Core Graphics (`BitmapDecode`), and, for a stack whose RAW Decoder is
/// Apple, camera RAW and mosaic DNGs through Core Image's RAW filter
/// (ADR-131; linear DNGs, and any DNG under LibRaw, go to the DNG SDK).
///
/// Each frame is opened once: the engine reads its header, then has it
/// draw into the buffer it allocated. A standard image is drawn as stored
/// with its profile, and the engine's input stage converts it (ADR-132).
/// Camera RAW is rendered upright into Display P3 at 16 bits.
///
/// Core Image is for camera RAW only: on a Mac it held about four times a
/// frame's bytes in caches that outlive the render, where `BitmapDecode`
/// holds about twice while drawing and nothing after
/// (`script/check-bitmap-decode.sh`). Its caches are let go after each
/// RAW render.
///
/// RAW uses the filter's defaults (as-shot white balance, Apple's tone
/// curve), so a RAW stack stacks like a 16-bit TIFF and can't export DNG.
///
/// Called from the engine's prefetch threads, several frames at once; Core
/// Image contexts are thread-safe and nothing else here is mutable.
nonisolated final class AppleFrameDecoder: FrameDecoder, @unchecked Sendable {
    private let context = CIContext(options: [
        .cacheIntermediates: false,
        .name: "Deptheos frame decoder",
    ])

    /// Camera RAW, mosaic DNGs included: sent here only under Apple's
    /// decoder, a linear DNG never (ADR-131).
    private static func isCameraRaw(_ url: URL) -> Bool {
        guard let type = UTType(filenameExtension: url.pathExtension.lowercased()) else { return false }
        return type.conforms(to: .rawImage)
    }

    /// What it throws is for the log: the user is told only that the frame
    /// couldn't be read.
    ///
    /// Some engine threads last as long as the app with no autorelease pool
    /// to drain, so what the frameworks autorelease (the RAW filter's
    /// objects above all) is drained here.
    func openFrame(path: String) throws -> OpenedFrame {
        try autoreleasepool {
            let url = URL(fileURLWithPath: path)
            if Self.isCameraRaw(url) {
                guard let raw = RawFrame(url, context: context) else {
                    throw EngineError.Failed(message: "\(url.lastPathComponent): could not be read")
                }
                return raw
            }
            do {
                return BitmapFrame(try BitmapDecode.open(url), name: url.lastPathComponent)
            } catch {
                throw EngineError.Failed(message: "\(url.lastPathComponent): \(error)")
            }
        }
    }
}

/// A standard image opened by `BitmapDecode`.
nonisolated private final class BitmapFrame: OpenedFrame, @unchecked Sendable {
    private let opened: BitmapDecode.Opened
    private let name: String

    init(_ opened: BitmapDecode.Opened, name: String) {
        self.opened = opened
        self.name = name
    }

    func header() -> FrameHeader {
        let color: FrameColor = switch opened.color {
        case .space: opened.icc.map { .icc(profile: $0) } ?? .untagged
        case .cmyk: .cmyk
        case .lab: .lab
        case .hdr: .hdr
        }
        let depth: SampleDepth = switch opened.depth {
        case .eight: .eight
        case .sixteen: .sixteen
        case .float: .float
        }
        let layout: FrameLayout = switch opened.layout {
        case .gray: .gray
        case .grayAlpha: .grayAlpha
        case .rgbx: .rgbx
        case .rgba: .rgba
        }
        return FrameHeader(
            width: UInt32(opened.width), height: UInt32(opened.height), depth: depth,
            layout: layout, color: color, orientation: UInt8(opened.orientation))
    }

    func decodeInto(destination: UInt64, length: UInt64) throws {
        guard let pixels = UnsafeMutableRawPointer(bitPattern: UInt(destination)) else {
            throw EngineError.Failed(message: "\(name): no destination")
        }
        // The prefetch threads have no pool to drain: what the frameworks
        // autorelease goes with the frame.
        try autoreleasepool {
            do {
                try opened.draw(into: pixels, length: Int(length))
            } catch {
                throw EngineError.Failed(message: "\(name): \(error)")
            }
        }
    }
}

/// Camera RAW through the RAW filter (upright, as-shot white balance) into
/// Display P3 at 16 bits, its caches let go after: several frames decode
/// at once.
nonisolated private final class RawFrame: OpenedFrame, @unchecked Sendable {
    private let image: CIImage
    private let context: CIContext
    private let name: String
    private static let p3 = CGColorSpace(name: CGColorSpace.displayP3)!

    init?(_ url: URL, context: CIContext) {
        guard let image = CIRAWFilter(imageURL: url)?.outputImage else { return nil }
        let extent = image.extent.integral
        self.image = image.transformed(
            by: CGAffineTransform(translationX: -extent.origin.x, y: -extent.origin.y))
        self.context = context
        name = url.lastPathComponent
    }

    private var size: (width: Int, height: Int) {
        let extent = image.extent.integral
        return (Int(extent.width), Int(extent.height))
    }

    func header() -> FrameHeader {
        let profile = Self.p3.copyICCData() as Data?
        return FrameHeader(
            width: UInt32(size.width), height: UInt32(size.height), depth: .sixteen,
            layout: .rgbx, color: profile.map { .icc(profile: $0) } ?? .untagged, orientation: 1)
    }

    func decodeInto(destination: UInt64, length: UInt64) throws {
        let (width, height) = size
        let rowBytes = width * 4 * 2
        guard UInt64(rowBytes * height) == length,
              let pixels = UnsafeMutableRawPointer(bitPattern: UInt(destination))
        else {
            throw EngineError.Failed(message: "\(name): destination buffer has the wrong size")
        }
        autoreleasepool {
            context.render(
                image, toBitmap: pixels, rowBytes: rowBytes,
                bounds: CGRect(x: 0, y: 0, width: width, height: height),
                format: .RGBA16, colorSpace: Self.p3)
            context.clearCaches()
        }
    }
}
