import Metal
import MetalPerformanceShaders
import CoreVideo

// MARK: - GPU ECC Pipelines

/// Shared Metal compute pipelines and MPS filters for GPU-accelerated ECC alignment.
/// Compiled once, shared across all pyramid levels. Only the Gaussian blur varies per level.
final class ECCPipelines: @unchecked Sendable {
    let context: MetalContext

    let warpTriple: MTLComputePipelineState
    let hessianPhase1: MTLComputePipelineState
    let projectionPhase1: MTLComputePipelineState
    let statsPhase1: MTLComputePipelineState
    let statsPhase2: MTLComputePipelineState

    let grayscaleConversion: MPSImageConversion
    let scaler: MPSImageBilinearScale
    let sobelX: MPSImageConvolution
    let sobelY: MPSImageConvolution

    /// Per-level blur filter, set before calling runECC.
    var gaussianBlur: MPSImageGaussianBlur

    init(context: MetalContext, initialBlurSigma: Float = 0.01) throws {
        self.context = context
        let lib = context.defaultLibrary
        let dev = context.device
        self.warpTriple = try Self.makePipeline(library: lib, device: dev, name: "ecc_warp_triple")
        self.hessianPhase1 = try Self.makePipeline(library: lib, device: dev, name: "ecc_hessian_phase1")
        self.projectionPhase1 = try Self.makePipeline(library: lib, device: dev, name: "ecc_projection_phase1")
        self.statsPhase1 = try Self.makePipeline(library: lib, device: dev, name: "ecc_stats_phase1")
        self.statsPhase2 = try Self.makePipeline(library: lib, device: dev, name: "ecc_stats_phase2")

        let srcCS = CGColorSpace(name: CGColorSpace.sRGB)!
        let dstCS = CGColorSpace(name: CGColorSpace.linearGray)!
        let conversionInfo = CGColorConversionInfo(src: srcCS, dst: dstCS)
        self.grayscaleConversion = MPSImageConversion(
            device: context.device, srcAlpha: .alphaIsOne, destAlpha: .alphaIsOne,
            backgroundColor: nil, conversionInfo: conversionInfo)
        self.grayscaleConversion.label = "ecc_grayscale"

        self.scaler = MPSImageBilinearScale(device: context.device)
        self.scaler.label = "ecc_scaler"
        self.gaussianBlur = MPSImageGaussianBlur(device: context.device, sigma: initialBlurSigma)
        self.gaussianBlur.edgeMode = .clamp
        self.gaussianBlur.label = "ecc_gaussian_blur"

        var kernelX: [Float] = [-0.5, 0, 0.5]
        var kernelY: [Float] = [-0.5, 0, 0.5]
        self.sobelX = MPSImageConvolution(device: context.device, kernelWidth: 3, kernelHeight: 1, weights: &kernelX)
        self.sobelX.edgeMode = .clamp
        self.sobelX.label = "ecc_sobel_x"
        self.sobelY = MPSImageConvolution(device: context.device, kernelWidth: 1, kernelHeight: 3, weights: &kernelY)
        self.sobelY.edgeMode = .clamp
        self.sobelY.label = "ecc_sobel_y"
    }

    func makeBlurFilter(sigma: Float) -> MPSImageGaussianBlur {
        let blur = MPSImageGaussianBlur(device: context.device, sigma: max(sigma, 0.01))
        blur.edgeMode = .clamp
        return blur
    }

    private static func makePipeline(library: MTLLibrary, device: MTLDevice, name: String) throws -> MTLComputePipelineState {
        guard let function = library.makeFunction(name: name) else {
            throw GPUECCError.allocationFailed("Shader function not found: \(name)")
        }
        return try device.makeComputePipelineState(function: function)
    }
}
