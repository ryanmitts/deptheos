import SwiftUI
import UniformTypeIdentifiers
import os
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// The Library: every stack as a tile by its newest render's day, a batch's
/// stacks in a card. Tap opens a stack at its working version; hold for its
/// menu; drag a tile out to Files or Photos. On iPad a sidebar filters and
/// jumps to a day; on iPhone the filter is a segmented control.
struct LibraryView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.libraryLocked) private var locked
    @Environment(\.horizontalSizeClass) private var sizeClass
    @State private var filter = LibraryFilter.all
    @State private var query = ""
    @State private var jump: LibrarySpan?
    /// Held above the size-class branch, so folding the Duo (a grid made
    /// again) keeps them.
    @State private var gridState = LibraryGridState()
    @State private var page = LibraryPage.stacks {
        didSet { stateLog.debug("library page: \(page.rawValue, privacy: .public)") }
    }
    @State private var sheet: LibrarySheet? {
        didSet { stateLog.debug("library sheet: \(sheet.map { "\($0)" } ?? "closed", privacy: .public)") }
    }

    /// The sheet over the Library, one at a time.
    ///
    /// Lifecycle: closed (`nil`), batch (New Batch…) or settings. Set by the
    /// bar's buttons and cleared as the sheet closes.
    ///
    ///     closed ──New Batch…──▶ batch ──closed──▶ closed
    ///     closed ──Settings──▶ settings ──Done, or swiped away──▶ closed
    enum LibrarySheet: String, Identifiable {
        case batch
        case settings
        var id: Self { self }
    }

    /// The Library's stacks, or Recently Deleted pushed over them.
    ///
    /// Lifecycle: stacks or recentlyDeleted. Bound to the navigation, so the
    /// Recently Deleted row, the way back and iPad's sidebar change it.
    ///
    ///     stacks ──Recently Deleted (the grid's last row, iPad's sidebar)──▶ recentlyDeleted
    ///     recentlyDeleted ──back, or a filter in iPad's sidebar──▶ stacks
    enum LibraryPage: String {
        case stacks
        case recentlyDeleted
    }

    var body: some View {
        NavigationSplitView(preferredCompactColumn: .constant(.detail)) {
            LibrarySidebar(
                selection: Binding(
                    get: { page == .recentlyDeleted ? .recentlyDeleted : .filter(filter) },
                    set: { chosen in
                        switch chosen {
                        case .filter(let chosen):
                            filter = chosen
                            page = .stacks
                        case .recentlyDeleted:
                            page = .recentlyDeleted
                        case nil:
                            break
                        }
                    }),
                filter: filter, query: query, jump: $jump)
        } detail: {
            NavigationStack {
                grid
                    .navigationBarBackButtonHidden(sizeClass == .compact)
            }
        }
        .modifier(LibraryDialogs(state: gridState))
        // Automation: `-ShowSettings YES` opens Settings.
        .automation(Automation.flag("ShowSettings"), after: .seconds(1)) { sheet = .settings }
        // Automation: `-TrySample YES` (or `hornet`) picks the hornet once
        // the engine has started, `-TrySample pcb` the circuit board.
        .automation(automatedSample != nil, after: .seconds(1)) {
            guard let sample = automatedSample else { return }
            Task {
                var tries = 0
                while model.engine == nil, tries < 50 {
                    try? await Task.sleep(for: .milliseconds(200))
                    tries += 1
                }
                await model.trySample(sample)
            }
        }
        .sheet(item: $sheet) { sheet in
            Group {
                switch sheet {
                case .batch: BatchSheet()
                case .settings: SettingsSheet()
                }
            }
            .errorAlert()
        }
    }

    private var cannotBatch: Bool {
        model.engine == nil || locked
    }

    /// The one it replaces keeps its working version.
    private func newStack() {
        Task { await model.startNewStack() }
    }

    /// `sample`, bundled with the app, as a new unstacked stack.
    private func trySample(_ sample: SampleStack) {
        Task { await model.trySample(sample) }
    }

    /// By bundle folder (`pcb`), or the hornet for `YES`.
    private var automatedSample: SampleStack? {
        Automation.string("TrySample").flatMap(SampleStack.init(rawValue:))
            ?? (Automation.flag("TrySample") ? .hornet : nil)
    }

    private var grid: some View {
        LibraryGrid(
            filter: $filter, query: query, jump: $jump, state: gridState, newStack: newStack, trySample: trySample,
            openDeleted: { page = .recentlyDeleted }
        )
            .navigationDestination(
                isPresented: Binding(
                    get: { page == .recentlyDeleted }, set: { page = $0 ? .recentlyDeleted : .stacks })
            ) {
                RecentlyDeletedView()
            }
            .navigationTitle(sizeClass == .compact ? LocalizedStringResource("Library", comment: "Title of the Library, the home screen with every stack.") : filter.title)
            .modifier(CountSubtitle(text: sizeClass == .compact || model.libraryIsEmpty ? nil : model.library(filter, query: query).tiles.countLine))
            .navigationBarTitleDisplayMode(.large)
            .searchable(
                text: $query,
                prompt: LocalizedStringResource(
                    "Stack and render names", comment: "Placeholder of the Library's search field: what it searches."))
            .toolbar {
                // The app's options, at the leading end on every size.
                ToolbarItem(placement: .topBarLeading) {
                    Button(SettingsSheet.title, systemImage: "gearshape") { sheet = .settings }
                        .help(LocalizedStringResource(
                            "Export, alignment and the app's other options", comment: "Pointer tooltip of the Library's Settings button."))
                }
                if sizeClass == .compact {
                    // One + for the three ways to start: no room for names.
                    ToolbarItem(placement: .primaryAction) {
                        Menu {
                            Button(LocalizedStringResource("New Stack", comment: "Menu item: starts a new, empty stack."), systemImage: "plus.app") { newStack() }
                            Button(CameraView.takeBurst, systemImage: "camera") { model.present(.camera) }
                            Button(LocalizedStringResource("New Batch…", comment: "Library: opens the New Batch sheet, which stacks many folders."), systemImage: "plus.square.on.square") {
                                sheet = .batch
                            }
                            .disabled(cannotBatch)
                            Menu(LibrarySample.title, systemImage: LibrarySample.symbol) {
                                SampleChoices(trySample: trySample)
                            }
                            .disabled(locked)
                        } label: {
                            Label(
                                LocalizedStringResource(
                                    "New", comment: "iPhone Library's + menu: New Stack, Take Burst, New Batch… and Try a Sample Stack."),
                                systemImage: "plus")
                        }
                        .disabled(model.engine == nil)
                    }
                    .keptVisible()
                } else {
                    // New Stack stays longest; New Batch… overflows first.
                    ToolbarItem(placement: .primaryAction) {
                        Button(CameraView.takeBurst, systemImage: "camera") { model.present(.camera) }
                            .disabled(locked)
                    }
                    ToolbarItem(placement: .primaryAction) {
                        Button(
                            LocalizedStringResource("New Batch…", comment: "Library: opens the New Batch sheet, which stacks many folders."),
                            systemImage: "plus.square.on.square"
                        ) {
                            sheet = .batch
                        }
                        .labelStyle(.titleAndIcon)
                        .disabled(cannotBatch)
                    }
                    .seldomUsed()
                    ToolbarItem(placement: .primaryAction) {
                        Menu(LibrarySample.title, systemImage: LibrarySample.symbol) {
                            SampleChoices(trySample: trySample)
                        }
                        .disabled(model.engine == nil || locked)
                    }
                    .seldomUsed()
                    ToolbarItem(placement: .primaryAction) {
                        Button(
                            LocalizedStringResource("New Stack", comment: "Menu item: starts a new, empty stack."),
                            systemImage: "plus.app"
                        ) {
                            newStack()
                        }
                        .labelStyle(.titleAndIcon)
                        .buttonStyle(.glassProminent)
                        .tint(.accentFill)
                        .disabled(model.engine == nil || locked)
                    }
                    .keptVisible()
                }
            }
            // A batch's progress shows in Stacking's viewer: the bar keeps it
            // in view and goes there.
            .safeAreaBar(edge: .bottom) {
                VStack(spacing: 0) {
                    // What was just deleted, with Undo.
                    DeletedNoticeBar()
                    if let run = model.batchRun {
                        BatchProgressBar(run: run, stacking: model.stacking) {
                            model.setScreen(.stacking)
                            model.setPanel(.closed)
                        }
                    }
                }
                .animation(.snappy, value: model.deleteUndo)
            }
    }
}

