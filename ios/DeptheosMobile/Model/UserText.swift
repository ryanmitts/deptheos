import Foundation
import SwiftUI

/// Text for the user: the app's own, looked up in the String Catalog when
/// shown, or a finished sentence from the engine or the system.
nonisolated enum UserText: Equatable, Sendable {
    /// An entry of `Localizable.xcstrings`.
    case app(LocalizedStringResource)
    /// Worded by the engine or the system (`localizedDescription`): never
    /// looked up.
    case verbatim(String)

    /// Where a `String` is needed (the system's progress, a share subject).
    var string: String {
        switch self {
        case .app(let resource): String(localized: resource)
        case .verbatim(let text): text
        }
    }

    /// "#4 · Depth · 46 frames · Edited".
    static func line(_ parts: [UserText]) -> UserText {
        .verbatim(parts.map(\.string).joined(separator: separator))
    }

    static var separator: String {
        String(
            localized: " · ",
            comment: "The separator between the items of a line of compact metadata, such as 'Pyramid · 24 of 30 frames'. Spaces included.")
    }
}

extension Text {
    init(_ text: UserText) {
        switch text {
        case .app(let resource): self.init(resource)
        case .verbatim(let string): self.init(verbatim: string)
        }
    }
}

extension Button where Label == Text {
    init(_ text: UserText, action: @escaping @MainActor () -> Void) {
        self.init(action: action) { Text(text) }
    }
}

extension LocalizedStringResource {
    /// Where an API wants a resource but nothing is said; never extracted
    /// into the catalog.
    nonisolated static let empty = LocalizedStringResource(String.LocalizationValue(String()))
}
