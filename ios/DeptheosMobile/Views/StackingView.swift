import SwiftUI
@preconcurrency import DeptheosEngine
import UniformTypeIdentifiers

private let stackingLog = AppLog(category: "stacking")

/// The Stacking screen: the sources as a column beside the viewer at
/// regular width (`StackingColumns`), Stack Settings a popover; at compact width the viewer with one
/// sheet over it following `Panel`. A size-class change builds the other
/// layout, so the zoom and Split's divider live in the model and an open
/// sheet becomes its column. ADR-163.
struct StackingView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var sizeClass
    /// Changed by their events (below).
    @State private var columns: StackingColumns
    /// By id.
    @State private var renaming: String?
    @State private var name = ""

    /// Built for a window `width` wide with its final columns, so nothing
    /// is torn down as it slides in; `framesEmpty` shows the Frames column
    /// in a narrow window.
    init(width: CGFloat, framesEmpty: Bool) {
        _columns = State(initialValue: StackingColumns(width: width, framesEmpty: framesEmpty))
    }

    private var compact: Bool { sizeClass == .compact }

    var body: some View {
        ZStack {
            NavigationSplitView(
                columnVisibility: $columns.columnVisibility,
                preferredCompactColumn: .constant(.detail)
            ) {
                NavigationStack {
                    SourcesSidebar(tab: $columns.columnTab)
                }
                .barsNeverMinimize()
                // Two fifths of an iPad held upright; the photo has the room.
                .navigationSplitViewColumnWidth(min: 280, ideal: 320, max: 400)
            } detail: {
                detail
            }
            .accessibilityHidden(!model.viewing)
            .allowsHitTesting(model.viewing)

            switch model.mode {
            case .viewing:
                EmptyView()
            case .retouching:
                RetouchSessionView()
                    .transition(.opacity)
                    .zIndex(2)
            case .adjusting:
                AdjustSessionView()
                    .transition(.opacity)
                    .zIndex(2)
            }
        }
        .animation(.snappy(duration: 0.28), value: model.viewing)
        // Stacking is dark in either mode, like the photo's backdrop; light
        // glass over it turns muddy gray. Its own views only, so the Library
        // under it stays as it is during the slide; each sheet asks for dark
        // itself.
        .environment(\.colorScheme, .dark)
        // For every column, not only the viewer's: whichever list scrolls.
        .barsNeverMinimize()
        .measure("stacking width", for: Bool.self) { $0.size.width >= StackingColumns.wideWidth } action: { wide in
            columns.on(.width(wide: wide, framesEmpty: model.rows.isEmpty))
        }
        .onChange(of: model.rows.isEmpty) {
            columns.on(.framesEmptied(model.rows.isEmpty))
        }
        .onChange(of: model.framesRequest) {
            columns.on(.framesAsked)
        }
        .onChange(of: sizeClass) { old, new in
            stackingLog.debug(
                "size class: \(String(describing: old), privacy: .public) → \(String(describing: new), privacy: .public), panel \(String(describing: model.panel), privacy: .public)")
            let event = StackingColumns.ColumnEvent.sizeClass(
                new, panel: model.panel, paint: !model.viewing,
                onStacking: model.screen == .stacking, renderInfo: model.renderInfo != nil)
            if let panel = columns.on(event) { model.setPanel(panel) }
        }
        // At regular width frames and settings are sidebar/popover: a sheet
        // asked for there is closed.
        .onChange(of: model.panel, initial: true) { _, panel in
            if let closed = columns.on(.panelOpened(panel, compact: compact)) {
                model.setPanel(closed)
            }
            // Render Info is pushed onto the sources' renders, and goes with
            // them.
            if compact, let id = model.renderInfo, panel.side != .sources { model.dismiss(.renderInfo(id)) }
        }
        // At compact width Render Info is pushed onto the sources' renders
        // (`SourcesSidebar`), not a sheet over the sheet.
        .onChange(of: model.renderInfo) { _, id in
            guard compact, id != nil, model.viewing else { return }
            if case .sources(.renders, _) = model.panel { return }
            model.setPanel(.sources(.renders, model.panel.height ?? .low))
        }
        // Frames picked in Files that ask or copy: the sources' frames, with
        // it pushed there.
        .onChange(of: model.adding.showsSheet, initial: true) { _, shows in
            guard compact, shows else { return }
            if case .sources(.frames, _) = model.panel { return }
            model.setPanel(.sources(.frames, model.panel.height ?? .tall))
        }
        // Entering Retouch or Adjust closes any open Stack Settings popover
        // or sheet.
        .onChange(of: model.viewing) { _, viewing in
            if !viewing {
                columns.showSettingsPopover = false
                if model.panel.isOpen { model.setPanel(.closed) }
            }
        }
        // Starting a stack closes the iPad Stack Settings popover to show progress.
        .onChange(of: model.stacking != nil) { _, stacking in
            if stacking { columns.showSettingsPopover = false }
        }
        // Each change is kept, and Edited follows it.
        .onChange(of: model.settings) {
            model.settingsChanged()
        }
        .stackingAutomation(model, compact: compact, columnTab: $columns.columnTab)
        // One presentation whose content is the sheet open, so the bar's
        // buttons change it in place.
        .sheet(isPresented: Binding(
            get: { compact && model.panel.isOpen },
            // At regular width the size class change closes it, having
            // opened its column.
            set: { if !$0, compact { model.setPanel(.closed) } })
        ) {
            PanelSheet()
                .errorAlert()
        }
        // Never over the compact window's sheet: Export closes it first.
        .sheet(isPresented: Binding(
            get: { model.showExport && !(compact && model.panel.isOpen) },
            set: { if !$0 { model.dismiss(.export) } })
        ) {
            if let result = model.result {
                ExportSheet(result: result.stack, renderID: result.renderID)
                    .preferredColorScheme(.dark)
                    .errorAlert()
            }
        }
        // Render Info at regular width: a sheet of its own.
        .sheet(isPresented: model.showsRenderInfo(!compact)) {
            RenderInfoSheet()
                .preferredColorScheme(.dark)
                .errorAlert()
        }
        // Stack ▸ Revert to #n, when it would lose unstacked changes.
        .revertDialog(Binding(
            get: { model.revertAsked },
            set: { if $0 == nil, let asked = model.revertAsked { model.dismiss(.revert(asked)) } }))
    }

    /// The viewer: the whole screen on iPhone, the detail column on iPad.
    private var detail: some View {
        ViewerView(
            phone: compact, sidebarShown: !compact && columns.columnVisibility != .detailOnly)
        .background(Color.viewer.ignoresSafeArea())
        // The stack's name with its menu, over the render shown and its
        // state.
        .navigationTitle(title)
        .navigationSubtitle(subtitle)
        .navigationBarBackButtonHidden(compact)
        // A tap on the photo hides the controls over it, the bars too.
        .toolbarVisibility(model.panel.hidesControls ? .hidden : .automatic, for: .navigationBar)
        .navigationBarTitleDisplayMode(.inline)
        .barsNeverMinimize()
        .modifier(TitleMenu(shown: true) {
            name = model.stackName ?? ""
            renaming = model.stackID
        })
        .renameStackAlert($renaming, name: $name)
        .toolbar { resultToolbar }
        .modifier(StackInVerticalBar(shown: model.viewing))
        // Light status bar and titles over the dark backdrop.
        .toolbarColorScheme(.dark, for: .navigationBar, .bottomBar)
        .modifier(VerticalBarEdgeReader())
    }

    /// The panel, its button and its sheet's title.
    static let stackSettings = LocalizedStringResource(
        "Stack Settings",
        comment: "Button that shows or hides the stack's settings (an anchored popover at regular width, a sheet at compact width), and its title.")

    /// Retouch's brush panel title.
    static let brush = LocalizedStringResource(
        "Brush", comment: "Retouch's brush settings title.")

    /// The stack's name.
    private var title: Text {
        Text(verbatim: model.stackName ?? "")
    }

    /// The render shown and its state ("#4 · Edited", "#4 · Depth · 44 of 46
    /// frames · Edited"), "Not stacked yet", or the compare.
    private var subtitle: Text {
        Text(model.viewerSubtitle(full: !compact))
    }

    /// Selected while its sheet is open, which it then closes
    /// (`togglePanel`).
    private func sideButton(_ side: Panel.Side, @ViewBuilder label: () -> some View) -> some View {
        let open = model.panel.side == side
        return Toggle(isOn: Binding(get: { open }, set: { _ in model.togglePanel(side) }), label: label)
            .toggleStyle(.button)
            .accessibilityAddTraits(open ? .isSelected : [])
    }

    /// Each item has a symbol and title so a side bar (iPhone Duo) can take
    /// it. Back, Frames and Stack Settings stay longest; Retouch and Export
    /// overflow first. Stack / Retouch live in the viewer deck.
    @ToolbarContentBuilder
    private var resultToolbar: some ToolbarContent {
        if compact {
            ToolbarItem(placement: .cancellationAction) {
                LibraryBackButton()
            }
            .keptVisible()
            ToolbarItem(placement: .primaryAction) {
                sideButton(.sources) {
                    Label(
                        LocalizedStringResource(
                            "Frames", comment: "iPhone toolbar button: opens the sheet on the stack's frames and renders, and closes it again."),
                        systemImage: "photo.on.rectangle.angled")
                }
                .help(LocalizedStringResource(
                    "Show or hide the frames and renders",
                    comment: "Pointer tooltip of the iPhone's Frames button: opens or closes the sheet on the frames and renders."))
            }
            .keptVisible()
            ToolbarItem(placement: .primaryAction) {
                sideButton(.settings) {
                    Label {
                        Text(Self.stackSettings)
                    } icon: {
                        Image("StackSettings")
                    }
                }
                .help(LocalizedStringResource(
                    "Show or hide the method, its sliders and alignment",
                    comment: "Pointer tooltip of the iPhone's Stack Settings button: opens or closes the sheet on Stack Settings."))
            }
            .keptVisible()
        } else {
            ToolbarItem(placement: .cancellationAction) {
                LibraryBackButton()
            }
            .keptVisible()
            ToolbarItem(placement: .primaryAction) {
                Button {
                    columns.showSettingsPopover.toggle()
                } label: {
                    Label {
                        Text(Self.stackSettings)
                    } icon: {
                        Image("StackSettings")
                    }
                }
                .help(LocalizedStringResource(
                    "Show or hide the method, its sliders and alignment",
                    comment: "Pointer tooltip of the Stack Settings button: opens the Stack Settings popover."))
                .popover(isPresented: $columns.showSettingsPopover, arrowEdge: .top) {
                    StackSettingsPopover {
                        columns.showSettingsPopover = false
                    }
                    .errorAlert()
                }
            }
            .keptVisible()
        }
        secondaryItems
    }

    /// The first to go to the overflow menu.
    @ToolbarContentBuilder
    private var secondaryItems: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            RetouchButton()
        }
        .seldomUsed()
        ToolbarItem(placement: .primaryAction) {
            AdjustButton()
        }
        .seldomUsed()
        ToolbarItem(placement: .primaryAction) {
            exportButton
        }
        .seldomUsed()
    }

    private var exportButton: some View {
        Button(
            LocalizedStringResource("Export…", comment: "Toolbar button: opens the Export sheet for the result shown."),
            systemImage: "square.and.arrow.up"
        ) {
            // One sheet at a time: the compact window's goes first.
            model.present(.export)
        }
            .disabled(!model.canExport)
            .accessibilityIdentifier("export")
    }
}

