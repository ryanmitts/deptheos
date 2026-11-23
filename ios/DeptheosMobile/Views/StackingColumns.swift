import SwiftUI

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Stacking's layout state at regular width: the Frames/Renders sidebar
/// (`columnVisibility` + `columnTab`) and the Stack Settings popover
/// (`showSettingsPopover`). Because Stack Settings opens as an anchored
/// popover rather than a third split column, it never collides with the
/// toolbar or resizes the photo, and needs no mutual-exclusion dance with
/// the sidebar.
struct StackingColumns: Equatable {
    /// Stack Settings popover on regular width (closed by default; opened on
    /// demand from the Stack Settings toolbar button).
    var showSettingsPopover: Bool
    var columnVisibility: NavigationSplitViewVisibility
    /// Room for the sidebar beside the photo without overlaying it.
    private(set) var wide: Bool
    /// The width was known when built; else the first width sets the
    /// columns as `init` would have.
    private(set) var sized: Bool
    /// At regular width. The iPhone's sheet keeps its own in
    /// `Panel.sources`, handed over when a sheet becomes the column.
    var columnTab = SourcesTab.frames

    /// Narrower windows than this keep the sidebar automatic/overlay unless empty.
    nonisolated static let wideWidth: CGFloat = 1100

    /// With what its rule reads at that moment.
    enum ColumnEvent {
        /// Now `wide` or not, or measured the first time.
        case width(wide: Bool, framesEmpty: Bool)
        case framesEmptied(Bool)
        case framesAsked
        /// `paint`: Retouch is open; `onStacking`: Stacking is the screen;
        /// `renderInfo`: Render Info is up.
        case sizeClass(
            UserInterfaceSizeClass?, panel: Panel, paint: Bool, onStacking: Bool, renderInfo: Bool)
        case panelOpened(Panel, compact: Bool)
    }

    /// `framesEmpty`: no frames yet (the Frames column shows in a narrow
    /// window).
    init(width: CGFloat, framesEmpty: Bool) {
        let wide = width >= Self.wideWidth
        self.wide = wide
        showSettingsPopover = Self.forcesInspector
        columnVisibility = !wide && (framesEmpty || Self.forcesSidebar) ? .all : .automatic
        sized = width > 0
    }

    /// The panel to set, if `event` calls for one.
    @discardableResult
    mutating func on(_ event: ColumnEvent) -> Panel? {
        let before = self
        let panel = apply(event)
        if self != before || panel != nil {
            stateLog.debug(
                "stacking columns: \(String(describing: event), privacy: .public) → \(String(describing: self), privacy: .public), panel \(panel.map { "\($0)" } ?? "unchanged", privacy: .public)")
        }
        return panel
    }

    private mutating func apply(_ event: ColumnEvent) -> Panel? {
        switch event {
        case .width(let wide, let framesEmpty):
            if !sized {
                sized = true
                self.wide = wide
                showSettingsPopover = Self.forcesInspector
                columnVisibility = narrowColumns(framesEmpty: framesEmpty)
                return nil
            }
            guard wide != self.wide else { return nil }
            self.wide = wide
            columnVisibility = narrowColumns(framesEmpty: framesEmpty)
        case .framesEmptied(let empty):
            columnVisibility = narrowColumns(framesEmpty: empty)
        case .framesAsked:
            columnTab = .frames
        case .sizeClass(let new, let panel, let paint, let onStacking, let renderInfo):
            if new == .compact {
                guard !panel.isOpen, onStacking else { return nil }
                if showSettingsPopover, !paint {
                    showSettingsPopover = false
                    return .settings(.low)
                } else if renderInfo, !paint {
                    // Pushed onto the renders at compact width: without them
                    // nothing would show it.
                    return .sources(.renders, .low)
                } else if sidebarOpen, !paint {
                    return .sources(columnTab, .low)
                }
                return nil
            }
            guard new == .regular else { return nil }
            switch panel {
            case .sources(let tab, _):
                columnTab = tab
                columnVisibility = wide ? .all : .doubleColumn
            case .settings:
                showSettingsPopover = true
            case .closed, .photoOnly:
                break
            }
            if panel.isOpen { return .closed }
        case .panelOpened(let panel, let compact):
            if !compact, panel.isOpen { return .closed }
        }
        return nil
    }

    /// As the reader asked, or as a wide window has it.
    private var sidebarOpen: Bool {
        switch columnVisibility {
        case .all, .doubleColumn: true
        case .automatic: wide
        default: false
        }
    }

    /// Shown while the list is empty, else as the system places it (over
    /// the photo in iPad portrait).
    private func narrowColumns(framesEmpty: Bool) -> NavigationSplitViewVisibility {
        if !wide, framesEmpty || Self.forcesSidebar { .all } else { .automatic }
    }

    /// Automation: `-ShowSidebar YES` shows the sidebar at any width.
    private static var forcesSidebar: Bool { Automation.flag("ShowSidebar") }

    /// Automation: `-ShowInspector YES` opens the Stack Settings popover.
    private static var forcesInspector: Bool { Automation.flag("ShowInspector") }
}
