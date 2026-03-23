import CoreImage
import CoreVideo
import CoreGraphics

/// Loads images from files into Metal-compatible CVPixelBuffers.
/// Handles standard formats (PNG, JPEG, TIFF, HEIF) and RAW formats
/// (CR2, NEF, ARW, ORF, etc.) via Apple's CIRAWFilter.
public struct ImageReader: Sendable {
    private let ciContext: CIContext

    public init(ciContext: CIContext) {
        self.ciContext = ciContext
    }

    /// Load an image, returning both the pixel buffer and detected source bit depth.
    public func loadWithBitDepth(url: URL) throws -> (CVPixelBuffer, Int) {
        guard var ciImage = CIImage(contentsOf: url) else {
            throw ImageError.readFailed(url.lastPathComponent)
        }

        if ciImage.extent.origin != .zero {
            ciImage = ciImage.transformed(by: CGAffineTransform(
                translationX: -ciImage.extent.origin.x,
                y: -ciImage.extent.origin.y
            ))
        }

        let width = Int(ciImage.extent.width)
        let height = Int(ciImage.extent.height)

        let depth = ciImage.properties[kCGImagePropertyDepth as String] as? Int ?? 8
        let isHighBitDepth = depth > 8

        // Always use 16-bit to preserve precision after linearization.
        // 8-bit linear values lose too much detail in darks.
        // Confirmed: This is really important.
        let pixelFormat: OSType = kCVPixelFormatType_64RGBALE

        guard let buffer = CVPixelBuffer.create(width: width, height: height, pixelFormat: pixelFormat) else {
            throw ImageError.contextCreationFailed
        }

        ciContext.render(ciImage, to: buffer, bounds: ciImage.extent,
                         colorSpace: CGColorSpace(name: CGColorSpace.linearSRGB))
        
        
        return (buffer, isHighBitDepth ? 16 : 8)
    }

    /// Load a Camera RAW file via Apple's CIRAWFilter.
    /// Returns sRGB-corrected 16-bit pixel buffer (Apple handles all demosaic + color science).
    public func loadRAW(url: URL) throws -> CVPixelBuffer {
        guard let filter = CIRAWFilter(imageURL: url) else {
            throw ImageError.readFailed(url.lastPathComponent)
        }
        guard let ciImage = filter.outputImage else {
            throw ImageError.decodeFailed(url.lastPathComponent)
        }

        let width = Int(ciImage.extent.width)
        let height = Int(ciImage.extent.height)

        guard let buffer = CVPixelBuffer.create(width: width, height: height, pixelFormat: kCVPixelFormatType_64RGBALE) else {
            throw ImageError.contextCreationFailed
        }

        ciContext.render(ciImage, to: buffer, bounds: ciImage.extent,
                         colorSpace: CGColorSpace(name: CGColorSpace.linearSRGB))
        return buffer
    }
}
