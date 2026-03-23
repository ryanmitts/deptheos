//
//  StackingSession.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-23.
//
import Foundation
import Metal
import Synchronization

/// Events emitted during a stacking pipeline.
///
/// Texture references in events are raw algorithm outputs (R32Float, linear float, etc.).
/// The consumer (UI layer) is responsible for any display conversion (colormaps, color pipeline).
public enum StackingEvent: @unchecked Sendable {
    /// A frame was accumulated (first pass).
    /// `focusMap` is the algorithm's live focus metric texture (R32Float, updates in-place each frame).
    case accumulated(current: Int, total: Int, focusMap: MTLTexture?)
    /// Finalization pipeline complete (wavelet inverse transform, depth smoothing, etc.).
    /// `bakedFocusMap` is the rendered focus map visualization (rgba8Unorm, sRGB) — ready for
    /// direct display without further rendering. Nil for algorithms with no meaningful focus map.
    case finalized(bakedFocusMap: MTLTexture?)
    /// A frame was blended (second pass, depth/wavelet only).
    case blended(current: Int, total: Int)
    /// Pipeline complete. The associated result contains all output textures.
    case complete(StackingResult)
}

public actor StackingSession {
    public let metalContext: MetalContext
    private let algorithm: any StackingAlgorithm
    private var chainAligner: ChainAligner?

    // Accumulated state
    private(set) var width: Int = 0
    private(set) var height: Int = 0
    public private(set) var imageTransforms: [AffineTransform2D] = []

    // Baked focus map (rendered once after finalization, reused for display + persistence)
    private var bakedFocusMap: MTLTexture?

    // Source metadata (captured from first frame)
    private var sourceBitDepth: Int = 16
    private var dngRenderParams: DeptheosDng.RenderParams?
    private var dngMetadata: DeptheosDng.ImageMetadata?

    // Cancellation — nonisolated for lock-free access from any context
    private let _isCancelled = Atomic<Bool>(false)
    nonisolated var isCancelled: Bool { _isCancelled.load(ordering: .relaxed) }

    /// Cancel the stacking pipeline. Takes effect between frames.
    nonisolated public func cancel() { _isCancelled.store(true, ordering: .relaxed) }

    public init(
        metalContext: MetalContext,
        algorithm: any StackingAlgorithm,
        aligner: ECCAligner? = nil
    ) {
        self.metalContext = metalContext
        self.algorithm = algorithm
        if let aligner {
            self.chainAligner = ChainAligner(aligner: aligner)
        }
    }

    // MARK: - Phase 1: Accumulate

    /// Process a single image: align (if enabled) then accumulate.
    private func processImage(_ image: DeptheosImage) async throws {
        let texture = try image.getTexture(cache: metalContext.textureCache)

        // Align against previous frame
        let transform: AffineTransform2D?
        if var aligner = chainAligner {
            // Perform async alignment separately from sync state mutation
            // to avoid sending actor-isolated ChainAligner across isolation.
            let pairwiseResult: AlignmentResult?
            if let prevTex = aligner.previousTextureForAlignment {
                // Copy references to locals that can cross the isolation boundary.
                nonisolated(unsafe) let ref = prevTex
                nonisolated(unsafe) let src = texture
                let alignerRef = aligner.aligner
                pairwiseResult = await alignerRef.findAlignment(
                    reference: ref, source: src
                )
            } else {
                pairwiseResult = nil
            }
            let cumulative = aligner.addImage(texture, pairwiseResult: pairwiseResult)
            self.chainAligner = aligner
            transform = algorithm.frameCount == 0 ? nil : cumulative
        } else {
            transform = nil
        }

        // Track dimensions and metadata from first frame
        if algorithm.frameCount == 0 {
            width = image.width
            height = image.height
            sourceBitDepth = image.sourceBitDepth
            dngRenderParams = image.dngRenderParams
            dngMetadata = image.dngMetadata
        } else if image.width != width || image.height != height {
            throw StackingError.dimensionMismatch(
                expected: (width, height), got: (image.width, image.height)
            )
        }

        try algorithm.accumulate(image: image, transform: transform)
    }

    // MARK: - Phase 2: Finalize

    /// Drain aligner transforms after accumulation is complete.
    private func drainAligner() {
        guard let aligner = chainAligner else { return }
        imageTransforms = aligner.getAllTransforms()
    }

    /// Finalize the algorithm after all frames are accumulated.
    private func finalize() async throws {
        drainAligner()
        try await algorithm.finalize()
    }

    // MARK: - Phase 3: Blend

    /// Run the blend pass for two-pass algorithms (Depth/Wavelet).
    /// Reloads each image, applies its stored transform, and blends.
    private func blendFrame(image: DeptheosImage, frameIndex: Int) throws {
        guard let blendable = algorithm as? BlendableAlgorithm else { return }
        let transform = transformForFrame(frameIndex)
        try blendable.blendFrame(frame: image, transform: transform, frameIndex: frameIndex)
    }

    /// Prepare for the blend phase. Call once before the blendFrame loop.
    private func beginBlend() throws {
        guard let blendable = algorithm as? BlendableAlgorithm else { return }
        try blendable.beginBlend()
    }

    /// Base URL for GPU trace capture (e.g. `/tmp/deptheos.gputrace`).
    /// When set, captures two traces: `<name>-accumulate.gputrace` and `<name>-blend.gputrace`.
    private var gpuTraceURL: URL?

    /// Set the GPU trace URL from outside the actor.
    public func setGPUTraceURL(_ url: URL?) {
        gpuTraceURL = url
    }

    private func traceURL(phase: String) -> URL? {
        guard let base = gpuTraceURL else { return nil }
        let stem = base.deletingPathExtension().path
        return URL(fileURLWithPath: "\(stem)-\(phase).gputrace")
    }

    // MARK: - GPU Backpressure

    /// Wait for all previously submitted GPU work to complete.
    /// Commits an empty command buffer that serializes behind everything in the queue.
    private func drainGPU() async {
        guard let cb = metalContext.commandQueue.makeCommandBuffer() else { return }
        cb.label = "backpressure-drain"
        cb.commit()
        await cb.completed()
    }

    // MARK: - Process Stack (UI)

    /// Run the full stacking pipeline, yielding events for UI progress.
    ///
    /// Cancellation: cancel the enclosing `Task`, or call `cancel()` on the session.
    public func processStack(
        urls: [URL],
        loader: UnifiedImageLoader,
        prefetchCount: Int = 4
    ) -> AsyncThrowingStream<StackingEvent, Error> {
        let session = self
        let total = urls.count

        return AsyncThrowingStream { continuation in
            let producer = Task {
                do {
                    try await session._processStackImpl(
                        source: .urls(urls, loader, prefetchCount),
                        total: total, continuation: continuation
                    )
                } catch is CancellationError {
                    continuation.finish(throwing: CancellationError())
                } catch {
                    continuation.finish(throwing: error)
                }
            }

            continuation.onTermination = { _ in
                session.cancel()
                producer.cancel()
            }
        }
    }

    /// Run the full stacking pipeline with pre-loaded in-memory images.
    ///
    /// Behaves identically to `processStack(urls:loader:prefetchCount:)` but
    /// accepts already-loaded images, avoiding disk I/O during the pipeline.
    public func processStack(
        images: [DeptheosImage]
    ) -> AsyncThrowingStream<StackingEvent, Error> {
        let session = self
        let total = images.count

        return AsyncThrowingStream { continuation in
            let producer = Task {
                do {
                    try await session._processStackImpl(
                        source: .images(images),
                        total: total, continuation: continuation
                    )
                } catch is CancellationError {
                    continuation.finish(throwing: CancellationError())
                } catch {
                    continuation.finish(throwing: error)
                }
            }

            continuation.onTermination = { _ in
                session.cancel()
                producer.cancel()
            }
        }
    }

    /// Image source for the stacking pipeline.
    private enum ImageSource {
        case urls([URL], UnifiedImageLoader, Int)
        case images([DeptheosImage])
    }

    /// Actor-isolated implementation of the full stacking pipeline.
    /// All access to actor state is direct (no await needed).
    private func _processStackImpl(
        source: ImageSource,
        total: Int,
        continuation: AsyncThrowingStream<StackingEvent, Error>.Continuation
    ) async throws {
        func checkCancelled() throws {
            if isCancelled || Task.isCancelled {
                throw CancellationError()
            }
        }

        // Phase 1: Accumulate
        let accumTraceURL = traceURL(phase: "accumulate")
        if let url = accumTraceURL { metalContext.startGPUTrace(to: url) }
        do {
            let hasAligner = self.chainAligner != nil
            switch source {
            case .urls(let urls, let loader, let prefetchCount):
                let prefetcher = ImagePrefetcher(
                    urls: urls, loader: loader,
                    metalContext: self.metalContext, prefetchCount: prefetchCount
                )
                for try await (index, image) in prefetcher {
                    try checkCancelled()
                    try await self.processImage(image)
                    if !hasAligner { await self.drainGPU() }
                    continuation.yield(.accumulated(
                        current: index + 1, total: total,
                        focusMap: self.algorithm.focusMapTexture
                    ))
                }
            case .images(let images):
                for index in 0..<total {
                    try checkCancelled()
                    try await self.processImage(images[index])
                    if !hasAligner { await self.drainGPU() }
                    continuation.yield(.accumulated(
                        current: index + 1, total: total,
                        focusMap: self.algorithm.focusMapTexture
                    ))
                }
            }
            if let url = accumTraceURL { metalContext.stopGPUTrace(url: url) }
        } catch {
            if accumTraceURL != nil { metalContext.stopGPUTrace(url: nil) }
            throw error
        }

        // Release cached texture wrappers from accumulation before blend phase
        metalContext.flushTextureCache()

        // Phases 2–4: Finalize, Blend, Final finalize
        let blendTraceURL = traceURL(phase: "blend")
        if let url = blendTraceURL { metalContext.startGPUTrace(to: url) }
        do {
            // Phase 2: Finalize
            try checkCancelled()
            if let blendable = self.algorithm as? BlendableAlgorithm {
                self.drainAligner()
                try await blendable.runFinalizationPipeline()
            } else {
                try await self.finalize()
                await self.bakeFocusMap()
                continuation.yield(.finalized(bakedFocusMap: self.bakedFocusMap))
                continuation.yield(.complete(try await self.buildResult()))
                continuation.finish()
                if let url = blendTraceURL { metalContext.stopGPUTrace(url: url) }
                return
            }
            await self.bakeFocusMap()
            continuation.yield(.finalized(bakedFocusMap: self.bakedFocusMap))

            // Phase 3: Blend (two-pass algorithms only)
            if let blendable = self.algorithm as? BlendableAlgorithm {
                try blendable.beginBlend()

                switch source {
                case .urls(let urls, let loader, let prefetchCount):
                    let prefetcher = ImagePrefetcher(
                        urls: urls, loader: loader,
                        metalContext: self.metalContext, prefetchCount: prefetchCount
                    )
                    for try await (index, image) in prefetcher {
                        try checkCancelled()
                        let transform = self.transformForFrame(index)
                        try blendable.blendFrame(
                            frame: image, transform: transform, frameIndex: index
                        )
                        await self.drainGPU()
                        continuation.yield(.blended(current: index + 1, total: total))
                    }
                case .images(let images):
                    for index in 0..<total {
                        try checkCancelled()
                        let transform = self.transformForFrame(index)
                        try blendable.blendFrame(
                            frame: images[index], transform: transform, frameIndex: index
                        )
                        await self.drainGPU()
                        continuation.yield(.blended(current: index + 1, total: total))
                    }
                }
            }

            // Phase 4: Finalize after blend
            try checkCancelled()
            try await self.algorithm.finalize()

            continuation.yield(.complete(try await self.buildResult()))
            continuation.finish()
            if let url = blendTraceURL { metalContext.stopGPUTrace(url: url) }
        } catch {
            if blendTraceURL != nil { metalContext.stopGPUTrace(url: nil) }
            throw error
        }
    }

    /// Render the focus map visualization once for display + persistence.
    /// Called after finalization, before blending.
    private func bakeFocusMap() async {
        guard let focusMap = algorithm.focusMapTexture else { return }
        guard let renderer = try? FocusMapRenderer(metalContext: metalContext) else { return }
        if algorithm is DepthStacker {
            bakedFocusMap = await renderer.renderDepthForExport(
                depthIndex: focusMap, totalFrames: algorithm.frameCount
            )
        } else {
            bakedFocusMap = await renderer.renderPyramidForExport(focusMap: focusMap)
        }
    }

    private func buildResult() async throws -> StackingResult {
        guard let resultTexture = algorithm.resultTexture else {
            throw StackingError.invalidState("No result texture after finalization")
        }

        return StackingResult(
            resultTexture: resultTexture,
            focusMapTexture: algorithm.focusMapTexture,
            bakedFocusMapTexture: bakedFocusMap,
            validArea: chainAligner?.getValidArea(),
            metalContext: metalContext,
            sourceBitDepth: sourceBitDepth,
            dngRenderParams: dngRenderParams ?? .srgbIdentity(),
            dngMetadata: dngMetadata
        )
    }

    // MARK: - Accessors

    /// Transform for a given frame index.
    private func transformForFrame(_ i: Int) -> AffineTransform2D? {
        guard i > 0, i < imageTransforms.count else { return nil }
        return imageTransforms[i]
    }
}
