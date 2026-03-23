import Metal
import QuartzCore
#if canImport(AppKit)
import AppKit
#endif
import simd

/// GPU-side uniforms for the compare shader.
struct CompareUniforms {
    var splitPosition: Float
}

/// GPU-side uniforms for the loupe shader.
struct LoupeUniforms {
    var center: SIMD2<Float>
    var radius: Float
    var magnification: Float
    var uvCenter: SIMD2<Float>
    var texelSize: SIMD2<Float>
}

/// GPU-side uniforms for the brush cursor overlay.
struct BrushCursorUniforms {
    var center: SIMD2<Float>    // drawable pixels
    var radius: Float           // drawable pixels
    var hardness: Float         // 0-1
    var opacity: Float          // brush opacity (0-1)
    var uvCenter: SIMD2<Float>  // image UV at cursor center
    var texelSize: SIMD2<Float> // 1/(imageSize*viewScale)
}

/// Drives rendering via CAMetalDisplayLink and blits textures from DisplayState
/// to a CAMetalLayer drawable with pan/zoom transform.
///
/// Double-buffered with in-flight semaphore (count = 2, matching preferredFrameLatency).
///
/// This is a generic texture viewer — it knows nothing about stacking or projects.
final class ViewportRenderer: NSObject {
    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue

    private weak var metalLayer: CAMetalLayer?
    private var displayLink: CAMetalDisplayLink?
    private var displayLinkProxy: DisplayLinkProxy?

    private let inflightSemaphore = DispatchSemaphore(value: 2)

    private var blitPipeline: MTLRenderPipelineState?
    private var comparePipeline: MTLRenderPipelineState?
    private var loupePipeline: MTLRenderPipelineState?
    private var brushCursorPipeline: MTLRenderPipelineState?

    // Reused each frame — only the texture attachment changes.
    private let passDesc: MTLRenderPassDescriptor = {
        let desc = MTLRenderPassDescriptor()
        desc.colorAttachments[0].loadAction = .clear
        desc.colorAttachments[0].storeAction = .store
        desc.colorAttachments[0].clearColor = MTLClearColor(red: 0.1, green: 0.1, blue: 0.1, alpha: 1)
        return desc
    }()

    weak var displayState: DisplayState?
    var viewTransform: ViewTransformController?

    // MARK: - Loupe

    private var loupeUniforms: LoupeUniforms?
    private let loupeLogicalRadius: Float = 100
    private let loupeMagnification: Float = 3.0

    func updateLoupe(screenX: Float, screenY: Float, dpr: Float) {
        guard let viewTransform, let metalLayer, let displayState else { return }

        let drawableW = Float(metalLayer.drawableSize.width)
        let drawableH = Float(metalLayer.drawableSize.height)
        guard drawableW > 0, drawableH > 0 else { return }

        let pixelX = screenX * dpr
        let pixelY = screenY * dpr
        let radius = loupeLogicalRadius * dpr

        let uvCenter: SIMD2<Float>
        let viewMode = displayState.viewMode
        let useSideBySide = (viewMode == .sideBySide || viewMode == .sideBySideVertical) && displayState.compareTexture != nil
        if useSideBySide {
            if viewMode == .sideBySideVertical {
                let halfH = drawableH / 2.0
                if pixelY < halfH {
                    uvCenter = viewTransform.screenToUV(pixelX: pixelX, pixelY: pixelY, viewportHeight: halfH)
                } else {
                    uvCenter = viewTransform.screenToUV(pixelX: pixelX, pixelY: pixelY - halfH, viewportHeight: halfH)
                }
            } else {
                let halfW = drawableW / 2.0
                if pixelX < halfW {
                    uvCenter = viewTransform.screenToUV(pixelX: pixelX, pixelY: pixelY, viewportWidth: halfW)
                } else {
                    uvCenter = viewTransform.screenToUV(pixelX: pixelX - halfW, pixelY: pixelY, viewportWidth: halfW)
                }
            }
        } else {
            uvCenter = viewTransform.screenToUV(screenX: screenX, screenY: screenY)
        }

        let imgW = viewTransform.imageWidth
        let imgH = viewTransform.imageHeight
        let viewScale = viewTransform.scale
        let texelSizeX = (imgW > 0 && viewScale > 0) ? 1.0 / (imgW * viewScale) : 0
        let texelSizeY = (imgH > 0 && viewScale > 0) ? 1.0 / (imgH * viewScale) : 0

        loupeUniforms = LoupeUniforms(
            center: SIMD2<Float>(pixelX, pixelY),
            radius: radius,
            magnification: loupeMagnification / viewScale,
            uvCenter: uvCenter,
            texelSize: SIMD2<Float>(texelSizeX, texelSizeY)
        )
        markDirty()
    }

