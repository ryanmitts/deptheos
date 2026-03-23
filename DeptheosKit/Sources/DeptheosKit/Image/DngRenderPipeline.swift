@preconcurrency import Metal
@preconcurrency import CoreVideo
import simd
import DeptheosShaders

/// GPU resources derived from a `DeptheosDng.RenderParams`.
///
/// Build once via `DngRenderPipeline.makeResources(for:)` and reuse for as long
/// as the params are valid; rebuild only when the params change. Owned by the
/// caller — hold it next to whatever owns the renderParams.
public struct DngRenderResources: @unchecked Sendable {
    let params: DngColorParams
    let toneCurve: MTLTexture
    let hueSatMap: MTLBuffer?
    let lookTable: MTLBuffer?
}

/// GPU compute pipeline that runs the `DngRender` Metal kernel.
///
/// Two-phase API: build a `DngRenderResources` once per renderParams change with
/// `makeResources(for:)`, then call `encode(...)` per frame. The pipeline itself
/// is stateless across calls and safe to share.
public final class DngRenderPipeline: @unchecked Sendable {

    public let context: MetalContext

    private let computePipeline: MTLComputePipelineState
    /// Single-element placeholder bound to the HSM/LookTable slots when a renderParams
    /// has no LUT. Built once in init and reused across every `encode` call so the
    /// no-LUT path allocates nothing.
    private let emptyLutBuffer: MTLBuffer

    public init(context: MetalContext) throws {
        self.context = context
        let dev = context.device
        let library = context.defaultLibrary

        guard let dngRenderFunc = library.makeFunction(name: "DngRender") else {
            throw DngRenderPipelineError.shaderNotFound("DngRender")
        }
        self.computePipeline = try dev.makeComputePipelineState(function: dngRenderFunc)

        var zero: Float = 0
        guard let emptyBuf = dev.makeBuffer(
            bytes: &zero,
            length: MemoryLayout<Float>.stride,
            options: .storageModeShared
        ) else {
            throw DngRenderPipelineError.bufferCreationFailed("empty LUT buffer")
        }
        emptyBuf.label = "dng_empty_lut"
        self.emptyLutBuffer = emptyBuf
    }

    // MARK: - Resource construction

    /// Build the GPU resources for a set of renderParams.
    ///
    /// Call this when the params change, not per frame. The returned value
    /// is safe to hold across multiple `encode` calls.
    public func makeResources(for renderParams: DeptheosDng.RenderParams) -> DngRenderResources {
        let params = DngColorParamsBuilder.buildParams(from: renderParams)

        // Precomposed (exposureTone + ACR3 toneCurve) at 4097 entries to match SDK resolution.
        let composedCurve = DngColorParamsBuilder.precomposeToneCurve(
            rawCurve: renderParams.toneCurve,
            tone: params.tone,
            count: 4097
        )
        let toneCurve = makeToneCurveTexture(from: composedCurve)

        let hsm = makeLutBuffer(renderParams.hueSatMapData, label: "dng_hue_sat_map")
        let lt  = makeLutBuffer(renderParams.lookTableData,  label: "dng_look_table")

        return DngRenderResources(
            params: params,
            toneCurve: toneCurve,
            hueSatMap: hsm,
            lookTable: lt
        )
    }

    // MARK: - Encoding

    /// Encode a single DNG render dispatch. Stateless and cheap — safe to call every frame.
    public func encode(
        commandBuffer: MTLCommandBuffer,
        input: MTLTexture,
        output: MTLTexture,
        resources: DngRenderResources
    ) {
        guard let encoder = commandBuffer.makeComputeCommandEncoder() else { return }
        encoder.setComputePipelineState(computePipeline)
        encoder.setTexture(input,  index: 0)
        encoder.setTexture(output, index: 1)
        encoder.setTexture(resources.toneCurve, index: 2)

        var p = resources.params
        encoder.setBytes(&p, length: MemoryLayout<DngColorParams>.size, index: 0)

        encoder.setBuffer(resources.hueSatMap ?? emptyLutBuffer, offset: 0, index: 1)
        encoder.setBuffer(resources.lookTable ?? emptyLutBuffer, offset: 0, index: 2)

        encoder.dispatchGrid(pipeline: computePipeline, width: output.width, height: output.height)
        encoder.endEncoding()
    }

    // MARK: - Internal helpers

    private func makeToneCurveTexture(from data: [Float]) -> MTLTexture {
        let desc = MTLTextureDescriptor()
        desc.textureType = .type1D
        desc.pixelFormat = .r32Float
        desc.width = data.count
        desc.usage = .shaderRead
        desc.storageMode = .shared
        // Descriptor is fully specified; this only fails on OOM, which is fatal anyway.
        let texture = context.device.makeTexture(descriptor: desc)!
        texture.label = "dng_tone_curve"
        var mutableData = data
        texture.replace(
            region: MTLRegionMake1D(0, data.count),
            mipmapLevel: 0,
            withBytes: &mutableData,
            bytesPerRow: data.count * MemoryLayout<Float>.size
        )
        return texture
    }

    private func makeLutBuffer(_ data: [Float]?, label: String) -> MTLBuffer? {
        guard let data, !data.isEmpty else { return nil }
        let buf = context.device.makeBuffer(
            bytes: data,
            length: data.count * MemoryLayout<Float>.stride,
            options: .storageModeShared
        )
        buf?.label = label
        return buf
    }
}

public enum DngRenderPipelineError: Error {
    case shaderNotFound(String)
    case bufferCreationFailed(String)
}
