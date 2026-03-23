import Metal
import DeptheosShaders

/// GPU resources used during the accumulation phase.
/// All fields allocated together on first frame. Transitioning to Finalized
/// drops everything except result and focus map.
private struct PyramidAccumResources {
    let width: Int
    let height: Int
    let numLevels: Int

    // Pyramid textures (one per level)
    var scratchGaussian: [MTLTexture]
    var bestRgbPyramid: [MTLTexture]
    var bestFocusPyramid: [MTLTexture]

    // Per-level WTA exponent buffers
    var wtaParamsBuffers: [MTLBuffer]
}

/// GPU resources that survive after finalization.
private struct PyramidFinalizedResources {
    let width: Int
    let height: Int
    let resultTexture: MTLTexture
    let bestFocusL0: MTLTexture
}

/// State machine for the pyramid stacker lifecycle.
private enum PyramidState {
    case uninitialized
    case accumulating(PyramidAccumResources)
    case finalized(PyramidFinalizedResources)
}

/// Configuration for pyramid focus stacking.
public struct PyramidConfig: Sendable {
    /// Smoothness of the focus blend. Range: 1.0 (sharp) to 10.0 (smooth).
    /// Controls the WTA exponent schedule across pyramid levels.
    public var smoothness: Float

    public init(smoothness: Float = 4.0) {
        self.smoothness = smoothness
    }
}

/// Pyramid-based focus stacking algorithm.
///
/// Implements Laplacian pyramid focus stacking (Burt & Adelson 1983):
/// 1. Build Gaussian pyramid for each input frame
/// 2. Compute Laplacian (high-frequency details at each scale)
/// 3. Per-level Winner-Take-All: keep pixel with highest focus metric
/// 4. Reconstruct final image by collapsing the best Laplacian pyramid
///
/// Port from deptheos-rust/deptheos_core/src/gpu/pyramid/mod.rs
public final class PyramidStacker: StackingAlgorithm, @unchecked Sendable {
    private let metalContext: MetalContext
    private let pipelines: PyramidPipelines
    private let warpPipeline: GPUWarp
    private let config: PyramidConfig
    private var state: PyramidState = .uninitialized
    public private(set) var frameCount: Int = 0

    public init(metalContext: MetalContext, config: PyramidConfig = PyramidConfig(), stackingPipelines: StackingPipelines? = nil) throws {
        self.metalContext = metalContext
        self.config = config
        let common = try stackingPipelines ?? StackingPipelines(metalContext: metalContext)
        self.pipelines = try PyramidPipelines(common: common)
        self.warpPipeline = try GPUWarp(library: common.shaderLibrary, device: metalContext.device)
    }

    // MARK: - StackingAlgorithm

    public func accumulate(image: DeptheosImage, transform: AffineTransform2D?) throws {
        if case .uninitialized = state {
            try allocateAndInit(width: image.width, height: image.height)
        }

        guard case .accumulating(let r) = state else {
            throw StackingError.invalidState("Expected Accumulating state")
        }

        let sourceTexture = try image.getTexture(cache: metalContext.textureCache)

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "pyramid_accumulate"

        try encodeAccumulate(
            commandBuffer: commandBuffer,
            resources: r,
            sourceTexture: sourceTexture,
            transform: transform
        )

        commandBuffer.commit()
        // No waitUntilCompleted — let GPU work overlap with next frame's CPU loading/alignment.
        // Metal retains textures referenced by in-flight command buffers, so the
        // DeptheosImage's backing IOSurface stays alive until the command buffer completes.

        frameCount += 1
    }

