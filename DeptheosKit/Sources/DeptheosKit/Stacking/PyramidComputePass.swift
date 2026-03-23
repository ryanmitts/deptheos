import Metal

/// Chainable compute pass for pyramid stacking dispatches.
/// Each method wraps a single kernel dispatch and returns `self` for chaining.
struct PyramidComputePass {
    let encoder: MTLComputeCommandEncoder
    let pipelines: PyramidPipelines

    @discardableResult
    func downsample(source: MTLTexture, dest: MTLTexture, level: Int) -> Self {
        encoder.dispatch("downsample_l\(level)", pipeline: pipelines.downsample, width: dest.width, height: dest.height) {
            $0.setTexture(source, index: 0)
            $0.setTexture(dest, index: 1)
        }
        return self
    }

    @discardableResult
    func fusedLaplacianWta(
        gaussian: MTLTexture,
        coarser: MTLTexture,
        bestRgb: MTLTexture,
        bestFocus: MTLTexture,
        wtaParams: MTLBuffer,
        level: Int
    ) -> Self {
        encoder.dispatch("fused_lap_wta_l\(level)", pipeline: pipelines.fusedLaplacianWta, width: gaussian.width, height: gaussian.height) {
            $0.setTexture(gaussian, index: 0)
            $0.setTexture(coarser, index: 1)
            $0.setTexture(bestRgb, index: 2)
            $0.setTexture(bestFocus, index: 3)
            $0.setBuffer(wtaParams, offset: 0, index: 0)
        }
        return self
    }

    @discardableResult
    func accumulateSum(source: MTLTexture, dest: MTLTexture) -> Self {
        encoder.dispatch("accumulate_coarsest", pipeline: pipelines.accumulateSum, width: source.width, height: source.height) {
            $0.setTexture(source, index: 0)
            $0.setTexture(dest, index: 1)
        }
        return self
    }

    @discardableResult
    func laplacianAdd(bestRgb: MTLTexture, coarser: MTLTexture, dest: MTLTexture, level: Int) -> Self {
        encoder.dispatch("laplacian_add_l\(level)", pipeline: pipelines.laplacianAdd, width: dest.width, height: dest.height) {
            $0.setTexture(bestRgb, index: 0)
            $0.setTexture(coarser, index: 1)
            $0.setTexture(dest, index: 2)
        }
        return self
    }

    @discardableResult
    func makeOpaque(source: MTLTexture, dest: MTLTexture) -> Self {
        encoder.dispatch("make_opaque", pipeline: pipelines.common.makeOpaque, width: dest.width, height: dest.height) {
            $0.setTexture(source, index: 0)
            $0.setTexture(dest, index: 1)
        }
        return self
    }
}