/// The title's menu at every width: Rename Stack…, Render Info, then every
/// render newest first, the one shown checked. Picking another shows it;
/// the one shown leaves a frame or the depth map.
struct StackTitleMenu: View {
    @Environment(AppModel.self) private var model
    let rename: () -> Void

    var body: some View {
        Section {
            Button(
                LocalizedStringResource("Rename Stack…", comment: "Stack menu item: asks for a new name for the stack."),
                systemImage: "pencil", action: rename)
                .disabled(!model.canRenameStack)
            Button(RenderInfoView.title, systemImage: "info.circle") {
                if let id = model.result?.renderID { model.present(.renderInfo(id)) }
            }
            .disabled(model.shownRender == nil)
        }
        Section {
            ForEach(model.stackRenders, id: \.id) { render in
                Toggle(
                    isOn: Binding(
                        get: { model.showsRenderShown && model.result?.renderID == render.id },
                        set: { _ in Task { await model.open(render) } })
                ) {
                    Label { Text(render.menuTitle) } icon: { Image(systemName: RenderInfo.symbol) }
                }
                .accessibilityLabel(Text(verbatim: render.spokenLabel))
            }
        }
        // Another render picked while one loads is shown instead.
        .disabled(model.held)
    }
}

extension View {
    /// While `stack` holds the id to rename, the name typed in `name`.
    func renameStackAlert(_ stack: Binding<String?>, name: Binding<String>) -> some View {
        modifier(RenameStackAlert(stack: stack, name: name))
    }
}

