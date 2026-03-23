import Foundation
import CJXLShim

/// Encodes and decodes lossless u16 RGBA images using JPEG XL.
///
/// This is a pure image codec — no metadata embedding.
/// Project metadata lives in a separate JSON sidecar file.
public enum JXLCodec {

    // MARK: - Errors

    public enum JXLError: LocalizedError {
        case encoderCreationFailed
        case encoderConfigFailed(String)
        case encodingFailed(String)
        case decoderCreationFailed
        case decodingFailed(String)

        public var errorDescription: String? {
            switch self {
            case .encoderCreationFailed: "Failed to create JXL encoder"
            case .encoderConfigFailed(let msg): "JXL encoder config failed: \(msg)"
            case .encodingFailed(let msg): "JXL encoding failed: \(msg)"
            case .decoderCreationFailed: "Failed to create JXL decoder"
            case .decodingFailed(let msg): "JXL decoding failed: \(msg)"
            }
        }
    }

    /// Encode u16 RGBA pixels into a lossless JPEG XL file using all CPU cores.
    /// Input is always linear sRGB.
    /// - Parameter effort: Compression effort 1 (fastest) to 9 (smallest). Default 3.
    public static func encode(
        pixels: Data,
        width: Int,
        height: Int,
        effort: Int = 3
    ) throws -> Data {
        guard let enc = JxlEncoderCreate(nil) else {
            throw JXLError.encoderCreationFailed
        }
        defer { JxlEncoderDestroy(enc) }

        // Multi-threaded parallel runner
        let numThreads = JxlThreadParallelRunnerDefaultNumWorkerThreads()
        guard let runner = JxlThreadParallelRunnerCreate(nil, numThreads) else {
            throw JXLError.encoderConfigFailed("JxlThreadParallelRunnerCreate failed")
        }
        defer { JxlThreadParallelRunnerDestroy(runner) }

        guard JxlEncoderSetParallelRunner(enc, JxlThreadParallelRunner, runner) == JXL_ENC_SUCCESS else {
            throw JXLError.encoderConfigFailed("JxlEncoderSetParallelRunner failed")
        }

        // Basic info: u16 RGBA (integer, no exponent)
        var info = JxlBasicInfo()
        JxlEncoderInitBasicInfo(&info)
        info.xsize = UInt32(width)
        info.ysize = UInt32(height)
        info.bits_per_sample = 16
        info.exponent_bits_per_sample = 0
        info.num_color_channels = 3
        info.num_extra_channels = 1
        info.alpha_bits = 16
        info.alpha_exponent_bits = 0
        info.uses_original_profile = JXL_TRUE

        guard JxlEncoderSetBasicInfo(enc, &info) == JXL_ENC_SUCCESS else {
            throw JXLError.encoderConfigFailed("JxlEncoderSetBasicInfo failed")
        }

        // Color encoding: linear sRGB
        var colorEncoding = JxlColorEncoding()
        JxlColorEncodingSetToLinearSRGB(&colorEncoding, JXL_FALSE)
        guard JxlEncoderSetColorEncoding(enc, &colorEncoding) == JXL_ENC_SUCCESS else {
            throw JXLError.encoderConfigFailed("JxlEncoderSetColorEncoding failed")
        }

        // Frame settings: lossless, effort 3 (falcon) for speed
        guard let frameSettings = JxlEncoderFrameSettingsCreate(enc, nil) else {
            throw JXLError.encoderConfigFailed("JxlEncoderFrameSettingsCreate failed")
        }
        guard JxlEncoderSetFrameLossless(frameSettings, JXL_TRUE) == JXL_ENC_SUCCESS else {
            throw JXLError.encoderConfigFailed("JxlEncoderSetFrameLossless failed")
        }
        guard JxlEncoderSetFrameDistance(frameSettings, 0) == JXL_ENC_SUCCESS else {
            throw JXLError.encoderConfigFailed("JxlEncoderSetFrameDistance failed")
        }
        JxlEncoderFrameSettingsSetOption(frameSettings, JXL_ENC_FRAME_SETTING_EFFORT, Int64(min(max(effort, 1), 9)))

        // Add image frame
        var pixelFormat = JxlPixelFormat(
            num_channels: 4,
            data_type: JXL_TYPE_UINT16,
            endianness: JXL_NATIVE_ENDIAN,
            align: 0
        )
        let expectedSize = width * height * 4 * 2
        guard pixels.count >= expectedSize else {
            throw JXLError.encodingFailed("Pixel data too small: \(pixels.count) < \(expectedSize)")
        }

        let addFrameStatus = pixels.withUnsafeBytes { rawBuf in
            JxlEncoderAddImageFrame(frameSettings, &pixelFormat, rawBuf.baseAddress!, rawBuf.count)
        }
        guard addFrameStatus == JXL_ENC_SUCCESS else {
            throw JXLError.encodingFailed("JxlEncoderAddImageFrame failed")
        }

        JxlEncoderCloseInput(enc)

        return try processEncoderOutput(enc)
    }

