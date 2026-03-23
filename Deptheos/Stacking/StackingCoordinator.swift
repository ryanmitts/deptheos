import Foundation
import Metal
import DeptheosKit

/// Coordinates the stacking pipeline, consuming the processStack event stream
/// and publishing progress state for the UI.
@Observable
@MainActor
final class StackingCoordinator {
    var isStacking = false
    var progress: Double = 0
    var progressMessage: String = ""
    var error: String?

    private var currentSession: StackingSession?
    private var stackingTask: Task<Void, Never>?
    private var hasBlendPhase = false

    /// Config captured at stacking start, used for project creation.
    private(set) var activeConfig: StackingConfig?

    /// Transforms from the last completed stacking session.
    private(set) var lastTransforms: [AffineTransform2D] = []

    /// Called on main thread when stacking completes successfully.
    var onComplete: (@MainActor (StackingResult) async -> Void)?

    /// Called after each frame is processed with the live focus map texture.
    /// Parameters: focusMap, width, height, currentFrame, totalFrames.
    var onFrameProcessed: (@MainActor (MTLTexture?, Int, Int, Int, Int) -> Void)?

    /// Called after finalization with the baked (rgba8Unorm) focus map for direct display.
    var onFinalizedFocusMap: (@MainActor (MTLTexture?) -> Void)?

    // MARK: - Stacking

    func stack(
        urls: [URL],
        config: StackingConfig,
        alignmentEnabled: Bool,
        cameraRAWBackend: CameraRAWBackend = .libRaw,
        dngConverterCache: DNGConverterCache? = nil,
        dcpProfileDirectory: String = AppSettings.defaultDcpProfileDirectory,
        metalContext: MetalContext
    ) {
        guard !isStacking else { return }

        isStacking = true
        progress = 0
        progressMessage = "Starting..."
        error = nil
        if case .pyramid = config {
            hasBlendPhase = false
        } else {
            hasBlendPhase = true
        }
        activeConfig = config
        lastTransforms = []

        stackingTask = Task {
            do {
                let result = try await runPipeline(
                    urls: urls, config: config, alignmentEnabled: alignmentEnabled,
                    cameraRAWBackend: cameraRAWBackend, dngConverterCache: dngConverterCache,
                    dcpProfileDirectory: dcpProfileDirectory, metalContext: metalContext
                )

                if let result {
                    let transforms = await currentSession!.imageTransforms
                    lastTransforms = transforms
                    isStacking = false
                    await onComplete?(result)
                }
            } catch is CancellationError {
                isStacking = false
                progressMessage = "Cancelled"
            } catch {
                isStacking = false
                self.error = error.localizedDescription
                progressMessage = "Error"
            }
        }
    }

    /// Run the stacking pipeline on the cooperative thread pool.
    /// Image loading is handled by DeptheosKit's ImagePrefetcher (DispatchQueue-bridged),
    /// and GPU work regularly suspends, so @concurrent is appropriate here.
    @concurrent
    private func runPipeline(
        urls: [URL],
        config: StackingConfig,
        alignmentEnabled: Bool,
        cameraRAWBackend: CameraRAWBackend,
        dngConverterCache: DNGConverterCache?,
        dcpProfileDirectory: String,
        metalContext: MetalContext
    ) async throws -> StackingResult? {
        let totalFrames = urls.count

        let algorithm: any StackingAlgorithm = switch config {
        case .pyramid(let smoothness):
            try PyramidStacker(
                metalContext: metalContext,
                config: PyramidConfig(smoothness: smoothness)
            )
        case .depth(let smoothing, let blendRange, let radius):
            try DepthStacker(
                metalContext: metalContext,
                config: DepthConfig(
                    radius: Float(radius), totalFrames: totalFrames,
                    smoothing: smoothing, blendRange: blendRange
                )
            )
        case .wavelet(let denoiseLevel, let consistency):
            try WaveletStacker(
                metalContext: metalContext,
                config: WaveletConfig(totalFrames: totalFrames, denoiseLevel: denoiseLevel, consistency: consistency)
            )
        }

        let aligner: ECCAligner? = alignmentEnabled ? try ECCAligner(context: metalContext) : nil

        let session = StackingSession(
            metalContext: metalContext,
            algorithm: algorithm,
            aligner: aligner
        )

        await setCurrentSession(session)

        var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
        loader.cameraRAWBackend = cameraRAWBackend
        loader.dngConverterCache = dngConverterCache
        loader.customProfileDirectories = [dcpProfileDirectory]

        var finalResult: StackingResult?

        #if os(iOS)
        let prefetchCount = 1
        #else
        let prefetchCount = 4
        #endif

        for try await event in await session.processStack(
            urls: urls, loader: loader, prefetchCount: prefetchCount
        ) {
            try Task.checkCancellation()
            await handleStackingEvent(event, totalFrames: totalFrames)
            if case .complete(let result) = event {
                finalResult = result
            }
        }

        return finalResult
    }

    private func setCurrentSession(_ session: StackingSession) {
        currentSession = session
    }

    /// Handle a single stacking event on the main actor.
    private func handleStackingEvent(_ event: StackingEvent, totalFrames: Int) {
        switch event {
        case .accumulated(let current, let total, let focusMap):
            let fraction = Double(current) / Double(total)
            progress = fraction * (hasBlendPhase ? 0.5 : 0.95)
            progressMessage = "Accumulating \(current)/\(total)"
            onFrameProcessed?(focusMap, focusMap?.width ?? 0, focusMap?.height ?? 0, current, total)
        case .finalized(let bakedFocusMap):
            progress = hasBlendPhase ? 0.5 : 0.98
            progressMessage = hasBlendPhase ? "Blending..." : "Finalizing..."
            onFinalizedFocusMap?(bakedFocusMap)
        case .blended(let current, let total):
            progress = 0.5 + Double(current) / Double(total) * 0.4
            progressMessage = "Blending \(current)/\(total)"
        case .complete:
            progress = 1.0
            progressMessage = "Complete"
        }
    }

    nonisolated deinit {
        MainActor.assumeIsolated { stackingTask?.cancel() }
    }

    // MARK: - Lifecycle

    func cancel() {
        isStacking = false
        currentSession?.cancel()
        stackingTask?.cancel()
        currentSession = nil
        stackingTask = nil
    }
}
