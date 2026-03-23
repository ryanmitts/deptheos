import Foundation
import Metal
import DeptheosKit

/// Orchestrates paint mode lifecycle: entering/exiting paint mode,
/// source image preparation, mouse event routing, and display updates.
@MainActor
final class PaintCoordinator {
    let paintState = PaintState()

    /// Whether paint mode is currently active.
    private(set) var isActive: Bool = false

    private var metalContext: MetalContext?
    private var displayBridge: DisplayBridge?
    private var project: StackingProject?
    var cameraRAWBackend: CameraRAWBackend = .libRaw
    var dngConverterCache: DNGConverterCache?
    var dcpProfileDirectory: String = AppSettings.defaultDcpProfileDirectory

    // MARK: - Enter / Exit

    /// Enter paint mode from a completed project.
    /// Initializes paint canvas from the project's result texture and auto-selects the first source.
    func enterPaintMode(
        project: StackingProject,
        metalContext: MetalContext,
        displayBridge: DisplayBridge
    ) {
        self.project = project
        self.metalContext = metalContext
        self.displayBridge = displayBridge

        paintState.initialize(
            from: project.resultTexture,
            existingPaint: project.paintTexture,
            device: metalContext.device,
            commandQueue: metalContext.commandQueue,
            dngRenderParams: project.dngRenderParams
        )

        isActive = true
    }

    /// Exit paint mode. Returns the paint texture for project syncing (if any painting was done).
    func exitPaintMode() -> MTLTexture? {
        isActive = false
        let texture = paintState.paintTexture
        paintState.teardown()
        project = nil
        metalContext = nil
        displayBridge = nil
        return texture
    }

    // MARK: - Image Loading

    /// Load an image off the main thread. LibRaw uses pthread_join internally,
    /// so we use DispatchQueue rather than @concurrent to avoid starving
    /// the cooperative thread pool.
    private func loadImageOffMainThread(
        url: URL, loader: UnifiedImageLoader, metalContext: MetalContext
    ) async throws -> DeptheosImage {
        try await withCheckedThrowingContinuation { cont in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    cont.resume(returning: try loader.load(url: url, metalContext: metalContext))
                } catch {
                    cont.resume(throwing: error)
                }
            }
        }
    }

    // MARK: - Source Preparation

    /// Load and prepare a source image for painting (warp + crop to match result).
    /// Keeps the source in linear space (no sRGB conversion) for correct paint blending.
    /// Also produces an sRGB display version for the brush cursor preview.
    func prepareSource(url: URL, frameIndex: Int) async {
        guard let ctx = metalContext, let project = project else { return }

        do {
            var loader = UnifiedImageLoader(ciContext: ctx.ciContext)
            loader.cameraRAWBackend = cameraRAWBackend
            loader.dngConverterCache = dngConverterCache
            loader.customProfileDirectories = [dcpProfileDirectory]
            let image = try await loadImageOffMainThread(url: url, loader: loader, metalContext: ctx)
            let sourceTexture = try image.getTexture(cache: ctx.textureCache)

            let transform = (frameIndex > 0 && frameIndex < project.transforms.count)
                ? project.transforms[frameIndex] : nil
            let workingTexture = try await ctx.applyWarpAndCrop(
                source: sourceTexture, transform: transform, cropArea: project.validArea
            )

            // Also produce sRGB display version for brush cursor
            let srgbTexture = try await displayBridge?.renderLinearToSRGB(
                texture: workingTexture,
                renderParams: project.dngRenderParams
            )

            paintState.setSource(workingTexture, displayTexture: srgbTexture)
            paintState.selectedSourceIndex = frameIndex

        } catch {
            print("[PaintCoordinator] Failed to prepare source: \(error)")
        }
    }

    // MARK: - Mouse Event Handling

    /// Begin a paint stroke at the given image-space pixel coordinate.
    func handleMouseDown(imageX: Float, imageY: Float) {
        paintState.beginStroke()
        paintState.lastDabPosition = nil

        let dab = makeSingleDab(x: imageX, y: imageY, settings: paintState.brushSettings)
        paintState.applyDabs([dab])
        paintState.lastDabPosition = SIMD2<Float>(imageX, imageY)
        fireDisplayUpdate()
    }

    /// Continue the current stroke with interpolated dabs.
    func handleMouseDragged(imageX: Float, imageY: Float) {
        guard let last = paintState.lastDabPosition else {
            // No previous position — treat as initial dab
            let dab = makeSingleDab(x: imageX, y: imageY, settings: paintState.brushSettings)
            paintState.applyDabs([dab])
            paintState.lastDabPosition = SIMD2<Float>(imageX, imageY)
            fireDisplayUpdate()
            return
        }

        let dabs = interpolateDabs(
            fromX: last.x, fromY: last.y,
            toX: imageX, toY: imageY,
            settings: paintState.brushSettings
        )

        if !dabs.isEmpty {
            paintState.applyDabs(dabs)
            paintState.lastDabPosition = SIMD2<Float>(dabs.last!.x, dabs.last!.y)
            fireDisplayUpdate()
        }
    }

    /// End the current stroke and update the display cache (full sync).
    func handleMouseUp() {
        paintState.endStroke()

        // Final display update (awaited for accuracy)
        guard let ctx = metalContext else { return }
        Task {
            await paintState.updateDisplayCache(metalContext: ctx)
        }
    }

    /// Fire-and-forget display update for real-time painting feedback.
    /// Commits the DNG pipeline without waiting — the display link picks it up.
    private func fireDisplayUpdate() {
        guard let ctx = metalContext else { return }
        paintState.updateDisplayCacheAsync(metalContext: ctx)
    }

    // MARK: - Display

    /// Fire-and-forget display update. The DNG pipeline commits without waiting,
    /// so the display link picks up the result on its next frame.
    func refreshDisplay() {
        guard let ctx = metalContext else { return }
        paintState.updateDisplayCacheAsync(metalContext: ctx)
    }

    /// Awaited display update (for initial setup).
    func refreshDisplayAsync() async {
        guard let ctx = metalContext else { return }
        await paintState.updateDisplayCache(metalContext: ctx)
    }

    // MARK: - Undo / Redo

    func undo() {
        paintState.undo()
        refreshDisplay()
    }

    func redo() {
        paintState.redo()
        refreshDisplay()
    }

    /// Undo all strokes in the current session (preservable via redo-all).
    func undoAll() {
        paintState.undoAll()
        refreshDisplay()
    }

    /// Redo all undone strokes.
    func redoAll() {
        paintState.redoAll()
        refreshDisplay()
    }

    /// Permanently reset the canvas to the original stacking result.
    /// Clears all undo/redo history.
    func resetToOriginal() {
        paintState.resetToOriginal()
        refreshDisplay()
    }
}