private extension LibraryFilter {
    var systemImage: String {
        switch self {
        case .all: "square.grid.2x2"
        case .starred: "star"
        case .failed: "xmark.circle"
        }
    }

    var title: LocalizedStringResource {
        switch self {
        case .all:
            LocalizedStringResource("All Stacks", comment: "iPad Library: title and sidebar row of the filter showing every stack.")
        case .starred:
            LocalizedStringResource(
                "Starred", comment: "iPad Library: title and sidebar row of the filter showing stacks with a starred render.")
        case .failed:
            LocalizedStringResource(
                "Failed", comment: "iPad Library: title and sidebar row of the filter showing stacks that didn't finish.")
        }
    }
}

/// What the iPad Library's sidebar has chosen: a filter's stacks, or
/// Recently Deleted.
enum LibrarySidebarItem: Hashable {
    case filter(LibraryFilter)
    case recentlyDeleted
}

/// The iPad Library's sidebar: All, Starred and Failed with counts, Jump
/// To, then Recently Deleted. Choosing a filter from Recently Deleted goes
/// back to the stacks.
private struct LibrarySidebar: View {
    @Environment(AppModel.self) private var model
    @Binding var selection: LibrarySidebarItem?
    let filter: LibraryFilter
    let query: String
    @Binding var jump: LibrarySpan?

