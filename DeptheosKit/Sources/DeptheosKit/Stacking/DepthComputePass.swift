import Metal
import DeptheosShaders

/// Chainable compute pass for depth stacking dispatches.
/// Each method wraps a single kernel dispatch and returns `self` for chaining.
struct DepthComputePass {
    let encoder: MTLComputeCommandEncoder
    let pipelines: DepthPipelines

    @discardableResult
    func warpToGray(
        source: MTLTexture,
        dest: MTLTexture,
        transform: AffineTransform2D
    ) -> Self {
        let (col0, col1, col2) = transform.gpuColumns
        var params = WarpToGrayParams(
            col0: col0, col1: col1, col2: col2,
            apply_srgb: 1
        )
        encoder.dispatch("warp_to_gray", pipeline: pipelines.warpToGray, width: dest.width, height: dest.height) {
            $0.setTexture(source, index: 0)
            $0.setTexture(dest, index: 1)
            $0.setSamplerState(self.pipelines.bilinearSampler, index: 0)
            $0.setBytes(&params, length: MemoryLayout<WarpToGrayParams>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func rgbToGray(source: MTLTexture, dest: MTLTexture) -> Self {
        var params = RgbToGrayParams(is_linear: 1)
        encoder.dispatch("rgb_to_gray", pipeline: pipelines.rgbToGray, width: dest.width, height: dest.height) {
            $0.setTexture(source, index: 0)
            $0.setTexture(dest, index: 1)
            $0.setBytes(&params, length: MemoryLayout<RgbToGrayParams>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func dtRfHorizontal(
        input: MTLTexture, output: MTLTexture, guide: MTLTexture,
        logA: Float, sigmaS: Float, sigmaR: Float,
        bgMaxWeight: Float, subjectMaxWeight: Float,
        width: Int, height: Int
    ) -> Self {
        var params = DTRFParams(
            log_a: logA, sigma_s: sigmaS, sigma_r: sigmaR,
            bg_max_weight: bgMaxWeight, subject_max_weight: subjectMaxWeight,
            width: UInt32(width), height: UInt32(height)
        )
        encoder.dispatch("dt_rf_horizontal", pipeline: pipelines.dtRfHorizontal,
                         threadgroups: MTLSize(width: (height + 63) / 64, height: 1, depth: 1),
                         threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1)) {
            $0.setTexture(input, index: 0)
            $0.setTexture(output, index: 1)
            $0.setTexture(guide, index: 2)
            $0.setBytes(&params, length: MemoryLayout<DTRFParams>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func dtRfVertical(
        input: MTLTexture, output: MTLTexture, guide: MTLTexture,
        logA: Float, sigmaS: Float, sigmaR: Float,
        bgMaxWeight: Float, subjectMaxWeight: Float,
        width: Int, height: Int
    ) -> Self {
        var params = DTRFParams(
            log_a: logA, sigma_s: sigmaS, sigma_r: sigmaR,
            bg_max_weight: bgMaxWeight, subject_max_weight: subjectMaxWeight,
            width: UInt32(width), height: UInt32(height)
        )
        encoder.dispatch("dt_rf_vertical", pipeline: pipelines.dtRfVertical,
                         threadgroups: MTLSize(width: (width + 63) / 64, height: 1, depth: 1),
                         threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1)) {
            $0.setTexture(input, index: 0)
            $0.setTexture(output, index: 1)
            $0.setTexture(guide, index: 2)
            $0.setBytes(&params, length: MemoryLayout<DTRFParams>.size, index: 0)
        }
        return self
    }



    @discardableResult
    func depthFeather(depthMap: MTLTexture, stepSize: Float, power: Float) -> Self {
        var params = DepthFeatherParams(step_size: stepSize, power: power)
        encoder.dispatch("depth_feather", pipeline: pipelines.depthFeather, width: depthMap.width, height: depthMap.height) {
            $0.setTexture(depthMap, index: 0)
            $0.setBytes(&params, length: MemoryLayout<DepthFeatherParams>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func laplacianFocus(laplacian: MTLTexture, gray: MTLTexture, dest: MTLTexture) -> Self {
        encoder.dispatch("laplacian_focus", pipeline: pipelines.laplacianFocus, width: dest.width, height: dest.height) {
            $0.setTexture(laplacian, index: 0)
            $0.setTexture(gray, index: 1)
            $0.setTexture(dest, index: 2)
        }
        return self
    }

    @discardableResult
    func bandpassDiff(original: MTLTexture, blurred: MTLTexture, dest: MTLTexture, amplifier: Float) -> Self {
        var params = BandpassDiffParams(amplifier: amplifier)
        encoder.dispatch("bandpass_diff", pipeline: pipelines.bandpassDiff, width: dest.width, height: dest.height) {
            $0.setTexture(original, index: 0)
            $0.setTexture(blurred, index: 1)
            $0.setTexture(dest, index: 2)
            $0.setBytes(&params, length: MemoryLayout<BandpassDiffParams>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func depthWtaAccumulate(
        focusMap: MTLTexture,
        bestFocus: MTLTexture,
        depthIndex: MTLTexture,
        opacityMap: MTLTexture,
        frameIndex: UInt32,
        numFrames: UInt32
    ) -> Self {
        var params = DepthWtaParams(frame_index: frameIndex, num_frames: numFrames)
        encoder.dispatch("depth_wta_accumulate", pipeline: pipelines.depthWtaAccumulate, width: focusMap.width, height: focusMap.height) {
            $0.setTexture(focusMap, index: 0)
            $0.setTexture(bestFocus, index: 1)
            $0.setTexture(depthIndex, index: 2)
            $0.setTexture(opacityMap, index: 3)
            $0.setBytes(&params, length: MemoryLayout<DepthWtaParams>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func opacityDiff(opacityMap: MTLTexture, bestFocus: MTLTexture) -> Self {
        encoder.dispatch("opacity_diff", pipeline: pipelines.opacityDiff, width: opacityMap.width, height: opacityMap.height) {
            $0.setTexture(opacityMap, index: 0)
            $0.setTexture(bestFocus, index: 1)
        }
        return self
    }

    @discardableResult
    func histogramPercentile(histogramBuffer: MTLBuffer, normParamsBuffer: MTLBuffer, totalPixelsBuffer: MTLBuffer) -> Self {
        encoder.dispatchSingle("histogram_percentile", pipeline: pipelines.histogramPercentile) {
            $0.setBuffer(histogramBuffer, offset: 0, index: 0)
            $0.setBuffer(normParamsBuffer, offset: 0, index: 1)
            $0.setBuffer(totalPixelsBuffer, offset: 0, index: 2)
        }
        return self
    }

    @discardableResult
    func normalizeOpacity(opacityMap: MTLTexture, normParamsBuffer: MTLBuffer) -> Self {
        encoder.dispatch("normalize_opacity", pipeline: pipelines.normalizeOpacity, width: opacityMap.width, height: opacityMap.height) {
            $0.setTexture(opacityMap, index: 0)
            $0.setBuffer(normParamsBuffer, offset: 0, index: 0)
        }
        return self
    }

}