    /// Result of decoding a JXL image.
    public struct DecodeResult {
        public let pixels: Data
        public let width: Int
        public let height: Int
    }

    /// Decode a JPEG XL file to u16 RGBA pixels using all CPU cores.
    public static func decode(data: Data) throws -> DecodeResult {
        guard let dec = JxlDecoderCreate(nil) else {
            throw JXLError.decoderCreationFailed
        }
        defer { JxlDecoderDestroy(dec) }

        // Multi-threaded parallel runner
        let numThreads = JxlThreadParallelRunnerDefaultNumWorkerThreads()
        guard let runner = JxlThreadParallelRunnerCreate(nil, numThreads) else {
            throw JXLError.decodingFailed("JxlThreadParallelRunnerCreate failed")
        }
        defer { JxlThreadParallelRunnerDestroy(runner) }

        guard JxlDecoderSetParallelRunner(dec, JxlThreadParallelRunner, runner) == JXL_DEC_SUCCESS else {
            throw JXLError.decodingFailed("JxlDecoderSetParallelRunner failed")
        }

        let events: Int32 = Int32(JXL_DEC_BASIC_INFO.rawValue)
            | Int32(JXL_DEC_FULL_IMAGE.rawValue)
        guard JxlDecoderSubscribeEvents(dec, events) == JXL_DEC_SUCCESS else {
            throw JXLError.decodingFailed("JxlDecoderSubscribeEvents failed")
        }

        return try data.withUnsafeBytes { inputBuf in
            guard JxlDecoderSetInput(
                dec,
                inputBuf.baseAddress!.assumingMemoryBound(to: UInt8.self),
                inputBuf.count
            ) == JXL_DEC_SUCCESS else {
                throw JXLError.decodingFailed("JxlDecoderSetInput failed")
            }
            JxlDecoderCloseInput(dec)

            var basicInfo = JxlBasicInfo()
            var outBuffer: UnsafeMutableRawPointer?
            var outByteCount = 0
            var pixelFormat = JxlPixelFormat(
                num_channels: 4,
                data_type: JXL_TYPE_UINT16,
                endianness: JXL_NATIVE_ENDIAN,
                align: 0
            )
            defer { outBuffer?.deallocate() }

            loop: while true {
                let status = JxlDecoderProcessInput(dec)
                switch status {
                case JXL_DEC_BASIC_INFO:
                    guard JxlDecoderGetBasicInfo(dec, &basicInfo) == JXL_DEC_SUCCESS else {
                        throw JXLError.decodingFailed("JxlDecoderGetBasicInfo failed")
                    }

                case JXL_DEC_NEED_IMAGE_OUT_BUFFER:
                    outByteCount = Int(basicInfo.xsize) * Int(basicInfo.ysize) * 4 * 2
                    outBuffer = UnsafeMutableRawPointer.allocate(
                        byteCount: outByteCount,
                        alignment: MemoryLayout<UInt16>.alignment
                    )
                    guard JxlDecoderSetImageOutBuffer(
                        dec, &pixelFormat, outBuffer!, outByteCount
                    ) == JXL_DEC_SUCCESS else {
                        throw JXLError.decodingFailed("JxlDecoderSetImageOutBuffer failed")
                    }

                case JXL_DEC_FULL_IMAGE:
                    break

                case JXL_DEC_SUCCESS:
                    break loop

                case JXL_DEC_ERROR:
                    throw JXLError.decodingFailed("JxlDecoderProcessInput returned error")

                default:
                    break
                }
            }

            guard let buffer = outBuffer else {
                throw JXLError.decodingFailed("No pixel data decoded")
            }

            return DecodeResult(
                pixels: Data(bytes: buffer, count: outByteCount),
                width: Int(basicInfo.xsize),
                height: Int(basicInfo.ysize)
            )
        }
    }

    private static func processEncoderOutput(_ enc: OpaquePointer) throws -> Data {
        var result = Data()
        var chunk = Data(count: 4 * 1024 * 1024)  // 4MB chunks

        while true {
            var availOut = chunk.count
            let status = chunk.withUnsafeMutableBytes { rawBuf -> JxlEncoderStatus in
                var next: UnsafeMutablePointer<UInt8>? = rawBuf.baseAddress!.assumingMemoryBound(to: UInt8.self)
                return JxlEncoderProcessOutput(enc, &next, &availOut)
            }
            let written = chunk.count - availOut
            if written > 0 {
                result.append(chunk.prefix(written))
            }
            switch status {
            case JXL_ENC_SUCCESS:
                return result
            case JXL_ENC_NEED_MORE_OUTPUT:
                continue
            default:
                throw JXLError.encodingFailed("JxlEncoderProcessOutput returned error")
            }
        }
    }
}
