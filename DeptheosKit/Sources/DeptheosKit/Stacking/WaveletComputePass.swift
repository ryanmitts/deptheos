import Metal
import DeptheosShaders

/// Chainable compute pass for wavelet stacking dispatches.
/// Each method wraps a single kernel dispatch and returns `self` for chaining.
/// Tracks last-set pipeline to avoid redundant `setComputePipelineState` calls.
struct WaveletComputePass {
    let encoder: MTLComputeCommandEncoder
    let pipelines: WaveletPipelines
    private let tracker: PipelineTracker

    init(encoder: MTLComputeCommandEncoder, pipelines: WaveletPipelines) {
        self.encoder = encoder
        self.pipelines = pipelines
        self.tracker = PipelineTracker()
    }

    /// Dispatch with pipeline state tracking — skips setComputePipelineState when unchanged.
    private func dispatch(
        _ label: String,
        pipeline: MTLComputePipelineState,
        width: Int, height: Int,
        _ bindings: (MTLComputeCommandEncoder) -> Void
    ) {
        encoder.pushDebugGroup(label)
        if tracker.lastPipeline !== pipeline {
            encoder.setComputePipelineState(pipeline)
            tracker.lastPipeline = pipeline
        }
        bindings(encoder)
        encoder.dispatchThreads(
            MTLSize(width: width, height: height, depth: 1),
            threadsPerThreadgroup: pipeline.optimalThreadgroupSize
        )
        encoder.memoryBarrier(scope: .textures)
        encoder.popDebugGroup()
    }

    @discardableResult
    func extractLuma(source: MTLTexture, dest: MTLTexture) -> Self {
        dispatch("extract_luma", pipeline: pipelines.extractLuma, width: dest.width, height: dest.height) {
            $0.setTexture(source, index: 0)
            $0.setTexture(dest, index: 1)
        }
        return self
    }