    public func finalize() async throws {
        if frameCount == 0 {
            throw StackingError.noFrames
        }

        guard case .accumulating(let accum) = state else {
            throw StackingError.invalidState("finalize() requires Accumulating state")
        }

        let lastIdx = accum.numLevels - 1

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "pyramid_finalize"

        // Normalize coarsest level (average over all frames) into scratch_gaussian
        if frameCount > 1 {
            var params = NormalizeParams(factor: 1.0 / Float(frameCount))
            guard let normEncoder = commandBuffer.makeComputeCommandEncoder() else {
                throw StackingError.metalError("Failed to create compute encoder for normalize_coarsest")
            }
            normEncoder.label = "normalize_coarsest"
            normEncoder.dispatch("normalize_level", pipeline: pipelines.normalizeLevel,
                                 width: accum.bestRgbPyramid[lastIdx].width,
                                 height: accum.bestRgbPyramid[lastIdx].height) {
                $0.setTexture(accum.bestRgbPyramid[lastIdx], index: 0)
                $0.setTexture(accum.scratchGaussian[lastIdx], index: 1)
                $0.setBytes(&params, length: MemoryLayout<NormalizeParams>.size, index: 0)
            }
            normEncoder.endEncoding()
        } else {
            try encodeCopy(commandBuffer: commandBuffer, src: accum.bestRgbPyramid[lastIdx], dst: accum.scratchGaussian[lastIdx])
        }

        // Reconstruct pyramid
        try encodePyramidReconstruction(commandBuffer: commandBuffer, accum: accum)

        // Clamp + alpha=1, converting rgba32Float → rgba16Unorm for output
        let resultTexture = try pipelines.common.makeWorkingTexture(
            width: accum.width, height: accum.height,
            format: .rgba16Unorm,
            usage: [.shaderRead, .shaderWrite],
            label: "pyramid_result"
        )
        guard let finalEncoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for pyramid_make_opaque")
        }
        finalEncoder.label = "pyramid_make_opaque"
        finalEncoder.dispatch("make_opaque", pipeline: pipelines.common.makeOpaque,
                              width: accum.width, height: accum.height) {
            $0.setTexture(accum.scratchGaussian[0], index: 0)
            $0.setTexture(resultTexture, index: 1)
        }
        finalEncoder.endEncoding()

        commandBuffer.commit()
        await commandBuffer.completed()

        // Transition to Finalized — drop everything except result + focus L0
        let bestFocusL0 = accum.bestFocusPyramid[0]

