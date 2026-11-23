import SwiftUI

private let layoutLog = AppLog(category: "layout")

extension View {
    /// `onGeometryChange`, watched for a layout loop (ADR-169): `name`
    /// says which reading in the log.
    func measure<T: Equatable & Sendable>(
        _ name: String, for type: T.Type = T.self,
        of transform: @escaping @Sendable (GeometryProxy) -> T,
        action: @escaping (T) -> Void
    ) -> some View {
        onGeometryChange(for: type, of: transform) { value in
            GeometryWatch.shared.changed(name) { action(value) }
        }
    }
}

/// Counts each reading's changes. One that changes more often than any
/// resize or animation would is feeding its own layout: it is logged at
/// error level and held a moment, then its latest value applied, so the
/// loop stalls into a log line rather than holding the main thread until
/// the watchdog ends the app.
@MainActor
final class GeometryWatch {
    static let shared = GeometryWatch()

    /// A loop changes about a thousand times a second; a window dragged
    /// across a screen, about a hundred and twenty.
    private static let limit = 200
    private static let window = Duration.seconds(1)
    private static let hold = Duration.milliseconds(250)

    /// Lifecycle: counting or holding, per reading, kept in `watches` by
    /// name and changed only by `changed`.
    ///
    ///     counting ──more than `limit` changes in `window`──▶ holding
    ///     holding ──after `hold`, the latest change applied──▶ counting
    private enum Watch {
        case counting(since: ContinuousClock.Instant, changes: Int)
        case holding(latest: () -> Void)
    }

    private var watches: [String: Watch] = [:]

    func changed(_ name: String, apply: @escaping () -> Void) {
        let now = ContinuousClock.now
        switch watches[name] {
        case .holding?:
            watches[name] = .holding(latest: apply)
        case .counting(let since, let changes)? where now - since < Self.window:
            guard changes + 1 > Self.limit else {
                watches[name] = .counting(since: since, changes: changes + 1)
                apply()
                return
            }
            layoutLog.error("layout loop: \(name, privacy: .public) changed \(changes + 1) times in a second; held")
            watches[name] = .holding(latest: apply)
            Task { @MainActor in
                try? await Task.sleep(for: Self.hold)
                self.release(name)
            }
        default:
            watches[name] = .counting(since: now, changes: 1)
            apply()
        }
    }

    private func release(_ name: String) {
        guard case .holding(let latest)? = watches[name] else { return }
        layoutLog.debug("layout watch: \(name, privacy: .public) counting")
        watches[name] = .counting(since: .now, changes: 0)
        latest()
    }
}
