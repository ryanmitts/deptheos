import Metal
import simd

// Swift mirrors of the structs in SlugShaderTypes.h.
// Layout must match exactly (simd types have same alignment in Swift and Metal).

struct SlugGlyphInstance {
    var scaleBias: SIMD4<Float>
    var glyphBandScale: SIMD4<Float>
    var bandMaxTexCoords: SIMD4<UInt32>
}

struct SlugUniforms {
    var color: SIMD4<Float>
}

/// Renders Slug font text using Metal instanced draw calls.
public final class SlugFontRenderer: @unchecked Sendable {

    private let context: SlugMetalContext
    private let font: SlugFont
    private let renderPipeline: MTLRenderPipelineState

    /// Create a renderer for the given font.
    /// - Parameters:
    ///   - context: Shared Metal context (device, queue, library).
    ///   - font: Loaded SlugFont with GPU textures.
    ///   - pixelFormat: Pixel format of the render target (default .bgra8Unorm).
    public init(context: SlugMetalContext, font: SlugFont, pixelFormat: MTLPixelFormat = .bgra8Unorm) throws {
        self.context = context
        self.font = font

        guard let vertexFn = context.library.makeFunction(name: "slug_vertex"),
              let fragmentFn = context.library.makeFunction(name: "slug_fragment") else {
            throw SlugMetalError.functionNotFound("slug_vertex or slug_fragment")
        }

        let desc = MTLRenderPipelineDescriptor()
        desc.vertexFunction = vertexFn
        desc.fragmentFunction = fragmentFn
        desc.colorAttachments[0].pixelFormat = pixelFormat
        desc.colorAttachments[0].isBlendingEnabled = true
        desc.colorAttachments[0].sourceRGBBlendFactor = .sourceAlpha
        desc.colorAttachments[0].destinationRGBBlendFactor = .oneMinusSourceAlpha
        desc.colorAttachments[0].sourceAlphaBlendFactor = .sourceAlpha
        desc.colorAttachments[0].destinationAlphaBlendFactor = .oneMinusSourceAlpha

        self.renderPipeline = try context.device.makeRenderPipelineState(descriptor: desc)
    }

    // MARK: - Public API

    /// Encode text rendering into an existing render command encoder.
    public func encode(
        encoder: MTLRenderCommandEncoder,
        text: String,
        origin: SIMD2<Float>,
        size: Float,
        color: SIMD4<Float>,
        viewportSize: SIMD2<Float>
    ) {
        let instances = buildInstances(text: text, origin: origin, size: size, viewportSize: viewportSize)
        guard !instances.isEmpty else { return }

        var uniforms = SlugUniforms(color: color)

        let instanceBuffer = context.device.makeBuffer(
            bytes: instances,
            length: instances.count * MemoryLayout<SlugGlyphInstance>.stride,
            options: .storageModeShared
        )

        encoder.setRenderPipelineState(renderPipeline)
        encoder.setVertexBuffer(instanceBuffer, offset: 0, index: 0)
        encoder.setVertexBytes(&uniforms, length: MemoryLayout<SlugUniforms>.stride, index: 1)
        encoder.setFragmentTexture(font.curvesTexture, index: 0)
        encoder.setFragmentTexture(font.bandsTexture, index: 1)
        encoder.setFragmentBytes(&uniforms, length: MemoryLayout<SlugUniforms>.stride, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: instances.count)
    }

