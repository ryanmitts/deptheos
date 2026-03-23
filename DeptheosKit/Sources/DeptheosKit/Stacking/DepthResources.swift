import Metal
import MetalPerformanceShaders
import DeptheosShaders

/// GPU resources used during the accumulation phase (first pass).
struct DepthAccumResources {
    let width: Int
    let height: Int
    let totalFrames: Int

    // Persistent accumulators
    var bestFocus: MTLTexture      // R16Float, read_write
    var depthIndex: MTLTexture     // R32Float, read_write (full precision avoids blend banding)
    var opacityMap: MTLTexture     // R16Float, read_write
    var wtaColor: MTLTexture       // RGBA16Float, read_write — WTA pixel colors

    // Per-frame scratch
    var grayTexture: MTLTexture    // R32Float (IIR scratch, matches depthIndex precision)
    var focusMap: MTLTexture       // R16Float
    var blurredGray: MTLTexture    // R16Float

    // Finalization scratch
    var histogramBuffer: MTLBuffer // 1024 × uint32
    var normParamsBuffer: MTLBuffer // NormOpacityParams (2 floats)
    var totalPixelsBuffer: MTLBuffer // 1 uint32
}

/// GPU resources for the blend phase (second pass).
/// Keeps depthIndex alive, drops everything else from accumulation.
struct DepthBlendResources {
    let width: Int
    let height: Int
    let totalFrames: Int
    let planeRange: UInt32         // Blend overlap in frame-step units
    let depthIndex: MTLTexture     // Kept from accum
    var finalizedBlend: MTLTexture  // Rgba32Float, render target
}

/// GPU resources that survive after finalization.
struct DepthFinalizedResources {
    let width: Int
    let height: Int
    let resultTexture: MTLTexture
    let depthIndexTexture: MTLTexture
}

/// Shared encoding helpers for depth-based stackers.
/// Pure functions that operate on resources and command buffers.
enum DepthEncoding {

