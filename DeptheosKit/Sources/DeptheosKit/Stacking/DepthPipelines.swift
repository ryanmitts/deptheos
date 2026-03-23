import Metal
import MetalPerformanceShaders

/// All pipeline states for depth stacking operations.
/// Created eagerly at init — reused for every frame.
/// Port from deptheos-rust/deptheos_core/src/gpu/depth/pipeline.rs
struct DepthPipelines {
    let common: StackingPipelines

    // Accumulation compute kernels
    let warpToGray: MTLComputePipelineState
    let rgbToGray: MTLComputePipelineState
    let bandpassDiff: MTLComputePipelineState
    let laplacianFocus: MTLComputePipelineState
    let depthWtaAccumulate: MTLComputePipelineState

    // Finalization compute kernels
    let opacityDiff: MTLComputePipelineState
    let histogramPercentile: MTLComputePipelineState
    let normalizeOpacity: MTLComputePipelineState
    let dtRfHorizontal: MTLComputePipelineState
    let dtRfVertical: MTLComputePipelineState
    let depthFeather: MTLComputePipelineState
    let wtaColorUpdate: MTLComputePipelineState

    // Blend render pipeline (vertex + fragment with additive accumulation)
    let blendRenderPipeline: MTLRenderPipelineState
    let blendNormalize: MTLComputePipelineState
    let blendFrameCompute: MTLComputePipelineState

    // Sampler for bilinear warp
    let bilinearSampler: MTLSamplerState

    init(common: StackingPipelines) throws {
        self.common = common
        let library = common.shaderLibrary
        let device = common.metalContext.device

        self.warpToGray = try Self.makeCompute(library: library, device: device, name: "warp_to_gray")
        self.rgbToGray = try Self.makeCompute(library: library, device: device, name: "rgb_to_gray")
        self.bandpassDiff = try Self.makeCompute(library: library, device: device, name: "bandpass_diff")
        self.laplacianFocus = try Self.makeCompute(library: library, device: device, name: "laplacian_focus")
        self.depthWtaAccumulate = try Self.makeCompute(library: library, device: device, name: "depth_wta_accumulate")
        self.opacityDiff = try Self.makeCompute(library: library, device: device, name: "opacity_diff")
        self.histogramPercentile = try Self.makeCompute(library: library, device: device, name: "histogram_percentile")
        self.normalizeOpacity = try Self.makeCompute(library: library, device: device, name: "normalize_opacity")
        self.dtRfHorizontal = try Self.makeCompute(library: library, device: device, name: "dt_rf_horizontal")
        self.dtRfVertical = try Self.makeCompute(library: library, device: device, name: "dt_rf_vertical")
        self.depthFeather = try Self.makeCompute(library: library, device: device, name: "depth_feather")
        self.wtaColorUpdate = try Self.makeCompute(library: library, device: device, name: "wta_color_update")

        // Blend render pipeline
        guard let vertexFn = library.makeFunction(name: "blend_vs"),
              let fragmentFn = library.makeFunction(name: "blend_fs") else {
            throw StackingError.metalError("Blend shader functions not found")
        }
        let renderDesc = MTLRenderPipelineDescriptor()
        renderDesc.vertexFunction = vertexFn
        renderDesc.fragmentFunction = fragmentFn
        renderDesc.colorAttachments[0].pixelFormat = .rgba32Float
        renderDesc.colorAttachments[0].isBlendingEnabled = true
        // Additive accumulation: pre-multiplied (color * weight, weight)
        renderDesc.colorAttachments[0].sourceRGBBlendFactor = .one
        renderDesc.colorAttachments[0].destinationRGBBlendFactor = .one
        renderDesc.colorAttachments[0].rgbBlendOperation = .add
        renderDesc.colorAttachments[0].sourceAlphaBlendFactor = .one
        renderDesc.colorAttachments[0].destinationAlphaBlendFactor = .one
        renderDesc.colorAttachments[0].alphaBlendOperation = .add
        self.blendRenderPipeline = try device.makeRenderPipelineState(descriptor: renderDesc)

        self.blendNormalize = try Self.makeCompute(library: library, device: device, name: "blend_normalize")
        self.blendFrameCompute = try Self.makeCompute(library: library, device: device, name: "blend_frame_compute")

        // Bilinear sampler for warp_to_gray
        let samplerDesc = MTLSamplerDescriptor()
        samplerDesc.minFilter = .linear
        samplerDesc.magFilter = .linear
        samplerDesc.sAddressMode = .clampToEdge
        samplerDesc.tAddressMode = .clampToEdge
        guard let sampler = device.makeSamplerState(descriptor: samplerDesc) else {
            throw StackingError.metalError("Failed to create bilinear sampler")
        }
        self.bilinearSampler = sampler
    }

    private static func makeCompute(library: MTLLibrary, device: MTLDevice, name: String) throws -> MTLComputePipelineState {
        guard let function = library.makeFunction(name: name) else {
            throw StackingError.metalError("Shader function not found: \(name)")
        }
        return try device.makeComputePipelineState(function: function)
    }

    // MARK: - MPS Kernel Factories

    /// Pre-blur Gaussian for depth accumulation (sigma = radius * 0.5, min 1.0).
    func makePreBlur(radius: Float, label: String = "depth_pre_blur") -> MPSImageGaussianBlur {
        let blur = MPSImageGaussianBlur(device: common.metalContext.device, sigma: max(radius * 0.5, 1.0))
        blur.edgeMode = .clamp
        blur.label = label
        return blur
    }

    /// Post-blur Gaussian for depth accumulation (sigma = radius * 2.0).
    func makePostBlur(radius: Float, label: String = "depth_post_blur") -> MPSImageGaussianBlur {
        let blur = MPSImageGaussianBlur(device: common.metalContext.device, sigma: radius * 2.0)
        blur.edgeMode = .clamp
        blur.label = label
        return blur
    }

    /// Histogram kernel for opacity normalization (256 bins, zero-on-encode).
    func makeHistogramKernel(label: String = "depth_histogram") -> MPSImageHistogram {
        var info = MPSImageHistogramInfo(
            numberOfHistogramEntries: 256,
            histogramForAlpha: false,
            minPixelValue: vector_float4(0, 0, 0, 0),
            maxPixelValue: vector_float4(1, 1, 1, 1))
        let kernel = MPSImageHistogram(device: common.metalContext.device, histogramInfo: &info)
        kernel.zeroHistogram = true
        kernel.label = label
        return kernel
    }

    // MARK: - Opacity Map Diagnostic Kernels

    func makeSobelKernel(label: String = "opacity_sobel") -> MPSImageSobel {
        let kernel = MPSImageSobel(device: common.metalContext.device)
        kernel.edgeMode = .clamp
        kernel.label = label
        return kernel
    }

    func makeMinMaxKernel(label: String = "opacity_minmax") -> MPSImageStatisticsMinAndMax {
        let kernel = MPSImageStatisticsMinAndMax(device: common.metalContext.device)
        kernel.label = label
        return kernel
    }

    func makeGradientStatsKernel(label: String = "gradient_stats") -> MPSImageStatisticsMeanAndVariance {
        let kernel = MPSImageStatisticsMeanAndVariance(device: common.metalContext.device)
        kernel.label = label
        return kernel
    }
}