    var body: some View {
        let shown = model.library(filter, query: query)
        List(selection: $selection) {
            Section {
                ForEach(LibraryFilter.allCases) { item in
                    HStack {
                        Label(item.title, systemImage: item.systemImage)
                        Spacer()
                        Text(shown.count(item), format: .number).foregroundStyle(.secondary).monospacedDigit()
                    }
                    .tag(LibrarySidebarItem.filter(item))
                }
            }
            let spans = shown.spans
            if !spans.isEmpty {
                Section(LocalizedStringResource(
                    "Jump To", comment: "iPad Library sidebar: the section that scrolls to the stacks of a time span.")) {
                    ForEach(spans) { span in
                        Button {
                            selection = .filter(filter)
                            jump = span.id
                        } label: {
                            HStack {
                                // The engine's name for the span.
                                Label(span.count.label, systemImage: "calendar")
                                Spacer()
                                Text(span.count.stacks, format: .number).foregroundStyle(.secondary).monospacedDigit()
                            }
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            Section {
                HStack {
                    Label(RecentlyDeletedView.title, systemImage: RecentlyDeletedView.symbol)
                    Spacer()
                    Text(model.history.deleted.count, format: .number).foregroundStyle(.secondary).monospacedDigit()
                }
                .tag(LibrarySidebarItem.recentlyDeleted)
            }
        }
        .navigationTitle(LocalizedStringResource("Library", comment: "Title of the Library, the home screen with every stack."))
    }
}

/// What the grid keeps across being remade (the Duo switching the
/// Library's layouts): folded batches, the day scrolled to, the rename, and
/// Export… on a render (`ExportOpen`).
@Observable
private final class LibraryGridState {
    var folded: Set<String> = []
    /// The day section scrolled to the top.
    var scrolledDay: Date?
    /// By id.
    var renaming: String?
    var name = ""
    var exportOpen = LibraryGrid.ExportOpen.idle {
        didSet { stateLog.debug("library export: \(self.exportOpen.name, privacy: .public)") }
    }

    /// While the sheet shows.
    var exporting: LibraryGrid.ExportTarget? {
        get { if case .ready(let target) = exportOpen { target } else { nil } }
        set { if newValue == nil, case .ready = exportOpen { exportOpen = .idle } }
    }
}

/// The rename alert and Export sheet over the whole Library, so they stay
/// up while the grid is remade.
private struct LibraryDialogs: ViewModifier {
    @Bindable var state: LibraryGridState

    func body(content: Content) -> some View {
        content
            .renameStackAlert($state.renaming, name: $state.name)
            .sheet(item: $state.exporting) { target in
                ExportSheet(result: target.result, renderID: target.renderID)
                    .errorAlert()
            }
    }
}

private struct LibraryGrid: View {
    @Environment(AppModel.self) private var model
    @Environment(\.libraryLocked) private var locked
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Binding var filter: LibraryFilter
    let query: String
    @Binding var jump: LibrarySpan?
    @Bindable var state: LibraryGridState
    let newStack: () -> Void
    let trySample: (SampleStack) -> Void
    let openDeleted: () -> Void
    /// The fold where it crosses the grid (iOS 27.1).
    @State private var gridArea = ReservedAreas()
    /// The empty Library's Add pickers, whose frames go into a new stack.
    @State private var picker: FramePicker?

    /// Export… on a render: its result loads, then the sheet shows.
    ///
    /// Lifecycle: idle, loading (the render's id) or ready (what the sheet
    /// exports). Held in `LibraryGridState.exportOpen`; changed only by a
    /// tile's Export…, the sheet's dismissal (`exporting`) and leaving the
    /// Library.
    ///
    ///     idle ──Export…──▶ loading(id) ──loaded──▶ ready ──dismissed──▶ idle
    ///
    /// A load applies only while still asked for, so no sheet turns up late
    /// or swaps under an export.
    enum ExportOpen {
        case idle
        case loading(String)
        case ready(ExportTarget)

        func isLoading(_ id: String) -> Bool {
            if case .loading(id) = self { true } else { false }
        }

        var name: String {
            switch self {
            case .idle: "idle"
            case .loading(let id): "loading \(id)"
            case .ready(let target): "ready \(target.renderID)"
            }
        }
    }

    struct ExportTarget: Identifiable {
        let result: StackResult
        let renderID: String
        var id: String { renderID }
    }

    private var compact: Bool { sizeClass == .compact }

    /// Tiles 150 to 260 points (170 at regular width); on a screen folding
    /// top to bottom, an even number of columns so the fold never crosses a
    /// tile.
    private var columns: [GridItem] {
        let minimum: CGFloat = compact ? 150 : 170
        guard let count = evenColumns else {
            return [GridItem(.adaptive(minimum: minimum, maximum: 260), spacing: 12, alignment: .top)]
        }
        return Array(repeating: GridItem(.flexible(minimum: minimum), spacing: 12, alignment: .top), count: count)
    }

    /// Even, where the screen folds top to bottom.
    private var evenColumns: Int? {
        guard gridArea.foldsVertically else { return nil }
        let minimum: CGFloat = compact ? 150 : 170
        // The grid's width, inside its 16-point sides.
        let width = gridArea.size.width - 32
        let fit = Int((width + 12) / (minimum + 12))
        return max(fit - fit % 2, 2)
    }

    var body: some View {
        let searching = !query.trimmingCharacters(in: .whitespaces).isEmpty
        let shown = model.library(filter, query: query)
        let (days, unstacked) = (shown.days, shown.unstacked)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 20, pinnedViews: []) {
                    if compact {
                        VStack(alignment: .leading, spacing: 8) {
                            Picker(
                                LocalizedStringResource("Show", comment: "iPhone Library: the filter picker's label (hidden; read by VoiceOver)."),
                                selection: $filter
                            ) {
                                ForEach(LibraryFilter.allCases) { Text($0.name).tag($0) }
                            }
                            .pickerStyle(.segmented)
                            .labelsHidden()
                            // The iPad has this under its title.
                            if !model.libraryIsEmpty {
                                // The engine's count line for the filter.
                                Text(verbatim: shown.tiles.countLine)
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    if filter == .failed, !days.isEmpty {
                        // All the failed stacks, unless a search leaves some
                        // out.
                        let failed = shown.failed
                        Button(
                            searching
                                ? LocalizedStringResource(
                                    "Delete \(failed.count) Failed Stacks",
                                    comment: "Library menu item, in the Failed filter while searching, and on a batch card: moves the failed stacks shown to Recently Deleted. The placeholder is how many; zero reads 'Delete Failed Stacks'.")
                                : LocalizedStringResource(
                                    "Delete All Failed Stacks",
                                    comment: "Library menu item in the Failed filter: moves every failed stack to Recently Deleted."),
                            systemImage: "trash", role: .destructive
                        ) {
                            Task { await model.deleteStacks(failed) }
                        }
                        .buttonStyle(.bordered)
                        .disabled(locked)
                    }
                    if filter == .all, !unstacked.isEmpty {
                        unstackedSection(unstacked)
                    }
                    ForEach(days) { day in
                        daySection(day)
                            .id(day.id)
                    }
                    // iPad has it in its sidebar.
                    if compact, !model.libraryIsEmpty {
                        deletedRow
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
                .scrollTargetLayout()
            }
            // Kept, so a remade grid (the Duo folded) opens where this was.
            .scrollPosition(id: $state.scrolledDay, anchor: .top)
            .measure("library reserved areas", for: ReservedAreas.self) { ReservedAreas.read($0) } action: { gridArea = $0 }
            .onChange(of: jump) { _, span in
                guard let span else { return }
                jump = nil
                if let day = model.library(filter, query: query).spans.first(where: { $0.id == span })?.day {
                    withAnimation { proxy.scrollTo(day, anchor: .top) }
                }
            }
        }
        // What is picked goes into a new stack, as New Stack then Add would.
        .framePickers($picker, into: .newStack)
        // Another stack going on screen keeps the one it replaces: reread
        // them on the way back.
        .task(id: model.screen) {
            if model.screen == .library {
                await model.history.refreshUnstacked()
            } else if case .loading = state.exportOpen {
                state.exportOpen = .idle
            }
        }
        .overlay {
            if filter == .all, !unstacked.isEmpty {
                // Something to show: no empty state over it.
            } else if model.libraryIsEmpty {
                emptyLibrary
            } else if days.isEmpty {
                if searching {
                    ContentUnavailableView.search(text: query)
                } else {
                    ContentUnavailableView(
                        filter == .starred
                            ? LocalizedStringResource(
                                "No Starred Renders", comment: "Library empty-state title of the Starred filter.")
                            : LocalizedStringResource(
                                "No Failed Stacks", comment: "Library empty-state title of the Failed filter."),
                        systemImage: filter.systemImage)
                }
            }
        }
    }

    /// The empty Library: what the app is for, in the desktop welcome's
    /// words, and the ways in; picks go into a new stack. Add Folder…
    /// leads: its frames are read in place, with no copy.
    private var emptyLibrary: some View {
        ContentUnavailableView {
            Label(
                LocalizedStringResource(
                    "Add a Focus Bracket", comment: "Library empty-state title: there are no stacks yet. A focus bracket is the photos of one subject at stepped focus."),
                systemImage: "square.3.layers.3d")
        } description: {
            Text(
                "The photos of one subject, each focused a little nearer or farther than the one before. Deptheos stacks them into one picture, sharp throughout.",
                comment: "Library empty-state text under Add a Focus Bracket, over its Add Folder…, Add from Photos…, Add from Files…, Take Burst and Try a Sample Stack buttons: what a focus bracket is, and what the app makes of it.")
        } actions: {
            Button(FramePickers.addFolder, systemImage: FramePickers.folderSymbol) { picker = .folder }
                .buttonStyle(.glassProminent)
                .tint(.accentFill)
            Button(FramePickers.addFromPhotos, systemImage: FramePickers.photosSymbol) { picker = .photos }
                .buttonStyle(.glass)
            Button(FramePickers.addFromFiles, systemImage: FramePickers.filesSymbol) { picker = .images }
                .buttonStyle(.glass)
            Button(CameraView.takeBurst, systemImage: "camera") {
                model.present(.camera)
            }
            .buttonStyle(.glass)
            Menu(LibrarySample.title, systemImage: LibrarySample.symbol) {
                SampleChoices(trySample: trySample)
            }
            .menuStyle(.button)
            .buttonStyle(.glass)
            // What was deleted can still come back.
            if !model.history.deleted.isEmpty {
                Button(RecentlyDeletedView.title, systemImage: RecentlyDeletedView.symbol, action: openDeleted)
                    .buttonStyle(.borderless)
            }
        }
        .disabled(model.engine == nil || locked)
    }

    // MARK: Sections

    private func daySection(_ day: LibraryDaySection) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(day.title).font(.title2.bold())
                Text(day.countText).font(.subheadline).foregroundStyle(.secondary)
            }
            // Loose stacks side by side; a batch in a card of its own.
            ForEach(day.groups) { group in
                switch group {
                case .loose(_, let entries):
                    tiles(entries)
                case .batch(let run):
                    batchCard(run)
                }
            }
        }
    }

    private func batchCard(_ run: LibraryBatchRun) -> some View {
        let open = !state.folded.contains(run.id)
        let failed = run.entries.filter(\.failed).map(\.stack)
        return VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top) {
                Button {
                    withAnimation(.snappy) {
                        if open { state.folded.insert(run.id) } else { state.folded.remove(run.id) }
                    }
                } label: {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: "chevron.down")
                            .font(.caption.weight(.semibold))
                            .rotationEffect(.degrees(open ? 0 : -90))
                        VStack(alignment: .leading, spacing: 2) {
                            // The engine's name for the batch.
                            Text(verbatim: run.batch.shownLabel).font(.headline)
                            run.summaryText
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityValue(open
                    ? LocalizedStringResource("Expanded", comment: "VoiceOver value of a batch card's header: its stacks are shown.")
                    : LocalizedStringResource("Collapsed", comment: "VoiceOver value of a batch card's header: its stacks are hidden."))
                Spacer()
                Menu {
                    Button(
                        LocalizedStringResource(
                                "Delete \(failed.count) Failed Stacks",
                                comment: "Library menu item, in the Failed filter while searching, and on a batch card: moves the failed stacks shown to Recently Deleted. The placeholder is how many; zero reads 'Delete Failed Stacks'."),
                        systemImage: "xmark.circle", role: .destructive
                    ) {
                        Task { await model.deleteStacks(failed) }
                    }
                    .disabled(locked || failed.isEmpty)
                    Button(
                        LocalizedStringResource(
                            "Delete Batch (\(run.entries.count) Stacks)",
                            comment: "Batch card menu item: moves every stack the batch made to Recently Deleted. The placeholder is how many."),
                        systemImage: "trash", role: .destructive
                    ) {
                        let stacks = run.entries.map(\.stack)
                        Task { await model.deleteStacks(stacks) }
                    }
                    .disabled(locked)
                } label: {
                    Image(systemName: "ellipsis").frame(width: 44, height: 44)
                        .contentShape(.rect)
                        .accessibilityLabel(LocalizedStringResource(
                            "Batch options", comment: "VoiceOver label of a batch card's ••• menu."))
                }
            }
            if open {
                tiles(run.entries)
            }
        }
        .padding(12)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 16))
    }

