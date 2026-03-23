import CLibRawBridge
import CoreVideo
import CxxStdlib
import Foundation
import os.log

private let logger = Logger(subsystem: "com.deptheos.raw", category: "CLibRawBridge")

/// Swift interface to LibRaw via the C++ bridge.
public enum DeptheosRaw {

    /// DNG-style color matrix for one illuminant.
    public struct DngColorInfo: Sendable {
        public let illuminant: UInt16
        public let colorMatrix: [Float]?
        public let forwardMatrix: [Float]?
        public let calibration: [Float]?
    }

    /// GPS info.
    public struct GPSInfo: Sendable {
        public let latitude: [Float]
        public let longitude: [Float]
        public let altitude: Float
        public let latRef: String
        public let longRef: String
        public let altRef: UInt8
    }

    /// Lens info.
    public struct LensInfo: Sendable {
        public let make: String
        public let model: String
        public let serial: String
        public let minFocal: Float
        public let maxFocal: Float
        public let maxApAtMinFocal: Float
        public let maxApAtMaxFocal: Float
        public let focalLengthIn35mm: UInt16
    }

    /// Metadata extracted from the raw file.
    public struct RawMetadata: Sendable {
        public let make: String
        public let model: String
        public let software: String
        public let uniqueCameraModel: String
        public let artist: String
        public let description: String
        public let orientation: Int32

        public let isoSpeed: Float
        public let shutter: Float
        public let aperture: Float
        public let focalLength: Float
        public let timestamp: Int32

        public let camMul: [Float]
        public let preMul: [Float]

        public let camXYZ: [Float]
        public let rgbCam: [Float]
        public let dngColor: [DngColorInfo]

        public let cblack: [UInt32]
        public let black: UInt32
        public let maximum: UInt32
        public let linearMax: [UInt32]

        public let asShotNeutral: [Float]?
        public let analogBalance: [Float]?
        public let baselineExposure: Float?
        public let linearResponseLimit: Float?

        public let rawColors: Int32

        public let lens: LensInfo
        public let gps: GPSInfo?
    }

    /// Result of reading a raw file.
    public struct ReadResult: Sendable {
        public let pixels: [UInt16]
        public let width: Int
        public let height: Int
        public let channels: Int
        public let metadata: RawMetadata
    }

    /// Read and process a camera raw file to sRGB 16-bit RGB pixels.
    public static func readRaw(at path: String) -> ReadResult? {
        guard let cxx = Optional(fromCxx: deptheos_read_raw(std.string(path))) else {
            logger.error("Failed to read raw file: \(path)")
            return nil
        }
        return mapResult(cxx)
    }

    /// Read and process a camera raw file from in-memory bytes.
    public static func readRaw(fromBytes bytes: [UInt8]) -> ReadResult? {
        bytes.withUnsafeBufferPointer { buf in
            guard let cxx = Optional(fromCxx: deptheos_read_raw_from_bytes(buf.baseAddress!, buf.count)) else {
                return nil
            }
            return mapResult(cxx)
        }
    }

    /// Read a camera raw file to linear 16-bit camera-native RGB pixels.
    public static func readRawLinear(at path: String) -> ReadResult? {
        guard let cxx = Optional(fromCxx: deptheos_read_raw_linear(std.string(path))) else {
            logger.error("Failed to read raw linear: \(path)")
            return nil
        }
        return mapResult(cxx)
    }

    /// Read linear camera-native RGB from in-memory bytes.
    public static func readRawLinear(fromBytes bytes: [UInt8]) -> ReadResult? {
        bytes.withUnsafeBufferPointer { buf in
            guard let cxx = Optional(fromCxx: deptheos_read_raw_linear_from_bytes(buf.baseAddress!, buf.count)) else {
                return nil
            }
            return mapResult(cxx)
        }
    }

