import Metal
import SlugFont
import DeptheosShaders

/// Renders annotated focus map previews for stacking algorithms.
///
/// - **Depth**: Viridis-colormapped depth map with sidebar gradient bar and frame index labels.
/// - **Pyramid**: Sqrt-scaled grayscale.
/// - **Wavelet**: No preview during accumulation (focusMap is nil).
///
/// Available in both the app (live preview) and CLI (export).
/// Not internally synchronized — all mutable state (cached output texture) must be
/// accessed from a single caller context (e.g. the stacking session actor).
public final class FocusMapRenderer: @unchecked Sendable {
    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue

    // Pipelines
    private let colorbarPipeline: MTLComputePipelineState
    private let pyramidBakePipeline: MTLComputePipelineState

    // SlugFont for frame index labels
    private let slugContext: SlugMetalContext
    private let slugFont: SlugFont
    private let slugRenderer: SlugFontRenderer

    // Cached output (for fire-and-forget live preview)
    private var outputTexture: MTLTexture?
    private var outputWidth: Int = 0
    private var outputHeight: Int = 0

    public init(metalContext: MetalContext) throws {
        self.device = metalContext.device
        self.commandQueue = metalContext.commandQueue

        let library = metalContext.defaultLibrary
        guard let colorbarFn = library.makeFunction(name: "focus_colorbar") else {
            throw FocusMapRendererError.shaderNotFound("focus_colorbar")
        }
        guard let pyramidFn = library.makeFunction(name: "focus_pyramid_bake") else {
            throw FocusMapRendererError.shaderNotFound("focus_pyramid_bake")
        }
        self.colorbarPipeline = try device.makeComputePipelineState(function: colorbarFn)
        self.pyramidBakePipeline = try device.makeComputePipelineState(function: pyramidFn)

        self.slugContext = try SlugMetalContext(device: device)
        self.slugFont = try SlugFont(device: device)
        self.slugRenderer = try SlugFontRenderer(context: slugContext, font: slugFont, pixelFormat: .rgba8Unorm)
    }

    // MARK: - Depth: Viridis + Colorbar + Labels

    /// Render depth focus map with viridis colormap, gradient sidebar, and frame index labels.
    /// `depthIndex` is R32Float with values pre-normalized to [0,1].
    /// Fire-and-forget — no waitUntilCompleted. GPU serialization handles ordering.
    public func renderDepth(depthIndex: MTLTexture, totalFrames: Int) -> MTLTexture? {
        let (output, params, layout) = prepareDepthRender(depthIndex: depthIndex)
        guard let output, let commandBuffer = commandQueue.makeCommandBuffer() else { return nil }
        commandBuffer.label = "focus_colorbar"

        encodeDepthRender(
            commandBuffer: commandBuffer, output: output, depthIndex: depthIndex,
            params: params, totalFrames: totalFrames, layout: layout
        )

        commandBuffer.commit()
        return output
    }

    /// Render depth focus map for export/persistence.
    /// Allocates a fresh texture and waits for GPU completion.
    public func renderDepthForExport(depthIndex: MTLTexture, totalFrames: Int) async -> MTLTexture? {
        let imageW = depthIndex.width
        let imageH = depthIndex.height
        let layout = sidebarLayout(imageW: imageW, imageH: imageH)

        guard let output = makeOutputTexture(width: layout.totalW, height: imageH, label: "depth_map_export"),
              let commandBuffer = commandQueue.makeCommandBuffer() else { return nil }
        commandBuffer.label = "focus_colorbar_export"

        let params = makeFocusColorbarParams(imageW: imageW, imageH: imageH, layout: layout)

        encodeDepthRender(
            commandBuffer: commandBuffer, output: output, depthIndex: depthIndex,
            params: params, totalFrames: totalFrames, layout: layout
        )

        commandBuffer.commit()
        await commandBuffer.completed()
        return output
    }

    // MARK: - Pyramid: Sqrt Grayscale

