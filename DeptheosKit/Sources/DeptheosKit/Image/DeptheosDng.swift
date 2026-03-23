import Accelerate
import CDNGBridge
import CoreVideo
import CxxStdlib
import Foundation
import os.log

private let logger = Logger(subsystem: "com.deptheos.dng", category: "CDNGBridge")

/// Swift interface to the C++ DNG SDK wrapper.
public enum DeptheosDng {

    /// Initialize the DNG/XMP SDK and the log handler.
    /// Call once at app launch.
    public static func initialize() {
        deptheos_dng_initialize()
        deptheos_set_dng_log_callback { level, message in
            guard let message else { return }
            let msg = String(cString: message)
            switch level {
            case DNG_LOG_TRACE, DNG_LOG_DEBUG:
                logger.debug("\(msg)")
            case DNG_LOG_INFO:
                logger.info("\(msg)")
            case DNG_LOG_WARN:
                logger.warning("\(msg)")
            case DNG_LOG_ERROR:
                logger.error("\(msg)")
            default:
                logger.log("\(msg)")
            }
        }
    }

    /// Read linear 16-bit pixels and full metadata from a DNG file.
    public static func readDng(at path: String) -> (pixels: [UInt16], width: Int, height: Int, channels: Int, metadata: ImageMetadata)? {
        guard let result = Optional(fromCxx: deptheos_dng_read(std.string(path))) else { return nil }
        return (
            pixels: Array(result.pixels),
            width: Int(result.width),
            height: Int(result.height),
            channels: Int(result.channels),
            metadata: mapFromCxx(result.metadata)
        )
    }

    /// Read linear 16-bit pixels and full metadata from in-memory DNG bytes.
    public static func readDng(fromBytes bytes: [UInt8]) -> (pixels: [UInt16], width: Int, height: Int, channels: Int, metadata: ImageMetadata)? {
        bytes.withUnsafeBufferPointer { buf in
            guard let result = Optional(fromCxx: deptheos_dng_read_from_bytes(buf.baseAddress!, buf.count)) else { return nil }
            return (
                pixels: Array(result.pixels),
                width: Int(result.width),
                height: Int(result.height),
                channels: Int(result.channels),
                metadata: mapFromCxx(result.metadata)
            )
        }
    }

    /// Read a DNG file and return an RGBA16 CVPixelBuffer. Pixels go directly
    /// from the DNG SDK into the CVPixelBuffer via vImage — no intermediate Swift Array.
    public static func readDngPixelBuffer(at path: String) -> (pixelBuffer: CVPixelBuffer, width: Int, height: Int, channels: Int, metadata: ImageMetadata)? {
        guard var handle = Optional(fromCxx: deptheos_open_dng(std.string(path))) else { return nil }
        defer { deptheos_close_dng(&handle) }
        return copyHandleToPixelBuffer(&handle)
    }

    /// Read a DNG from in-memory bytes, returning an RGBA16 CVPixelBuffer.
    public static func readDngPixelBuffer(fromBytes bytes: [UInt8]) -> (pixelBuffer: CVPixelBuffer, width: Int, height: Int, channels: Int, metadata: ImageMetadata)? {
        bytes.withUnsafeBufferPointer { buf in
            guard var handle = Optional(fromCxx: deptheos_open_dng_from_bytes(buf.baseAddress!, buf.count)) else { return nil }
            defer { deptheos_close_dng(&handle) }
            return copyHandleToPixelBuffer(&handle)
        }
    }

