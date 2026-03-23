import Foundation
import MemberwiseInit

extension DeptheosDng {

    /// Unsigned rational number (EXIF).
    @MemberwiseInit(.public)
    public struct URational: Sendable, Codable, Equatable {
        public let num: UInt32
        public let den: UInt32

        public var doubleValue: Double? {
            den != 0 ? Double(num) / Double(den) : nil
        }
    }

    /// Signed rational number (EXIF).
    @MemberwiseInit(.public)
    public struct SRational: Sendable, Codable, Equatable {
        public let num: Int32
        public let den: Int32

        public var doubleValue: Double? {
            den != 0 ? Double(num) / Double(den) : nil
        }
    }

    /// HueSatMap / LookTable dimensions.
    @MemberwiseInit(.public)
    public struct HueSatDims: Sendable, Codable, Equatable {
        public let hueDivisions: UInt32
        public let satDivisions: UInt32
        public let valDivisions: UInt32
    }

    /// GPU-ready render parameters, precomputed by the DNG SDK's dng_color_spec.
    @MemberwiseInit(.public)
    public struct RenderParams: Sendable, Codable {
        public let cameraToProPhoto: [Float]  // 9 elements
        public let cameraWhite: [Float]  // 3 elements
        public let wbBakedIn: Bool
        public let blackLevel: Double
        public let baselineExposure: Double
        public let shadowScale: Double
        public let hueSatMapDims: HueSatDims?
        public let hueSatMapData: [Float]?
        public let lookTableDims: HueSatDims?
        public let lookTableData: [Float]?
        public let toneCurve: [Float]  // 257 elements

        /// Identity render params for linearized sRGB images.
        public static func srgbIdentity() -> RenderParams {
            let srgbToProPhoto: [Float] = [
                 0.529360, 0.330103, 0.140661,
                 0.098388, 0.873463, 0.028098,
                 0.016851, 0.117711, 0.865681,
            ]

            return RenderParams(
                cameraToProPhoto: srgbToProPhoto,
                cameraWhite: [1, 1, 1],
                wbBakedIn: true,
                blackLevel: 0,
                baselineExposure: 0,
                shadowScale: 0,
                hueSatMapDims: nil, hueSatMapData: nil,
                lookTableDims: nil, lookTableData: nil,
                toneCurve: (0...256).map { Float($0) / 256.0 }
            )
        }
    }

    /// 3x3 matrix in row-major order.
    @MemberwiseInit(.public)
    public struct Matrix3x3: Sendable, Codable, Equatable {
        public var values: [Double]  // 9 elements
        public var isValid: Bool
    }

    /// Complete DNG image metadata — Codable, safe to persist to disk.
    @MemberwiseInit(.public)
    public struct ImageMetadata: Sendable, Codable, Equatable {

        @MemberwiseInit(.public)
        public struct CameraInfo: Sendable, Codable, Equatable {
            public var make: String
            public var model: String
            public var uniqueCameraModel: String
            public var serialNumber: String
            public var software: String
            public var artist: String
            public var copyright: String
        }

        @MemberwiseInit(.public)
        public struct LensInfo: Sendable, Codable, Equatable {
            public var make: String
            public var model: String
            public var serialNumber: String
            public var info: [URational]
            public var focalLengthIn35mm: UInt32
        }

        @MemberwiseInit(.public)
        public struct ExposureInfo: Sendable, Codable, Equatable {
            public var exposureTime: URational
            public var fnumber: URational
            public var focalLength: URational
            public var maxApertureValue: URational
            public var exposureBias: SRational
            public var isoSpeed: [UInt32]
            public var program: UInt32
            public var mode: UInt32
            public var meteringMode: UInt32
            public var flash: UInt32
            public var whiteBalance: UInt32
            public var sceneCaptureType: UInt32
            public var focalPlaneXRes: URational
            public var focalPlaneYRes: URational
            public var focalPlaneResUnit: UInt32
        }

        @MemberwiseInit(.public)
        public struct DateInfo: Sendable, Codable, Equatable {
            public var original: String
            public var digitized: String
        }

        @MemberwiseInit(.public)
        public struct GPSInfo: Sendable, Codable, Equatable {
            public var versionId: UInt32
            public var latitudeRef: String
            public var latitude: [URational]
            public var longitudeRef: String
            public var longitude: [URational]
            public var altitudeRef: UInt32
            public var altitude: URational
        }

        @MemberwiseInit(.public)
        public struct GeometryInfo: Sendable, Codable, Equatable {
            public var orientation: UInt32
            public var channels: Int32
            public var useJXL: Bool
        }

        @MemberwiseInit(.public)
        public struct ColorScienceInfo: Sendable, Codable, Equatable {
            public var colorMatrix1: Matrix3x3
            public var colorMatrix2: Matrix3x3
            public var forwardMatrix1: Matrix3x3
            public var forwardMatrix2: Matrix3x3
            public var cameraCalibration1: Matrix3x3
            public var cameraCalibration2: Matrix3x3
            public var calibrationIlluminant1: UInt32
            public var calibrationIlluminant2: UInt32
            public var cameraCalibrationSig: String
            public var profileCalibrationSig: String
            public var profileName: String
            public var asShotNeutral: [Double]
            public var analogBalance: [Double]
            public var baselineExposure: Double?
            public var baselineNoise: Double?
            public var baselineSharpness: Double?
            public var shadowScale: Double?
            public var linearResponseLimit: Double?
        }

        @MemberwiseInit(.public)
        public struct LUTData: Sendable, Codable, Equatable {
            public var hueSatMapDims1: HueSatDims?
            public var hueSatMapData1: [Float]?
            public var hueSatMapDims2: HueSatDims?
            public var hueSatMapData2: [Float]?
            public var hueSatMapEncoding: UInt32
            public var lookTableDims: HueSatDims?
            public var lookTableData: [Float]?
            public var lookTableEncoding: UInt32
            public var profileToneCurve: [Float]? = nil
        }

        @MemberwiseInit(.public)
        public struct NoiseProfileData: Sendable, Codable, Equatable {
            public var scale: [Double]
            public var offset: [Double]
        }

        @MemberwiseInit(.public)
        public struct OpcodeData: Sendable, Codable, Equatable {
            public var opcodeList2: Data?
            public var opcodeList3: Data?
        }

        public var camera: CameraInfo
        public var lens: LensInfo
        public var exposure: ExposureInfo
        public var dates: DateInfo
        public var gps: GPSInfo
        public var geometry: GeometryInfo
        public var colorScience: ColorScienceInfo
        public var luts: LUTData
        public var noiseProfile: NoiseProfileData
        public var opcodes: OpcodeData

    }

    /// Parsed DCP profile data — color science fields only.
    public struct DcpProfileData: Sendable {
        public let colorScience: ImageMetadata.ColorScienceInfo
        public let luts: ImageMetadata.LUTData
        public let baselineExposureOffset: Double?
    }
}
