import Accelerate
import Foundation
import DeptheosKit
import Testing

/// Compute PSNR (Peak Signal-to-Noise Ratio) between two 16-bit pixel arrays.
/// Returns `Double.infinity` if the arrays are identical.
/// Typical thresholds: >40 dB visually identical, >60 dB essentially lossless.
func psnr(_ a: [UInt16], _ b: [UInt16]) -> Double {
    precondition(a.count == b.count, "Pixel arrays must be the same size")
    var mse: Double = 0
    for i in a.indices {
        let diff = Double(Int(a[i]) - Int(b[i]))
        mse += diff * diff
    }
    mse /= Double(a.count)
    if mse == 0 { return .infinity }
    let maxVal: Double = 65535
    return 10.0 * log10((maxVal * maxVal) / mse)
}

/// Strip alpha from RGBA16 pixels to produce RGB16 using vImage.
func stripAlpha(rgba: [UInt16], width: Int, height: Int) -> [UInt16] {
    var rgb = [UInt16](repeating: 0, count: width * height * 3)
    rgba.withUnsafeBufferPointer { srcBuf in
        rgb.withUnsafeMutableBufferPointer { dstBuf in
            var src = vImage_Buffer(
                data: UnsafeMutableRawPointer(mutating: srcBuf.baseAddress!),
                height: vImagePixelCount(height),
                width: vImagePixelCount(width),
                rowBytes: width * 4 * MemoryLayout<UInt16>.stride
            )
            var dst = vImage_Buffer(
                data: dstBuf.baseAddress!,
                height: vImagePixelCount(height),
                width: vImagePixelCount(width),
                rowBytes: width * 3 * MemoryLayout<UInt16>.stride
            )
            vImageConvert_RGBA16UtoRGB16U(&src, &dst, vImage_Flags(kvImageNoFlags))
        }
    }
    return rgb
}

/// Compare all metadata fields except colorScience, luts, and noiseProfile
/// (which may have float precision differences after a round-trip).
func expectMetadataEqual(
    _ a: DeptheosDng.ImageMetadata,
    _ b: DeptheosDng.ImageMetadata
) {
    #expect(a.camera == b.camera)
    #expect(a.lens == b.lens)
    #expect(a.exposure == b.exposure)
    #expect(a.dates == b.dates)
    #expect(a.gps == b.gps)
    #expect(a.geometry == b.geometry)
    #expect(a.opcodes == b.opcodes)
}
