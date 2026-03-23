import Metal
import MetalPerformanceShaders
import DeptheosShaders
import os

private let depthLogger = Logger(subsystem: "com.deptheos.kit", category: "DepthStacker")
private let enableOpacityDiagnostics = false

/// Configuration for the depth stacking algorithm.
public struct DepthConfig: Sendable {
    /// Bandpass filter radius in pixels. 1-25. Default: 8.0
    public var radius: Float

    /// Total number of frames. Must be set before processing.
    public var totalFrames: Int

    /// Depth map smoothing strength. 0 = off, 50 = maximum. Default: 5.0
    /// Maps to IIR spatial sigma internally.
    public var smoothing: Float

    /// Blend range. 0-10. 10 = smooth transitions, 0 = hard plane snap. Default: 5.0
    public var blendRange: Float

    public init(
        radius: Float = 8.0,
        totalFrames: Int = 0,
        smoothing: Float = 5.0,
        blendRange: Float = 5.0
    ) {
        self.radius = radius
        self.totalFrames = totalFrames
        self.smoothing = smoothing
        self.blendRange = blendRange
    }

    /// Spatial sigma derived from the smoothing slider (Gastal & Oliveira, σ_s).
    var sigmaS: Float { smoothing * 10.0 }

    /// Range sigma — controls edge sensitivity of the guide signal (σ_r).
    /// The guide (opacity map) is in [0,1], so 0.2 preserves edges where confidence
    /// changes by ~20% or more while smoothing through gradual transitions.
    static let sigmaR: Float = 0.2

    /// Fixed spatial sigma for the domain transform (edge detection).
    /// Decoupled from the smoothing slider so sr_ratio stays constant.
    /// The paper couples them (sr_ratio = σ_s/σ_r, Eq. 11), but that makes
    /// high smoothing resist even tiny background gradients. Subject protection
    /// is handled by the confidence weight clamp instead.
    /// Set to 0 to disable gradient-based edge detection entirely (d=1 everywhere),
    /// relying solely on the confidence weight clamp for subject protection.
    static let domainSigmaS: Float = 25.0

    /// Max blend weight at confidence=0 (background/uncertain regions).
    /// 1.0 = unclamped, full DT-RF smoothing.
    static let bgMaxWeight: Float = 1.0

    /// Max blend weight at confidence=1 (subject/confident regions).
    /// Lower = more subject protection. 0.5 caps blending at 50% per step.
    static let subjectMaxWeight: Float = 0.5

    /// Number of DT RF iterations. 3 matches the paper's recommendation (Section 5.1).
    static let iterations: Int = 3
}

/// State machine for the depth stacker lifecycle.
private enum DepthState {
    case uninitialized
    case accumulating(DepthAccumResources)
    case blending(DepthBlendResources)
    case finalized(DepthFinalizedResources)
}

/// Depth stacking algorithm.
///
/// Two-pass algorithm:
/// 1. Accumulate: build depth map via bandpass focus metric + WTA
/// 2. Finalize: smooth depth map with cascaded IIR recursive filter (DT-RF),
///    guided by the opacity/confidence map
/// 3. Blend: composite frames using depth map with configurable blend range
public final class DepthStacker: BlendableAlgorithm, @unchecked Sendable {
    private let metalContext: MetalContext
    private let pipelines: DepthPipelines
    private let config: DepthConfig
    private var state: DepthState = .uninitialized
    public private(set) var frameCount: Int = 0

    // Pre-created MPS kernels
    private let preBlur: MPSImageGaussianBlur
    private let postBlur: MPSImageGaussianBlur
    private let histogramKernel: MPSImageHistogram
    private let sobelKernel: MPSImageSobel
    private let minMaxKernel: MPSImageStatisticsMinAndMax
    private let gradientStatsKernel: MPSImageStatisticsMeanAndVariance