        state = .finalized(PyramidFinalizedResources(
            width: accum.width,
            height: accum.height,
            resultTexture: resultTexture,
            bestFocusL0: bestFocusL0
        ))
    }

    public var resultTexture: MTLTexture? {
        if case .finalized(let r) = state { return r.resultTexture }
        return nil
    }

    public var focusMapTexture: MTLTexture? {
        switch state {
        case .accumulating(let r): return r.bestFocusPyramid.first
        case .finalized(let r): return r.bestFocusL0
        case .uninitialized: return nil
        }
    }

    public func releaseGPUResources() {
        state = .uninitialized
    }

    public func releaseResultTexture() {
        if case .finalized(let r) = state {
            state = .finalized(PyramidFinalizedResources(
                width: r.width, height: r.height,
                resultTexture: (try? pipelines.common.makeDummyTexture()) ?? r.resultTexture,
                bestFocusL0: r.bestFocusL0
            ))
        }
    }

    // MARK: - Resource Allocation

    private func allocateAndInit(width: Int, height: Int) throws {
        // Calculate number of levels (top level ~64px)
        let minDim = min(width, height)
        let numLevels = max(1, Int(floor(log2(Float(minDim) / 64.0))) + 1)

        // Create textures for each level
        var w = width
        var h = height

        var scratchGaussian: [MTLTexture] = []
        var bestRgbPyramid: [MTLTexture] = []
        var bestFocusPyramid: [MTLTexture] = []

        let make = pipelines.common.makeWorkingTexture

        for level in 0..<numLevels {
            scratchGaussian.append(try make(w, h, .rgba32Float, [.shaderRead, .shaderWrite], "scratch_gaussian_\(level)"))
            bestRgbPyramid.append(try make(w, h, .rgba32Float, [.shaderRead, .shaderWrite], "best_rgb_\(level)"))
            bestFocusPyramid.append(try make(w, h, .r16Float, [.shaderRead, .shaderWrite], "best_focus_\(level)"))
            w = (w + 1) / 2
            h = (h + 1) / 2
        }

        // Create per-level WTA params buffers
        let lastIdx = numLevels - 1
        let finestExponent = 13.0 - config.smoothness
        let coarsestExponent = max(finestExponent * 0.5, 1.0)
        var wtaParamsBuffers: [MTLBuffer] = []

        for level in 0..<lastIdx {
            let t = Float(level) / Float(max(numLevels - 1, 1))
            let exponent = finestExponent + (coarsestExponent - finestExponent) * t
            var params = WtaParams(exponent: exponent)
            guard let buffer = metalContext.device.makeBuffer(
                bytes: &params,
                length: MemoryLayout<WtaParams>.size,
                options: .storageModeShared
            ) else {
                throw StackingError.metalError("Failed to allocate WTA params buffer")
            }
            buffer.label = "wta_params_\(level)"
            wtaParamsBuffers.append(buffer)
        }

        state = .accumulating(PyramidAccumResources(
            width: width,
            height: height,
            numLevels: numLevels,
            scratchGaussian: scratchGaussian,
            bestRgbPyramid: bestRgbPyramid,
            bestFocusPyramid: bestFocusPyramid,
            wtaParamsBuffers: wtaParamsBuffers
        ))

        // Initialize accumulators — clear best_rgb to zero, best_focus to -inf
        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer for init_accumulators")
        }
        commandBuffer.label = "init_accumulators"
        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for pyramid_clear_accumulators")
        }
        encoder.label = "pyramid_clear_accumulators"

        let zero = SIMD4<Float>(0, 0, 0, 0)
        let sentinel = SIMD4<Float>(-.infinity, 0, 0, 1)
        for tex in bestRgbPyramid {
            pipelines.common.encodeComputeClear(encoder: encoder, texture: tex, value: zero)
        }
        for tex in bestFocusPyramid {
            pipelines.common.encodeComputeClear(encoder: encoder, texture: tex, value: sentinel)
        }

        encoder.endEncoding()
        commandBuffer.commit()
        // No wait — the first accumulate command buffer will serialize after this.
    }

    // MARK: - GPU Encode

    private func encodeAccumulate(
        commandBuffer: MTLCommandBuffer,
        resources r: PyramidAccumResources,
        sourceTexture: MTLTexture,
        transform: AffineTransform2D?
    ) throws {
        // Step 1: Transfer input → scratch_gaussian[0] with optional warp.
        // All processing stays in linear space.
        // Warp is always used (identity when no transform) to handle format conversion
        // (e.g. rgba16Unorm source → rgba32Float scratch).
        try warpPipeline.encodeWarp(
            commandBuffer: commandBuffer,
            src: sourceTexture,
            dst: r.scratchGaussian[0],
            transform: transform ?? .identity,
            applySrgb: false
        )

        let lastIdx = r.numLevels - 1

        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for pyramid_accumulate")
        }
        encoder.label = "pyramid_accumulate"
        let pass = PyramidComputePass(encoder: encoder, pipelines: pipelines)

        // Step 2: Build Gaussian pyramid using fused 5x5 downsample
        for level in 0..<lastIdx {
            pass.downsample(source: r.scratchGaussian[level], dest: r.scratchGaussian[level + 1], level: level)
        }

        // Step 3: Process each Laplacian level (fused Laplacian + WTA)
        for level in 0..<lastIdx {
            pass.fusedLaplacianWta(
                gaussian: r.scratchGaussian[level],
                coarser: r.scratchGaussian[level + 1],
                bestRgb: r.bestRgbPyramid[level],
                bestFocus: r.bestFocusPyramid[level],
                wtaParams: r.wtaParamsBuffers[level],
                level: level
            )
        }

        // Step 4: Coarsest level — accumulate sum (for averaging later)
        pass.accumulateSum(source: r.scratchGaussian[lastIdx], dest: r.bestRgbPyramid[lastIdx])

        encoder.endEncoding()
    }

    /// Encode pyramid reconstruction from Laplacian levels.
    /// G[i] = L[i] + Upsample(G[i+1])
    private func encodePyramidReconstruction(
        commandBuffer: MTLCommandBuffer,
        accum: PyramidAccumResources
    ) throws {
        let lastIdx = accum.numLevels - 1

        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for pyramid_reconstruction")
        }
        encoder.label = "pyramid_reconstruction"
        let pass = PyramidComputePass(encoder: encoder, pipelines: pipelines)

        // Collapse pyramid (reconstruction) — iterate from coarsest to finest
        for level in stride(from: lastIdx - 1, through: 0, by: -1) {
            pass.laplacianAdd(
                bestRgb: accum.bestRgbPyramid[level],
                coarser: accum.scratchGaussian[level + 1],
                dest: accum.scratchGaussian[level],
                level: level
            )
        }

        encoder.endEncoding()
    }

    // MARK: - Shader Encode Helpers

    private func encodeCopy(commandBuffer: MTLCommandBuffer, src: MTLTexture, dst: MTLTexture) throws {
        guard let blit = commandBuffer.makeBlitCommandEncoder() else {
            throw StackingError.metalError("Failed to create blit encoder for copy")
        }
        blit.copy(from: src, to: dst)
        blit.endEncoding()
    }

}
