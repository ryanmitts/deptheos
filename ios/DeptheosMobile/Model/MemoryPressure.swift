import Dispatch
@preconcurrency import DeptheosEngine

/// iOS's memory pressure, heard as it happens (ADR-164). A dispatch source
/// on its own queue hands each warning to the engine at once: the main
/// thread's can come too late when it is busy. The app then sheds its own
/// on the main actor (`shed`).
///
/// The one global queue besides the bookmark resolver's
/// (`ios/build-xcframework.sh` allows exactly these): it only asks the
/// engine's holders to let go, waiting on nothing.
nonisolated final class MemoryPressure: Sendable {
    private let source: any DispatchSourceMemoryPressure

    /// `then` runs on the main actor after the engine has shed.
    init(then: @escaping @MainActor @Sendable (MemoryShed) -> Void) {
        let source = DispatchSource.makeMemoryPressureSource(
            eventMask: [.warning, .critical], queue: DispatchQueue.global(qos: .userInitiated))
        source.setEventHandler { [weak source] in
            guard let source else { return }
            let level: MemoryShed = source.data.contains(.critical) ? .critical : .moderate
            shedMemory(level: level)
            Task { @MainActor in then(level) }
        }
        source.activate()
        self.source = source
    }

    deinit {
        source.cancel()
    }
}