    public init(metalContext: MetalContext, config: DepthConfig, stackingPipelines: StackingPipelines? = nil) throws {
        self.metalContext = metalContext
        self.config = config
        let common = try stackingPipelines ?? StackingPipelines(metalContext: metalContext)
        self.pipelines = try DepthPipelines(common: common)

        self.preBlur = self.pipelines.makePreBlur(radius: config.radius, label: "depth_pre_blur")
        self.postBlur = self.pipelines.makePostBlur(radius: config.radius, label: "depth_post_blur")
        self.histogramKernel = self.pipelines.makeHistogramKernel(label: "depth_histogram")
        self.sobelKernel = self.pipelines.makeSobelKernel()
        self.minMaxKernel = self.pipelines.makeMinMaxKernel()
        self.gradientStatsKernel = self.pipelines.makeGradientStatsKernel()
    }

    // MARK: - BlendableAlgorithm

    public func accumulate(image: DeptheosImage, transform: AffineTransform2D?) throws {
        if case .uninitialized = state {
            let resources = try DepthEncoding.allocateAccumResources(
                metalContext: metalContext, pipelines: pipelines,
                width: image.width, height: image.height, totalFrames: config.totalFrames
            )
            state = .accumulating(resources)
        }

        guard case .accumulating(let r) = state else {
            throw StackingError.invalidState("Expected Accumulating state")
        }

        let sourceTexture = try image.getTexture(cache: metalContext.textureCache)

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "depth_accumulate"

        try DepthEncoding.encodeAccumulate(
            commandBuffer: commandBuffer, resources: r, pipelines: pipelines,
            sourceTexture: sourceTexture, transform: transform,
            frameCount: frameCount, radius: config.radius,
            preBlur: preBlur, postBlur: postBlur
        )

        commandBuffer.commit()
        frameCount += 1
    }

    public func runFinalizationPipeline() async throws {
        if frameCount == 0 {
            throw StackingError.noFrames
        }

        guard case .accumulating(let accum) = state else {
            throw StackingError.invalidState("runFinalizationPipeline() requires Accumulating state")
        }

        // Phase 1: Opacity normalization
        guard let cb1 = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        cb1.label = "depth_opacity_normalization"
        try DepthEncoding.encodeOpacityNormalization(
            commandBuffer: cb1, resources: accum,
            pipelines: pipelines, histogramKernel: histogramKernel
        )
        cb1.commit()
        await cb1.completed()

        // Phase 1.5: Opacity map diagnostics (validates σ_r = 0.2)
        if enableOpacityDiagnostics {
            try await logOpacityDiagnostics(resources: accum)
        }

        // Phase 2: IIR smoothing (multiple iterations)
        try await encodeIIRSmoothing(resources: accum)

        // Phase 3: Depth feathering — snap depth values toward frame steps.
        // blendRange controls transition width: 0 = hard snap, 10 = no feathering.
        let featherPower = 1.0 + (10.0 - Double(config.blendRange)) * 0.5
        if featherPower > 1.0 && config.totalFrames > 1 {
            let stepSize = 1.0 / Float(config.totalFrames - 1)
            guard let cbF = metalContext.commandQueue.makeCommandBuffer() else {
                throw StackingError.metalError("Failed to create command buffer for feathering")
            }
            cbF.label = "depth_feather"
            guard let encF = cbF.makeComputeCommandEncoder() else {
                throw StackingError.metalError("Failed to create encoder for depth_feather")
            }
            encF.label = "depth_feather"
            DepthComputePass(encoder: encF, pipelines: pipelines)
                .depthFeather(depthMap: accum.depthIndex, stepSize: stepSize, power: Float(featherPower))
            encF.endEncoding()
            cbF.commit()
            await cbF.completed()
        }

    }

