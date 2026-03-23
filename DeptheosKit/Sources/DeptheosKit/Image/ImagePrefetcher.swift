import Foundation

/// Prefetches images N-ahead while maintaining order and backpressure.
///
/// Uses a sliding window of async tasks. New loads are only submitted
/// after the consumer advances, so at most `prefetchCount` images are
/// buffered in memory at any time.
///
/// ```swift
/// let prefetcher = ImagePrefetcher(urls: urls, loader: loader, metalContext: ctx)
/// for try await (index, image) in prefetcher {
///     try session.processImage(image)
/// }
/// ```
public struct ImagePrefetcher: AsyncSequence, Sendable {
    public typealias Element = (index: Int, image: DeptheosImage)

    public let urls: [URL]
    public let loader: UnifiedImageLoader
    public let metalContext: MetalContext
    public let prefetchCount: Int

    public init(
        urls: [URL],
        loader: UnifiedImageLoader,
        metalContext: MetalContext,
        prefetchCount: Int = 4
    ) {
        self.urls = urls
        self.loader = loader
        self.metalContext = metalContext
        self.prefetchCount = prefetchCount
    }

    public func makeAsyncIterator() -> PrefetchIterator {
        PrefetchIterator(
            urls: urls,
            loader: loader,
            metalContext: metalContext,
            windowSize: Swift.max(prefetchCount, 1)
        )
    }
}

/// Pull-based async iterator that loads images ahead with bounded memory.
///
/// `@unchecked Sendable` is safe: `next()` is only called sequentially
/// by `for try await`, and `deinit` cannot overlap with `next()`.
public final class PrefetchIterator: AsyncIteratorProtocol, @unchecked Sendable {
    public typealias Element = (index: Int, image: DeptheosImage)

    private let urls: [URL]
    private let loader: UnifiedImageLoader
    private let metalContext: MetalContext
    private let windowSize: Int

    private var nextToSubmit: Int = 0
    private var nextToReturn: Int = 0
    private var pending: [Int: Task<DeptheosImage, Error>] = [:]

    init(urls: [URL], loader: UnifiedImageLoader, metalContext: MetalContext, windowSize: Int) {
        self.urls = urls
        self.loader = loader
        self.metalContext = metalContext
        self.windowSize = windowSize
        fillWindow()
    }

    deinit {
        for (_, task) in pending { task.cancel() }
    }

    public func next() async throws -> Element? {
        try Task.checkCancellation()

        let index = nextToReturn
        guard index < urls.count else { return nil }

        guard let task = pending.removeValue(forKey: index) else { return nil }
        let image = try await withTaskCancellationHandler {
            try await task.value
        } onCancel: {
            task.cancel()
        }
        nextToReturn += 1

        // Refill window now that consumer has advanced
        fillWindow()

        return (index: index, image: image)
    }

    private func fillWindow() {
        while nextToSubmit < urls.count && (nextToSubmit - nextToReturn) < windowSize {
            let idx = nextToSubmit
            let url = urls[idx]
            let loader = self.loader
            let ctx = self.metalContext
            pending[idx] = Task {
                try Task.checkCancellation()
                // Hop to GCD to avoid blocking Swift's cooperative thread pool.
                // UnifiedImageLoader.load() can block via pthread_join (LibRaw's
                // large-stack thread), which would starve the pool at high concurrency.
                return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<DeptheosImage, Error>) in
                    withUnsafeCurrentTask { currentTask in
                        DispatchQueue.global(qos: .userInitiated).async {
                            guard currentTask?.isCancelled != true else {
                                cont.resume(throwing: CancellationError())
                                return
                            }
                            do {
                                cont.resume(returning: try loader.load(url: url, metalContext: ctx))
                            } catch {
                                cont.resume(throwing: error)
                            }
                        }
                    }
                }
            }
            nextToSubmit += 1
        }
    }
}
