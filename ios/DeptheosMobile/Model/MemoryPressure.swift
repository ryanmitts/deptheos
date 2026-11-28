import Dispatch
@preconcurrency import DeptheosEngine

/// iOS's memory pressure, heard as it happens (ADR-164). A dispatch source
/// on its own queue hands each warning to the engine at once: the main
/// thread's can come too late when it is busy. The app then sheds its own
/// on the main actor (`shed`), and is told when pressure ends (`normal`),
/// for Adjust's copies to be made again (ADR-153).
///
/// The one global queue besides the bookmark resolver's
/// (`ios/build-xcframework.sh` allows exactly these): it only asks the
/// engine's holders to let go, waiting on nothing.
nonisolated final class MemoryPressure: Sendable {
    private let source: any DispatchSourceMemoryPressure

    /// `then` runs on the main actor after the engine has shed, `normal`
    /// once the pressure is over.
    init(
        then: @escaping @MainActor @Sendable (MemoryShed) -> Void,
        normal: @escaping @MainActor @Sendable () -> Void
    ) {
        let source = DispatchSource.makeMemoryPressureSource(
            eventMask: [.normal, .warning, .critical], queue: DispatchQueue.global(qos: .userInitiated))
        source.setEventHandler { [weak source] in
            guard let source else { return }
            if source.data == .normal {
                Task { @MainActor in normal() }
                return
            }
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