    public func beginBlend() throws {
        guard case .accumulating(let accum) = state else {
            throw StackingError.invalidState("beginBlend() requires Accumulating state")
        }

        // Feathering snaps depth values to frame steps, so the blend only needs
        // to cover the narrow transition between adjacent frames.
        let planeRange: UInt32 = 1

        let blendResources = try DepthEncoding.beginBlend(
            from: accum, pipelines: pipelines, metalContext: metalContext,
            planeRange: planeRange
        )
        state = .blending(blendResources)
    }

    public func blendFrame(
        frame: DeptheosImage,
        transform: AffineTransform2D?,
        frameIndex: Int
    ) throws {
        guard case .blending(let r) = state else {
            throw StackingError.invalidState("blendFrame() requires Blending state")
        }

        let sourceTexture = try frame.getTexture(cache: metalContext.textureCache)

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "depth_blend_frame_\(frameIndex)"

        try DepthEncoding.encodeBlendFrame(
            commandBuffer: commandBuffer, resources: r, pipelines: pipelines,
            sourceTexture: sourceTexture, transform: transform, frameIndex: frameIndex
        )

        commandBuffer.commit()
    }

    public func finalize() async throws {
        guard case .blending(let blend) = state else {
            throw StackingError.invalidState("finalize() requires Blending state")
        }

        let finalized = try await DepthEncoding.encodeFinalizeBlend(
            blend: blend, pipelines: pipelines, metalContext: metalContext
        )
        state = .finalized(finalized)
    }

    public var resultTexture: MTLTexture? {
        switch state {
        case .blending(let r): return r.finalizedBlend
        case .finalized(let r): return r.resultTexture
        default: return nil
        }
    }

    public var focusMapTexture: MTLTexture? {
        switch state {
        case .accumulating(let r): return r.depthIndex
        case .blending(let r): return r.depthIndex
        case .finalized(let r): return r.depthIndexTexture
        case .uninitialized: return nil
        }
    }

    public func releaseGPUResources() {
        state = .uninitialized
    }

    public func releaseResultTexture() {
        if case .finalized(let r) = state {
            state = .finalized(DepthFinalizedResources(
                width: r.width, height: r.height,
                resultTexture: (try? pipelines.common.makeDummyTexture()) ?? r.resultTexture,
                depthIndexTexture: r.depthIndexTexture
            ))
        }
    }

    // MARK: - Opacity Map Diagnostics

    /// Log diagnostic statistics for the normalized opacity map.
    /// Validates that σ_r = 0.2 is appropriate: mean gradient should be well below 0.2,
    /// and the opacity range should span [0, 1] after normalization.
    private func logOpacityDiagnostics(resources r: DepthAccumResources) async throws {
        let device = metalContext.device

        // Tiny 2x1 output textures for stats readback (shared for CPU access)
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .r32Float, width: 2, height: 1, mipmapped: false
        )
        desc.usage = [.shaderRead, .shaderWrite]
        desc.storageMode = .shared
        guard let minMaxTex = device.makeTexture(descriptor: desc),
              let gradStatsTex = device.makeTexture(descriptor: desc) else {
            return // Non-fatal: skip diagnostics if allocation fails
        }
        minMaxTex.label = "diag_opacity_minmax"
        gradStatsTex.label = "diag_gradient_stats"

        guard let cb = metalContext.commandQueue.makeCommandBuffer() else { return }
        cb.label = "opacity_diagnostics"

        // Min/max of normalized opacity map
        minMaxKernel.encode(commandBuffer: cb, sourceTexture: r.opacityMap, destinationTexture: minMaxTex)

        // Sobel gradient magnitude → reuse focusMap scratch (R16Float, same dimensions)
        sobelKernel.encode(commandBuffer: cb, sourceTexture: r.opacityMap, destinationTexture: r.focusMap)

        // Mean/variance of the gradient magnitude
        gradientStatsKernel.encode(commandBuffer: cb, sourceTexture: r.focusMap, destinationTexture: gradStatsTex)

        cb.commit()
        await cb.completed()

