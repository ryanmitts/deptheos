import Metal
import CoreVideo

// MARK: - CVPixelBuffer → MTLTexture

extension CVPixelBuffer {

    /// Convert to MTLTexture via texture cache (zero-copy).
    ///
    /// **Important:** The returned `MTLTexture` is only valid while the `CVMetalTexture`
    /// reference is alive. Use `metalTextureWithRef` if you need to keep the texture beyond
    /// the immediate scope, or blit to a standalone texture.
    ///
    /// - Parameters:
    ///   - cache: Metal texture cache.
    ///   - usage: Optional texture usage flags. Pass `.shaderWrite` for compute shader destinations.
    public func metalTexture(using cache: CVMetalTextureCache, usage: MTLTextureUsage? = nil, label: String? = nil) -> MTLTexture? {
        metalTextureWithRef(using: cache, usage: usage, label: label)?.texture
    }

    /// Convert to MTLTexture via texture cache, returning both the
    /// `MTLTexture` and the backing `CVMetalTexture` reference that must be kept alive.
    public func metalTextureWithRef(using cache: CVMetalTextureCache, usage: MTLTextureUsage? = nil, label: String? = nil) -> (texture: MTLTexture, ref: CVMetalTexture)? {
        var cvTexture: CVMetalTexture?
        let w = CVPixelBufferGetWidth(self)
        let h = CVPixelBufferGetHeight(self)
        let format = CVPixelBufferGetPixelFormatType(self)

        let mtlFormat: MTLPixelFormat
        switch format {
        case kCVPixelFormatType_32BGRA:
            mtlFormat = .bgra8Unorm
        case kCVPixelFormatType_64RGBALE:
            mtlFormat = .rgba16Unorm
        case kCVPixelFormatType_128RGBAFloat:
            mtlFormat = .rgba32Float
        case kCVPixelFormatType_OneComponent8:
            mtlFormat = .r8Unorm
        case kCVPixelFormatType_OneComponent32Float:
            mtlFormat = .r32Float
        default:
            assertionFailure("Unsupported pixel format: \(format.description)")
            return nil
        }

        let textureAttributes: CFDictionary?
        if let usage = usage {
            textureAttributes = [kCVMetalTextureUsage as String: usage.rawValue] as CFDictionary
        } else {
            textureAttributes = nil
        }

        CVMetalTextureCacheCreateTextureFromImage(nil, cache, self, textureAttributes, mtlFormat, w, h, 0, &cvTexture)

        guard let tex = cvTexture, let mtlTex = CVMetalTextureGetTexture(tex) else { return nil }
        if let label { mtlTex.label = label }
        return (texture: mtlTex, ref: tex)
    }

    /// Upload to a standalone GPU texture via blit copy.
    ///
    /// Unlike `metalTexture(using:)`, the returned texture has no dependency on
    /// the CVMetalTexture cache — it is safe to use indefinitely.
    /// The CVMetalTexture backing is kept alive via the command buffer's completion
    /// handler, guaranteeing the GPU can read from it before it's released.
    public func standaloneMetalTexture(
        device: MTLDevice,
        commandQueue: MTLCommandQueue,
        cache: CVMetalTextureCache,
        label: String? = nil
    ) async -> MTLTexture? {
        guard let result = metalTextureWithRef(using: cache) else { return nil }

        let w = result.texture.width
        let h = result.texture.height
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: result.texture.pixelFormat,
            width: w, height: h, mipmapped: false
        )
        desc.usage = [.shaderRead]
        desc.storageMode = .private

        guard let standalone = device.makeTexture(descriptor: desc),
              let commandBuffer = commandQueue.makeCommandBuffer(),
              let blit = commandBuffer.makeBlitCommandEncoder() else { return nil }

        if let label { standalone.label = label }
        blit.copy(from: result.texture, to: standalone)
        blit.endEncoding()

        // Capture the CVMetalTexture ref in the completion handler so ARC cannot
        // release it at the await suspension point before the GPU finishes reading.
        let ref = result.ref
        commandBuffer.addCompletedHandler { _ in
            _ = ref
        }

        commandBuffer.commit()
        await commandBuffer.completed()
        return standalone
    }
}

// MARK: - CGImage → MTLTexture

extension CGImage {

    /// Create an MTLTexture from this image (shared storage, suitable for display placeholders).
    public func metalTexture(device: MTLDevice) -> MTLTexture? {
        let w = width
        let h = height
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: w, height: h, mipmapped: false
        )
        desc.storageMode = .shared
        desc.usage = [.shaderRead]
        guard let texture = device.makeTexture(descriptor: desc) else { return nil }

        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
        guard let ctx = CGContext(
            data: nil, width: w, height: h,
            bitsPerComponent: 8, bytesPerRow: w * 4,
            space: colorSpace,
            bitmapInfo: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue
        ) else { return nil }