    /// Read a camera raw file to linear camera-native RGB and return an RGBA16
    /// CVPixelBuffer. Pixels go directly from LibRaw into the CVPixelBuffer
    /// via vImage — no intermediate Swift Array.
    public static func readRawLinearPixelBuffer(at path: String) -> (pixelBuffer: CVPixelBuffer, width: Int, height: Int, channels: Int, metadata: RawMetadata)? {
        guard var handle = Optional(fromCxx: deptheos_open_raw_linear(std.string(path))) else {
            logger.error("Failed to open raw linear: \(path)")
            return nil
        }
        defer { deptheos_close_raw(&handle) }

        let w = Int(handle.metadata.width)
        let h = Int(handle.metadata.height)
        let ch = Int(handle.metadata.channels)

        guard let pb = CVPixelBuffer.create(width: w, height: h, pixelFormat: kCVPixelFormatType_64RGBALE) else {
            return nil
        }

        guard let rawData = deptheos_raw_pixel_data(&handle) else { return nil }
        pb.fillFromRGB16(
            source: rawData,
            width: w, height: h, channels: ch
        )

        let metadata = mapMetadata(handle.metadata)
        return (pixelBuffer: pb, width: w, height: h, channels: ch, metadata: metadata)
    }

    private static func mapResult(_ r: CLibRawBridge.RawReadResult) -> ReadResult {
        ReadResult(
            pixels: Array(r.pixels),
            width: Int(r.width),
            height: Int(r.height),
            channels: Int(r.channels),
            metadata: mapMetadata(r)
        )
    }

    private static func mapMetadata(_ r: CLibRawBridge.RawReadResult) -> RawMetadata {
        let dngColors = (0..<r.dngColor.size()).map { i -> DngColorInfo in
            let c = r.dngColor[i]
            return DngColorInfo(
                illuminant: c.illuminant,
                colorMatrix: c.hasColorMatrix ? Array(c.colorMatrix) : nil,
                forwardMatrix: c.hasForwardMatrix ? Array(c.forwardMatrix) : nil,
                calibration: c.hasCalibration ? Array(c.calibration) : nil
            )
        }

        return RawMetadata(
            make: String(r.make),
            model: String(r.model),
            software: String(r.software),
            uniqueCameraModel: String(r.uniqueCameraModel),
            artist: String(r.artist),
            description: String(r.description),
            orientation: r.orientation,
            isoSpeed: r.isoSpeed,
            shutter: r.shutter,
            aperture: r.aperture,
            focalLength: r.focalLen,
            timestamp: r.timestamp,
            camMul: Array(r.camMul),
            preMul: Array(r.preMul),
            camXYZ: Array(r.camXYZ),
            rgbCam: Array(r.rgbCam),
            dngColor: dngColors,
            cblack: Array(r.cblack),
            black: r.black,
            maximum: r.maximum,
            linearMax: Array(r.linearMax),
            asShotNeutral: r.asShotNeutral.empty() ? nil : Array(r.asShotNeutral),
            analogBalance: r.analogBalance.empty() ? nil : Array(r.analogBalance),
            baselineExposure: Optional(fromCxx: r.baselineExposure),
            linearResponseLimit: Optional(fromCxx: r.linearResponseLimit),
            rawColors: r.rawColors,
            lens: LensInfo(
                make: String(r.lens.make),
                model: String(r.lens.model),
                serial: String(r.lens.serial),
                minFocal: r.lens.minFocal,
                maxFocal: r.lens.maxFocal,
                maxApAtMinFocal: r.lens.maxApAtMinFocal,
                maxApAtMaxFocal: r.lens.maxApAtMaxFocal,
                focalLengthIn35mm: r.lens.focalLengthIn35mm
            ),
            gps: r.gps.parsed ? GPSInfo(
                latitude: Array(r.gps.latitude),
                longitude: Array(r.gps.longitude),
                altitude: r.gps.altitude,
                latRef: String(UnicodeScalar(UInt8(bitPattern: r.gps.latref))),
                longRef: String(UnicodeScalar(UInt8(bitPattern: r.gps.longref))),
                altRef: UInt8(bitPattern: r.gps.altref)
            ) : nil
        )
    }
}
