import DeptheosEngine
import Foundation
import UIKit

nonisolated private let loadLog = AppLog(category: "view")

/// A request's number, in the order asked: a held one is replaced only by
/// a newer one, and a result lands only while the state holds its ticket.
nonisolated struct Ticket: Comparable, Hashable, Sendable {
    /// For the log.
    let number: Int

    static func < (a: Ticket, b: Ticket) -> Bool { a.number < b.number }
}

/// Numbers requests, the newest current, as the web's `latest()`: after
/// each `await` a result applies only while `isCurrent`.
final class Tickets {
    private var last = 0

    func next() -> Ticket {
        last += 1
        return Ticket(number: last)
    }

    func isCurrent(_ ticket: Ticket) -> Bool { ticket.number == last }
}

protocol LoadRequest {
    var ticket: Ticket { get }
}

/// What a load makes, numbered by its request, so a held one is replaced
/// only by a newer one.
protocol LoadMade {
    var request: Ticket { get }
    /// For the log: no file names.
    var loadName: String { get }
}

extension LoadMade {
    var loadName: String { "loaded" }
}

/// Something shown that loads through a `LatestLoad`, one at a time and
/// only the newest. A load overtaken while it ran is held in place of the
/// one shown if it may stand in (`holds`) and is newer, so a drag shows
/// each frame as it lands; one that `answers` the request ends the
/// loading. A picture made again is a load too.
///
/// Lifecycle: none, loading (the request, with what it shows meanwhile),
/// loaded or failed (the request). Driven by a `LatestLoad`, landing each
/// load as `landing` says. Used for `AppModel.previewLoad`, `FrameFull`,
/// `AppModel.alignedLoad` and `MapLight.picture`.
///
///     none, loaded, failed ──r asked──▶ loading(r, showing: what was shown, if it may stand in)
///     loading(r, showing: a) ──an overtaken load lands, newer than a, that may stand in for r──▶ loading(r, showing: it)
///     loading(r, _) ──s asked──▶ loading(s, showing: the same, if it may stand in)
///     loading(r, _) ──r lands, or a load that answers r──▶ loaded
///     loading(r, _) ──r lands with nothing──▶ failed(r)
///
/// Its owner changes and logs it, and hands its `LatestLoad` the way to.
enum Latest<Asked: LoadRequest, Made: LoadMade> {
    case none
    case loading(Asked, showing: Made?)
    case loaded(Made)
    case failed(Asked)

    /// What is loaded, or while another loads, what is held.
    var shown: Made? {
        switch self {
        case .loaded(let value): value
        case .loading(_, let showing): showing
        case .none, .failed: nil
        }
    }

    var asked: Asked? {
        if case .loading(let request, _) = self { request } else { nil }
    }

    func waits(for request: Asked) -> Bool {
        asked?.ticket == request.ticket
    }

    /// Held meanwhile where `keeps` says it may stand in.
    func asking(_ request: Asked, keeping keeps: (Made) -> Bool = { _ in true }) -> Latest {
        .loading(request, showing: shown.flatMap { keeps($0) ? $0 : nil })
    }

    /// Each held value through `transform`: a picture let go of, or put
    /// back.
    func map(_ transform: (Made) -> Made) -> Latest {
        switch self {
        case .loaded(let value): .loaded(transform(value))
        case .loading(let request, let showing): .loading(request, showing: showing.map(transform))
        case .none, .failed: self
        }
    }

    /// Left before it landed: what it holds, loaded, else nothing.
    var abandoned: Latest {
        switch self {
        case .loading(_, let showing): showing.map { .loaded($0) } ?? .none
        case .none, .loaded, .failed: self
        }
    }

    /// How a load lands.
    enum Landing {
        case landed(Latest)
        /// Overtaken, held while the newer one loads.
        case held(Latest)
        /// Overtaken or no longer awaited: changes nothing.
        case dropped
    }

    /// Load `request` landing with `value` (`nil`: nothing made): lands if
    /// awaited, or if `value` `answers` the request awaited; overtaken, it
    /// is held if `holds` lets it stand in and it is newer than the one
    /// held; else dropped.
    func landing(
        _ request: Asked, with value: Made?,
        answers: (Made, _ asked: Asked) -> Bool,
        holds: (Made, _ overtaken: Asked, _ asked: Asked) -> Bool
    ) -> Landing {
        guard case .loading(let asked, let showing) = self else { return .dropped }
        if asked.ticket == request.ticket {
            return .landed(value.map { .loaded($0) } ?? .failed(asked))
        }
        guard let value else { return .dropped }
        if answers(value, asked) { return .landed(.loaded(value)) }
        guard holds(value, request, asked), showing.map({ value.request > $0.request }) ?? true
        else { return .dropped }
        return .held(.loading(asked, showing: value))
    }

    /// For the log: no file names.
    var logName: String {
        switch self {
        case .none: "none"
        case .loading(_, let showing): showing == nil ? "loading" : "loading, the one before held"
        case .loaded(let value): value.loadName
        case .failed: "failed"
        }
    }
}