        ctx.draw(self, in: CGRect(x: 0, y: 0, width: w, height: h))
        guard let data = ctx.data else { return nil }

        texture.replace(
            region: MTLRegion(origin: MTLOrigin(), size: MTLSize(width: w, height: h, depth: 1)),
            mipmapLevel: 0,
            withBytes: data,
            bytesPerRow: w * 4
        )
        return texture
    }
}

// MARK: - MTLTexture Export & Readback

extension MTLTexture {

    /// Export to a CVPixelBuffer via blit copy.
    ///
    /// Supports RGBA formats (`.rgba16Unorm`, `.bgra8Unorm`, `.rgba32Float`)
    /// and grayscale (`.r32Float`).
    public func pixelBuffer(
        cache: CVMetalTextureCache,
        commandQueue: MTLCommandQueue,
        pixelFormat: MTLPixelFormat
    ) async -> CVPixelBuffer? {
        let pixelFormatType: OSType
        switch pixelFormat {
        case .rgba16Unorm:
            pixelFormatType = kCVPixelFormatType_64RGBALE
        case .bgra8Unorm:
            pixelFormatType = kCVPixelFormatType_32BGRA
        case .rgba32Float:
            pixelFormatType = kCVPixelFormatType_128RGBAFloat
        case .r32Float:
            pixelFormatType = kCVPixelFormatType_OneComponent32Float
        default:
            assertionFailure("Unsupported MTLPixelFormat for pixel buffer export")
            return nil
        }

        var pb: CVPixelBuffer?
        let attrs: [CFString: Any] = [kCVPixelBufferMetalCompatibilityKey: true]
        CVPixelBufferCreate(nil, width, height, pixelFormatType, attrs as CFDictionary, &pb)
        guard let pixelBuffer = pb else { return nil }

        var cvTexture: CVMetalTexture?
        CVMetalTextureCacheCreateTextureFromImage(nil, cache, pixelBuffer, nil, pixelFormat, width, height, 0, &cvTexture)

        guard let tex = cvTexture,
              let destTexture = CVMetalTextureGetTexture(tex),
              let cmd = commandQueue.makeCommandBuffer(),
              let blit = cmd.makeBlitCommandEncoder() else { return nil }

        destTexture.label = "export_dest"
        blit.copy(from: self, to: destTexture)
        blit.endEncoding()
        cmd.commit()
        await cmd.completed()

        return pixelBuffer
    }

    /// Read raw pixel bytes from this texture.
    ///
    /// For GPU-private textures, this creates a staging blit via the command queue.
    /// For shared/managed textures, it reads directly.
    ///
    /// - Returns: Raw pixel bytes (format matches the texture's pixelFormat).
    public func readPixelData(commandQueue: MTLCommandQueue) async -> Data? {
        let bpp = pixelFormat.bytesPerPixel
        guard bpp > 0 else { return nil }

        let bytesPerRow = width * bpp
        let totalBytes = bytesPerRow * height

        if storageMode == .private {
            let desc = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: pixelFormat,
                width: width, height: height, mipmapped: false
            )
            desc.storageMode = .shared
            desc.usage = []
            guard let staging = device.makeTexture(descriptor: desc),
                  let cmd = commandQueue.makeCommandBuffer(),
                  let blit = cmd.makeBlitCommandEncoder() else { return nil }

            blit.copy(from: self, to: staging)
            blit.endEncoding()
            cmd.commit()
            await cmd.completed()

            var data = Data(count: totalBytes)
            data.withUnsafeMutableBytes { rawBuf in
                staging.getBytes(rawBuf.baseAddress!, bytesPerRow: bytesPerRow,
                                 from: MTLRegion(origin: MTLOrigin(), size: MTLSize(width: width, height: height, depth: 1)),
                                 mipmapLevel: 0)
            }
            return data
        } else {
            var data = Data(count: totalBytes)
            data.withUnsafeMutableBytes { rawBuf in
                self.getBytes(rawBuf.baseAddress!, bytesPerRow: bytesPerRow,
                              from: MTLRegion(origin: MTLOrigin(), size: MTLSize(width: width, height: height, depth: 1)),
                              mipmapLevel: 0)
            }
            return data
        }
    }
}

// MARK: - MTLDevice Pixel Upload

extension MTLDevice {