    /// Render pyramid focus map as sqrt-scaled grayscale.
    /// Fire-and-forget for live preview.
    public func renderPyramid(focusMap: MTLTexture) -> MTLTexture? {
        let w = focusMap.width
        let h = focusMap.height

        if outputTexture == nil || outputWidth != w || outputHeight != h {
            outputTexture = makeOutputTexture(width: w, height: h, label: "pyramid_focus_display")
            outputWidth = w
            outputHeight = h
        }

        guard let output = outputTexture,
              let commandBuffer = commandQueue.makeCommandBuffer(),
              let encoder = commandBuffer.makeComputeCommandEncoder() else { return nil }

        encoder.setComputePipelineState(pyramidBakePipeline)
        encoder.setTexture(focusMap, index: 0)
        encoder.setTexture(output, index: 1)

        encoder.dispatchGrid(pipeline: pyramidBakePipeline, width: w, height: h)
        encoder.endEncoding()

        commandBuffer.commit()
        return output
    }

    /// Render pyramid focus map for export/persistence.
    /// Allocates a fresh texture and waits for GPU completion.
    public func renderPyramidForExport(focusMap: MTLTexture) async -> MTLTexture? {
        let w = focusMap.width
        let h = focusMap.height

        guard let output = makeOutputTexture(width: w, height: h, label: "pyramid_focus_export"),
              let commandBuffer = commandQueue.makeCommandBuffer(),
              let encoder = commandBuffer.makeComputeCommandEncoder() else { return nil }

        encoder.setComputePipelineState(pyramidBakePipeline)
        encoder.setTexture(focusMap, index: 0)
        encoder.setTexture(output, index: 1)

        encoder.dispatchGrid(pipeline: pyramidBakePipeline, width: w, height: h)
        encoder.endEncoding()

        commandBuffer.commit()
        await commandBuffer.completed()
        return output
    }

    // MARK: - Reset

    public func reset() {
        outputTexture = nil
        outputWidth = 0
        outputHeight = 0
    }

    // MARK: - Private Helpers

    private struct SidebarLayout {
        let sidebarW: Int
        let totalW: Int
        let padding: Int
        let fontSize: Float
        let gradientTop: Int
        let gradientBottom: Int
        let gradientX: Int
        let gradientW: Int
    }

    private func sidebarLayout(imageW: Int, imageH: Int) -> SidebarLayout {
        let longerDim = max(imageW, imageH)
        let sidebarW = max(longerDim / 12, 120)
        let totalW = imageW + sidebarW
        let padding = sidebarW / 6
        let fontSize = Float(sidebarW) / 4.0
        let gradientTop = padding + Int(fontSize * 0.7)
        let gradientBottom = imageH - padding - Int(fontSize * 0.7)
        let gradientX = imageW + padding
        let gradientW = sidebarW / 4
        return SidebarLayout(
            sidebarW: sidebarW, totalW: totalW, padding: padding,
            fontSize: fontSize, gradientTop: gradientTop, gradientBottom: gradientBottom,
            gradientX: gradientX, gradientW: gradientW
        )
    }

    private func makeFocusColorbarParams(imageW: Int, imageH: Int, layout: SidebarLayout) -> FocusColorbarParams {
        FocusColorbarParams(
            image_width: UInt32(imageW),
            total_width: UInt32(layout.totalW),
            total_height: UInt32(imageH),
            sidebar_x: UInt32(imageW),
            gradient_top: UInt32(layout.gradientTop),
            gradient_bottom: UInt32(layout.gradientBottom),
            gradient_x: UInt32(layout.gradientX),
            gradient_w: UInt32(layout.gradientW)
        )
    }

