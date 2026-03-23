import Metal
import MetalPerformanceShaders
import CoreGraphics
import DeptheosShaders

/// Shared GPU pipelines for stacking operations.
///
/// Color conversion, texture copy, make opaque, and crop — used across
/// Pyramid, Depth, and Wavelet stacking algorithms.
/// Port from deptheos-rust/deptheos_core/src/gpu/stacking/mod.rs
public class StackingPipelines {
    public let metalContext: MetalContext

    // Color conversions via MPS
    public let srgbToLinearConversion: MPSImageConversion
    public let linearToSrgbConversion: MPSImageConversion

    // Texture operations
    public let copyTexture: MTLComputePipelineState
    public let makeOpaque: MTLComputePipelineState
    public let clearTexture: MTLComputePipelineState

    private let library: MTLLibrary

    public init(metalContext: MetalContext) throws {
        self.metalContext = metalContext
        self.library = metalContext.defaultLibrary

        // MPS color conversions (sRGB ↔ linear sRGB)
        let srgbCS = CGColorSpace(name: CGColorSpace.sRGB)!
        let linearCS = CGColorSpace(name: CGColorSpace.linearSRGB)!

        let srgbToLinearInfo = CGColorConversionInfo(src: srgbCS, dst: linearCS)
        self.srgbToLinearConversion = MPSImageConversion(
            device: metalContext.device, srcAlpha: .alphaIsOne, destAlpha: .alphaIsOne,
            backgroundColor: nil, conversionInfo: srgbToLinearInfo)

        let linearToSrgbInfo = CGColorConversionInfo(src: linearCS, dst: srgbCS)
        self.linearToSrgbConversion = MPSImageConversion(
            device: metalContext.device, srcAlpha: .alphaIsOne, destAlpha: .alphaIsOne,
            backgroundColor: nil, conversionInfo: linearToSrgbInfo)

        self.copyTexture = try Self.makePipeline(library: library, device: metalContext.device, name: "copy_texture")
        self.makeOpaque = try Self.makePipeline(library: library, device: metalContext.device, name: "make_opaque")
        self.clearTexture = try Self.makePipeline(library: library, device: metalContext.device, name: "clear_texture")
    }

    /// Access the compiled shader library (for algorithm-specific pipelines to load from).
    public var shaderLibrary: MTLLibrary { library }

    // MARK: - Convenience Encoders

    /// Encode sRGB→linear conversion, allocating and returning a new texture.
    public func encodeSrgbToLinear(
        commandBuffer: MTLCommandBuffer,
        src: MTLTexture
    ) throws -> MTLTexture {
        let dst = try makeWorkingTexture(
            width: src.width, height: src.height,
            format: .rgba16Unorm,
            usage: [.shaderRead, .shaderWrite],
            label: "srgb_to_linear"
        )

        srgbToLinearConversion.encode(commandBuffer: commandBuffer, sourceTexture: src, destinationTexture: dst)

        return dst
    }

    /// Encode linear→sRGB conversion from src into dst via MPS.
    public func encodeLinearToSrgb(
        commandBuffer: MTLCommandBuffer,
        src: MTLTexture,
        dst: MTLTexture
    ) {
        linearToSrgbConversion.encode(commandBuffer: commandBuffer, sourceTexture: src, destinationTexture: dst)
    }

    /// Clear a texture to a constant value using a compute shader.
    /// Unlike render-pass clears, this does not require `.renderTarget` usage,
    /// preserving Apple Silicon's Lossless Texture Compression.
    public func encodeComputeClear(
        encoder: MTLComputeCommandEncoder,
        texture: MTLTexture,
        value: SIMD4<Float>
    ) {
        var clearValue = value
        encoder.setComputePipelineState(clearTexture)
        encoder.setTexture(texture, index: 0)
        encoder.setBytes(&clearValue, length: MemoryLayout<SIMD4<Float>>.size, index: 0)
        encoder.dispatchGrid(pipeline: clearTexture, width: texture.width, height: texture.height)
    }

    // MARK: - Helpers

    public func makeWorkingTexture(
        width: Int,
        height: Int,
        format: MTLPixelFormat = .rgba16Float,
        usage: MTLTextureUsage = [.shaderRead, .shaderWrite],
        label: String? = nil
    ) throws -> MTLTexture {
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: format,
            width: width,
            height: height,
            mipmapped: false
        )
        desc.usage = usage
        desc.storageMode = .private
        guard let texture = metalContext.device.makeTexture(descriptor: desc) else {
            throw MetalContextError.metalError("Failed to allocate \(width)×\(height) \(format) texture")
        }
        if let label { texture.label = label }
        return texture
    }

    /// Create a 1x1 dummy texture for use when releasing result textures.
    /// Avoids optionals in finalized resource structs while freeing GPU memory.
    public func makeDummyTexture() throws -> MTLTexture {
        try makeWorkingTexture(
            width: 1, height: 1,
            format: .rgba8Unorm,
            usage: .shaderRead,
            label: "released_result"
        )
    }

    // MARK: - Private

    private static func makePipeline(library: MTLLibrary, device: MTLDevice, name: String) throws -> MTLComputePipelineState {
        guard let function = library.makeFunction(name: name) else {
            throw StackingError.metalError("Shader function not found: \(name)")
        }
        return try device.makeComputePipelineState(function: function)
    }
}