    /// Upload raw pixel data to a new GPU-private texture.
    public func uploadPixelData(
        _ data: Data,
        width: Int,
        height: Int,
        pixelFormat: MTLPixelFormat,
        commandQueue: MTLCommandQueue
    ) async -> MTLTexture? {
        let bpp = pixelFormat.bytesPerPixel
        guard bpp > 0 else { return nil }

        let bytesPerRow = width * bpp

        let stagingDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: pixelFormat,
            width: width, height: height, mipmapped: false
        )
        stagingDesc.storageMode = .shared
        stagingDesc.usage = []
        guard let staging = makeTexture(descriptor: stagingDesc) else { return nil }

        data.withUnsafeBytes { rawBuf in
            staging.replace(
                region: MTLRegion(origin: MTLOrigin(), size: MTLSize(width: width, height: height, depth: 1)),
                mipmapLevel: 0,
                withBytes: rawBuf.baseAddress!,
                bytesPerRow: bytesPerRow
            )
        }

        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: pixelFormat,
            width: width, height: height, mipmapped: false
        )
        desc.storageMode = .private
        desc.usage = [.shaderRead]
        guard let tex = makeTexture(descriptor: desc),
              let cmd = commandQueue.makeCommandBuffer(),
              let blit = cmd.makeBlitCommandEncoder() else { return nil }

        blit.copy(from: staging, to: tex)
        blit.endEncoding()
        cmd.commit()
        await cmd.completed()
        return tex
    }
}

// MARK: - MTLPixelFormat Helpers

extension MTLPixelFormat {
    /// Bytes per pixel for common texture formats.
    var bytesPerPixel: Int {
        switch self {
        case .rgba16Float, .rgba16Unorm: 8   // 4 × 2 bytes
        case .rgba32Float: 16                 // 4 × 4 bytes
        case .bgra8Unorm, .rgba8Unorm: 4     // 4 × 1 byte
        case .r32Float: 4
        case .r16Float, .r16Unorm: 2
        case .r8Unorm: 1
        default: 0
        }
    }
}

// MARK: - Pipeline Threadgroup Size

extension MTLComputePipelineState {
    /// Optimal 2D threadgroup size based on SIMD width and max occupancy.
    public var optimalThreadgroupSize: MTLSize {
        let w = threadExecutionWidth
        let h = maxTotalThreadsPerThreadgroup / w
        return MTLSize(width: w, height: h, depth: 1)
    }
}

// MARK: - Declarative Compute Dispatch

extension MTLComputeCommandEncoder {

    /// Dispatch a 2D compute grid using the pipeline's optimal threadgroup size.
    public func dispatchGrid(pipeline: MTLComputePipelineState, width: Int, height: Int) {
        let threadsPerGrid = MTLSize(width: width, height: height, depth: 1)
        dispatchThreads(threadsPerGrid, threadsPerThreadgroup: pipeline.optimalThreadgroupSize)
    }

    /// Standard 2D grid dispatch with debug label and pipeline setup.
    public func dispatch(
        _ label: String,
        pipeline: MTLComputePipelineState,
        width: Int, height: Int,
        _ bindings: (MTLComputeCommandEncoder) -> Void
    ) {
        pushDebugGroup(label)
        setComputePipelineState(pipeline)
        bindings(self)
        dispatchThreads(
            MTLSize(width: width, height: height, depth: 1),
            threadsPerThreadgroup: pipeline.optimalThreadgroupSize
        )
        popDebugGroup()
    }

    /// Custom threadgroup dispatch (1D IIR filters, etc.) with debug label and pipeline setup.
    public func dispatch(
        _ label: String,
        pipeline: MTLComputePipelineState,
        threadgroups: MTLSize,
        threadsPerThreadgroup: MTLSize,
        _ bindings: (MTLComputeCommandEncoder) -> Void
    ) {
        pushDebugGroup(label)
        setComputePipelineState(pipeline)
        bindings(self)
        dispatchThreadgroups(threadgroups, threadsPerThreadgroup: threadsPerThreadgroup)
        popDebugGroup()
    }

    /// Single-thread dispatch (e.g. histogram reduction) with debug label and pipeline setup.
    public func dispatchSingle(
        _ label: String,
        pipeline: MTLComputePipelineState,
        _ bindings: (MTLComputeCommandEncoder) -> Void
    ) {
        pushDebugGroup(label)
        setComputePipelineState(pipeline)
        bindings(self)
        dispatchThreadgroups(
            MTLSize(width: 1, height: 1, depth: 1),
            threadsPerThreadgroup: MTLSize(width: 1, height: 1, depth: 1)
        )
        popDebugGroup()
    }
}