    /// Allocate GPU resources for the accumulation phase.
    static func allocateAccumResources(
        metalContext: MetalContext,
        pipelines: DepthPipelines,
        width: Int, height: Int, totalFrames: Int
    ) throws -> DepthAccumResources {
        let make = pipelines.common.makeWorkingTexture
        let device = metalContext.device

        let bestFocus = try make(width, height, .r16Float, [.shaderRead, .shaderWrite], "best_focus")
        let depthIndex = try make(width, height, .r32Float, [.shaderRead, .shaderWrite], "depth_index")
        let opacityMap = try make(width, height, .r16Float, [.shaderRead, .shaderWrite], "opacity_map")
        let wtaColor = try make(width, height, .rgba16Float, [.shaderRead, .shaderWrite], "wta_color")
        let grayTexture = try make(width, height, .r16Float, [.shaderRead, .shaderWrite], "gray_texture")
        let focusMap = try make(width, height, .r16Float, [.shaderRead, .shaderWrite], "focus_map")
        let blurredGray = try make(width, height, .r16Float, [.shaderRead, .shaderWrite], "blurred_gray")

        guard let histogramBuffer = device.makeBuffer(
            length: 256 * MemoryLayout<UInt32>.size,
            options: .storageModeShared
        ) else {
            throw StackingError.metalError("Failed to allocate histogram buffer")
        }
        histogramBuffer.label = "histogram_buffer"

        guard let normParamsBuffer = device.makeBuffer(
            length: MemoryLayout<NormOpacityParams>.size,
            options: .storageModeShared
        ) else {
            throw StackingError.metalError("Failed to allocate norm params buffer")
        }
        normParamsBuffer.label = "norm_params_buffer"

        guard let totalPixelsBuffer = device.makeBuffer(
            length: MemoryLayout<UInt32>.size,
            options: .storageModeShared
        ) else {
            throw StackingError.metalError("Failed to allocate total pixels buffer")
        }
        totalPixelsBuffer.label = "total_pixels_buffer"

        let resources = DepthAccumResources(
            width: width, height: height, totalFrames: totalFrames,
            bestFocus: bestFocus, depthIndex: depthIndex,
            opacityMap: opacityMap, wtaColor: wtaColor,
            grayTexture: grayTexture, focusMap: focusMap,
            blurredGray: blurredGray,
            histogramBuffer: histogramBuffer,
            normParamsBuffer: normParamsBuffer,
            totalPixelsBuffer: totalPixelsBuffer
        )

        // Initialize bestFocus to -inf so the first frame always wins every pixel.
        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer for depth_init_accumulators")
        }
        commandBuffer.label = "depth_init_accumulators"
        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_clear_best_focus")
        }
        encoder.label = "depth_clear_best_focus"
        pipelines.common.encodeComputeClear(encoder: encoder, texture: bestFocus,
                                            value: SIMD4<Float>(-.infinity, 0, 0, 1))
        encoder.endEncoding()
        commandBuffer.commit()

        return resources
    }

    /// Encode the per-frame accumulation pipeline (grayscale → focus metric → WTA).
    static func encodeAccumulate(
        commandBuffer: MTLCommandBuffer,
        resources r: DepthAccumResources,
        pipelines: DepthPipelines,
        sourceTexture: MTLTexture,
        transform: AffineTransform2D?,
        frameCount: Int,
        radius: Float,
        preBlur: MPSImageGaussianBlur,
        postBlur: MPSImageGaussianBlur
    ) throws {
        let width = r.width
        let height = r.height

        // Step 1: Convert to grayscale (with optional warp + sRGB conversion)
        guard let enc1 = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_to_gray")
        }
        enc1.label = "depth_to_gray"
        let pass1 = DepthComputePass(encoder: enc1, pipelines: pipelines)
        if let transform = transform {
            pass1.warpToGray(source: sourceTexture, dest: r.grayTexture, transform: transform)
        } else {
            pass1.rgbToGray(source: sourceTexture, dest: r.grayTexture)
        }
        enc1.endEncoding()

        // Step 2: Pre-blur (Gaussian)
        preBlur.encode(commandBuffer: commandBuffer,
                       sourceTexture: r.grayTexture,
                       destinationTexture: r.blurredGray)

        // Step 3: Bandpass focus metric |original - blurred| + Weber contrast
        let preSigma = max(radius * 0.5, 1.0)
        let amplifier = Float(min(4.0 / preSigma, 10.0))
        guard let enc3 = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_bandpass")
        }
        enc3.label = "depth_bandpass"
        DepthComputePass(encoder: enc3, pipelines: pipelines)
            .bandpassDiff(original: r.grayTexture, blurred: r.blurredGray, dest: r.focusMap, amplifier: amplifier)
        enc3.endEncoding()

        // Step 4: Post-blur to smooth focus metric spatially (Gaussian)
        postBlur.encode(commandBuffer: commandBuffer,
                        sourceTexture: r.focusMap,
                        destinationTexture: r.blurredGray)

        // Step 5: Winner-takes-all depth accumulation
        guard let enc5 = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_wta")
        }
        enc5.label = "depth_wta"
        DepthComputePass(encoder: enc5, pipelines: pipelines)
            .depthWtaAccumulate(
                focusMap: r.blurredGray, bestFocus: r.bestFocus,
                depthIndex: r.depthIndex, opacityMap: r.opacityMap,
                frameIndex: UInt32(frameCount), numFrames: UInt32(r.totalFrames)
            )
        enc5.endEncoding()

        // Step 6: Copy winning pixel's color to WTA composite
        guard let enc6 = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_wta_color")
        }
        enc6.label = "depth_wta_color"
        var wtaColorParams: WtaColorUpdateParams
        if let transform = transform {
            let (col0, col1, col2) = transform.gpuColumns
            wtaColorParams = WtaColorUpdateParams(
                frame_index: UInt32(frameCount),
                num_frames: UInt32(r.totalFrames),
                has_warp: 1,
                col0: col0, col1: col1, col2: col2
            )
        } else {
            wtaColorParams = WtaColorUpdateParams(
                frame_index: UInt32(frameCount),
                num_frames: UInt32(r.totalFrames),
                has_warp: 0,
                col0: SIMD4<Float>(0, 0, 0, 0),
                col1: SIMD4<Float>(0, 0, 0, 0),
                col2: SIMD4<Float>(0, 0, 0, 0)
            )
        }
        enc6.dispatch("wta_color_update", pipeline: pipelines.wtaColorUpdate,
                      width: width, height: height) {
            $0.setSamplerState(pipelines.bilinearSampler, index: 0)
            $0.setTexture(sourceTexture, index: 0)
            $0.setTexture(r.depthIndex, index: 1)
            $0.setTexture(r.wtaColor, index: 2)
            $0.setBytes(&wtaColorParams, length: MemoryLayout<WtaColorUpdateParams>.size, index: 0)
        }
        enc6.endEncoding()
    }

    /// Encode opacity normalization (opacity_diff + histogram + normalize).
    static func encodeOpacityNormalization(
        commandBuffer: MTLCommandBuffer,
        resources r: DepthAccumResources,
        pipelines: DepthPipelines,
        histogramKernel: MPSImageHistogram
    ) throws {
        let width = r.width
        let height = r.height

        guard let enc1 = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for finalize_opacity")
        }
        enc1.label = "finalize_opacity"
        DepthComputePass(encoder: enc1, pipelines: pipelines)
            .opacityDiff(opacityMap: r.opacityMap, bestFocus: r.bestFocus)
        enc1.endEncoding()

        histogramKernel.encode(to: commandBuffer,
                               sourceTexture: r.opacityMap,
                               histogram: r.histogramBuffer,
                               histogramOffset: 0)

        var totalPixels = UInt32(width * height)
        memcpy(r.totalPixelsBuffer.contents(), &totalPixels, MemoryLayout<UInt32>.size)

        guard let enc2 = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for finalize_histogram")
        }
        enc2.label = "finalize_histogram"
        DepthComputePass(encoder: enc2, pipelines: pipelines)
            .histogramPercentile(histogramBuffer: r.histogramBuffer, normParamsBuffer: r.normParamsBuffer, totalPixelsBuffer: r.totalPixelsBuffer)
            .normalizeOpacity(opacityMap: r.opacityMap, normParamsBuffer: r.normParamsBuffer)
        enc2.endEncoding()
    }

    /// Encode a single blend frame using the compute overwrite pipeline.
    /// Uses painter's algorithm: frames close to the depth map value overwrite
    /// the destination; boundary frames blend partially. This prevents distant
    /// frames from leaking in (halo reduction).
    static func encodeBlendFrame(
        commandBuffer: MTLCommandBuffer,
        resources r: DepthBlendResources,
        pipelines: DepthPipelines,
        sourceTexture: MTLTexture,
        transform: AffineTransform2D?,
        frameIndex: Int
    ) throws {
        var blendParams = BlendIterativeParams(
            scaled_frame_idx: Float(frameIndex) / Float(max(r.totalFrames - 1, 1)),
            inter_frame_dist: 1.0 / Float(max(r.totalFrames - 1, 1)),
            plane_range: r.planeRange,
            num_frames: UInt32(r.totalFrames),
            width: UInt32(r.width),
            height: UInt32(r.height),
            first_layer: frameIndex == 0 ? 1 : 0
        )

        var blendWarp: BlendWarpParams
        if let transform = transform {
            let (col0, col1, col2) = transform.gpuColumns
            blendWarp = BlendWarpParams(
                col0: col0, col1: col1, col2: col2,
                lanczos_a: 5, apply_srgb: 0
            )
        } else {
            blendWarp = BlendWarpParams(
                col0: SIMD4<Float>(0, 0, 0, 0),
                col1: SIMD4<Float>(0, 0, 0, 0),
                col2: SIMD4<Float>(0, 0, 0, 0),
                lanczos_a: 0, apply_srgb: 0
            )
        }

        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_blend_frame")
        }
        encoder.label = "depth_blend_frame"
        encoder.dispatch("blend_frame_compute", pipeline: pipelines.blendFrameCompute,
                         width: r.width, height: r.height) {
            $0.setTexture(sourceTexture, index: 0)
            $0.setTexture(r.depthIndex, index: 1)
            $0.setTexture(r.finalizedBlend, index: 2)
            $0.setBytes(&blendParams, length: MemoryLayout<BlendIterativeParams>.size, index: 0)
            $0.setBytes(&blendWarp, length: MemoryLayout<BlendWarpParams>.size, index: 1)
        }
        encoder.endEncoding()
    }

    /// Transition from accumulation to blend phase.
    static func beginBlend(
        from accum: DepthAccumResources,
        pipelines: DepthPipelines,
        metalContext: MetalContext,
        planeRange: UInt32
    ) throws -> DepthBlendResources {
        let make = pipelines.common.makeWorkingTexture
        let finalizedBlend = try make(
            accum.width, accum.height,
            .rgba32Float,
            [.shaderRead, .shaderWrite, .renderTarget],
            "finalized_blend"
        )

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "depth_begin_blend"
        guard let clearEncoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_clear_blend")
        }
        clearEncoder.label = "depth_clear_blend"
        pipelines.common.encodeComputeClear(encoder: clearEncoder, texture: finalizedBlend,
                                            value: SIMD4<Float>(0, 0, 0, 0))
        clearEncoder.endEncoding()
        commandBuffer.commit()

        return DepthBlendResources(
            width: accum.width, height: accum.height,
            totalFrames: accum.totalFrames,
            planeRange: planeRange,
            depthIndex: accum.depthIndex,
            finalizedBlend: finalizedBlend
        )
    }

    /// Encode the final blend and produce the result texture.
    /// The compute blend writes final colors directly (no normalization needed).
    static func encodeFinalizeBlend(
        blend: DepthBlendResources,
        pipelines: DepthPipelines,
        metalContext: MetalContext
    ) async throws -> DepthFinalizedResources {
        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer")
        }
        commandBuffer.label = "depth_finalize"

        // Normalize accumulated weighted average: RGB /= A (total weight).
        guard let normEncoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for blend_normalize")
        }
        normEncoder.label = "blend_normalize"
        normEncoder.dispatch("blend_normalize", pipeline: pipelines.blendNormalize,
                             width: blend.width, height: blend.height) {
            $0.setTexture(blend.finalizedBlend, index: 0)
        }
        normEncoder.endEncoding()

        let opaqueResult = try pipelines.common.makeWorkingTexture(
            width: blend.width, height: blend.height,
            format: .rgba16Unorm,
            usage: [.shaderRead, .shaderWrite],
            label: "depth_result"
        )
        guard let encoder = commandBuffer.makeComputeCommandEncoder() else {
            throw StackingError.metalError("Failed to create compute encoder for depth_make_opaque")
        }
        encoder.label = "depth_make_opaque"
        encoder.dispatch("make_opaque", pipeline: pipelines.common.makeOpaque,
                         width: blend.width, height: blend.height) {
            $0.setTexture(blend.finalizedBlend, index: 0)
            $0.setTexture(opaqueResult, index: 1)
        }
        encoder.endEncoding()

        commandBuffer.commit()
        await commandBuffer.completed()

        return DepthFinalizedResources(
            width: blend.width, height: blend.height,
            resultTexture: opaqueResult,
            depthIndexTexture: blend.depthIndex
        )
    }
}
