import Metal
import DeptheosShaders

/// GPU resources used during the wavelet accumulation phase.
/// All fields allocated together on first frame. Transitioning to Reassigning
/// drops forward transform scratch, keeping only merged luma + blend accumulators.
private struct WaveletAccumResources {
    let width: Int
    let height: Int
    let paddedWidth: Int
    let paddedHeight: Int
    let numLevels: Int
    let totalFrames: Int

    // Per-level detail WTA (3 subbands: LH, HL, HH)
    var bestCoeffs: [[MTLTexture]]      // [level][subband], RG16Float
    var bestMagnitude: [[MTLTexture]]    // [level][subband], R16Float
    var bestIndex: [[MTLTexture]]        // [level][subband], R16Uint — frame index

    // 2nd-best per detail subband (for consistency filter re-fetch)
    var secondBestCoeffs: [[MTLTexture]] // [level][subband], RG16Float
    var secondBestMag: [[MTLTexture]]    // [level][subband], R16Float
    var secondBestIndex: [[MTLTexture]]  // [level][subband], R16Uint

    // Coarsest LL band
    var bestLL: MTLTexture              // RG16Float
    var bestLLMag: MTLTexture           // R16Float
    var bestLLIndex: MTLTexture         // R16Uint

    // Scratch textures (reused per frame)
    var lumaTexture: MTLTexture         // RG16Float, original dimensions
    var paddedLuma: MTLTexture          // RG16Float, padded dimensions
    var scratchTempLow: MTLTexture      // RG16Float, padded dimensions (reused at each level)
    var scratchTempHigh: MTLTexture     // RG16Float, padded dimensions
    var subbandLL: [MTLTexture]         // [2] ping-pong, RG16Float
    var subbandDetail: [MTLTexture]     // [3] LH/HL/HH, RG16Float

    // For warp+color conversion during accumulation
    var processingTexture: MTLTexture   // Rgba16Float, sRGB working copy

    // Live preview: sum of level-0 detail subband magnitudes
    var energyMapTexture: MTLTexture    // R16Float, half-resolution
}

/// GPU resources for the color reassignment (blend) phase.
private struct WaveletReassignResources {
    let width: Int
    let height: Int
    let mergedLuma: MTLTexture          // R16Float (real part of inverse transform output)
    var bestColor: MTLTexture           // Rgba16Float, read_write
    var bestDistance: MTLTexture        // R16Float, read_write
    var processingTexture: MTLTexture   // Rgba16Float (for warped input during reassignment)
}

/// GPU resources that survive after finalization.
private struct WaveletFinalizedResources {
    let width: Int
    let height: Int
    let resultTexture: MTLTexture
    let mergedLuma: MTLTexture          // For focusMapTexture
}

/// State machine for the wavelet stacker lifecycle.
private enum WaveletState {
    case uninitialized
    case accumulating(WaveletAccumResources)
    case reassigning(WaveletReassignResources)
    case finalized(WaveletFinalizedResources)
}

/// Wavelet-based focus stacking algorithm.
///
/// Two-pass algorithm:
/// 1. Accumulate: wavelet decomposition + winner-take-all on detail subbands
/// 2. Inverse transform to produce merged grayscale focus map
/// 3. Blend: reassign original source colors using luma proximity to merged focus map
///
/// Port from deptheos-rust/deptheos_core/src/gpu/wavelet/mod.rs
/// Configuration for wavelet focus stacking.
public struct WaveletConfig: Sendable {
    /// Maximum decomposition levels.
    public var maxLevels: Int
    /// Minimum dimension at coarsest level.
    public var minCoarseDimension: Int
    /// Total number of frames. Must be set before processing.
    public var totalFrames: Int
    /// Wavelet denoise strength. 0 = disabled, 1.0 = default. Same scale as focus-stack.
    /// Internally converted to a magnitude² threshold for 0–1 normalized luma.
    public var denoiseLevel: Float
    /// Consistency filter level. 0 = off, 1 = cross-subband voting, 2 = subband + spatial neighbor.
    public var consistency: Int