    /// Stacks with frames and no render, above the days: tap to bring their
    /// frames and settings back.
    private func unstackedSection(_ stacks: [UnstackedInfo]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(
                    "Not Stacked Yet", comment: "Library heading over the stacks that have frames and no render yet."
                ).font(.title3.weight(.semibold))
                Text(stacksCount(stacks.count))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            LazyVGrid(columns: columns, alignment: .leading, spacing: 14) {
                ForEach(stacks, id: \.stack) { stack in
                    UnstackedTile(stack: stack)
                        .contextMenu {
                            Button(LocalizedStringResource("Open", comment: "Library tile menu item: opens the stack in Stacking."), systemImage: "arrow.up.forward.square") {
                                Task { await model.openStack(stack.stack) }
                            }
                            .disabled(locked)
                            Divider()
                            Button(Self.deleteStack, systemImage: "trash", role: .destructive) {
                                Task { await model.deleteStacks([stack.stack]) }
                            }
                            .disabled(locked)
                        }
                }
            }
        }
    }

    private func tiles(_ entries: [LibraryStack]) -> some View {
        LazyVGrid(columns: columns, alignment: .leading, spacing: 14) {
            ForEach(entries) { entry in
                switch entry.content {
                case .renders(_, let cover):
                    StackTile(tile: entry, cover: cover)
                        .overlay {
                            if state.exportOpen.isLoading(cover.id) {
                                ProgressView()
                                    .padding(10)
                                    .background(.regularMaterial, in: Circle())
                                    .accessibilityLabel(LocalizedStringResource(
                                        "Opening to export", comment: "VoiceOver label of the spinner on a tile while its render loads for Export."))
                            }
                        }
                        .contextMenu { stackMenu(entry, cover: cover) }
                case .failure(let failure):
                    FailureTile(failure: failure)
                        .contextMenu {
                            Button(
                                LocalizedStringResource(
                                    "Open to Retry", comment: "Failed tile menu item: opens the stack's frames and settings to stack them again."),
                                systemImage: "arrow.clockwise"
                            ) {
                                model.setScreen(.stacking)
                                Task { await model.retry(failure) }
                            }
                            .disabled(locked)
                            Divider()
                            Button(
                                LocalizedStringResource(
                                    "Delete Failed Stack", comment: "Failed tile menu item: moves the failed stack to Recently Deleted."),
                                systemImage: "trash", role: .destructive
                            ) {
                                Task { await model.deleteStacks([failure.stack]) }
                            }
                            .disabled(locked)
                        }
                }
            }
        }
    }

    /// Star and Export act on the tile's render (`cover`); each render's
    /// own actions are on its tile in Stacking.
    @ViewBuilder
    private func stackMenu(_ tile: LibraryStack, cover: RenderInfo) -> some View {
        // The render is named only when the stack has more than one.
        let named = tile.total != 1
        let number = cover.number
        Button(LocalizedStringResource("Open", comment: "Library tile menu item: opens the stack in Stacking."), systemImage: "arrow.up.forward.square") {
            Task { await model.openStack(tile.stack) }
        }
        .disabled(locked)
        let starTitle: LocalizedStringResource =
            switch (cover.picked, named) {
            case (true, true):
                LocalizedStringResource(
                    "Unstar #\(number)",
                    comment: "Library tile menu item: takes the star off the render the tile shows. The placeholder is its number.")
            case (true, false):
                LocalizedStringResource("Unstar", comment: "Library tile menu item: takes the star off the stack's one render.")
            case (false, true):
                LocalizedStringResource(
                    "Star #\(number)",
                    comment: "Library tile menu item: stars the render the tile shows. The placeholder is its number.")
            case (false, false):
                LocalizedStringResource("Star", comment: "Library tile menu item: stars the stack's one render.")
            }
        Button(
            starTitle,
            systemImage: cover.picked ? "star.slash" : "star"
        ) {
            Task { await model.history.togglePicked(cover) }
        }
        Button(
            named
                ? LocalizedStringResource(
                    "Export #\(number)…",
                    comment: "Library tile menu item: opens the Export sheet for the render the tile shows. The placeholder is its number.")
                : LocalizedStringResource("Export…", comment: "Library tile menu item: opens the Export sheet for the stack's one render."),
            systemImage: "square.and.arrow.up"
        ) {
            let id = cover.id
            // An export sheet on screen keeps its render.
            if case .ready = state.exportOpen { return }
            state.exportOpen = .loading(id)
            Task {
                let result = await model.ask({ try await $0.loadRenderPixels(request: $1, id: id) })
                guard state.exportOpen.isLoading(id) else { return }
                state.exportOpen = result.map { .ready(ExportTarget(result: $0, renderID: id)) } ?? .idle
            }
        }
        Button(
            LocalizedStringResource("Rename Stack…", comment: "Stack menu item: asks for a new name for the stack."),
            systemImage: "pencil"
        ) {
            state.name = cover.shownStackName
            state.renaming = tile.stack
        }
        Divider()
        // The cleanup once the result wanted is starred.
        let unstarred = model.history.unstarred(of: tile.stack)
        Button(
            unstarred.isEmpty
                ? LocalizedStringResource(
                    "Keep Only Starred", comment: "Library tile menu item, off: the stack has no starred render, or nothing else to delete.")
                : LocalizedStringResource(
                    "Delete \(unstarred.count) Unstarred Renders",
                    comment: "Library tile menu item, Keep Only Starred with how many it deletes: moves the stack's unstarred renders to Recently Deleted. The placeholder is how many."),
            systemImage: "star.circle", role: .destructive
        ) {
            Task { await model.deleteRenders(unstarred) }
        }
        .disabled(locked || unstarred.isEmpty)
        Button(Self.deleteStack, systemImage: "trash", role: .destructive) {
            Task { await model.deleteStacks([tile.stack]) }
        }
        .disabled(locked)
    }

    static let deleteStack = LocalizedStringResource(
        "Delete Stack", comment: "Library tile menu item: moves the stack with every render to Recently Deleted.")

    /// The iPhone Library's last row, with its record count; iPad has it in
    /// the sidebar.
    private var deletedRow: some View {
        Button(action: openDeleted) {
            HStack {
                Label(RecentlyDeletedView.title, systemImage: RecentlyDeletedView.symbol)
                Spacer()
                Text(model.history.deleted.count, format: .number).foregroundStyle(.secondary).monospacedDigit()
                Image(systemName: "chevron.forward")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 12))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .padding(.top, 8)
    }
}

