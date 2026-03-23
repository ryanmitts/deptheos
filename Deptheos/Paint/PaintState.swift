import Metal
import DeptheosKit

/// GPU-side parameters matching PaintShaders.metal `PaintParams` struct.
struct PaintParams {
    var center_x: Float
    var center_y: Float
    var radius: Float
    var hardness: Float
    var opacity: Float
    var color_tolerance: Float
    var bbox_x: UInt32
    var bbox_y: UInt32
}

/// Core paint engine — owns the paint canvas texture, compute pipeline,
/// undo manager, and applies dabs via Metal compute shader.
@Observable
@MainActor
final class PaintState {

    // MARK: - Textures

    /// Working paint canvas (rgba16Float, GPU-private).
    /// Initialized as a copy of the stacking result.
    private(set) var paintTexture: MTLTexture?

    /// Immutable copy of the original result, used as eraser source.
    private(set) var eraserSource: MTLTexture?

    /// Warped+cropped source image for painting (set by PaintCoordinator).
    private(set) var sourceTexture: MTLTexture?

    /// sRGB display-ready version of the paint canvas (for viewport).
    private(set) var displayTexture: MTLTexture?

    /// sRGB display-ready version of the source (for brush cursor preview).
    private(set) var sourceDisplayTexture: MTLTexture?

    // MARK: - State

    private(set) var width: Int = 0
    private(set) var height: Int = 0
    let brushSettings = BrushSettings()
    private(set) var undoManager: PaintUndoManager?

    var canUndo: Bool { undoManager?.canUndo ?? false }
    var canRedo: Bool { undoManager?.canRedo ?? false }

    /// Index of the source image currently selected for painting.
    var selectedSourceIndex: Int?

    /// Last dab position for interpolation continuity.
    var lastDabPosition: SIMD2<Float>?

    // MARK: - GPU Resources

    private var paintPipeline: MTLComputePipelineState?
    private var device: MTLDevice?
    private var commandQueue: MTLCommandQueue?
    private var dngRenderPipeline: DngRenderPipeline?
    private var dngRenderResources: DngRenderResources?

    /// DNG render params for display cache conversion.
    var dngRenderParams: DeptheosDng.RenderParams?

    // MARK: - Initialize

    /// Set up the paint canvas from a stacking result texture.
    /// If `existingPaint` is provided (e.g., from a saved project), the canvas starts
    /// from the painted state instead of the raw result.
    func initialize(
        from resultTexture: MTLTexture,
        existingPaint: MTLTexture? = nil,
        device: MTLDevice,
        commandQueue: MTLCommandQueue,
        dngRenderParams: DeptheosDng.RenderParams
    ) {
        self.device = device
        self.commandQueue = commandQueue
        self.width = resultTexture.width
        self.height = resultTexture.height
        self.dngRenderParams = dngRenderParams

        // Use the same pixel format as the result texture (rgba16Unorm) so blit copy
        // works correctly. Metal handles float↔unorm conversion automatically in shaders.
        let pixelFormat = resultTexture.pixelFormat

        let canvasDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: pixelFormat,
            width: width, height: height, mipmapped: false
        )
        canvasDesc.usage = [.shaderRead, .shaderWrite]
        canvasDesc.storageMode = .private
        paintTexture = device.makeTexture(descriptor: canvasDesc)
        paintTexture?.label = "paint_canvas"

        // Create eraser source (immutable copy of the original result — used by eraser tool and undo-all)
        eraserSource = device.makeTexture(descriptor: canvasDesc)
        eraserSource?.label = "paint_eraser_source"

        guard let canvas = paintTexture, let eraser = eraserSource,
              let cb = commandQueue.makeCommandBuffer(),
              let blit = cb.makeBlitCommandEncoder() else { return }

        let size = MTLSize(width: width, height: height, depth: 1)
        let origin = MTLOrigin(x: 0, y: 0, z: 0)

        // Canvas starts from existing paint data if available, otherwise from the result
        let canvasSource = existingPaint ?? resultTexture
        blit.copy(from: canvasSource, sourceSlice: 0, sourceLevel: 0,
                  sourceOrigin: origin, sourceSize: size,
                  to: canvas, destinationSlice: 0, destinationLevel: 0,
                  destinationOrigin: origin)