private struct RenameStackAlert: ViewModifier {
    @Environment(AppModel.self) private var model
    @Binding var stack: String?
    @Binding var name: String

    func body(content: Content) -> some View {
        content.alert(
            Text("Rename Stack", comment: "Title of the alert that renames a stack."),
            isPresented: Binding(get: { stack != nil }, set: { if !$0 { stack = nil } }),
            presenting: stack
        ) { stack in
            TextField(LocalizedStringResource("Name", comment: "Rename Stack alert: placeholder of the name field."), text: $name)
            Button(LocalizedStringResource("Rename", comment: "Rename Stack alert: gives the stack the name typed.")) {
                Task { await model.renameStack(stack, to: name) }
            }
            Button(LocalizedStringResource("Cancel", comment: "Rename Stack alert: closes it without renaming."), role: .cancel) {}
        } message: { _ in
            Text(
                "Every render of this stack takes the new name. Leave it empty to use the folder's name.",
                comment: "Rename Stack alert's text.")
        }
    }
}

/// Stack the working version's included frames, whatever render is on
/// screen. Off only when the app can't stack, with why for VoiceOver and
/// the pointer.
struct StackButton: View {
    @Environment(AppModel.self) private var model
    /// In the system's bar, with a symbol, for its side bar.
    var inBar = false

