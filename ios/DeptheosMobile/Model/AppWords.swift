import DeptheosEngine
import Foundation

/// The engine's language (ADR-161): the one iOS chose for this app, not
/// the system's first, so the engine's sentences match the String Catalog.
nonisolated enum AppWords {
    /// The app's language as a tag ("en"), given to `Engine`.
    static let language = Bundle.main.preferredLocalizations.first ?? "en"

    /// For the engine's functions that word without an `Engine`.
    static let words = Words(language: language)
}
