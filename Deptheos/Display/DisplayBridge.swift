import Foundation
@preconcurrency import Metal
import DeptheosKit

/// Connects the data layer (StackingResult, StackingProject) to the viewport layer (DisplayState).
///
/// Runs the DNG color pipeline (linear → sRGB) before setting textures on the viewport.
/// Handles alignment transforms and cropping for source image comparison.
/// This is the only layer that knows about both stacking and the viewport.
@MainActor
final class DisplayBridge {
    private let metalContext: MetalContext
    private lazy var dngRenderPipeline: DngRenderPipeline = try! DngRenderPipeline(context: metalContext)
    var cameraRAWBackend: CameraRAWBackend = .libRaw
    var dngConverterCache: DNGConverterCache?
    var dcpProfileDirectory: String = AppSettings.defaultDcpProfileDirectory

    init(metalContext: MetalContext) {
        self.metalContext = metalContext
    }

    // MARK: - Result Display

    /// Render a stacking result through the DNG color pipeline and display it.
    func showResult(_ result: StackingResult, in displayState: DisplayState) async throws {
        let srgbTexture = try await result.rendered()
        displayState.setMainTexture(
            srgbTexture,
            width: srgbTexture.width,
            height: srgbTexture.height
        )
    }

    /// Display a project's result texture (already persisted, needs color pipeline).
    func showProject(_ project: StackingProject, in displayState: DisplayState) async {
        guard let srgbTexture = try? await renderLinearToSRGB(
            texture: project.resultTexture,
            renderParams: project.dngRenderParams
        ) else { return }
        displayState.setMainTexture(
            srgbTexture,
            width: srgbTexture.width,
            height: srgbTexture.height
        )
    }

    // MARK: - Source Image Compare

    /// Prepare a source image for comparison: load, warp, crop, color pipeline → compare slot.
    func showSourceCompare(
        url: URL,
        frameIndex: Int,
        transforms: [AffineTransform2D?],
        validArea: Rect?,
        renderParams: DeptheosDng.RenderParams,
        in displayState: DisplayState
    ) async {
        do {
            guard let texture = try await loadAndPrepareSource(
                url: url, frameIndex: frameIndex,
                transforms: transforms, validArea: validArea,
                renderParams: renderParams
            ) else {
                print("[DisplayBridge] showSourceCompare: loadAndPrepareSource returned nil")
                return
            }
            displayState.setCompareTexture(texture)
        } catch {
            print("[DisplayBridge] showSourceCompare failed: \(error)")
        }
    }

    // MARK: - Source Preview

    /// Load a source image and display it as the main texture (pre-stacking preview).
    func showSourcePreview(url: URL, in displayState: DisplayState) async {
        do {
            var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
            loader.cameraRAWBackend = cameraRAWBackend
            loader.dngConverterCache = dngConverterCache
            loader.customProfileDirectories = [dcpProfileDirectory]
            let image = try await loadImageOffMainThread(url: url, loader: loader, metalContext: metalContext)
            let sourceTexture = try image.getTexture(cache: metalContext.textureCache)
            guard let srgbTexture = try await renderLinearToSRGB(
                texture: sourceTexture, renderParams: image.dngRenderParams
            ) else { return }
            displayState.setMainTexture(srgbTexture, width: srgbTexture.width, height: srgbTexture.height)
        } catch {
            print("[DisplayBridge] showSourcePreview failed: \(error)")
        }
    }

    // MARK: - Direct Texture Display

    /// Display a texture directly (already in sRGB, no pipeline needed).
    func showTexture(_ texture: MTLTexture, in displayState: DisplayState) {
        displayState.setMainTexture(texture, width: texture.width, height: texture.height)
    }

    /// Clear the display.
    func clear(_ displayState: DisplayState) {
        displayState.clear()
    }

    // MARK: - Color Pipeline

    /// Run the DNG color pipeline: linear → sRGB.
    func renderLinearToSRGB(
        texture: MTLTexture,
        renderParams: DeptheosDng.RenderParams
    ) async throws -> MTLTexture? {
        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else { return nil }

        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba16Unorm,
            width: texture.width, height: texture.height, mipmapped: false
        )
        desc.usage = [.shaderRead, .shaderWrite]
        desc.storageMode = .private
        guard let outputTexture = metalContext.device.makeTexture(descriptor: desc) else { return nil }
        outputTexture.label = "display_srgb"

        let resources = dngRenderPipeline.makeResources(for: renderParams)
        dngRenderPipeline.encode(
            commandBuffer: commandBuffer,
            input: texture,
            output: outputTexture,
            resources: resources
        )

        commandBuffer.commit()
        await commandBuffer.completed()
        return outputTexture
    }

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

    /// Load a source image, apply alignment transform and crop, run color pipeline.
    private func loadAndPrepareSource(
        url: URL,
        frameIndex: Int,
        transforms: [AffineTransform2D?],
        validArea: Rect?,
        renderParams: DeptheosDng.RenderParams
    ) async throws -> MTLTexture? {
        var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
        loader.cameraRAWBackend = cameraRAWBackend
        loader.dngConverterCache = dngConverterCache
        loader.customProfileDirectories = [dcpProfileDirectory]
        let image = try await loadImageOffMainThread(url: url, loader: loader, metalContext: metalContext)
        let sourceTexture = try image.getTexture(cache: metalContext.textureCache)

        let transform = (frameIndex > 0 && frameIndex < transforms.count) ? transforms[frameIndex] : nil
        let workingTexture = try await metalContext.applyWarpAndCrop(
            source: sourceTexture, transform: transform, cropArea: validArea
        )

        // Run color pipeline
        return try await renderLinearToSRGB(texture: workingTexture, renderParams: renderParams)
    }
}