    public init(maxLevels: Int = 10, minCoarseDimension: Int = 8, totalFrames: Int = 0, denoiseLevel: Float = 0, consistency: Int = 2) {
        self.maxLevels = maxLevels
        self.minCoarseDimension = minCoarseDimension
        self.totalFrames = totalFrames
        self.denoiseLevel = denoiseLevel
        self.consistency = consistency
    }
}

public final class WaveletStacker: BlendableAlgorithm, @unchecked Sendable {
    private let metalContext: MetalContext
    private let pipelines: WaveletPipelines
    private let warpPipeline: GPUWarp
    private let config: WaveletConfig
    private var state: WaveletState = .uninitialized
    public private(set) var frameCount: Int = 0

    public init(metalContext: MetalContext, config: WaveletConfig = WaveletConfig(), stackingPipelines: StackingPipelines? = nil) throws {
        self.metalContext = metalContext
        self.config = config
        let common = try stackingPipelines ?? StackingPipelines(metalContext: metalContext)
        self.pipelines = try WaveletPipelines(common: common)
        self.warpPipeline = try GPUWarp(library: common.shaderLibrary, device: metalContext.device)
    }

    // MARK: - BlendableAlgorithm

    public func accumulate(image: DeptheosImage, transform: AffineTransform2D?) throws {
        if case .uninitialized = state {
            try allocateAndInit(width: image.width, height: image.height)
        }

        guard case .accumulating(var r) = state else {
            throw StackingError.invalidState("Expected Accumulating state")
        }

        let sourceTexture = try image.getTexture(cache: metalContext.textureCache)

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "wavelet_accumulate"

        try encodeAccumulate(
            commandBuffer: commandBuffer,
            resources: &r,
            sourceTexture: sourceTexture,
            transform: transform
        )

        // Write back mutated resources (texture swaps from WTA)
        state = .accumulating(r)

        commandBuffer.commit()
        // No waitUntilCompleted — let GPU work overlap with next frame's CPU loading/alignment.

        frameCount += 1
    }

    public func runFinalizationPipeline() async throws {
        if frameCount == 0 {
            throw StackingError.noFrames
        }

        guard case .accumulating(let accum) = state else {
            throw StackingError.invalidState("runFinalizationPipeline() requires Accumulating state")
        }

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "wavelet_finalize_pipeline"

        let make = pipelines.common.makeWorkingTexture

        // -- Inverse wavelet transform (all compute, one encoder) --

        let scratchA = try make(accum.paddedWidth, accum.paddedHeight, .rg16Float, [.shaderRead, .shaderWrite], "inv_scratch_a")
        let scratchB = try make(accum.paddedWidth, accum.paddedHeight, .rg16Float, [.shaderRead, .shaderWrite], "inv_scratch_b")
        let scratchC = try make(accum.paddedWidth, accum.paddedHeight, .rg16Float, [.shaderRead, .shaderWrite], "inv_scratch_c")
        let scratchD = try make(accum.paddedWidth, accum.paddedHeight, .rg16Float, [.shaderRead, .shaderWrite], "inv_scratch_d")

        let mergedLuma = try make(accum.width, accum.height, .r16Float, [.shaderRead, .shaderWrite], "merged_luma")
        let bestColor = try make(accum.width, accum.height, .rgba16Unorm, [.shaderRead, .shaderWrite], "best_color")
        let bestDistance = try make(accum.width, accum.height, .r16Float, [.shaderRead, .shaderWrite], "best_distance")

        // -- Consistency filters + denoise (before inverse transform) --

        guard let filterEncoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for wavelet_consistency")
        }
        filterEncoder.label = "wavelet_consistency_denoise"
        let filterPass = WaveletComputePass(encoder: filterEncoder, pipelines: pipelines)

        // 1) Cross-subband consistency: 2-of-3 voting (coarse → fine)
        if config.consistency >= 1 {
            for level in stride(from: accum.numLevels - 1, through: 0, by: -1) {
                let levelW = accum.paddedWidth >> (level + 1)
                let levelH = accum.paddedHeight >> (level + 1)
                filterPass.subbandConsistency(
                    bestIndex: accum.bestIndex, bestCoeffs: accum.bestCoeffs,
                    secondBestCoeffs: accum.secondBestCoeffs, secondBestIndex: accum.secondBestIndex,
                    level: level, width: levelW, height: levelH
                )
            }
        }