    /// Render text into a render pass descriptor (e.g., from MTKView).
    public func encodeRender(
        commandBuffer: MTLCommandBuffer,
        text: String,
        origin: SIMD2<Float>,
        size: Float,
        color: SIMD4<Float>,
        renderPassDescriptor: MTLRenderPassDescriptor,
        viewportSize: SIMD2<Float>
    ) {
        guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPassDescriptor) else { return }
        encoder.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(viewportSize.x), height: Double(viewportSize.y), znear: 0, zfar: 1))
        encode(encoder: encoder, text: text, origin: origin, size: size, color: color, viewportSize: viewportSize)
        encoder.endEncoding()
    }

    /// Render text onto an arbitrary texture (preserves existing content).
    public func renderToTexture(
        commandBuffer: MTLCommandBuffer,
        text: String,
        origin: SIMD2<Float>,
        size: Float,
        color: SIMD4<Float>,
        texture: MTLTexture
    ) {
        let rpd = MTLRenderPassDescriptor()
        rpd.colorAttachments[0].texture = texture
        rpd.colorAttachments[0].loadAction = .load
        rpd.colorAttachments[0].storeAction = .store

        let viewportSize = SIMD2<Float>(Float(texture.width), Float(texture.height))
        encodeRender(
            commandBuffer: commandBuffer,
            text: text, origin: origin, size: size, color: color,
            renderPassDescriptor: rpd, viewportSize: viewportSize
        )
    }

    // MARK: - Private

    private func buildInstances(
        text: String,
        origin: SIMD2<Float>,
        size: Float,
        viewportSize: SIMD2<Float>
    ) -> [SlugGlyphInstance] {
        var instances: [SlugGlyphInstance] = []

        // Scale: `size` pixels = 1 em (ascender - descender).
        let m = font.metrics
        let emFontUnits = Float(m.ascender - m.descender)
        let pxPerUnit = size / emFontUnits

        // Baseline position in pixel space.
        // origin.y is top of line; baseline is ascender distance below that.
        let baseline = origin.y + Float(m.ascender) * pxPerUnit

        var cursorX = origin.x
        var prevCodePoint: UInt32? = nil

        for scalar in text.unicodeScalars {
            let cp = scalar.value

            // Apply kerning between previous and current glyph
            if let prev = prevCodePoint {
                let kernAdj = font.kern(left: prev, right: cp)
                cursorX += Float(kernAdj) * pxPerUnit
            }

            // Space character: advance cursor by a space-width estimate
            if cp == 0x20 {
                if let spaceGlyph = font.glyphs[0x20] {
                    cursorX += Float(spaceGlyph.advanceWidth) * pxPerUnit
                } else {
                    cursorX += emFontUnits * 0.25 * pxPerUnit
                }
                prevCodePoint = cp
                continue
            }

            guard let glyph = font.glyphs[cp] else {
                prevCodePoint = cp
                continue
            }
            guard glyph.height > 0 else {
                prevCodePoint = cp
                continue
            }

            let pixelW = Float(glyph.width) * pxPerUnit
            let pixelH = Float(glyph.height) * pxPerUnit

            // Position glyph using its left side bearing and bearingY.
            // bearingY is the top of the bounding box relative to the font baseline.
            let glyphX = cursorX + Float(glyph.leftSideBearing) * pxPerUnit
            let glyphY = baseline - Float(glyph.bearingY) * pxPerUnit

            // Convert pixel coords to NDC [-1,1]
            let ndcW = (pixelW / viewportSize.x) * 2.0
            let ndcH = (pixelH / viewportSize.y) * 2.0
            let ndcX = (glyphX / viewportSize.x) * 2.0 - 1.0
            let ndcY = 1.0 - ((glyphY + pixelH) / viewportSize.y) * 2.0

            let inst = SlugGlyphInstance(
                scaleBias: SIMD4<Float>(ndcW, ndcH, ndcX, ndcY),
                glyphBandScale: SIMD4<Float>(
                    Float(glyph.width),
                    Float(glyph.height),
                    Float(glyph.width) / Float(glyph.bandDimX),
                    Float(glyph.height) / Float(glyph.bandDimY)
                ),
                bandMaxTexCoords: SIMD4<UInt32>(
                    glyph.bandCount - 1,
                    glyph.bandCount - 1,
                    UInt32(glyph.bandsTexCoordX),
                    UInt32(glyph.bandsTexCoordY)
                )
            )

            instances.append(inst)

            // Advance cursor by the glyph's real advance width
            cursorX += Float(glyph.advanceWidth) * pxPerUnit
            prevCodePoint = cp
        }

        return instances
    }
}