    private static func copyHandleToPixelBuffer(_ handle: inout CDNGBridge.DngDecodeHandle) -> (pixelBuffer: CVPixelBuffer, width: Int, height: Int, channels: Int, metadata: ImageMetadata)? {
        let w = Int(handle.metadata.width)
        let h = Int(handle.metadata.height)
        let ch = Int(handle.metadata.channels)

        guard let pb = CVPixelBuffer.create(width: w, height: h, pixelFormat: kCVPixelFormatType_64RGBALE) else {
            return nil
        }

        if ch >= 3 {
            // Write RGB directly into RGBA layout via stride-4, no temp buffer needed
            CVPixelBufferLockBaseAddress(pb, [])
            defer { CVPixelBufferUnlockBaseAddress(pb, []) }
            guard let baseAddress = CVPixelBufferGetBaseAddress(pb) else { return nil }
            let rowBytes = CVPixelBufferGetBytesPerRow(pb)
            deptheos_copy_dng_pixels_rgba(&handle, baseAddress, Int32(rowBytes))

            // Fill alpha channel with 0xFFFF using vImage (RGBA: alpha is bit 0x1)
            var buf = vImage_Buffer(
                data: baseAddress, height: vImagePixelCount(h),
                width: vImagePixelCount(w), rowBytes: rowBytes
            )
            var pixel: [UInt16] = [0, 0, 0, 0xFFFF]
            vImageOverwriteChannelsWithPixel_ARGB16U(&pixel, &buf, &buf, 0x1, vImage_Flags(kvImageNoFlags))
        } else {
            // Monochrome: use temp buffer + vImage path
            let pixelCount = w * h * ch
            let tempBuffer = UnsafeMutablePointer<UInt16>.allocate(capacity: pixelCount)
            defer { tempBuffer.deallocate() }
            deptheos_copy_dng_pixels(&handle, tempBuffer)
            pb.fillFromRGB16(source: tempBuffer, width: w, height: h, channels: ch)
        }

        let metadata = mapFromCxx(handle.metadata.metadata)
        return (pixelBuffer: pb, width: w, height: h, channels: ch, metadata: metadata)
    }

    /// Write linear 16-bit data as a DNG file using metadata.
    public static func writeDng(
        to outputPath: String,
        metadata: ImageMetadata,
        pixels: [UInt16],
        width: Int, height: Int, channels: Int
    ) -> Bool {
        let cxxMeta = mapToCxx(metadata)
        return pixels.withUnsafeBufferPointer { buf in
            deptheos_write_dng(
                std.string(outputPath), cxxMeta,
                buf.baseAddress!, buf.count,
                Int32(width), Int32(height), Int32(channels)
            )
        }
    }

    /// Write linear 16-bit data as DNG bytes in memory.
    public static func writeDng(
        metadata: ImageMetadata,
        pixels: [UInt16],
        width: Int, height: Int, channels: Int
    ) -> Data? {
        let cxxMeta = mapToCxx(metadata)
        return pixels.withUnsafeBufferPointer { buf in
            guard let result = Optional(fromCxx: deptheos_write_dng_to_bytes(
                cxxMeta, buf.baseAddress!, buf.count,
                Int32(width), Int32(height), Int32(channels)
            )) else { return nil }
            return Data(Array(result))
        }
    }

    /// Render linear camera-RGB pixels to gamma-corrected sRGB 16-bit
    /// using color science from the metadata.
    public static func renderToSrgb(
        pixels: [UInt16],
        width: Int, height: Int, channels: Int,
        metadata: ImageMetadata
    ) -> (pixels: [UInt16], width: Int, height: Int, channels: Int)? {
        let cxxMeta = mapToCxx(metadata)
        return pixels.withUnsafeBufferPointer { buf in
            guard let result = Optional(fromCxx: deptheos_render_to_srgb(
                buf.baseAddress!, buf.count,
                Int32(width), Int32(height), Int32(channels), cxxMeta
            )) else { return nil }
            return (
                pixels: Array(result.pixels),
                width: Int(result.width),
                height: Int(result.height),
                channels: Int(result.channels)
            )
        }
    }

    /// Compute precomputed GPU-ready render parameters from metadata.
    public static func computeRenderParams(
        metadata: ImageMetadata
    ) -> RenderParams? {
        let cxxMeta = mapToCxx(metadata)
        guard let cxxParams = Optional(fromCxx: deptheos_compute_render_params(cxxMeta)) else { return nil }
        return mapRenderParamsFromCxx(cxxParams)
    }

    /// Load a DCP (DNG Camera Profile) file and return the extracted profile data.
    public static func loadDcpProfile(at path: String) -> DcpProfileData? {
        guard let result = Optional(fromCxx: deptheos_load_dcp(std.string(path))) else { return nil }
        let full = mapFromCxx(result.profile)
        return DcpProfileData(
            colorScience: full.colorScience,
            luts: full.luts,
            baselineExposureOffset: result.baselineExposureOffset != 0 ? result.baselineExposureOffset : nil
        )
    }

    /// Validate a DNG file by running the full pipeline.
    public static func validate(at path: String) -> Bool {
        deptheos_validate_dng(std.string(path))
    }

}