extension LibraryBatchRun {
    /// "Batch · 3 stacks · 14:05 · 1 failed", the failures in red.
    var summaryText: Text {
        let parts: [UserText] = [
            .app(LocalizedStringResource("Batch", comment: "First item of a batch card's line in the Library, such as 'Batch · 3 stacks · 14:05'.")),
            .app(stacksCount(entries.count)),
            // Formatted in the user's locale already.
            .verbatim(started.formatted(date: .omitted, time: .shortened)),
        ]
        let line = UserText.line(parts).string
        guard failed > 0 else { return Text(verbatim: line) }
        let failedPart = Text(
            "\(failed) failed",
            comment: "Last item of a batch card's line in the Library, in red: how many of its stacks didn't finish."
        ).foregroundStyle(.red)
        return Text("\(line)\(UserText.separator)\(failedPart)")
    }
}

/// The counts under the title on iPad; iPhone has them under its filter.
private struct CountSubtitle: ViewModifier {
    let text: String?

    func body(content: Content) -> some View {
        if let text {
            content.navigationSubtitle(Text(verbatim: text))
        } else {
            content
        }
    }
}

/// Try a Sample Stack in the empty Library and the + menu (iPad: the bar's
/// overflow menu).
enum LibrarySample {
    static let title = LocalizedStringResource(
        "Try a Sample Stack",
        comment: "Button of the empty Library and item of the Library's + menu: opens a menu of the focus brackets that come with the app. The one picked is copied into Deptheos and opened as a new stack, ready to stack.")
    static let symbol = "testtube.2"
}

/// Each sample by name, with its frames and how it was shot.
struct SampleChoices: View {
    let trySample: (SampleStack) -> Void

    var body: some View {
        ForEach(SampleStack.offered) { sample in
            Button {
                trySample(sample)
            } label: {
                Text(sample.title)
                Text(sample.detail)
            }
        }
    }
}