    func clearLoupe() {
        loupeUniforms = nil
        markDirty()
    }

    // MARK: - Brush Cursor

    private var brushCursorUniforms: BrushCursorUniforms?
    /// Source display texture for the brush cursor preview.
    var brushCursorSourceTexture: MTLTexture?

    func updateBrushCursor(screenX: Float, screenY: Float, brushRadius: Float, hardness: Float, opacity: Float, dpr: Float) {
        guard let viewTransform, let displayState else { return }

        let pixelX = screenX * dpr
        let pixelY = screenY * dpr
        let radius = brushRadius * viewTransform.scale

        let uvCenter: SIMD2<Float>
        let viewMode = displayState.viewMode
        let useSideBySide = (viewMode == .sideBySide || viewMode == .sideBySideVertical) && displayState.compareTexture != nil
        if useSideBySide, let metalLayer {
            if viewMode == .sideBySideVertical {
                let halfH = Float(metalLayer.drawableSize.height) / 2.0
                // Main texture is on the bottom half — remap relative to bottom pane
                uvCenter = viewTransform.screenToUV(pixelX: pixelX, pixelY: pixelY - halfH, viewportHeight: halfH)
            } else {
                let halfW = Float(metalLayer.drawableSize.width) / 2.0
                // Main texture is on the right half — remap relative to right pane
                uvCenter = viewTransform.screenToUV(pixelX: pixelX - halfW, pixelY: pixelY, viewportWidth: halfW)
            }
        } else {
            uvCenter = viewTransform.screenToUV(screenX: screenX, screenY: screenY)
        }

        let imgW = viewTransform.imageWidth
        let imgH = viewTransform.imageHeight
        let viewScale = viewTransform.scale
        let texelSizeX = (imgW > 0 && viewScale > 0) ? 1.0 / (imgW * viewScale) : 0
        let texelSizeY = (imgH > 0 && viewScale > 0) ? 1.0 / (imgH * viewScale) : 0

        brushCursorUniforms = BrushCursorUniforms(
            center: SIMD2<Float>(pixelX, pixelY),
            radius: radius,
            hardness: hardness,
            opacity: opacity,
            uvCenter: uvCenter,
            texelSize: SIMD2<Float>(texelSizeX, texelSizeY)
        )
        markDirty()
    }

    /// Hides the brush cursor overlay. The source texture is preserved so it
    /// survives mouse-exit/enter cycles without needing to be reloaded.
    func clearBrushCursor() {
        brushCursorUniforms = nil
        markDirty()
    }

    init(device: MTLDevice, commandQueue: MTLCommandQueue) {
        self.device = device
        self.commandQueue = commandQueue
        super.init()
        buildPipelines(pixelFormat: .bgra8Unorm)
    }

    // MARK: - Pipeline Setup

    private func buildPipelines(pixelFormat: MTLPixelFormat) {
        guard let library = device.makeDefaultLibrary() else {
            print("ViewportRenderer: Failed to load default Metal library")
            return
        }

        let desc = MTLRenderPipelineDescriptor()
        desc.vertexFunction = library.makeFunction(name: "blit_vertex")
        desc.colorAttachments[0].pixelFormat = pixelFormat

        // Blit pipeline
        desc.fragmentFunction = library.makeFunction(name: "blit_fragment")
        blitPipeline = try? device.makeRenderPipelineState(descriptor: desc)

        // Compare pipeline
        desc.fragmentFunction = library.makeFunction(name: "compare_fragment")
        comparePipeline = try? device.makeRenderPipelineState(descriptor: desc)

        // Loupe pipeline (premultiplied alpha blending)
        desc.fragmentFunction = library.makeFunction(name: "loupe_fragment")
        desc.colorAttachments[0].isBlendingEnabled = true
        desc.colorAttachments[0].sourceRGBBlendFactor = .one
        desc.colorAttachments[0].destinationRGBBlendFactor = .oneMinusSourceAlpha
        desc.colorAttachments[0].sourceAlphaBlendFactor = .one
        desc.colorAttachments[0].destinationAlphaBlendFactor = .oneMinusSourceAlpha
        loupePipeline = try? device.makeRenderPipelineState(descriptor: desc)

        // Brush cursor pipeline (premultiplied alpha, same blend state as loupe)
        desc.fragmentFunction = library.makeFunction(name: "brush_cursor_fragment")
        brushCursorPipeline = try? device.makeRenderPipelineState(descriptor: desc)
    }

    // MARK: - Layer Configuration