/// Runs one load at a time, and of those asked meanwhile only the newest:
/// a drag asks at every tick, and only the frames it is on as each load
/// ends load, so loads land in order. A load into a `Latest` checks, as it
/// starts and lands, that the state still awaits its `Ticket`, and lands
/// as `landing` says. Every viewer load goes through one. A load canceled
/// by leaving the foreground is no failure: it is asked again once back
/// (`askAgain`).
final class LatestLoad {
    private let tickets = Tickets()
    /// In the log.
    let name: String
    private var running = false
    private var next: (@MainActor () async -> Void)?
    /// Waiting in `settled`.
    private var waiting: [CheckedContinuation<Void, Never>] = []
    /// Stopped by leaving the foreground; asked again by `askAgain`.
    private var stopped: (@MainActor () -> Void)?

    init(name: String) {
        self.name = name
    }

    func ticket() -> Ticket { tickets.next() }

    /// Run `load` once the running one has, unless a newer one is asked
    /// first.
    func run(_ load: @escaping @MainActor () async -> Void) {
        next = load
        guard !running else { return }
        running = true
        Task { @MainActor in
            while let load = next {
                next = nil
                await load()
            }
            running = false
            let waiting = waiting
            self.waiting = []
            for waiter in waiting { waiter.resume() }
        }
    }

    /// Back in front: the stopped load asks again, if still awaited.
    func askAgain() {
        let again = stopped
        stopped = nil
        again?()
    }

    /// Returns once every load asked, and any asked meanwhile, has run or
    /// been overtaken.
    func settled() async {
        guard running else { return }
        await withCheckedContinuation { waiting.append($0) }
    }

    /// Load `request` into the `Latest` that `latest` reads (`nil` once its
    /// holder is gone) and `set` changes: skipped if no longer awaited as it
    /// starts; else `make`, landing as `landing` says with `answers` and
    /// `holds`. `landed` is told once it lands as awaited (`nil` on failure).
    /// If `make` throws `Canceled` while still awaited, nothing lands:
    /// `again` (by default this load) is asked once back in front.
    func load<Asked: LoadRequest, Made: LoadMade>(
        _ request: Asked,
        latest: @escaping @MainActor () -> Latest<Asked, Made>?,
        set: @escaping @MainActor (Latest<Asked, Made>) -> Void,
        answers: @escaping @MainActor (Made, Asked) -> Bool = { _, _ in false },
        holds: @escaping @MainActor (Made, Asked, Asked) -> Bool = { _, _, _ in false },
        make: @escaping @MainActor () async throws -> Made?,
        landed: @escaping @MainActor (Made?) -> Void = { _ in },
        again: (@MainActor () -> Void)? = nil
    ) {
        let name = name
        let number = request.ticket.number
        run { [weak self] in
            // Overtaken before it started: nothing to load.
            guard latest()?.waits(for: request) == true else {
                loadLog.debug("\(name, privacy: .public) \(number) skipped: overtaken")
                return
            }
            let value: Made?
            do {
                value = try await make()
            } catch EngineError.Canceled {
                guard let self, latest()?.waits(for: request) == true else {
                    loadLog.debug("\(name, privacy: .public) \(number) stopped: no longer wanted")
                    return
                }
                loadLog.debug("\(name, privacy: .public) \(number) stopped: asked again in front")
                let ask = again ?? { [weak self] in
                    self?.load(
                        request, latest: latest, set: set, answers: answers, holds: holds, make: make,
                        landed: landed)
                }
                self.stop {
                    guard latest()?.waits(for: request) == true else { return }
                    ask()
                }
                return
            } catch {
                loadLog.error("\(name, privacy: .public) \(number) failed: \(error.localizedDescription, privacy: .public)")
                value = nil
            }
            guard let now = latest() else {
                loadLog.debug("\(name, privacy: .public) \(number) dropped: no longer shown")
                return
            }
            switch now.landing(request, with: value, answers: answers, holds: holds) {
            case .landed(let state):
                set(state)
                if !now.waits(for: request) {
                    loadLog.debug("\(name, privacy: .public) \(number) overtaken, by a load of the same")
                }
                if case .failed = state {
                    loadLog.debug("\(name, privacy: .public) \(number) couldn't be made")
                }
                landed(value)
            case .held(let held):
                set(held)
                loadLog.debug("\(name, privacy: .public) \(number) overtaken: held while the newer one loads")
            case .dropped:
                loadLog.debug("\(name, privacy: .public) \(number) dropped: overtaken")
            }
        }
    }

    /// `again` asks once back (`askAgain`), or at once if back already.
    private func stop(_ again: @escaping @MainActor () -> Void) {
        if UIApplication.shared.applicationState == .background { stopped = again } else { again() }
    }
}

extension Request {
    /// `run` for a `LatestLoad`: `nil` on failure, `Canceled` thrown on so
    /// a load stopped by leaving the foreground is asked again.
    nonisolated func load<T>(_ call: (Request) async throws -> T?) async throws -> T? {
        do {
            return try await run(call)
        } catch EngineError.Canceled {
            throw EngineError.Canceled
        } catch {
            return nil
        }
    }
}
