import Accelerate
import CoreVideo
import Foundation

extension CVPixelBuffer {

    /// Create an empty Metal-compatible CVPixelBuffer with the given dimensions and pixel format.
    public static func create(width: Int, height: Int, pixelFormat: OSType) -> CVPixelBuffer? {
        var pb: CVPixelBuffer?
        let attrs: [String: Any] = [
            kCVPixelBufferMetalCompatibilityKey as String: true,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:]
        ]
        let status = CVPixelBufferCreate(nil, width, height, pixelFormat, attrs as CFDictionary, &pb)
        guard status == kCVReturnSuccess else { return nil }
        return pb
    }

    /// Create a Metal-compatible RGBA16 CVPixelBuffer from interleaved 16-bit pixels.
    ///
    /// Accepts RGB (3 channels) or monochrome (1 channel) input.
    /// The output has an opaque alpha channel (0xFFFF).
    public static func fromRGB16(
        pixels: [UInt16], width: Int, height: Int, channels: Int
    ) -> CVPixelBuffer? {
        guard let pixelBuffer = create(width: width, height: height, pixelFormat: kCVPixelFormatType_64RGBALE) else {
            return nil
        }
        pixels.withUnsafeBufferPointer { srcBuf in
            pixelBuffer.fillFromRGB16(
                source: UnsafeMutableRawPointer(mutating: srcBuf.baseAddress!),
                width: width, height: height, channels: channels
            )
        }
        return pixelBuffer
    }

    /// Fill this RGBA16 pixel buffer from interleaved 16-bit source data (1 or 3 channels).
    /// Handles locking, vImage conversion, and mono→RGB replication.
    public func fillFromRGB16(
        source: UnsafeMutableRawPointer,
        width: Int, height: Int, channels: Int
    ) {
        CVPixelBufferLockBaseAddress(self, [])
        defer { CVPixelBufferUnlockBaseAddress(self, []) }
        guard let baseAddress = CVPixelBufferGetBaseAddress(self) else { return }
        let dstBytesPerRow = CVPixelBufferGetBytesPerRow(self)

        var src = vImage_Buffer(
            data: source, height: vImagePixelCount(height),
            width: vImagePixelCount(width),
            rowBytes: width * channels * MemoryLayout<UInt16>.stride
        )
        var dst = vImage_Buffer(
            data: baseAddress, height: vImagePixelCount(height),
            width: vImagePixelCount(width), rowBytes: dstBytesPerRow
        )

        if channels == 1 {
            let rgbRowBytes = width * 3 * MemoryLayout<UInt16>.stride
            let rgbData = UnsafeMutableRawPointer.allocate(
                byteCount: height * rgbRowBytes, alignment: MemoryLayout<UInt16>.alignment
            )
            defer { rgbData.deallocate() }
            var rgb = vImage_Buffer(
                data: rgbData, height: vImagePixelCount(height),
                width: vImagePixelCount(width), rowBytes: rgbRowBytes
            )
            vImageConvert_Planar16UtoRGB16U(&src, &src, &src, &rgb, vImage_Flags(kvImageNoFlags))
            vImageConvert_RGB16UtoRGBA16U(&rgb, nil, 0xFFFF, &dst, false, vImage_Flags(kvImageNoFlags))
        } else {
            vImageConvert_RGB16UtoRGBA16U(&src, nil, 0xFFFF, &dst, false, vImage_Flags(kvImageNoFlags))
        }
    }

    /// Extract interleaved RGB 16-bit pixels from an RGBA16 CVPixelBuffer.
    public func toRGB16Pixels() -> (pixels: [UInt16], width: Int, height: Int, channels: Int)? {
        let w = CVPixelBufferGetWidth(self)
        let h = CVPixelBufferGetHeight(self)

        CVPixelBufferLockBaseAddress(self, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(self, .readOnly) }

        guard let baseAddress = CVPixelBufferGetBaseAddress(self) else { return nil }
        let srcBytesPerRow = CVPixelBufferGetBytesPerRow(self)

        let rgbCount = w * h * 3
        let result = [UInt16](unsafeUninitializedCapacity: rgbCount) { dstBuf, count in
            var src = vImage_Buffer(
                data: baseAddress, height: vImagePixelCount(h),
                width: vImagePixelCount(w), rowBytes: srcBytesPerRow
            )
            var dst = vImage_Buffer(
                data: dstBuf.baseAddress!, height: vImagePixelCount(h),
                width: vImagePixelCount(w),
                rowBytes: w * 3 * MemoryLayout<UInt16>.stride
            )
            vImageConvert_RGBA16UtoRGB16U(&src, &dst, vImage_Flags(kvImageNoFlags))
            count = rgbCount
        }

        return (result, w, h, 3)
    }
}
