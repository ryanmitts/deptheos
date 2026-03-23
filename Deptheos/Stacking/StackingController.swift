import Foundation
import Metal
import DeptheosKit

/// Wraps StackingCoordinator with orchestration logic: starting stacks, handling
/// completion, and focus map visualization.
@Observable
@MainActor
final class StackingController {
    let coordinator: StackingCoordinator
    private let focusMapRenderer: FocusMapRenderer
    private var previewInitialized = false

    // Injected references
    private let metalContext: MetalContext
    private let workspace: WorkspaceState
    private let viewport: ViewportState
    private let displayBridge: DisplayBridge

    /// Called before stacking starts — lets the app exit paint mode and auto-save.
    var onWillStack: (() -> Void)?
    /// Called when stacking completes — receives the result for project creation.
    var onStackingComplete: ((StackingResult) async -> Void)?

    init(
        metalContext: MetalContext,
        workspace: WorkspaceState,
        viewport: ViewportState,
        displayBridge: DisplayBridge
    ) throws {
        self.metalContext = metalContext
        self.workspace = workspace
        self.viewport = viewport
        self.displayBridge = displayBridge
        self.coordinator = StackingCoordinator()
        self.focusMapRenderer = try FocusMapRenderer(metalContext: metalContext)

        // Wire live preview during stacking
        coordinator.onFrameProcessed = { [weak self] focusMap, width, height, currentFrame, totalFrames in
            guard let self, let focusMap else { return }

            let displayTexture: MTLTexture?
            switch self.workspace.selectedAlgorithm {
            case .depth:
                displayTexture = self.focusMapRenderer.renderDepth(
                    depthIndex: focusMap, totalFrames: totalFrames
                )
            case .pyramid, .wavelet:
                displayTexture = self.focusMapRenderer.renderPyramid(focusMap: focusMap)
            }

            if let displayTexture {
                if !self.previewInitialized {
                    self.viewport.displayState.setMainTexture(
                        displayTexture,
                        width: displayTexture.width,
                        height: displayTexture.height
                    )
                    self.viewport.fitToTexture(
                        width: displayTexture.width,
                        height: displayTexture.height
                    )
                    self.previewInitialized = true
                }
                self.viewport.renderer.markDirty()
            }
        }

        // Wire post-finalization focus map display (baked rgba8Unorm, direct blit)
        coordinator.onFinalizedFocusMap = { [weak self] bakedFocusMap in
            guard let self, let texture = bakedFocusMap else { return }
            self.viewport.displayState.setMainTexture(
                texture,
                width: texture.width,
                height: texture.height
            )
            self.viewport.renderer.markDirty()
        }

        // Wire stacking completion
        coordinator.onComplete = { [weak self] result in
            guard let self else { return }
            await self.onStackingComplete?(result)
        }
    }

    // MARK: - Stacking

    func restartStacking(appSettings: AppSettings) {
        coordinator.cancel()
        startStacking(appSettings: appSettings)
    }

    func startStacking(appSettings: AppSettings) {
        let urls = workspace.includedURLs
        guard urls.count >= 2, !coordinator.isStacking else { return }

        onWillStack?()

        // Clear previous display for fresh stacking
        viewport.displayState.clear()
        workspace.selectedImageIndex = nil
        previewInitialized = false

        focusMapRenderer.reset()

        let config: StackingConfig = switch workspace.selectedAlgorithm {
        case .pyramid: .pyramid(smoothness: workspace.pyramidSmoothness)
        case .depth: .depth(smoothing: workspace.depthSmoothing, blendRange: workspace.depthBlendRange, radius: Int(workspace.depthRadius))
        case .wavelet: .wavelet(denoiseLevel: workspace.waveletDenoiseLevel, consistency: workspace.waveletConsistency)
        }

        coordinator.stack(
            urls: urls,
            config: config,
            alignmentEnabled: appSettings.alignmentEnabled,
            cameraRAWBackend: workspace.cameraRAWBackend,
            dngConverterCache: workspace.dngConverterCache,
            dcpProfileDirectory: appSettings.dcpProfileDirectory,
            metalContext: metalContext
        )
    }
}