        // 2) Spatial neighbor outlier filter (per level, per subband)
        if config.consistency >= 2 {
            for level in 0..<accum.numLevels {
                let levelW = accum.paddedWidth >> (level + 1)
                let levelH = accum.paddedHeight >> (level + 1)
                for s in 0..<3 {
                    filterPass.neighborConsistency(
                        bestIndex: accum.bestIndex[level][s], bestCoeffs: accum.bestCoeffs[level][s],
                        secondBestCoeffs: accum.secondBestCoeffs[level][s],
                        secondBestIndex: accum.secondBestIndex[level][s],
                        width: levelW, height: levelH
                    )
                }
            }
        }

        // 3) Wavelet denoise: soft thresholding on detail subbands (skip LL)
        // denoiseLevel uses the same scale as focus-stack (0-10, default 1.0).
        // focus-stack operates on 0-255 pixel values (magnitude² up to 65025),
        // while we use 0-1 normalized luma. Scale by 1/65025 to match.
        if config.denoiseLevel > 0 {
            let threshold = config.denoiseLevel / 65025.0
            for level in 0..<accum.numLevels {
                let levelW = accum.paddedWidth >> (level + 1)
                let levelH = accum.paddedHeight >> (level + 1)
                for s in 0..<3 {
                    filterPass.waveletDenoise(
                        coeffs: accum.bestCoeffs[level][s],
                        threshold: threshold,
                        width: levelW, height: levelH
                    )
                }
            }
        }

        filterEncoder.endEncoding()

