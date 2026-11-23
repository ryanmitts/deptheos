import Metal
import MetalPerformanceShaders

/// Focus peaking: marks the parts of a camera frame in optical focus.
///
/// Focus is how *abruptly* brightness changes, not how *much*. A defocused
/// edge's one-pixel slope is about its average slope over several pixels;
/// an in-focus edge's is several times it. Comparing the two makes the
/// measure independent of contrast: bright lettering on a blurred key
/// scores low, a faint crisp edge high.
///
/// Per frame, all on the GPU:
/// 1. Resample to the output size, one channel.
/// 2. Denoise with a self-guided guided filter (He, Sun & Tang 2010) from
///    box means: noise flattened, edges kept.
/// 3. Per pixel, the strongest one-pixel step over four directions ("fine")
///    and the average slope over a few pixels that way ("coarse"), squared
///    ("energy") and averaged over a small Gaussian window.
/// 4. Score = (fine − coarse energy) / (coarse energy + noise floor), the
///    floor from the frame's mean fine energy, so flat noisy areas score
///    near zero at any exposure. Only pixels on an edge keep their score.
/// 5. Grow marks a pixel so they read as solid lines; measure the score's
///    mean and spread.
/// 6. Three tiers (somewhat / clearly / very sharp), each needing both a
///    fixed level (keeps blurred frames quiet) and one from the frame's
///    statistics.
///
/// A `borderMargin` band at the edge is never marked nor counted: reads
/// past the edge are clamped and fake sharp steps.
///
/// Output is opaque color where marked, (0,0,0,0) elsewhere. No CPU
/// readback; intermediate textures are cached per output size.
nonisolated final class FocusPeaking: @unchecked Sendable {

    // Tuning, set on real iPhone preview frames at 1398×1864.

    /// Guided filter window radius in pixels.
    private static let guideRadius = 2
    /// As a standard deviation in 0…1 units: local variation below about
    /// this is noise.
    private static let guideSigma: Float = 0.03
    /// Spacing in pixels of the coarse gradient.
    private static let coarseStep: Int32 = 4
    /// Gaussian sigma, in pixels, of the energies' window.
    private static let energySigma: Float = 2.5
    /// Noise energy = max(floorMin², floorScale × frame mean fine energy);
    /// floorMin is a one-pixel slope in 0…1 units.
    private static let floorScale: Float = 1.0
    private static let floorMin: Float = 0.01
    /// Multiple of the coarse energy subtracted from the fine energy.
    private static let coarseWeight: Float = 1.0
    /// A pixel's own fine energy must exceed this multiple of the noise.
    private static let contrastMin: Float = 2.0
    /// ... and this fraction of its window's mean fine energy.
    private static let gateFraction: Float = 0.6
    /// Never marked nor counted within this many pixels of the border; at
    /// least one past the coarse step, whose edge reads are clamped.
    private static let borderMargin = max(8, Int(coarseStep) + 1)
    /// Marks are grown by this many pixels.
    private static let growRadius = 1
    /// Tier thresholds: absolute levels and standard deviations above mean.
    private static let absolute: SIMD3<Float> = [1.0, 1.5, 2.2]
    private static let spread: SIMD3<Float> = [2.0, 3.0, 4.5]
    /// Tier colors (RGBA, opaque).
    private static let colors: (SIMD4<Float>, SIMD4<Float>, SIMD4<Float>) = (
        [0.20, 0.90, 1.00, 1],
        [1.00, 0.85, 0.10, 1],
        [1.00, 0.20, 0.15, 1]
    )

    /// Applied before squaring so energies sit inside half-float range.
    private static let gradientGain: Float = 64

    private let device: MTLDevice
    private let lumaPipeline: MTLComputePipelineState
    private let coefPipeline: MTLComputePipelineState
    private let applyPipeline: MTLComputePipelineState
    private let gradPipeline: MTLComputePipelineState
    private let scorePipeline: MTLComputePipelineState
    private let overlayPipeline: MTLComputePipelineState
    private let statistics: MPSImageStatisticsMeanAndVariance
    private let boxFilter: MPSImageBox
    private let energyBlur: MPSImageGaussianBlur
    private let grow: MPSImageAreaMax

    private struct Targets {
        let width: Int, height: Int
        let lumaPair: MTLTexture    // rg32f: L, L²
        let meanPair: MTLTexture    // rg32f: box means of the above (32-bit: E[L²] − E[L]² cancels in half)
        let coef: MTLTexture        // rg16f: guided-filter a, b
        let coefMean: MTLTexture    // rg16f: box means of a, b
        let clean: MTLTexture       // r16f: denoised channel
        let grad: MTLTexture        // rg16f: fine energy, coarse energy
        let gradAvg: MTLTexture     // rg16f: window averages of grad
        let score: MTLTexture       // r16f
        let scoreGrown: MTLTexture  // r16f
        let gradStats: MTLTexture   // 2×1: mean at (0,0), variance at (1,0)
        let scoreStats: MTLTexture  // 2×1: mean at (0,0), variance at (1,0)
    }
    private var targets: Targets?

    private struct LumaParams {
        var tapOffset: SIMD2<Float>; var multiTap: UInt32
        var pad: UInt32 = 0  // the Metal struct is padded to 16 bytes by float2 alignment
    }
    private struct CoefParams { var epsilon: Float }
    private struct GradParams { var coarseStep: Int32; var gain: Float }
    private struct ScoreParams {
        var floorScale: Float; var floorMin: Float; var coarseWeight: Float
        var contrastMin: Float; var gateFraction: Float; var margin: UInt32
    }
    private struct OverlayParams {
        var absolute: SIMD3<Float>; var spread: SIMD3<Float>
        var color0: SIMD4<Float>; var color1: SIMD4<Float>; var color2: SIMD4<Float>
        var margin: UInt32
    }

    init?(device: MTLDevice) {
        guard MPSSupportsMTLDevice(device),
              let library = try? device.makeLibrary(source: FocusPeakingShader.source, options: nil)
        else { return nil }
        func pipeline(_ name: String) -> MTLComputePipelineState? {
            guard let fn = library.makeFunction(name: name) else { return nil }
            return try? device.makeComputePipelineState(function: fn)
        }
        guard let luma = pipeline("fpLuma"),
              let coef = pipeline("fpGuideCoefficients"),
              let apply = pipeline("fpGuideApply"),
              let grad = pipeline("fpGradients"),
              let score = pipeline("fpScore"),
              let overlay = pipeline("fpOverlay")
        else { return nil }
        self.device = device
        lumaPipeline = luma
        coefPipeline = coef
        applyPipeline = apply
        gradPipeline = grad
        scorePipeline = score
        overlayPipeline = overlay
        statistics = MPSImageStatisticsMeanAndVariance(device: device)
        let boxSize = 2 * Self.guideRadius + 1
        boxFilter = MPSImageBox(device: device, kernelWidth: boxSize, kernelHeight: boxSize)
        energyBlur = MPSImageGaussianBlur(device: device, sigma: Self.energySigma)
        let growSize = 2 * Self.growRadius + 1
        grow = MPSImageAreaMax(device: device, kernelWidth: growSize, kernelHeight: growSize)
        boxFilter.edgeMode = .clamp
        energyBlur.edgeMode = .clamp
        grow.edgeMode = .clamp
    }

    /// Peaking for `source` (BGRA camera frame, .bgra8Unorm, scaled to
    /// `output`'s size) into `output` (.bgra8Unorm, shaderWrite). On a
    /// background video queue, once per frame.
    func encode(into commandBuffer: MTLCommandBuffer, source: MTLTexture, output: MTLTexture) {
        guard let t = prepareTargets(width: output.width, height: output.height) else { return }

        // 1. Resample to the output size, one channel.
        let ratio = max(Float(source.width) / Float(output.width), Float(source.height) / Float(output.height))
        var lp = LumaParams(
            tapOffset: [1 / Float(output.width), 1 / Float(output.height)],
            multiTap: ratio > 1.25 ? 1 : 0)
        dispatch(commandBuffer, lumaPipeline, [source, t.lumaPair], &lp, t)

        // 2. Self-guided filter.
        boxFilter.encode(commandBuffer: commandBuffer, sourceTexture: t.lumaPair, destinationTexture: t.meanPair)
        var cp = CoefParams(epsilon: Self.guideSigma * Self.guideSigma)
        dispatch(commandBuffer, coefPipeline, [t.meanPair, t.coef], &cp, t)
        boxFilter.encode(commandBuffer: commandBuffer, sourceTexture: t.coef, destinationTexture: t.coefMean)
        var none: UInt32 = 0
        dispatch(commandBuffer, applyPipeline, [t.lumaPair, t.coefMean, t.clean], &none, t)

        // 3. Fine and coarse gradient energies, their frame statistics and
        //    their window averages.
        var gp = GradParams(coarseStep: Self.coarseStep, gain: Self.gradientGain)
        dispatch(commandBuffer, gradPipeline, [t.clean, t.grad], &gp, t)
        let margin = Self.borderMargin
        statistics.clipRectSource = MTLRegionMake2D(margin, margin,
                                                    max(1, t.width - 2 * margin), max(1, t.height - 2 * margin))
        statistics.encode(commandBuffer: commandBuffer, sourceTexture: t.grad, destinationTexture: t.gradStats)
        energyBlur.encode(commandBuffer: commandBuffer, sourceTexture: t.grad, destinationTexture: t.gradAvg)

        // 4. Score.
        let gain2 = Self.gradientGain * Self.gradientGain
        var sp = ScoreParams(floorScale: Self.floorScale, floorMin: Self.floorMin * Self.floorMin * gain2,
                             coarseWeight: Self.coarseWeight, contrastMin: Self.contrastMin,
                             gateFraction: Self.gateFraction, margin: UInt32(margin))
        dispatch(commandBuffer, scorePipeline, [t.grad, t.gradAvg, t.gradStats, t.score], &sp, t)

        // 5. Grow marks, then frame statistics of the score.
        grow.encode(commandBuffer: commandBuffer, sourceTexture: t.score, destinationTexture: t.scoreGrown)
        statistics.encode(commandBuffer: commandBuffer, sourceTexture: t.scoreGrown, destinationTexture: t.scoreStats)

        // 6. Tiered overlay.
        var op = OverlayParams(absolute: Self.absolute, spread: Self.spread,
                               color0: Self.colors.0, color1: Self.colors.1, color2: Self.colors.2,
                               margin: UInt32(margin))
        dispatch(commandBuffer, overlayPipeline, [t.scoreGrown, t.scoreStats, output], &op, t)
    }

    // MARK: - Helpers

    private func dispatch<P: BitwiseCopyable>(_ commandBuffer: MTLCommandBuffer, _ pipeline: MTLComputePipelineState,
                                              _ textures: [MTLTexture], _ params: inout P, _ t: Targets) {
        guard let enc = commandBuffer.makeComputeCommandEncoder() else { return }
        enc.setComputePipelineState(pipeline)
        for (i, tex) in textures.enumerated() { enc.setTexture(tex, index: i) }
        // Metal sizes a struct to its alignment (OverlayParams is 96 bytes
        // there) where Swift's `size` stops at the last field (84): send the
        // zero-padded stride, or the shader reads past the bytes set.
        var bytes = [UInt8](repeating: 0, count: MemoryLayout<P>.stride)
        withUnsafeBytes(of: &params) { bytes.replaceSubrange(0..<$0.count, with: $0) }
        enc.setBytes(bytes, length: bytes.count, index: 0)
        let w = pipeline.threadExecutionWidth
        let h = max(1, pipeline.maxTotalThreadsPerThreadgroup / w)
        enc.dispatchThreads(MTLSize(width: t.width, height: t.height, depth: 1),
                            threadsPerThreadgroup: MTLSize(width: w, height: h, depth: 1))
        enc.endEncoding()
    }

    private func prepareTargets(width: Int, height: Int) -> Targets? {
        if let t = targets, t.width == width, t.height == height { return t }
        func make(_ format: MTLPixelFormat, _ w: Int, _ h: Int) -> MTLTexture? {
            let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: format, width: w, height: h, mipmapped: false)
            d.usage = [.shaderRead, .shaderWrite]
            d.storageMode = .private
            return device.makeTexture(descriptor: d)
        }
        guard let lumaPair = make(.rg32Float, width, height),
              let meanPair = make(.rg32Float, width, height),
              let coef = make(.rg16Float, width, height),
              let coefMean = make(.rg16Float, width, height),
              let clean = make(.r16Float, width, height),
              let grad = make(.rg16Float, width, height),
              let gradAvg = make(.rg16Float, width, height),
              let score = make(.r16Float, width, height),
              let scoreGrown = make(.r16Float, width, height),
              let gradStats = make(.rg32Float, 2, 1),
              let scoreStats = make(.r32Float, 2, 1)
        else { return nil }
        let t = Targets(width: width, height: height, lumaPair: lumaPair, meanPair: meanPair,
                        coef: coef, coefMean: coefMean, clean: clean, grad: grad,
                        gradAvg: gradAvg, score: score, scoreGrown: scoreGrown, gradStats: gradStats, scoreStats: scoreStats)
        targets = t
        return t
    }
}