        // Read back the 2x1 textures
        let region = MTLRegion(origin: MTLOrigin(), size: MTLSize(width: 2, height: 1, depth: 1))
        let bytesPerRow = 2 * MemoryLayout<Float>.size
        var minMax: [Float] = [0, 0]
        var gradStats: [Float] = [0, 0]
        minMaxTex.getBytes(&minMax, bytesPerRow: bytesPerRow, from: region, mipmapLevel: 0)
        gradStatsTex.getBytes(&gradStats, bytesPerRow: bytesPerRow, from: region, mipmapLevel: 0)

        depthLogger.info("""
            Opacity map diagnostics — \
            range: [\(minMax[0], format: .fixed(precision: 3)), \(minMax[1], format: .fixed(precision: 3))], \
            gradient mean: \(gradStats[0], format: .fixed(precision: 4)), \
            gradient variance: \(gradStats[1], format: .fixed(precision: 4))
            """)
    }

    // MARK: - IIR Smoothing

    /// Encode domain-transform recursive filter smoothing on the depth map,
    /// guided by the opacity map (Gastal & Oliveira, SIGGRAPH 2011, Section 6.3).
    ///
    /// The filter coefficient at each pixel depends on the gradient of the guide
    /// between neighbors: large opacity gradients → edge preserved, small gradients
    /// → smooth freely. Per-iteration sigma scaling follows Eq. 14.
    ///
    private func encodeIIRSmoothing(resources r: DepthAccumResources) async throws {
        let width = r.width
        let height = r.height
        let sigmaS = config.sigmaS
        let sigmaR = DepthConfig.sigmaR
        let bgMaxWeight = DepthConfig.bgMaxWeight
        let subjectMaxWeight = DepthConfig.subjectMaxWeight
        let n = DepthConfig.iterations

        if sigmaS <= 0 { return }

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer for DT RF smoothing")
        }
        commandBuffer.label = "depth_smoothing"

        for i in 0..<n {
            // Per-iteration sigma (Eq. 14): halves each iteration for convergence.
            let sigmaH_i = sigmaS * Float(3.0).squareRoot()
                * powf(2.0, Float(n - 1 - i))
                / (powf(4.0, Float(n)) - 1.0).squareRoot()
            // Feedback coefficient (Appendix): a = exp(-sqrt(2) / sigma_H)
            let logA_i = -Float(2.0).squareRoot() / sigmaH_i

            // Horizontal pass: depthIndex → grayTexture, guided by opacityMap
            guard let encH = commandBuffer.makeComputeCommandEncoder() else {
                throw StackingError.metalError("Failed to create encoder for dt_rf_horizontal")
            }
            encH.label = "dt_rf_horizontal_\(i)"
            DepthComputePass(encoder: encH, pipelines: pipelines)
                .dtRfHorizontal(
                    input: r.depthIndex, output: r.grayTexture, guide: r.opacityMap,
                    logA: logA_i, sigmaS: DepthConfig.domainSigmaS, sigmaR: sigmaR,
                    bgMaxWeight: bgMaxWeight, subjectMaxWeight: subjectMaxWeight,
                    width: width, height: height
                )
            encH.endEncoding()

            // Vertical pass: grayTexture → depthIndex, guided by opacityMap
            guard let encV = commandBuffer.makeComputeCommandEncoder() else {
                throw StackingError.metalError("Failed to create encoder for dt_rf_vertical")
            }
            encV.label = "dt_rf_vertical_\(i)"
            DepthComputePass(encoder: encV, pipelines: pipelines)
                .dtRfVertical(
                    input: r.grayTexture, output: r.depthIndex, guide: r.opacityMap,
                    logA: logA_i, sigmaS: DepthConfig.domainSigmaS, sigmaR: sigmaR,
                    bgMaxWeight: bgMaxWeight, subjectMaxWeight: subjectMaxWeight,
                    width: width, height: height
                )
            encV.endEncoding()
        }

        commandBuffer.commit()
        await commandBuffer.completed()
    }

}
