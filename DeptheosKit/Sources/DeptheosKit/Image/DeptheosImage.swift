import CoreVideo
import Metal


public final class DeptheosImage: @unchecked Sendable {
    private let sourcePixelBuffer: CVPixelBuffer
    private let textureLock = NSLock()
    private var cachedTexture: MTLTexture?
    private var cachedCVTexture: CVMetalTexture?

    public let sourceBitDepth: Int
    public let sourceFormat: ImageFormat
    public let dngMetadata: DeptheosDng.ImageMetadata?
    public let dngRenderParams: DeptheosDng.RenderParams

    public var pixelBuffer: CVPixelBuffer { sourcePixelBuffer }

    public let width: Int
    public let height: Int

    public init(
        pixelBuffer: CVPixelBuffer,
        dngMetadata: DeptheosDng.ImageMetadata? = nil,
        dngRenderParams: DeptheosDng.RenderParams,
        sourceBitDepth: Int = 8,
        sourceFormat: ImageFormat = .standard
    ) {
        self.sourcePixelBuffer = pixelBuffer
        self.dngMetadata = dngMetadata
        self.dngRenderParams = dngRenderParams
        self.sourceBitDepth = sourceBitDepth
        self.sourceFormat = sourceFormat
        self.width = CVPixelBufferGetWidth(pixelBuffer)
        self.height = CVPixelBufferGetHeight(pixelBuffer)
    }

    /// MTLTexture from the backing CVPixelBuffer via CVMetalTextureCache.
    /// Cached after first call — the texture stays valid as long as this image is alive.
    public func getTexture(cache: CVMetalTextureCache) throws -> MTLTexture {
        textureLock.lock()
        defer { textureLock.unlock() }

        if let cached = cachedTexture { return cached }

        let pixelFormat: MTLPixelFormat = .rgba16Unorm

        var cvTexture: CVMetalTexture?
        let status = CVMetalTextureCacheCreateTextureFromImage(
            nil, cache, pixelBuffer, nil,
            pixelFormat, width, height, 0, &cvTexture
        )
        guard status == kCVReturnSuccess, let cvTex = cvTexture else {
            throw ImageError.textureCacheFailed(status)
        }
        guard let metalTexture = CVMetalTextureGetTexture(cvTex) else {
            throw ImageError.textureCacheFailed(status)
        }

        cachedCVTexture = cvTex
        cachedTexture = metalTexture
        return metalTexture
    }
}