        // -- Inverse wavelet transform (all compute, one encoder) --

        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for wavelet_inverse_transform")
        }
        encoder.label = "wavelet_inverse_transform"

        var currentLL = accum.bestLL
        var pingPong = 0

        for level in stride(from: accum.numLevels - 1, through: 0, by: -1) {
            let halfW = accum.paddedWidth >> (level + 1)
            let halfH = accum.paddedHeight >> (level + 1)
            let outW = halfW * 2
            let outH = halfH * 2

            // a) Inverse vertical: (currentLL, LH) → scratchA
            do {
                var dims = WaveletDims(
                    src_width: UInt32(halfW), src_height: UInt32(halfH),
                    dst_width: UInt32(halfW), dst_height: UInt32(outH),
                    direction: 1
                )
                encoder.dispatch("inverse_reconstruct_v_low_l\(level)", pipeline: pipelines.inverseReconstruct, width: halfW, height: outH) {
                    $0.setTexture(currentLL, index: 0)
                    $0.setTexture(accum.bestCoeffs[level][0], index: 1)
                    $0.setTexture(scratchA, index: 2)
                    $0.setBytes(&dims, length: MemoryLayout<WaveletDims>.size, index: 0)
                }
            }

            // b) Inverse vertical: (HL, HH) → scratchB
            do {
                var dims = WaveletDims(
                    src_width: UInt32(halfW), src_height: UInt32(halfH),
                    dst_width: UInt32(halfW), dst_height: UInt32(outH),
                    direction: 1
                )
                encoder.dispatch("inverse_reconstruct_v_high_l\(level)", pipeline: pipelines.inverseReconstruct, width: halfW, height: outH) {
                    $0.setTexture(accum.bestCoeffs[level][1], index: 0)
                    $0.setTexture(accum.bestCoeffs[level][2], index: 1)
                    $0.setTexture(scratchB, index: 2)
                    $0.setBytes(&dims, length: MemoryLayout<WaveletDims>.size, index: 0)
                }
            }

            // c) Inverse horizontal: (scratchA, scratchB) → result
            let result = (pingPong % 2 == 0) ? scratchC : scratchD
            do {
                var dims = WaveletDims(
                    src_width: UInt32(halfW), src_height: UInt32(outH),
                    dst_width: UInt32(outW), dst_height: UInt32(outH),
                    direction: 0
                )
                encoder.dispatch("inverse_reconstruct_h_l\(level)", pipeline: pipelines.inverseReconstruct, width: outW, height: outH) {
                    $0.setTexture(scratchA, index: 0)
                    $0.setTexture(scratchB, index: 1)
                    $0.setTexture(result, index: 2)
                    $0.setBytes(&dims, length: MemoryLayout<WaveletDims>.size, index: 0)
                }
            }

            currentLL = result
            pingPong += 1
        }

        // Extract merged luma: crop real part (.r) from padded RG32Float → R32Float
        encoder.dispatch("copy_real_channel", pipeline: pipelines.common.copyTexture, width: accum.width, height: accum.height) {
            $0.setTexture(currentLL, index: 0)
            $0.setTexture(mergedLuma, index: 1)
        }

        // Initialize bestDistance to 1e10
        do {
            var dims = MagnitudeDims(width: UInt32(accum.width), height: UInt32(accum.height))
            encoder.dispatch("init_best_distance", pipeline: pipelines.initBestDistance, width: accum.width, height: accum.height) {
                $0.setTexture(bestDistance, index: 0)
                $0.setBytes(&dims, length: MemoryLayout<MagnitudeDims>.size, index: 0)
            }
        }

        encoder.endEncoding()

        commandBuffer.commit()
        await commandBuffer.completed()

        // Transition to reassigning — drop all accumulation resources
        state = .reassigning(WaveletReassignResources(
            width: accum.width,
            height: accum.height,
            mergedLuma: mergedLuma,
            bestColor: bestColor,
            bestDistance: bestDistance,
            processingTexture: accum.processingTexture
        ))
    }

    public func beginBlend() throws {
        guard case .reassigning = state else {
            throw StackingError.invalidState("beginBlend() requires Reassigning state")
        }
        // Resources already allocated in runFinalizationPipeline — no-op.
    }

    public func blendFrame(
        frame: DeptheosImage,
        transform: AffineTransform2D?,
        frameIndex: Int
    ) throws {
        guard case .reassigning(let r) = state else {
            throw StackingError.invalidState("blendFrame() requires Reassigning state")
        }

        let sourceTexture = try frame.getTexture(cache: metalContext.textureCache)

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "wavelet_blend"

        // Determine the input texture (warp if needed, keep original color space)
        let inputTexture: MTLTexture
        if let transform = transform {
            try warpPipeline.encodeWarp(
                commandBuffer: commandBuffer,
                src: sourceTexture,
                dst: r.processingTexture,
                transform: transform,
                applySrgb: false
            )
            inputTexture = r.processingTexture
        } else {
            inputTexture = sourceTexture
        }

        // Color reassignment: compare source luma to merged luma, update bestColor/bestDistance
        var reassignParams = ReassignParams(
            width: UInt32(r.width),
            height: UInt32(r.height)
        )
        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for wavelet_reassign")
        }
        encoder.label = "wavelet_reassign"
        encoder.dispatch("color_reassign_rw", pipeline: pipelines.colorReassignRw, width: r.width, height: r.height) {
            $0.setTexture(inputTexture, index: 0)
            $0.setTexture(r.mergedLuma, index: 1)
            $0.setTexture(r.bestDistance, index: 2)
            $0.setTexture(r.bestColor, index: 3)
            $0.setBytes(&reassignParams, length: MemoryLayout<ReassignParams>.size, index: 0)
        }
        encoder.endEncoding()

        commandBuffer.commit()
        // No waitUntilCompleted — Metal serializes within the same queue.
    }

    public func finalize() async throws {
        guard case .reassigning(let r) = state else {
            throw StackingError.invalidState("finalize() requires Reassigning state")
        }

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "wavelet_finalize"

        // Make opaque (clamp + alpha=1) — output as rgba16Unorm
        let resultTexture = try pipelines.common.makeWorkingTexture(
            width: r.width, height: r.height,
            format: .rgba16Unorm,
            usage: [.shaderRead, .shaderWrite],
            label: "wavelet_result"
        )

        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for wavelet_finalize")
        }
        encoder.label = "wavelet_finalize"
        encoder.dispatch("make_opaque", pipeline: pipelines.common.makeOpaque, width: r.width, height: r.height) {
            $0.setTexture(r.bestColor, index: 0)
            $0.setTexture(resultTexture, index: 1)
        }
        encoder.endEncoding()

        commandBuffer.commit()
        await commandBuffer.completed()

        state = .finalized(WaveletFinalizedResources(
            width: r.width,
            height: r.height,
            resultTexture: resultTexture,
            mergedLuma: r.mergedLuma
        ))
    }

    public var resultTexture: MTLTexture? {
        switch state {
        case .accumulating(let r): return r.processingTexture  // Latest aligned frame as live preview
        case .finalized(let r): return r.resultTexture
        default: return nil
        }
    }

    public var focusMapTexture: MTLTexture? {
        switch state {
        case .accumulating(let r): return r.energyMapTexture
        case .reassigning(let r): return r.mergedLuma
        case .finalized(let r): return r.mergedLuma
        default: return nil
        }
    }

    public func releaseGPUResources() {
        state = .uninitialized
    }

    public func releaseResultTexture() {
        if case .finalized(let r) = state {
            state = .finalized(WaveletFinalizedResources(
                width: r.width, height: r.height,
                resultTexture: (try? pipelines.common.makeDummyTexture()) ?? r.resultTexture,
                mergedLuma: r.mergedLuma
            ))
        }
    }

    // MARK: - Resource Allocation

    private func allocateAndInit(width: Int, height: Int) throws {
        // Calculate number of wavelet decomposition levels
        let minDim = min(width, height)
        var levels = 1
        var size = minDim
        while size > config.minCoarseDimension && levels < config.maxLevels {
            size /= 2
            levels += 1
        }
        let numLevels = max(levels - 1, 1)

        // Pad dimensions to power-of-2 alignment for the decomposition depth
        let alignment = 1 << numLevels
        let paddedWidth = (width + alignment - 1) & ~(alignment - 1)
        let paddedHeight = (height + alignment - 1) & ~(alignment - 1)

        let make = pipelines.common.makeWorkingTexture

        // Per-level detail WTA textures
        var bestCoeffs: [[MTLTexture]] = []
        var bestMagnitude: [[MTLTexture]] = []
        var bestIndex: [[MTLTexture]] = []
        var secondBestCoeffs: [[MTLTexture]] = []
        var secondBestMag: [[MTLTexture]] = []
        var secondBestIndex: [[MTLTexture]] = []

        for level in 0..<numLevels {
            let levelW = paddedWidth >> (level + 1)
            let levelH = paddedHeight >> (level + 1)

            var coeffs: [MTLTexture] = []
            var mags: [MTLTexture] = []
            var idxs: [MTLTexture] = []
            var sCoeffs: [MTLTexture] = []
            var sMags: [MTLTexture] = []
            var sIdxs: [MTLTexture] = []
            for s in 0..<3 {
                let subbandName = ["LH", "HL", "HH"][s]
                coeffs.append(try make(levelW, levelH, .rg16Float, [.shaderRead, .shaderWrite], "best_coeff_l\(level)_\(subbandName)"))
                mags.append(try make(levelW, levelH, .r16Float, [.shaderRead, .shaderWrite], "best_mag_l\(level)_\(subbandName)"))
                idxs.append(try make(levelW, levelH, .r16Uint, [.shaderRead, .shaderWrite], "best_idx_l\(level)_\(subbandName)"))
                sCoeffs.append(try make(levelW, levelH, .rg16Float, [.shaderRead, .shaderWrite], "second_coeff_l\(level)_\(subbandName)"))
                sMags.append(try make(levelW, levelH, .r16Float, [.shaderRead, .shaderWrite], "second_mag_l\(level)_\(subbandName)"))
                sIdxs.append(try make(levelW, levelH, .r16Uint, [.shaderRead, .shaderWrite], "second_idx_l\(level)_\(subbandName)"))
            }
            bestCoeffs.append(coeffs)
            bestMagnitude.append(mags)
            bestIndex.append(idxs)
            secondBestCoeffs.append(sCoeffs)
            secondBestMag.append(sMags)
            secondBestIndex.append(sIdxs)
        }

        // Coarsest LL band
        let coarseW = paddedWidth >> numLevels
        let coarseH = paddedHeight >> numLevels
        let bestLL = try make(coarseW, coarseH, .rg16Float, [.shaderRead, .shaderWrite], "best_ll")
        let bestLLMag = try make(coarseW, coarseH, .r16Float, [.shaderRead, .shaderWrite], "best_ll_mag")
        let bestLLIndex = try make(coarseW, coarseH, .r16Uint, [.shaderRead, .shaderWrite], "best_ll_idx")

        // Scratch textures (reused per frame)
        let lumaTexture = try make(width, height, .rg16Float, [.shaderRead, .shaderWrite], "luma")
        let paddedLuma = try make(paddedWidth, paddedHeight, .rg16Float, [.shaderRead, .shaderWrite], "padded_luma")
        let scratchTempLow = try make(paddedWidth / 2, paddedHeight, .rg16Float, [.shaderRead, .shaderWrite], "scratch_temp_low")
        let scratchTempHigh = try make(paddedWidth / 2, paddedHeight, .rg16Float, [.shaderRead, .shaderWrite], "scratch_temp_high")

        // Level-0 half dimensions — the largest any subband scratch needs to be
        let l0HalfW = paddedWidth / 2
        let l0HalfH = paddedHeight / 2

        // Ping-pong LL
        let subbandLL = [
            try make(l0HalfW, l0HalfH, .rg16Float, [.shaderRead, .shaderWrite], "subband_ll_0"),
            try make(l0HalfW, l0HalfH, .rg16Float, [.shaderRead, .shaderWrite], "subband_ll_1")
        ]

        // Detail subbands (LH, HL, HH)
        let subbandDetail = [
            try make(l0HalfW, l0HalfH, .rg16Float, [.shaderRead, .shaderWrite], "subband_lh"),
            try make(l0HalfW, l0HalfH, .rg16Float, [.shaderRead, .shaderWrite], "subband_hl"),
            try make(l0HalfW, l0HalfH, .rg16Float, [.shaderRead, .shaderWrite], "subband_hh")
        ]

        // Processing texture for warp+color conversion
        let processingTexture = try make(width, height, .rgba16Unorm, [.shaderRead, .shaderWrite], "processing")

        // Energy map for live preview (sum of level-0 detail magnitudes)
        let energyMapTexture = try make(l0HalfW, l0HalfH, .r16Float, [.shaderRead, .shaderWrite], "energy_map")

        state = .accumulating(WaveletAccumResources(
            width: width,
            height: height,
            paddedWidth: paddedWidth,
            paddedHeight: paddedHeight,
            numLevels: numLevels,
            totalFrames: config.totalFrames,
            bestCoeffs: bestCoeffs,
            bestMagnitude: bestMagnitude,
            bestIndex: bestIndex,
            secondBestCoeffs: secondBestCoeffs,
            secondBestMag: secondBestMag,
            secondBestIndex: secondBestIndex,
            bestLL: bestLL,
            bestLLMag: bestLLMag,
            bestLLIndex: bestLLIndex,
            lumaTexture: lumaTexture,
            paddedLuma: paddedLuma,
            scratchTempLow: scratchTempLow,
            scratchTempHigh: scratchTempHigh,
            subbandLL: subbandLL,
            subbandDetail: subbandDetail,
            processingTexture: processingTexture,
            energyMapTexture: energyMapTexture
        ))

        // Initialize accumulators — clear bestMagnitude to 0 (matches Rust TRANSPARENT)
        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer for wavelet_init_accumulators")
        }
        commandBuffer.label = "wavelet_init_accumulators"
        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for wavelet_clear_accumulators")
        }
        encoder.label = "wavelet_clear_accumulators"

        let zero = SIMD4<Float>(0, 0, 0, 0)
        for level in 0..<numLevels {
            for s in 0..<3 {
                pipelines.common.encodeComputeClear(encoder: encoder, texture: bestMagnitude[level][s], value: zero)
                pipelines.common.encodeComputeClear(encoder: encoder, texture: secondBestMag[level][s], value: zero)
            }
        }
        pipelines.common.encodeComputeClear(encoder: encoder, texture: bestLLMag, value: zero)

        encoder.endEncoding()
        commandBuffer.commit()
        // No wait — the first accumulate command buffer will serialize after this.
    }

    // MARK: - GPU Encode: Accumulation

    private func encodeAccumulate(
        commandBuffer: MTLCommandBuffer,
        resources r: inout WaveletAccumResources,
        sourceTexture: MTLTexture,
        transform: AffineTransform2D?
    ) throws {
        let width = r.width
        let height = r.height

        // Step 1: Warp + linear→sRGB conversion into processingTexture
        if let transform = transform {
            try warpPipeline.encodeWarp(
                commandBuffer: commandBuffer,
                src: sourceTexture,
                dst: r.processingTexture,
                transform: transform,
                applySrgb: true
            )
        } else {
            pipelines.common.encodeLinearToSrgb(
                commandBuffer: commandBuffer,
                src: sourceTexture,
                dst: r.processingTexture
            )
        }

        // Steps 2-3: Extract luma + pad reflect (shared encoder)
        guard let prepEncoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for wavelet_luma_pad")
        }
        prepEncoder.label = "wavelet_luma_pad"
        WaveletComputePass(encoder: prepEncoder, pipelines: pipelines)
            .extractLuma(source: r.processingTexture, dest: r.lumaTexture)
            .padReflect(source: r.lumaTexture, dest: r.paddedLuma, srcWidth: width, srcHeight: height)
        prepEncoder.endEncoding()

        // Step 4: Forward wavelet transform + WTA for all levels (single encoder)
        guard let decompEncoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for wavelet_decompose")
        }
        decompEncoder.label = "wavelet_decompose"
        let pass = WaveletComputePass(encoder: decompEncoder, pipelines: pipelines)

        var currentW = r.paddedWidth
        var currentH = r.paddedHeight

        for level in 0..<r.numLevels {
            let halfW = currentW / 2
            let halfH = currentH / 2
            let horizontalInput = (level == 0) ? r.paddedLuma : r.subbandLL[(level - 1) % 2]

            // a-c) Three forward decompositions: H → V(low) → V(high)
            pass.forwardDecompose(input: horizontalInput, low: r.scratchTempLow, high: r.scratchTempHigh,
                                  srcWidth: currentW, srcHeight: currentH, direction: 0, label: "forward_decompose_h_l\(level)")
                .forwardDecompose(input: r.scratchTempLow, low: r.subbandLL[level % 2], high: r.subbandDetail[0],
                                  srcWidth: halfW, srcHeight: currentH, direction: 1, label: "forward_decompose_v_low_l\(level)")
                .forwardDecompose(input: r.scratchTempHigh, low: r.subbandDetail[1], high: r.subbandDetail[2],
                                  srcWidth: halfW, srcHeight: currentH, direction: 1, label: "forward_decompose_v_high_l\(level)")

            // d) In-place WTA on detail subbands (LH, HL, HH)
            let fidx = UInt32(frameCount)
            for s in 0..<3 {
                pass.coupledWta(detail: r.subbandDetail[s], bestCoeffs: r.bestCoeffs[level][s],
                                bestMagnitude: r.bestMagnitude[level][s],
                                bestIndex: r.bestIndex[level][s],
                                secondBestCoeffs: r.secondBestCoeffs[level][s],
                                secondBestMag: r.secondBestMag[level][s],
                                secondBestIndex: r.secondBestIndex[level][s],
                                width: halfW, height: halfH, frameIndex: fidx)
            }

            // e) At coarsest level: in-place WTA on LL band
            if level == r.numLevels - 1 {
                pass.llWta(subband: r.subbandLL[level % 2], bestLL: r.bestLL, bestLLMag: r.bestLLMag,
                           bestLLIndex: r.bestLLIndex, width: halfW, height: halfH, frameIndex: fidx)
            }

            currentW = halfW
            currentH = halfH
        }

        // Update energy map preview from level-0 bestMagnitude
        let l0W = r.paddedWidth / 2
        let l0H = r.paddedHeight / 2
        pass.sumEnergyMap(
            magLH: r.bestMagnitude[0][0],
            magHL: r.bestMagnitude[0][1],
            magHH: r.bestMagnitude[0][2],
            energy: r.energyMapTexture,
            width: l0W, height: l0H
        )

        decompEncoder.endEncoding()
    }
}
