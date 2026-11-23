import Foundation
@preconcurrency import DeptheosEngine

/// The app's two screens: the Library, home; and Stacking, one stack over
/// it with a way back.
///
/// Lifecycle: library or stacking. Changed only by `setScreen`
/// (`Model/AppModel.swift`).
///
/// `library` → `stacking`: a tile opened, New Stack, a burst taken, a
/// failed stack tried again, a batch's progress tapped.
/// `stacking` → `library`: Library, at the leading edge of Stacking's
/// bar; a batch finishing.
enum AppScreen: Hashable {
    case library
    case stacking
}

/// What is presented over the screen, one at a time, each modal: Export,
/// Render Info (a sheet at regular width, pushed onto the renders at
/// compact width), the menu bar's Revert confirmation, or the camera.
/// Export and the camera close the compact sheet first.
///
/// Lifecycle: none, export, renderInfo (its render), revert (what it
/// reverts to) or camera. Changed only by `present` and `dismiss`.
///
///     none ──Export… (bar, ⌘E)──▶ export, the sheet closed
///     none ──Render Info (title menu, a render's menu, ⌘I)──▶ renderInfo(id)
///     none ──Stack ▸ Revert to #n, when it would lose changes──▶ revert
///     none ──Take Burst (Library, sources, ⇧⌘N)──▶ camera, the sheet closed
///     any ──another of these, from the menu bar──▶ that one
///     export ──Done, dragged down──▶ none
///     renderInfo ──Close, its way back, the compact sheet closed or turned off the sources──▶ none
///     revert ──Revert, Cancel──▶ none
///     camera ──closed, a burst taken──▶ none
enum Presented: Equatable {
    case none
    case export
    case renderInfo(String)
    case revert(RevertInfo)
    case camera

    var name: String {
        switch self {
        case .none: "none"
        case .export: "export"
        case .renderInfo: "render info"
        case .revert: "revert"
        case .camera: "camera"
        }
    }
}

/// What is over Stacking's photo: its controls, nothing (a tap hid them),
/// or one of the compact window's sheets. A sheet covers without rescaling
/// the photo; a low sheet's depth only limits how far a zoomed photo drags
/// clear. At regular width the sheets are columns (`StackingColumns`).
/// The sheets are one presentation whose content changes, so nothing is
/// ever over another sheet (ADR-163).
///
/// Lifecycle: closed, sources (`SourcesTab`) or settings, each with its
/// `Height`, or photoOnly. Changed only by `setPanel`,
/// `togglePanel`, `revealViewer`, `toggleControls` and `showControls`
/// (`Model/AppModel.swift`).
///
///     closed ──Frames, a stack never stacked opened, Show (frames left out)──▶ sources(frames)
///     closed ──Stack Settings──▶ settings
///     sources, settings ──their own button──▶ closed
///     sources(frames) ◀──the sources' switch──▶ sources(renders)
///     sources, settings ──another's button (same height)──▶ that one
///     closed, settings ──Render Info (title menu, ⌘I)──▶ sources(renders), it pushed
///     any sheet ──Close, dragged down, Stack, the camera, the Library, Retouch opened or left, the size class──▶ closed
///     low ◀──dragged · a frame or render tapped (tall → low)──▶ tall
///     closed ──a tap on the photo, View ▸ Hide Controls (not while VoiceOver runs)──▶ photoOnly
///     photoOnly ──a tap, View ▸ Show Controls, VoiceOver, another stack, a sheet opened, a failed stack, a notice──▶ closed, or a sheet
enum Panel: Equatable {
    case closed
    /// The stack's frames or renders, as the switch at their head shows.
    case sources(SourcesTab, Height)
    /// Stack Settings, with Stack in its bar.
    case settings(Height)
    /// Every control over the photo hidden: bars, bottom row and labels.
    case photoOnly

    /// By the button that opens it.
    enum Side: Equatable {
        case sources
        case settings
    }

    /// How much of the screen the sheet takes.
    enum Height: Equatable {
        /// Covers the photo's lower half; the photo above stays in use.
        case low
        case tall
    }

    var isOpen: Bool { height != nil }

    var hidesControls: Bool { self == .photoOnly }

    var height: Height? {
        switch self {
        case .closed, .photoOnly: nil
        case .sources(_, let height), .settings(let height): height
        }
    }

    var side: Side? {
        switch self {
        case .closed, .photoOnly: nil
        case .sources: .sources
        case .settings: .settings
        }
    }

    /// The sources on their frames.
    static func open(_ side: Side, at height: Height) -> Panel {
        switch side {
        case .sources: .sources(.frames, height)
        case .settings: .settings(height)
        }
    }

    func at(_ height: Height) -> Panel {
        switch self {
        case .closed, .photoOnly: self
        case .sources(let tab, _): .sources(tab, height)
        case .settings: .settings(height)
        }
    }
}

/// Frames or renders, as the switch heading the Frames column, the
/// iPhone's sheet and Retouch's source column picks.
enum SourcesTab: Equatable {
    case frames
    case renders
}