    func configureLayer(_ layer: CAMetalLayer) {
        self.metalLayer = layer
        layer.device = device
        layer.framebufferOnly = true
        layer.isOpaque = true

        var pixelFormat: MTLPixelFormat = .bgra8Unorm

        #if canImport(AppKit)
        if let screenColorSpace = NSScreen.main?.colorSpace,
           screenColorSpace.colorSpaceModel == .rgb {
            pixelFormat = .bgr10a2Unorm
            layer.colorspace = CGColorSpace(name: CGColorSpace.sRGB)
        }
        #endif

        layer.pixelFormat = pixelFormat
        layer.wantsExtendedDynamicRangeContent = true

        if pixelFormat != .bgra8Unorm {
            buildPipelines(pixelFormat: pixelFormat)
        }
    }

    // MARK: - Display Link

    /// Weak proxy that breaks the retain cycle between ViewportRenderer and CAMetalDisplayLink.
    private final class DisplayLinkProxy: NSObject, CAMetalDisplayLinkDelegate {
        weak var target: ViewportRenderer?

        init(_ target: ViewportRenderer) {
            self.target = target
        }

        func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
            target?.metalDisplayLink(link, needsUpdate: update)
        }
    }

    func startDisplayLink(_ layer: CAMetalLayer) {
        guard displayLink == nil else { return }
        let link = CAMetalDisplayLink(metalLayer: layer)
        link.preferredFrameLatency = 2
        let proxy = DisplayLinkProxy(self)
        link.delegate = proxy
        link.add(to: .main, forMode: .common)
        link.isPaused = false
        displayLink = link
        displayLinkProxy = proxy
    }

    func stop() {
        displayLink?.isPaused = true
        displayLink?.remove(from: .main, forMode: .common)
        displayLink?.invalidate()
        displayLink = nil
        displayLinkProxy = nil
    }

    // MARK: - Render Loop

    private var contentGeneration: UInt64 = 0
    private var lastRenderedGeneration: UInt64 = 0
    private var previousTargetTimestamp: CFTimeInterval = 0
    private var lastMainTexture: MTLTexture?

    func markDirty() {
        contentGeneration &+= 1
    }

    // MARK: - CAMetalDisplayLinkDelegate

    func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
        guard let metalLayer, let viewTransform, let displayState else { return }

        let scale = metalLayer.contentsScale

        // Sync drawableSize with layer bounds (CAMetalLayer doesn't auto-resize)
        let targetW = Int(metalLayer.bounds.width * scale)
        let targetH = Int(metalLayer.bounds.height * scale)
        if targetW > 0, targetH > 0 {
            let targetSize = CGSize(width: targetW, height: targetH)
            if metalLayer.drawableSize != targetSize {
                metalLayer.drawableSize = targetSize
                markDirty()
            }
        }

        // Advance zoom animation
        let now = update.targetPresentationTimestamp
        if previousTargetTimestamp > 0 {
            let dt = Float(now - previousTargetTimestamp)
            if dt > 0 && dt < 0.5 {
                _ = viewTransform.tick(dt: dt)
            }
        }
        previousTargetTimestamp = now

        let texture = displayState.mainTexture
        let compareTexture = displayState.compareTexture
        let viewMode = displayState.viewMode
        let splitPosition = displayState.splitPosition

        guard let texture else {
            // No texture — draw a blank frame to clear the previous content.
            if lastMainTexture != nil {
                lastMainTexture = nil
                inflightSemaphore.wait()
                autoreleasepool {
                    let drawable = update.drawable
                    guard let commandBuffer = commandQueue.makeCommandBuffer() else {
                        inflightSemaphore.signal()
                        return
                    }
                    let sem = inflightSemaphore
                    commandBuffer.addCompletedHandler { _ in sem.signal() }
                    passDesc.colorAttachments[0].texture = drawable.texture
                    guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: passDesc) else {
                        commandBuffer.commit()
                        return
                    }
                    encoder.endEncoding()
                    commandBuffer.present(drawable)
                    commandBuffer.commit()
                }
            }
            viewTransform.clearDirty()
            return
        }

        // Skip if nothing changed
        let contentChanged = texture !== lastMainTexture || contentGeneration != lastRenderedGeneration
        guard viewTransform.isDirty || contentChanged else { return }

        inflightSemaphore.wait()

        autoreleasepool {
            let drawable = update.drawable

            let w = drawable.texture.width
            let h = drawable.texture.height
            guard w > 0, h > 0 else {
                inflightSemaphore.signal()
                return
            }

            viewTransform.setViewportSize(width: Float(w), height: Float(h))
            viewTransform.contentsScale = Float(scale)

            guard let commandBuffer = commandQueue.makeCommandBuffer() else {
                inflightSemaphore.signal()
                return
            }

            let sem = inflightSemaphore
            commandBuffer.addCompletedHandler { _ in sem.signal() }

            passDesc.colorAttachments[0].texture = drawable.texture

            guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: passDesc) else {
                commandBuffer.commit()
                return
            }

            var transform = viewTransform.transformMatrix()

            let useCompare = viewMode == .compare && compareTexture != nil
            let useSideBySide = (viewMode == .sideBySide || viewMode == .sideBySideVertical) && compareTexture != nil
            let isVerticalSplit = viewMode == .sideBySideVertical

            if useSideBySide, let blitPipeline {
                let gap = 1.0

                if isVerticalSplit {
                    let halfH = Double(h) / 2.0
                    var halfTransform = viewTransform.transformMatrixForHeight(Float(halfH))

                    // TOP: compare texture (MTLViewport originY is measured from top)
                    encoder.setRenderPipelineState(blitPipeline)
                    encoder.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(w), height: halfH - gap, znear: 0, zfar: 1))
                    encoder.setVertexBytes(&halfTransform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                    encoder.setFragmentTexture(compareTexture, index: 0)
                    encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)

                    // BOTTOM: main texture
                    encoder.setViewport(MTLViewport(originX: 0, originY: halfH + gap, width: Double(w), height: halfH - gap, znear: 0, zfar: 1))
                    encoder.setVertexBytes(&halfTransform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                    encoder.setFragmentTexture(texture, index: 0)
                    encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
                } else {
                    let halfW = Double(w) / 2.0
                    var halfTransform = viewTransform.transformMatrixForWidth(Float(halfW))

                    // LEFT: compare texture
                    encoder.setRenderPipelineState(blitPipeline)
                    encoder.setViewport(MTLViewport(originX: 0, originY: 0, width: halfW - gap, height: Double(h), znear: 0, zfar: 1))
                    encoder.setVertexBytes(&halfTransform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                    encoder.setFragmentTexture(compareTexture, index: 0)
                    encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)

                    // RIGHT: main texture
                    encoder.setViewport(MTLViewport(originX: halfW + gap, originY: 0, width: halfW - gap, height: Double(h), znear: 0, zfar: 1))
                    encoder.setVertexBytes(&halfTransform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                    encoder.setFragmentTexture(texture, index: 0)
                    encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
                }

                encoder.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(w), height: Double(h), znear: 0, zfar: 1))

            } else if useCompare, let comparePipeline {
                encoder.setRenderPipelineState(comparePipeline)
                encoder.setVertexBytes(&transform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                encoder.setFragmentTexture(texture, index: 0)
                encoder.setFragmentTexture(compareTexture, index: 1)

                let splitPixels = splitPosition * Float(w)
                var uniforms = CompareUniforms(splitPosition: splitPixels)
                encoder.setFragmentBytes(&uniforms, length: MemoryLayout<CompareUniforms>.size, index: 0)

                encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
            } else if let blitPipeline {
                encoder.setRenderPipelineState(blitPipeline)
                encoder.setVertexBytes(&transform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                encoder.setFragmentTexture(texture, index: 0)
                encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
            }

            // Loupe overlay pass
            if let loupeUniforms, let loupePipeline {
                encoder.setRenderPipelineState(loupePipeline)
                encoder.setVertexBytes(&transform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                encoder.setFragmentTexture(texture, index: 0)
                var loupe = loupeUniforms
                encoder.setFragmentBytes(&loupe, length: MemoryLayout<LoupeUniforms>.size, index: 0)
                encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
            }

            // Brush cursor overlay pass — previews blended paint result.
            // Source texture for what will be painted, canvas texture for current state.
            if let brushCursorUniforms, let brushCursorPipeline {
                let sourceTexture = brushCursorSourceTexture ?? texture
                encoder.setRenderPipelineState(brushCursorPipeline)
                encoder.setVertexBytes(&transform, length: MemoryLayout<simd_float4x4>.size, index: 0)
                encoder.setFragmentTexture(sourceTexture, index: 0)
                encoder.setFragmentTexture(texture, index: 1)
                var cursor = brushCursorUniforms
                encoder.setFragmentBytes(&cursor, length: MemoryLayout<BrushCursorUniforms>.size, index: 0)
                encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
            }

            encoder.endEncoding()

            commandBuffer.present(drawable)
            commandBuffer.commit()

            viewTransform.clearDirty()
            lastMainTexture = texture
            lastRenderedGeneration = contentGeneration
        }
    }

    deinit {
        stop()
    }
}