    var body: some View {
        // The word shows: it is the main action. The symbol is for the
        // overflow menu.
        Button {
            // The live map and the progress are in the viewer.
            model.setPanel(.closed)
            Task { await model.stack() }
        } label: {
            if inBar {
                Label(Self.stack, systemImage: ViewerDeck.stackSymbol)
            } else {
                Text(Self.stack)
            }
        }
        .lineLimit(1)
        .buttonStyle(.glassProminent)
        .tint(.accentFill)
        .disabled(!model.canStack)
        .accessibilityIdentifier("stack")
        .accessibilityHint(model.canStack ? .empty : hint)
        .help(model.canStack
              ? LocalizedStringResource(
                "Make a render from the working version", comment: "Pointer tooltip of the Stack button.")
              : hint)
    }

    private static let stack = LocalizedStringResource(
        "Stack", comment: "The Stack button: makes a render from the working version.")

    private var hint: LocalizedStringResource {
        switch model.stackHint {
        case .app(let resource)?: resource
        case .verbatim(let text)?: LocalizedStringResource(String.LocalizationValue(text))
        case nil: .empty
        }
    }
}

extension EnvironmentValues {
    /// The sources are the compact window's sheet, not the iPad's column.
    @Entry var phoneLayout = false

    /// The system's side bar edge (iPhone Duo, iOS 27.1), copied by
    /// `VerticalBarEdgeReader`; `nil` where bars are never vertical and
    /// before iOS 27.1.
    @Entry var verticalBarEdge: HorizontalEdge? = nil
}

/// Where the system's bars are a vertical strip (the Duo's outer display),
/// Stack is in it and the deck keeps to one row. Read where the deck reads
/// it, so from the first frame Stack is in exactly one of them.
private struct StackInVerticalBar: ViewModifier {
    @Environment(\.verticalBarEdge) private var edge
    /// Not in Retouch.
    let shown: Bool

    func body(content: Content) -> some View {
        content.toolbar {
            if edge != nil, shown {
                ToolbarItem(placement: .confirmationAction) {
                    StackButton(inBar: true)
                }
            }
        }
    }
}

/// Copies `toolbarVerticalEdge` (iOS 27.1) into `verticalBarEdge`, which
/// iOS 26 reads as `nil`.
struct VerticalBarEdgeReader: ViewModifier {
    func body(content: Content) -> some View {
        if #available(iOS 27.1, *) {
            content.modifier(Reader())
        } else {
            content
        }
    }

    @available(iOS 27.1, *)
    private struct Reader: ViewModifier {
        @Environment(\.toolbarVerticalEdge) private var edge

        func body(content: Content) -> some View {
            content.environment(\.verticalBarEdge, edge)
        }
    }
}

extension View {
    /// The navigation bar is not minimized as a list scrolls (iOS 27).
    @ViewBuilder
    func barsNeverMinimize() -> some View {
        if #available(iOS 27, *) {
            toolbarMinimizationBehavior(.never, for: .navigationBar)
        } else {
            self
        }
    }
}

extension View {
    /// A sheet whose bar has one item (or Stack Settings' Close and Stack)
    /// keeps it horizontal where the system would make it a side bar (the
    /// Duo's outer display, iOS 27.1), so its controls get the whole width.
    /// Not the sources' sheet, whose bar carries the list's commands.
    @ViewBuilder
    func singleItemSheetBar() -> some View {
        if #available(iOS 27.1, *) {
            toolbarVerticalBehavior(.disabled)
        } else {
            self
        }
    }
}

