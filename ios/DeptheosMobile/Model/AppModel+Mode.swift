import Foundation

private let stateLog = AppLog(category: "state")

/// What is being done to the render shown: nothing, Retouch or Adjust, so
/// the two can never both be open (ADR-163, ADR-171).
///
/// Lifecycle: viewing, retouching a render (Retouch's canvas, view and
/// touch) or adjusting one (Adjust's status, picture and tool). Its case
/// changed only by `setMode` (`AppModel.swift`); inside a case only `PaintModel` and
/// `AdjustModel` change anything, through `whileRetouching` and
/// `whileAdjusting`:
///
///     viewing ──Retouch──▶ retouching ──Done, or its render deleted──▶ viewing
///     viewing ──Adjust──▶ adjusting ──Done, or its render deleted──▶ viewing
///     retouching, adjusting ──another render, another stack──▶ viewing
///
/// Retouch's states inside its case are `PaintModel.Open`'s; Adjust's are
/// `AdjustModel.Open`'s.
enum RenderMode {
    case viewing
    case retouching(PaintModel.Open)
    case adjusting(AdjustModel.Open)

    var logName: String {
        switch self {
        case .viewing: "viewing"
        case .retouching(let open): "retouching \(open.renderID)"
        case .adjusting(let open): "adjusting \(open.renderID)"
        }
    }
}

extension AppModel {
    /// Nothing is being done to the render shown.
    var viewing: Bool {
        if case .viewing = mode { true } else { false }
    }

    var retouching: Bool {
        if case .retouching = mode { true } else { false }
    }

    var adjusting: Bool {
        if case .adjusting = mode { true } else { false }
    }
}