        // Eraser source is always the original result (immutable)
        blit.copy(from: resultTexture, sourceSlice: 0, sourceLevel: 0,
                  sourceOrigin: origin, sourceSize: size,
                  to: eraser, destinationSlice: 0, destinationLevel: 0,
                  destinationOrigin: origin)
        blit.endEncoding()
        cb.commit()
        cb.waitUntilCompleted()

        // Build compute pipeline
        guard let library = device.makeDefaultLibrary(),
              let function = library.makeFunction(name: "paint_brush") else {
            print("[PaintState] Failed to load paint_brush shader")
            return
        }
        paintPipeline = try? device.makeComputePipelineState(function: function)

        // Create undo manager (matching the canvas pixel format)
        undoManager = PaintUndoManager(device: device, pixelFormat: pixelFormat)
    }

    // MARK: - Source Management

    /// Set the warped+cropped source texture for painting.
    func setSource(_ texture: MTLTexture?, displayTexture: MTLTexture?) {
        sourceTexture = texture
        sourceDisplayTexture = displayTexture
    }

    // MARK: - Paint Dabs

    /// Apply a batch of dabs to the paint canvas.
    /// Uses a single compute encoder with read_write canvas access for all dabs.
    func applyDabs(_ dabs: [Dab]) {
        guard let canvas = paintTexture,
              let pipeline = paintPipeline,
              let queue = commandQueue,
              let undo = undoManager else { return }

        // Determine which texture to sample from (source for brush, eraser source for eraser)
        let isEraser = dabs.first?.isEraser ?? false
        guard let source = isEraser ? eraserSource : sourceTexture else { return }

        guard let cb = queue.makeCommandBuffer(),
              let encoder = cb.makeComputeCommandEncoder() else { return }

        encoder.setComputePipelineState(pipeline)
        encoder.setTexture(source, index: 0)
        encoder.setTexture(canvas, index: 1)

        for dab in dabs {
            let radius = dab.radius
            let bboxX = max(Int(dab.x - radius), 0)
            let bboxY = max(Int(dab.y - radius), 0)
            let bboxRight = min(Int(dab.x + radius) + 1, width)
            let bboxBottom = min(Int(dab.y + radius) + 1, height)
            let bboxW = bboxRight - bboxX
            let bboxH = bboxBottom - bboxY
            guard bboxW > 0, bboxH > 0 else { continue }

            // Snapshot tiles before modification (synchronous readback for tiles not yet captured)
            undo.snapshotTilesBeforeDab(
                dabBBoxX: bboxX, dabBBoxY: bboxY, dabBBoxW: bboxW, dabBBoxH: bboxH,
                paintTexture: canvas, commandQueue: queue
            )

            // Encode compute dispatch
            var params = PaintParams(
                center_x: dab.x,
                center_y: dab.y,
                radius: dab.radius,
                hardness: dab.hardness,
                opacity: dab.opacity,
                color_tolerance: dab.colorTolerance,
                bbox_x: UInt32(bboxX),
                bbox_y: UInt32(bboxY)
            )

            encoder.setBytes(&params, length: MemoryLayout<PaintParams>.size, index: 0)
            encoder.dispatchGrid(pipeline: pipeline, width: bboxW, height: bboxH)
            encoder.memoryBarrier(scope: .textures)
        }

        encoder.endEncoding()
        cb.commit()
        // Don't wait — let the GPU work while we handle more mouse events.
        // Display update will pick up the latest committed state.
    }

    /// End the current stroke (called on mouseUp).
    func endStroke() {
        undoManager?.endStroke()
        lastDabPosition = nil
    }

    /// Begin a new stroke (called on mouseDown).
    func beginStroke() {
        undoManager?.beginStroke()
    }

    // MARK: - Undo / Redo

    func undo() {
        guard let canvas = paintTexture, let queue = commandQueue, let undo = undoManager else { return }
        _ = undo.undo(paintTexture: canvas, commandQueue: queue)
    }

    func redo() {
        guard let canvas = paintTexture, let queue = commandQueue, let undo = undoManager else { return }
        _ = undo.redo(paintTexture: canvas, commandQueue: queue)
    }

    /// Undo all strokes in the current session (loops the undo stack → redo stack).
    func undoAll() {
        guard let canvas = paintTexture, let queue = commandQueue, let undo = undoManager else { return }
        while undo.canUndo {
            _ = undo.undo(paintTexture: canvas, commandQueue: queue)
        }
    }

    /// Redo all undone strokes (loops the redo stack → undo stack).
    func redoAll() {
        guard let canvas = paintTexture, let queue = commandQueue, let undo = undoManager else { return }
        while undo.canRedo {
            _ = undo.redo(paintTexture: canvas, commandQueue: queue)
        }
    }

    /// Permanently reset the canvas to the original stacking result.
    /// Clears all undo/redo history.
    func resetToOriginal() {
        guard let canvas = paintTexture, let eraser = eraserSource,
              let queue = commandQueue else { return }

        guard let cb = queue.makeCommandBuffer(),
              let blit = cb.makeBlitCommandEncoder() else { return }
        blit.copy(
            from: eraser, sourceSlice: 0, sourceLevel: 0,
            sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
            sourceSize: MTLSize(width: width, height: height, depth: 1),
            to: canvas, destinationSlice: 0, destinationLevel: 0,
            destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0)
        )
        blit.endEncoding()
        cb.commit()
        cb.waitUntilCompleted()

        undoManager?.reset()
    }

    // MARK: - Display Cache

    /// Ensure the display texture exists (call once during init).
    func ensureDisplayTexture(metalContext: MetalContext) {
        if displayTexture == nil || displayTexture!.width != width || displayTexture!.height != height {
            let desc = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: .rgba16Unorm,
                width: width, height: height, mipmapped: false
            )
            desc.usage = [.shaderRead, .shaderWrite]
            desc.storageMode = .private
            displayTexture = metalContext.device.makeTexture(descriptor: desc)
            displayTexture?.label = "paint_display"
        }
    }

    /// Run the DNG color pipeline on the paint canvas to produce an sRGB display texture.
    /// Async version — waits for GPU completion. Use for initial display and after undo/redo.
    func updateDisplayCache(metalContext: MetalContext) async {
        guard let canvas = paintTexture else { return }
        ensureDisplayTexture(metalContext: metalContext)
        guard let (pipeline, resources) = ensureDngPipeline(metalContext: metalContext),
              let output = displayTexture,
              let cb = metalContext.commandQueue.makeCommandBuffer() else { return }

        pipeline.encode(
            commandBuffer: cb,
            input: canvas,
            output: output,
            resources: resources
        )
        cb.commit()
        await cb.completed()
    }

    /// Fire-and-forget display update — commits the DNG pipeline without waiting.
    /// Use during active painting for real-time feedback. The renderer will pick up
    /// the updated display texture on the next frame via CAMetalDisplayLink.
    func updateDisplayCacheAsync(metalContext: MetalContext) {
        guard let canvas = paintTexture else { return }
        ensureDisplayTexture(metalContext: metalContext)
        guard let (pipeline, resources) = ensureDngPipeline(metalContext: metalContext),
              let output = displayTexture,
              let cb = metalContext.commandQueue.makeCommandBuffer() else { return }

        pipeline.encode(
            commandBuffer: cb,
            input: canvas,
            output: output,
            resources: resources
        )
        cb.commit()
        // Don't wait — the display link will sample the texture once the GPU finishes.
    }

    /// Lazily build the DNG render pipeline and its per-renderParams resources.
    /// `dngRenderParams` is set once in `initialize` and doesn't change for the
    /// lifetime of a paint session, so the resources are built once and reused
    /// across every stroke.
    private func ensureDngPipeline(metalContext: MetalContext) -> (DngRenderPipeline, DngRenderResources)? {
        guard let params = dngRenderParams else { return nil }
        if dngRenderPipeline == nil {
            dngRenderPipeline = try? DngRenderPipeline(context: metalContext)
        }
        guard let pipeline = dngRenderPipeline else { return nil }
        if dngRenderResources == nil {
            dngRenderResources = pipeline.makeResources(for: params)
        }
        guard let resources = dngRenderResources else { return nil }
        return (pipeline, resources)
    }

    // MARK: - Cleanup

    func teardown() {
        paintTexture = nil
        eraserSource = nil
        sourceTexture = nil
        sourceDisplayTexture = nil
        displayTexture = nil
        paintPipeline = nil
        dngRenderResources = nil
        undoManager?.reset()
        undoManager = nil
        lastDabPosition = nil
        selectedSourceIndex = nil
        width = 0
        height = 0
    }
}
