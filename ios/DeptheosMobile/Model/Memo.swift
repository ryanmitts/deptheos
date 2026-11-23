import Foundation
import Observation

/// An answer kept with the key it was worked out for, so a view asking
/// several times a redraw doesn't ask the engine again.
///
/// Invariant: read only through `answer`, which hands out the kept value
/// only while its key equals the one asked with, else remakes it. So it
/// can't disagree with its inputs, as long as `Key` holds all of them.
struct Memo<Key: Equatable, Value> {
    private var kept: (key: Key, value: Value)?

    mutating func answer(for key: Key, make: (Key) -> Value) -> Value {
        if let kept, kept.key == key { return kept.value }
        let value = make(key)
        kept = (key, value)
        return value
    }
}

private let memoLog = AppLog(category: "state")

/// An engine answer asked on the pool (ADR-143), kept with the key it was
/// asked for so a view reading it several times a redraw asks once. A read
/// with a new key asks; until the answer lands the last one stands in
/// where `standsIn` allows, so nothing flickers. An answer lands only while
/// its key is still the one asked.
///
/// Lifecycle: waiting (what stands in), asking (the key, its task, what
/// stands in), answered (the key, the value). Changed only by `answer` and
/// the landing of the task it starts.
///
///     waiting(s), answered(j, v) ──k read──▶ asking(k, standing in: s or v)
///     asking(j, t, s) ──k read──▶ asking(k, standing in: s), t canceled
///     asking(k, _, _) ──k's answer lands──▶ answered(k, v)
///     asking(k, _, s) ──no answer (canceled, failed)──▶ waiting(s), asked again at the next read
@Observable
final class EngineMemo<Key: Equatable & Sendable, Value: Sendable> {
    private enum State {
        case waiting(standing: Value?)
        case asking(Key, Task<Void, Never>, standing: Value?)
        case answered(Key, Value)

        var name: String {
            switch self {
            case .waiting: "waiting"
            case .asking: "asking"
            case .answered: "answered"
            }
        }
    }

    /// In the log.
    @ObservationIgnored private let name: String
    /// Whether the answer for the first key may stand in for the second.
    @ObservationIgnored private let standsIn: (Key, Key) -> Bool
    @ObservationIgnored private var state = State.waiting(standing: nil)
    /// Bumped as an answer lands, so the views that read it redraw.
    private var landings = 0

    init(name: String, standsIn: @escaping (Key, Key) -> Bool = { _, _ in true }) {
        self.name = name
        self.standsIn = standsIn
    }

    /// The answer for `key`: meanwhile the one standing in, `nil` before
    /// any. `ask` is the engine's call, `nil` when it has none.
    func answer(for key: Key, ask: @escaping (Key) async -> Value?) -> Value? {
        _ = landings
        let standing: Value?
        switch state {
        case .answered(let asked, let value):
            if asked == key { return value }
            standing = standsIn(asked, key) ? value : nil
        case .asking(let asked, let task, let held):
            if asked == key { return held }
            task.cancel()
            standing = held
        case .waiting(let held):
            standing = held
        }
        let task = Task { [weak self] in
            let value = await ask(key)
            self?.land(key, value)
        }
        set(.asking(key, task, standing: standing))
        return standing
    }

    private func land(_ key: Key, _ value: Value?) {
        guard case .asking(let asked, _, let standing) = state, asked == key else {
            let name = name
            memoLog.debug("\(name, privacy: .public) answer dropped: overtaken")
            return
        }
        guard let value else {
            set(.waiting(standing: standing))
            return
        }
        set(.answered(key, value))
        landings &+= 1
    }

    private func set(_ new: State) {
        let (name, old) = (name, state.name)
        state = new
        memoLog.debug("\(name, privacy: .public): \(old, privacy: .public) → \(new.name, privacy: .public)")
    }
}