    @discardableResult
    func padReflect(
        source: MTLTexture, dest: MTLTexture,
        srcWidth: Int, srcHeight: Int
    ) -> Self {
        var dims = WaveletDims(
            src_width: UInt32(srcWidth), src_height: UInt32(srcHeight),
            dst_width: UInt32(dest.width), dst_height: UInt32(dest.height),
            direction: 0
        )
        dispatch("pad_reflect", pipeline: pipelines.padReflect, width: dest.width, height: dest.height) {
            $0.setTexture(source, index: 0)
            $0.setTexture(dest, index: 1)
            $0.setBytes(&dims, length: MemoryLayout<WaveletDims>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func forwardDecompose(
        input: MTLTexture, low: MTLTexture, high: MTLTexture,
        srcWidth: Int, srcHeight: Int, direction: UInt32,
        label: String
    ) -> Self {
        let dstW = low.width
        let dstH = low.height
        var dims = WaveletDims(
            src_width: UInt32(srcWidth), src_height: UInt32(srcHeight),
            dst_width: UInt32(dstW), dst_height: UInt32(dstH),
            direction: direction
        )
        dispatch(label, pipeline: pipelines.forwardDecompose, width: dstW, height: dstH) {
            $0.setTexture(input, index: 0)
            $0.setTexture(low, index: 1)
            $0.setTexture(high, index: 2)
            $0.setBytes(&dims, length: MemoryLayout<WaveletDims>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func coupledWta(
        detail: MTLTexture, bestCoeffs: MTLTexture, bestMagnitude: MTLTexture,
        bestIndex: MTLTexture, secondBestCoeffs: MTLTexture,
        secondBestMag: MTLTexture, secondBestIndex: MTLTexture,
        width: Int, height: Int, frameIndex: UInt32
    ) -> Self {
        var dims = MagnitudeDims(width: UInt32(width), height: UInt32(height))
        var fidx = frameIndex
        dispatch("coupled_wta", pipeline: pipelines.coupledWta, width: width, height: height) {
            $0.setTexture(detail, index: 0)
            $0.setTexture(bestCoeffs, index: 1)
            $0.setTexture(bestMagnitude, index: 2)
            $0.setTexture(bestIndex, index: 3)
            $0.setTexture(secondBestCoeffs, index: 4)
            $0.setTexture(secondBestMag, index: 5)
            $0.setTexture(secondBestIndex, index: 6)
            $0.setBytes(&dims, length: MemoryLayout<MagnitudeDims>.size, index: 0)
            $0.setBytes(&fidx, length: MemoryLayout<UInt32>.size, index: 1)
        }
        return self
    }

    @discardableResult
    func llWta(
        subband: MTLTexture, bestLL: MTLTexture, bestLLMag: MTLTexture,
        bestLLIndex: MTLTexture, width: Int, height: Int, frameIndex: UInt32
    ) -> Self {
        var fidx = frameIndex
        dispatch("ll_wta", pipeline: pipelines.llWta, width: width, height: height) {
            $0.setTexture(subband, index: 0)
            $0.setTexture(bestLL, index: 1)
            $0.setTexture(bestLLMag, index: 2)
            $0.setTexture(bestLLIndex, index: 3)
            $0.setBytes(&fidx, length: MemoryLayout<UInt32>.size, index: 0)
        }
        return self
    }
    @discardableResult
    func subbandConsistency(
        bestIndex: [[MTLTexture]], bestCoeffs: [[MTLTexture]],
        secondBestCoeffs: [[MTLTexture]], secondBestIndex: [[MTLTexture]],
        level: Int, width: Int, height: Int
    ) -> Self {
        var dims = MagnitudeDims(width: UInt32(width), height: UInt32(height))
        dispatch("subband_consistency_l\(level)", pipeline: pipelines.subbandConsistency, width: width, height: height) {
            // LH (subband 0)
            $0.setTexture(bestIndex[level][0], index: 0)
            $0.setTexture(bestCoeffs[level][0], index: 1)
            $0.setTexture(secondBestCoeffs[level][0], index: 2)
            $0.setTexture(secondBestIndex[level][0], index: 3)
            // HL (subband 1)
            $0.setTexture(bestIndex[level][1], index: 4)
            $0.setTexture(bestCoeffs[level][1], index: 5)
            $0.setTexture(secondBestCoeffs[level][1], index: 6)
            $0.setTexture(secondBestIndex[level][1], index: 7)
            // HH (subband 2)
            $0.setTexture(bestIndex[level][2], index: 8)
            $0.setTexture(bestCoeffs[level][2], index: 9)
            $0.setTexture(secondBestCoeffs[level][2], index: 10)
            $0.setTexture(secondBestIndex[level][2], index: 11)
            $0.setBytes(&dims, length: MemoryLayout<MagnitudeDims>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func neighborConsistency(
        bestIndex: MTLTexture, bestCoeffs: MTLTexture,
        secondBestCoeffs: MTLTexture, secondBestIndex: MTLTexture,
        width: Int, height: Int
    ) -> Self {
        var dims = MagnitudeDims(width: UInt32(width), height: UInt32(height))
        dispatch("neighbor_consistency", pipeline: pipelines.neighborConsistency, width: width, height: height) {
            $0.setTexture(bestIndex, index: 0)
            $0.setTexture(bestCoeffs, index: 1)
            $0.setTexture(secondBestCoeffs, index: 2)
            $0.setTexture(secondBestIndex, index: 3)
            $0.setBytes(&dims, length: MemoryLayout<MagnitudeDims>.size, index: 0)
        }
        return self
    }

    @discardableResult
    func waveletDenoise(
        coeffs: MTLTexture, threshold: Float, width: Int, height: Int
    ) -> Self {
        var thresh = threshold
        var dims = MagnitudeDims(width: UInt32(width), height: UInt32(height))
        dispatch("wavelet_denoise", pipeline: pipelines.waveletDenoise, width: width, height: height) {
            $0.setTexture(coeffs, index: 0)
            $0.setBytes(&thresh, length: MemoryLayout<Float>.size, index: 0)
            $0.setBytes(&dims, length: MemoryLayout<MagnitudeDims>.size, index: 1)
        }
        return self
    }

    @discardableResult
    func sumEnergyMap(
        magLH: MTLTexture, magHL: MTLTexture, magHH: MTLTexture,
        energy: MTLTexture, width: Int, height: Int
    ) -> Self {
        dispatch("sum_energy_map", pipeline: pipelines.sumEnergyMap, width: width, height: height) {
            $0.setTexture(magLH, index: 0)
            $0.setTexture(magHL, index: 1)
            $0.setTexture(magHH, index: 2)
            $0.setTexture(energy, index: 3)
        }
        return self
    }
}

/// Reference-type pipeline state tracker for use within a struct-based compute pass.
private class PipelineTracker {
    var lastPipeline: MTLComputePipelineState?
}
