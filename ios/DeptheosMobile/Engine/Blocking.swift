import Foundation
import os

/// The app's own blocking work (ImageIO previews, file copies, iCloud
/// downloads, listings, sweeps) on two `OperationQueue`s: never Swift's
/// cooperative pool, whose few threads must not block, nor the engine's
/// threads (ADR-143). Engine calls go through `Request.run`.
///
/// Nothing here recurses deeply or decodes with the engine, so GCD-sized
/// stacks suffice.
nonisolated enum Blocking {
    enum Lane {
        /// What someone waits for now: a preview, a read before a sheet.
        case interactive
        /// Copies, downloads, sweeps.
        case background
    }

    private static let interactive = queue("Deptheos interactive", .userInitiated, width: 3)
    private static let background = queue("Deptheos background", .utility, width: 2)

    private static func queue(_ name: String, _ quality: QualityOfService, width: Int) -> OperationQueue {
        let queue = OperationQueue()
        queue.name = name
        queue.maxConcurrentOperationCount = width
        queue.qualityOfService = quality
        return queue
    }

    /// `work` on `lane`'s queue; `urgent` goes ahead of what is queued. A
    /// task canceled before `work` starts dequeues it and throws
    /// `CancellationError`; once started, it runs to its end.
    static func run<T: Sendable>(
        _ lane: Lane = .interactive, urgent: Bool = false, _ work: @escaping @Sendable () throws -> T
    ) async throws -> T {
        let job = Job<T>()
        let operation = BlockOperation {
            guard let continuation = job.start() else { return }
            continuation.resume(with: Result { try work() })
        }
        if urgent { operation.queuePriority = .high }
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                guard job.wait(continuation) else {
                    continuation.resume(throwing: CancellationError())
                    return
                }
                (lane == .interactive ? interactive : background).addOperation(operation)
            }
        } onCancel: {
            guard let continuation = job.cancel() else { return }
            operation.cancel()
            continuation.resume(throwing: CancellationError())
        }
    }

    /// One `run`'s continuation, taken once: by the operation starting or
    /// a cancel before then.
    private final class Job<T: Sendable>: Sendable {
        private enum State {
            case new
            case waiting(CheckedContinuation<T, any Error>)
            case taken
        }

        private let state = OSAllocatedUnfairLock<State>(uncheckedState: .new)

        /// False if the task was canceled first.
        func wait(_ continuation: CheckedContinuation<T, any Error>) -> Bool {
            state.withLockUnchecked { state in
                guard case .new = state else { return false }
                state = .waiting(continuation)
                return true
            }
        }

        func start() -> CheckedContinuation<T, any Error>? { take() }

        func cancel() -> CheckedContinuation<T, any Error>? { take() }

        private func take() -> CheckedContinuation<T, any Error>? {
            state.withLockUnchecked { state in
                defer { state = .taken }
                if case .waiting(let continuation) = state { return continuation }
                return nil
            }
        }
    }
}