extension ToolbarContent {
    /// Last to go to the overflow menu when the bar runs short (iOS 27).
    @ToolbarContentBuilder
    func keptVisible() -> some ToolbarContent {
        if #available(iOS 27, *) {
            visibilityPriority(.high)
        } else {
            self
        }
    }

    /// First to go to the overflow menu when the bar runs short (iOS 27).
    @ToolbarContentBuilder
    func seldomUsed() -> some ToolbarContent {
        if #available(iOS 27, *) {
            visibilityPriority(.low)
        } else {
            self
        }
    }
}

/// The compact window's sheet (`Panel`): Frames' (`SourcesSheet`), Stack
/// Settings' (`StackSettingsSheet`) or the Brush (`BrushSheet`). One
/// presentation whose content changes, so another button swaps it in
/// place. Low, the photo above stays in use.
private struct PanelSheet: View {
    @Environment(AppModel.self) private var model
    /// For while it slides away.
    @State private var shown = Panel.Side.sources

    var body: some View {
        Group {
            switch model.panel.side ?? shown {
            case .sources: SourcesSheet()
            case .settings: StackSettingsSheet()
            }
        }
        .barsNeverMinimize()
        .onChange(of: model.panel.side, initial: true) { _, side in
            if let side { shown = side }
        }
        .presentationDetents([.medium, .large], selection: detent)
        .presentationDragIndicator(.visible)
        .presentationBackgroundInteraction(.enabled(upThrough: .medium))
        // Dark as Stacking is: a sheet is a presentation of its own.
        .preferredColorScheme(.dark)
        // Frames picked in Files wait on the sheet's answer or copy.
        .interactiveDismissDisabled(model.adding.showsSheet)
        // Solid: a low sheet's glass over the dark backdrop turns its lists
        // muddy gray.
        .presentationBackground(Color(.systemGroupedBackground))
    }

    private var detent: Binding<PresentationDetent> {
        Binding(
            get: { model.panel.height == .tall ? .large : .medium },
            set: { model.setPanel(model.panel.at($0 == .large ? .tall : .low)) })
    }
}

/// The stack's sources (`SourcesSidebar`), with Close, Add and the list's
/// commands; Render Info and the Add Frames questions push onto it.
private struct SourcesSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationStack {
            SourcesSidebar(tab: tab)
        }
        .environment(\.phoneLayout, true)
    }

    private var tab: Binding<SourcesTab> {
        Binding(
            get: { if case .sources(let tab, _) = model.panel { tab } else { .frames } },
            set: { model.setPanel(.sources($0, model.panel.height ?? .low)) })
    }
}

/// Regular-width Stack Settings: an anchored Liquid Glass popover with its own
/// `NavigationStack`, title/subtitle and `Stack` button, so it never collides
/// with the viewer's toolbar or resizes the photo.
private struct StackSettingsPopover: View {
    @Environment(AppModel.self) private var model
    let dismiss: () -> Void

    var body: some View {
        NavigationStack {
            InspectorView()
                .navigationTitle(Text(StackingView.stackSettings))
                .navigationSubtitle(Text(model.stackedFrames))
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    if model.stacking == nil, model.batchRun == nil {
                        ToolbarItem(placement: .confirmationAction) {
                            StackButton()
                        }
                    }
                }
        }
        .frame(idealWidth: 380, idealHeight: 540)
        .preferredColorScheme(.dark)
        .onChange(of: model.stacking != nil) { _, running in
            if running { dismiss() }
        }
    }
}

/// The working version's state, then the method and the rest
/// (`InspectorView`); Close, and Stack opposite, which closes the sheet to
/// show the run. The subtitle is what Stack stacks ("62 of 66 frames").
/// While running, Stack gives way to progress and Cancel at the form's
/// head. Pushed Alignment and Advanced have Back in Close's place and no
/// Stack.
private struct StackSettingsSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationStack {
            InspectorView()
                .navigationTitle(Text(StackingView.stackSettings))
                .navigationSubtitle(Text(model.stackedFrames))
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(role: .close) { model.setPanel(.closed) }
                    }
                    if model.stacking == nil, model.batchRun == nil {
                        ToolbarItem(placement: .confirmationAction) {
                            StackButton()
                        }
                    }
                }
        }
        .singleItemSheetBar()
    }
}

/// The viewer's title as the stack's menu, at every width.
private struct TitleMenu: ViewModifier {
    let shown: Bool
    let rename: () -> Void

    @ViewBuilder
    func body(content: Content) -> some View {
        // Applied while the result's viewer is there, so it keeps its
        // identity and zoom. Retouch, with its own canvas, has no menu.
        if shown {
            content.toolbarTitleMenu {
                StackTitleMenu(rename: rename)
            }
        } else {
            content
        }
    }
}
