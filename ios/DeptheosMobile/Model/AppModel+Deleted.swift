import Foundation
import UIKit
@preconcurrency import DeptheosEngine

// Recently Deleted (ADR-140): a delete asks nothing and moves what it takes
// there, the notice offering Undo; Restore and Empty are
// `RecentlyDeletedView`'s.

/// The notice after a delete, with its Undo, at the foot of the Library and
/// the Renders list.
///
/// Lifecycle: none, offered (the records a delete made) or restoring them.
/// Changed only by `setDeleteUndo` (ADR-140).
///
///     none ──a delete──▶ offered(o)
///     offered(o) ──a few seconds, another delete, Delete Library, Empty──▶ none, or offered(newer)
///     offered(o) ──Undo──▶ restoring(o) ──restored, or failed (said)──▶ none
///
/// A timer or restore that finds another offer than its own changes
/// nothing.
enum DeleteUndo: Equatable {
    case none
    case offered(DeletedOffer)
    case restoring(DeletedOffer)

    var name: String {
        switch self {
        case .none: "none"
        case .offered(let offer): "offered \(offer.token)"
        case .restoring(let offer): "restoring \(offer.token)"
        }
    }

    /// Offered or being restored.
    var offer: DeletedOffer? {
        switch self {
        case .none: nil
        case .offered(let offer), .restoring(let offer): offer
        }
    }
}

/// One delete's notice: the engine's sentence and its records.
struct DeletedOffer: Equatable {
    /// Counts the offers, so a timer or a restore knows its own.
    let token: Int
    /// "Bee #2 moved to Recently Deleted.", the engine's words.
    let text: String
    /// By id.
    let ids: [String]
}

extension AppModel {
    /// Long enough to read and reach Undo; longer with VoiceOver, which
    /// reads it out first.
    private var undoShown: Duration {
        UIAccessibility.isVoiceOverRunning ? .seconds(20) : .seconds(8)
    }

    /// Until it times out or another takes its place.
    func offerUndo(_ records: [DeletedInfo]) {
        guard !records.isEmpty else { return }
        let token = (deleteUndo.offer?.token ?? 0) &+ 1
        let text = AppWords.words.deletedNotice(items: records, untitled: String(localized: Self.untitledStack))
        let offer = DeletedOffer(token: token, text: text, ids: records.map(\.id))
        setDeleteUndo(.offered(offer))
        let shown = undoShown
        Task { [weak self] in
            try? await Task.sleep(for: shown)
            guard let self, case .offered(offer) = self.deleteUndo else { return }
            self.setDeleteUndo(.none)
        }
    }

    /// A tap on the notice.
    func dismissUndo() {
        if case .offered = deleteUndo { setDeleteUndo(.none) }
    }

    func undoDelete() async {
        guard case .offered(let offer) = deleteUndo else { return }
        setDeleteUndo(.restoring(offer))
        let failure = await restoreDeleted(offer.ids)
        if case .restoring(offer) = deleteUndo { setDeleteUndo(.none) }
        if let failure { errorMessage = failure }
    }

    /// Put records back (Undo, Restore) and reread the history. A Library
    /// write: never canceled. What went wrong, led by the engine's
    /// "couldn't restore", if not all came back.
    func restoreDeleted(_ ids: [String]) async -> ErrorMessage? {
        guard !ids.isEmpty, let engine else { return nil }
        // Restored from the list, the notice's Undo would find them gone.
        if case .offered(let offer) = deleteUndo, !Set(offer.ids).isDisjoint(with: ids) {
            setDeleteUndo(.none)
        }
        appLog.info("restoring \(ids.count) records of Recently Deleted")
        var failure: ErrorMessage?
        do {
            try await engine.restoreDeleted(ids: ids)
        } catch {
            var message = ErrorMessage(Self.message(error))
            message.title = .verbatim(AppWords.words.restoreFailed())
            failure = message
        }
        await history.refresh()
        return failure
    }

    /// For good (the list asks first). What went wrong, if not all went.
    func emptyDeleted() async -> ErrorMessage? {
        guard let engine else { return nil }
        appLog.info("emptying Recently Deleted: \(self.history.deleted.count) records")
        setDeleteUndo(.none)
        var failure: ErrorMessage?
        do {
            try await engine.emptyDeleted()
        } catch {
            var message = ErrorMessage(Self.message(error))
            message.title = .verbatim(AppWords.words.emptyDeletedFailed())
            failure = message
        }
        await history.refreshDeleted()
        return failure
    }
}

private let appLog = AppLog(category: "app")
