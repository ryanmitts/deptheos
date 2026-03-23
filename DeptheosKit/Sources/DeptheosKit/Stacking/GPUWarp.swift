import Metal
import simd
import DeptheosShaders

/// GPU pipeline for affine image warping with Lanczos interpolation.
public class GPUWarp {
    private let pipeline: MTLComputePipelineState

    public init(library: MTLLibrary, device: MTLDevice) throws {
        guard let function = library.makeFunction(name: "warp_lanczos") else {
            throw StackingError.metalError("Shader function not found: warp_lanczos")
        }
        self.pipeline = try device.makeComputePipelineState(function: function)
    }

    /// Encode a warp operation into a command buffer.
    ///
    /// - Parameters:
    ///   - commandBuffer: Command buffer to encode into
    ///   - src: Source texture (any readable format)
    ///   - dst: Destination texture (rgba16Float, shaderWrite)
    ///   - transform: Affine transform (backward mapping: dst → src coordinates)
    ///   - applySrgb: If true, apply linear→sRGB gamma after interpolation
    ///   - lanczosA: Lanczos lobe count (3 or 5, default 3)
    public func encodeWarp(
        commandBuffer: MTLCommandBuffer,
        src: MTLTexture,
        dst: MTLTexture,
        transform: AffineTransform2D,
        applySrgb: Bool,
        lanczosA: UInt32 = 3
    ) throws {
        let (col0, col1, col2) = transform.gpuColumns
        var params = WarpParams(
            col0: col0, col1: col1, col2: col2,
            lanczos_a: lanczosA,
            apply_srgb: applySrgb ? 1 : 0
        )

        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for warp_lanczos")
        }
        encoder.label = "warp_lanczos"
        encoder.setComputePipelineState(pipeline)
        encoder.setTexture(src, index: 0)
        encoder.setTexture(dst, index: 1)
        encoder.setBytes(&params, length: MemoryLayout<WarpParams>.stride, index: 0)
        encoder.dispatchGrid(pipeline: pipeline, width: dst.width, height: dst.height)
        encoder.endEncoding()
    }
}