    private func makeOutputTexture(width: Int, height: Int, label: String) -> MTLTexture? {
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba8Unorm,
            width: width, height: height, mipmapped: false
        )
        desc.usage = [.shaderRead, .shaderWrite, .renderTarget]
        desc.storageMode = .private
        let tex = device.makeTexture(descriptor: desc)
        tex?.label = label
        return tex
    }

    private func prepareDepthRender(depthIndex: MTLTexture) ->
        (MTLTexture?, FocusColorbarParams, SidebarLayout) {
        let imageW = depthIndex.width
        let imageH = depthIndex.height
        let layout = sidebarLayout(imageW: imageW, imageH: imageH)

        if outputTexture == nil || outputWidth != layout.totalW || outputHeight != imageH {
            outputTexture = makeOutputTexture(width: layout.totalW, height: imageH, label: "annotated_focus_map")
            outputWidth = layout.totalW
            outputHeight = imageH
        }

        let params = makeFocusColorbarParams(imageW: imageW, imageH: imageH, layout: layout)
        return (outputTexture, params, layout)
    }

    private func encodeDepthRender(
        commandBuffer: MTLCommandBuffer,
        output: MTLTexture,
        depthIndex: MTLTexture,
        params: FocusColorbarParams,
        totalFrames: Int,
        layout: SidebarLayout
    ) {
        // Step 1: Compute — viridis colormap + sidebar
        guard let encoder = commandBuffer.makeComputeCommandEncoder() else { return }
        encoder.label = "focus_colorbar"

        var params = params
        encoder.setComputePipelineState(colorbarPipeline)
        encoder.setTexture(depthIndex, index: 0)
        encoder.setTexture(output, index: 1)
        encoder.setBytes(&params, length: MemoryLayout<FocusColorbarParams>.stride, index: 0)

        encoder.dispatchGrid(pipeline: colorbarPipeline, width: Int(params.total_width), height: Int(params.total_height))
        encoder.endEncoding()

        // Step 2: Render text labels
        encodeTextLabels(
            commandBuffer: commandBuffer, output: output,
            totalFrames: totalFrames, layout: layout
        )
    }

    private func encodeTextLabels(
        commandBuffer: MTLCommandBuffer,
        output: MTLTexture,
        totalFrames: Int,
        layout: SidebarLayout
    ) {
        let labelX = Float(layout.gradientX + layout.gradientW + layout.padding / 2)
        let effectiveFrames = max(totalFrames, 2)
        let ticks = tickIndices(totalFrames: effectiveFrames)

        let rpd = MTLRenderPassDescriptor()
        rpd.colorAttachments[0].texture = output
        rpd.colorAttachments[0].loadAction = .load
        rpd.colorAttachments[0].storeAction = .store
        let viewportSize = SIMD2<Float>(Float(output.width), Float(output.height))

        guard let labelEncoder = commandBuffer.makeRenderCommandEncoder(descriptor: rpd) else { return }
        labelEncoder.setViewport(MTLViewport(
            originX: 0, originY: 0,
            width: Double(viewportSize.x), height: Double(viewportSize.y),
            znear: 0, zfar: 1
        ))
        for idx in ticks {
            let t = Float(idx) / Float(effectiveFrames - 1)
            let y = Float(layout.gradientTop) + t * Float(layout.gradientBottom - layout.gradientTop) - layout.fontSize * 0.4
            slugRenderer.encode(
                encoder: labelEncoder,
                text: "\(idx)",
                origin: SIMD2<Float>(labelX, y),
                size: layout.fontSize,
                color: SIMD4<Float>(0.9, 0.9, 0.9, 1.0),
                viewportSize: viewportSize
            )
        }
        labelEncoder.endEncoding()
    }

    private func tickIndices(totalFrames: Int) -> [Int] {
        if totalFrames <= 20 {
            return Array(0..<totalFrames)
        }
        let step: Int
        if totalFrames <= 50 {
            step = 5
        } else if totalFrames <= 100 {
            step = 10
        } else {
            step = max(totalFrames / 10, 1)
        }
        var indices: [Int] = []
        var i = 0
        while i < totalFrames {
            indices.append(i)
            i += step
        }
        if indices.last != totalFrames - 1 {
            indices.append(totalFrames - 1)
        }
        return indices
    }
}

public enum FocusMapRendererError: Error {
    case shaderNotFound(String)
}
