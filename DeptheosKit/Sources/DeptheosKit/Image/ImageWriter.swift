import CoreImage
import CoreVideo
import UniformTypeIdentifiers

/// Writes CVPixelBuffers to standard image formats.
public struct ImageWriter: Sendable {
    private static let ciContext = CIContext()

    public init() {}

    /// Supported output formats.
    public enum Format: String, Sendable {
        case png
        case jpeg
        case tiff
        case heif
    }

    /// Write a CVPixelBuffer to a file.
    /// - Parameters:
    ///   - pixelBuffer: The image data to write (sRGB color space expected)
    ///   - url: Destination file URL
    ///   - format: Output format
    ///   - quality: JPEG/HEIF compression quality 0.0-1.0 (default: 0.9)
    ///   - bitDepth: Source bit depth (8 or 16). PNG/TIFF use 16-bit when source is 16-bit.
    public func write(
        _ pixelBuffer: CVPixelBuffer,
        to url: URL,
        format: Format = .jpeg,
        quality: Double = 0.9,
        bitDepth: Int = 8
    ) throws {
        let ciImage = CIImage(cvPixelBuffer: pixelBuffer)
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        let highBitDepth = bitDepth > 8

        switch format {
        case .png:
            try Self.ciContext.writePNGRepresentation(
                of: ciImage, to: url,
                format: highBitDepth ? .RGBA16 : .RGBA8,
                colorSpace: colorSpace
            )
        case .jpeg:
            try Self.ciContext.writeJPEGRepresentation(
                of: ciImage, to: url,
                colorSpace: colorSpace,
                options: [kCGImageDestinationLossyCompressionQuality as CIImageRepresentationOption: quality]
            )
        case .tiff:
            try Self.ciContext.writeTIFFRepresentation(
                of: ciImage, to: url,
                format: highBitDepth ? .RGBA16 : .RGBA8,
                colorSpace: colorSpace
            )
        case .heif:
            try Self.ciContext.writeHEIFRepresentation(
                of: ciImage, to: url,
                format: highBitDepth ? .RGBA16 : .RGBA8, colorSpace: colorSpace,
                options: [kCGImageDestinationLossyCompressionQuality as CIImageRepresentationOption: quality]
            )
        }
    }
}
