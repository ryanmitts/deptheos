import PhotosUI
import SwiftUI
@preconcurrency import DeptheosEngine
import UniformTypeIdentifiers

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// The stack's sources under a "Frames | Renders" switch: Stacking's left
/// column, the iPhone's Frames sheet, and Retouch's source column. In
/// Stacking the frames are the working version's list (`ListScreen`) and a
/// render tapped is shown; in Retouch both pick what the brush paints
/// from. ADR-163.
struct SourcesSidebar: View {
    @Environment(AppModel.self) private var model
    @Environment(\.phoneLayout) private var phoneLayout
    /// Kept by whoever shows the sources: `Panel.sources`, Stacking's
    /// column, or paint mode's open state.
    @Binding var tab: SourcesTab

    var body: some View {
        Group {
            switch tab {
            case .frames: ListScreen()
            case .renders: RendersList()
            }
        }
        // Pinned over the list, so 46 frames don't scroll it away.
        .safeAreaBar(edge: .top) {
            head
                .padding(.horizontal, 20)
                .padding(.bottom, 6)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .modifier(StackingCommands(active: !model.paint.active, tab: tab, showFrames: { tab = .frames }))
        // In the compact sheet, what would be a sheet over it is pushed:
        // Render Info on the renders, Files' questions on the frames.
        .navigationDestination(isPresented: pushed(tab == .renders && model.renderInfoRender != nil) {
            if let id = model.renderInfo { model.dismiss(.renderInfo(id)) }
        }) {
            if let render = model.renderInfoRender {
                RenderInfoView(render: render)
            }
        }
        .navigationDestination(isPresented: pushed(tab == .frames && model.adding.showsSheet) {}) {
            AddFrames()
        }
    }

    /// Pushed in the sheet while `shown`; `back` when the way back is taken.
    /// Never at regular width, where these are sheets.
    private func pushed(_ shown: Bool, back: @escaping () -> Void) -> Binding<Bool> {
        Binding(get: { phoneLayout && shown }, set: { if !$0 { back() } })
    }

    /// Retouch's title, the switch, and in Stacking how many frames are
    /// included and where they are.
    private var head: some View {
        VStack(alignment: .leading, spacing: 8) {
            if model.paint.active, !phoneLayout {
                Text(Self.paintFrom)
                    .font(.title2.bold())
                    .padding(.top, 4)
                    .accessibilityAddTraits(.isHeader)
            }
            Picker(Self.switchLabel, selection: tabBinding) {
                Text(ListScreen.frames).tag(SourcesTab.frames)
                Text(RendersList.title).tag(SourcesTab.renders)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            if tab == .frames, !model.paint.active, !model.rows.isEmpty {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(LocalizedStringResource(
                        "\(model.includedRows.count) of \(model.rows.count) included",
                        comment: "Under the switch at the head of the Frames list: how many frames are included, of how many."))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Spacer(minLength: 8)
                    StorageBadge()
                    // In the sheet, where the bar has no room for it.
                    if phoneLayout {
                        Button(model.frameSelecting.isOn
                               ? LocalizedStringResource("Done", comment: "iPhone Frames list head: stops selecting frames.")
                               : LocalizedStringResource("Select", comment: "iPhone Frames list head: starts selecting frames.")) {
                            withAnimation { model.setFrameSelecting(model.frameSelecting.isOn ? .off : .on([])) }
                        }
                        .font(.subheadline)
                        .disabled(model.held)
                    }
                }
            }
        }
    }

    /// Switching to the renders ends a selection of frames.
    private var tabBinding: Binding<SourcesTab> {
        Binding(
            get: { tab },
            set: { new in
                if new != .frames { model.setFrameSelecting(.off) }
                tab = new
            })
    }

    static let paintFrom = LocalizedStringResource(
        "Paint From", comment: "Title of the iPad column in paint mode: the frames and renders the brush can paint from.")

    static let switchLabel = LocalizedStringResource(
        "Show", comment: "VoiceOver label of the Frames | Renders switch at the head of the stack's sources.")
}

/// The bar's + menu's pickers, for the Frames list's empty state too.
struct FramePickers {
    let photos: () -> Void
    let files: () -> Void
    let folder: () -> Void

    static let addFromPhotos = LocalizedStringResource(
        "Add from Photos…",
        comment: "Add menu item, and a button of the empty Frames list and of the empty Library (there into a new stack): picks photos from the Photos library.")
    static let addFromFiles = LocalizedStringResource(
        "Add from Files…",
        comment: "Add menu item, and a button of the empty Frames list and of the empty Library (there into a new stack): picks single frames in Files, which are copied into Deptheos.")
    static let addFolder = LocalizedStringResource(
        "Add Folder…",
        comment: "Add menu item, and a button of the empty Frames list and of the empty Library (there into a new stack): picks one folder in Files, whose frames are used where they are.")
    static let photosSymbol = "photo.on.rectangle"
    static let filesSymbol = "doc.on.doc"
    static let folderSymbol = "folder"
}

/// The frame picker that is up, for the stack on screen or (from the empty
/// Library) a new one. Files has a picker each for images and a folder: one
/// taking both only opens a tapped folder and never offers to pick it.
///
/// Lifecycle: none, photos, images or folder. An `Optional` in each view
/// offering them, set by its Add buttons and cleared as the picker closes;
/// shown by `framePickers`.
///
///     none ──Add from Photos…──▶ photos ──picked or dismissed──▶ none
///     none ──Add from Files…───▶ images ──picked or dismissed──▶ none
///     none ──Add Folder…───────▶ folder ──picked or dismissed──▶ none
enum FramePicker: Equatable {
    case photos
    /// Any number, each copied in unless a folder Deptheos reads holds it.
    case images
    /// One folder, used in place.
    case folder

    var name: String {
        switch self {
        case .photos: "photos"
        case .images: "images"
        case .folder: "folder"
        }
    }
}

/// Where what a frame picker picks goes.
enum PickedInto {
    /// The stack on screen.
    case stackOnScreen
    /// As New Stack then the picker (the empty Library).
    case newStack
}

extension View {
    /// Shown while `picker` names one, its picks added `into` a stack.
    func framePickers(_ picker: Binding<FramePicker?>, into: PickedInto) -> some View {
        modifier(FramePickerSheets(picker: picker, into: into))
    }
}

/// Photos' picker and Files' for `framePickers`.
private struct FramePickerSheets: ViewModifier {
    @Environment(AppModel.self) private var model
    @Binding var picker: FramePicker?
    let into: PickedInto
    @State private var photos: [PhotosPickerItem] = []

    func body(content: Content) -> some View {
        content
            .onChange(of: picker) { _, now in
                stateLog.debug("frame picker: \(now?.name ?? "none", privacy: .public)")
            }
            .photosPicker(
                isPresented: shown { $0 == .photos }, selection: $photos, matching: .images,
                preferredItemEncoding: .current)
            .onChange(of: photos) { _, picked in
                guard !picked.isEmpty else { return }
                photos = []
                add { await model.addPhotos(picked) }
            }
            .fileImporter(
                isPresented: shown { $0 != .photos },
                allowedContentTypes: picker == .folder ? [.folder] : [.image],
                allowsMultipleSelection: picker != .folder
            ) { picked in
                switch picked {
                case .success(let urls):
                    add { await model.addPicked(urls.map(Picked.init(fromFiles:))) }
                case .failure(let error): model.report(error)
                }
            }
    }

    /// Dismissed, none is.
    private func shown(_ up: @escaping (FramePicker) -> Bool) -> Binding<Bool> {
        Binding(
            get: { picker.map(up) ?? false },
            set: { if !$0, picker.map(up) == true { picker = nil } })
    }

    private func add(_ adding: @escaping @MainActor () async -> Void) {
        switch into {
        case .stackOnScreen: Task { await adding() }
        case .newStack: Task { await model.addToNewStack { await adding() } }
        }
    }
}

extension EnvironmentValues {
    /// While the bar's + menu has them (Stacking, not Retouch).
    @Entry var framePickers: FramePickers? = nil
}

/// Stacking's frame commands in the column's or sheet's bar: Take Burst
/// (iPad), Add, and the rest in the overflow menu, with their pickers and
/// dialogs. `showFrames` turns the sources to their frames for a list
/// command.
private struct StackingCommands: ViewModifier {
    @Environment(AppModel.self) private var model
    @Environment(\.phoneLayout) private var phoneLayout
    let active: Bool
    let tab: SourcesTab
    let showFrames: () -> Void
    @State private var picker: FramePicker?
    @State private var confirmClear = false

    func body(content: Content) -> some View {
        if active {
            commands(
                content
                    // The empty list's buttons open the same pickers.
                    .environment(\.framePickers, FramePickers(
                        photos: { picker = .photos }, files: { picker = .images },
                        folder: { picker = .folder })))
        } else {
            content
        }
    }

    private func commands(_ content: some View) -> some View {
        content
        // The switch says what shows, so the bar keeps room; the title is
        // for the way back and VoiceOver.
        .navigationTitle(Text(tab == .frames ? ListScreen.frames : RendersList.title))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(removing: .title)
        .toolbar {
            // The way back to the Library leads the viewer's bar; the
            // iPhone's sheet closes.
            if phoneLayout {
                ToolbarItem(placement: .cancellationAction) {
                    Button(role: .close) { model.setPanel(.closed) }
                }
            }
            ToolbarItemGroup(placement: .primaryAction) {
                if !phoneLayout {
                    Button(CameraView.takeBurst, systemImage: "camera") { model.present(.camera) }
                        .disabled(fixed)
                }
                addMenu
            }
            // The bar's own overflow menu; before iOS 27, a menu of the
            // list's commands.
            if #available(iOS 27.0, *) {
                ToolbarOverflowMenu { moreItems }
            } else {
                ToolbarItem(placement: .primaryAction) {
                    Menu { moreItems } label: {
                        Label(FrameActions.more, systemImage: "checklist")
                    }
                }
            }
        }
        // On iPhone the viewer, with a batch's progress, is under the sheet:
        // the bar keeps it in view and closes the sheet.
        .safeAreaBar(edge: .bottom) {
            if phoneLayout, let run = model.batchRun {
                BatchProgressBar(run: run, stacking: model.stacking) {
                    model.setPanel(.closed)
                }
            }
        }
        .framePickers($picker, into: .stackOnScreen)
        // At regular width; the compact window's sheet pushes it.
        .sheet(isPresented: .constant(model.adding.showsSheet && !phoneLayout)) {
            // Dark as Stacking is: a sheet is a presentation of its own.
            AddSheet()
                .preferredColorScheme(.dark)
                .errorAlert()
        }
        .confirmationDialog(
            Text("Remove all frames?", comment: "Title of the dialog after Remove All Frames, for a stack never stacked."),
            isPresented: $confirmClear, titleVisibility: .visible
        ) {
            Button(LocalizedStringResource(
                "Remove All", comment: "Destructive button of the Remove all frames? dialog."), role: .destructive) { model.clearList() }
        } message: {
            Text(
                "These frames were never stacked, so the list can't be brought back. The files stay where they are.",
                comment: "Text of the Remove all frames? dialog.")
        }
    }

    /// Nothing in the list changes while a stack runs or another opens.
    private var fixed: Bool { model.held }

    private var addMenu: some View {
        Menu {
            if phoneLayout {
                Button(CameraView.takeBurst, systemImage: "camera") { model.present(.camera) }
            }
            Button(FramePickers.addFromPhotos, systemImage: FramePickers.photosSymbol) {
                showFrames()
                picker = .photos
            }
                .disabled(model.adding.isActive)
            // Frames still being read into the list come first.
            Button(FramePickers.addFromFiles, systemImage: FramePickers.filesSymbol) {
                showFrames()
                picker = .images
            }
                .disabled(model.adding.isActive)
            Button(FramePickers.addFolder, systemImage: FramePickers.folderSymbol) {
                showFrames()
                picker = .folder
            }
                .disabled(model.adding.isActive)
        } label: {
            Label(
                LocalizedStringResource("Add", comment: "The Frames list's + menu: adds frames from Photos, Files or the camera."),
                systemImage: "plus")
        }
        .disabled(fixed)
    }

    /// Select, Include All, Exclude All, Remove All Frames, Copy Frames into
    /// Deptheos (where the head doesn't offer it), New Stack.
    @ViewBuilder private var moreItems: some View {
        Button(
            LocalizedStringResource("Select", comment: "Frames list's ••• menu item: starts selecting frames."),
            systemImage: "checkmark.circle"
        ) {
            showFrames()
            if !model.frameSelecting.isOn { withAnimation { model.setFrameSelecting(.on([])) } }
        }
        .disabled(fixed || model.rows.isEmpty)
        Button(
            LocalizedStringResource("Include All", comment: "Frames list's ••• menu item: includes every frame."),
            systemImage: "eye"
        ) { model.setAllIncluded(true) }
            .disabled(fixed || model.rows.isEmpty)
        Button(
            LocalizedStringResource("Exclude All", comment: "Frames list's ••• menu item: excludes every frame."),
            systemImage: "eye.slash"
        ) { model.setAllIncluded(false) }
            .disabled(fixed || model.rows.isEmpty)
        Divider()
        // A stack with a render gets its frames back with Revert, so only
        // one never stacked asks.
        Button(
            LocalizedStringResource("Remove All Frames", comment: "Frames list's ••• menu item: empties the list."),
            systemImage: "xmark.circle", role: .destructive
        ) {
            if model.baseRender == nil { confirmClear = true } else { model.clearList() }
        }
        .disabled(fixed || model.rows.isEmpty)
        // Frames used in place, copied in after all; where the head names
        // their place, its badge offers this.
        if !model.outsideFolders.isEmpty, model.framesPlace == nil {
            Button(StorageBadge.copyIn, systemImage: StorageBadge.copyInSymbol) {
                Task { await model.copyFramesIn(model.outsideFolders) }
            }
            .disabled(fixed)
        }
        Divider()
        Button(
            LocalizedStringResource("New Stack", comment: "Menu item: starts a new, empty stack."),
            systemImage: "plus.app"
        ) {
            Task { await model.newStack() }
        }
        .disabled(model.busy)
    }
}

// MARK: Where the frames are

/// Beside the included count: where the frames are read from on a drive,
/// server or iCloud Drive ("On T7 SSD"), with Copy Frames into Deptheos; or
/// "T7 SSD Not Connected" with Locate…. Nothing for frames on the device
/// (ADR-145).
struct StorageBadge: View {
    static let copyIn = LocalizedStringResource(
        "Copy Frames into Deptheos",
        comment: "Menu item (the Frames head's place, or the Frames list's other commands): copies the frames used where they are in Files into the app.")
    static let copyInSymbol = "square.and.arrow.down.on.square"

    @Environment(AppModel.self) private var model
    @State private var locating = false

    var body: some View {
        Group {
            if let away = model.offlinePlace, !model.missing.isEmpty {
                Menu {
                    Section(away.connectInList(model.missing.count)) {
                        Button(ListScreen.locate, systemImage: "folder") { locating = true }
                            .disabled(model.held)
                    }
                } label: {
                    Label {
                        Text(away.notConnected)
                    } icon: {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundStyle(.orange)
                    }
                }
            } else if let place = model.framesPlace {
                Menu {
                    if !model.outsideFolders.isEmpty {
                        Button(Self.copyIn, systemImage: Self.copyInSymbol) {
                            Task { await model.copyFramesIn(model.outsideFolders) }
                        }
                        .disabled(model.held || model.adding.isActive)
                    }
                } label: {
                    Label(place.onLine, systemImage: place.symbol)
                }
                .menuIndicator(.hidden)
            }
        }
        // A capsule, so it reads as a place rather than a command.
        .font(.footnote.weight(.medium))
        .labelStyle(.titleAndIcon)
        .lineLimit(1)
        .truncationMode(.middle)
        .buttonStyle(.bordered)
        .buttonBorderShape(.capsule)
        .controlSize(.small)
        .tint(.secondary)
        .foregroundStyle(.primary)
        .locateImporter($locating)
    }
}

// MARK: Retouch

/// The render's frames, a dot on each painted from; a tap paints from it.
private struct PaintFrames: View {
    @Environment(AppModel.self) private var model

    private var paint: PaintModel { model.paint }

    var body: some View {
        List {
            Section {
                ForEach(Array(paint.frames.enumerated()), id: \.element) { index, frame in
                    Button {
                        paint.pick(frame)
                    } label: {
                        HStack(spacing: 10) {
                            Text(index + 1, format: .number.grouping(.never))
                                .font(.caption)
                                .monospacedDigit()
                                .foregroundStyle(.secondary)
                                .frame(minWidth: 22, alignment: .trailing)
                            Thumbnail(image: model.frameThumbnail(frame), side: 36)
                            Text(verbatim: (frame as NSString).lastPathComponent)
                                .font(.callout)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer(minLength: 8)
                            if paint.paintedFrom.contains(frame) {
                                PaintedDot()
                            }
                        }
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(
                        paint.sourceFrame == frame ? Color.accentColor.opacity(0.25) : nil)
                    .accessibilityAddTraits(paint.sourceFrame == frame ? .isSelected : [])
                }
            } footer: {
                PaintedFooter()
            }
        }
        .listStyle(.insetGrouped)
    }
}

/// The stack's other renders, newest first; a tap paints from one.
private struct PaintRenders: View {
    @Environment(AppModel.self) private var model

    private var paint: PaintModel { model.paint }

    var body: some View {
        List {
            Section {
                ForEach(model.paintRenders, id: \.id) { render in
                    Button {
                        model.paintFrom(render: render.id)
                    } label: {
                        RenderCard(
                            render: render, selected: paint.sourceRender == render.id,
                            // The engine's words.
                            lines: [AppWords.words.algorithmSummary(algorithm: render.settings.algorithm)]
                        ) {
                            if paint.paintedFromRenders.contains(render.id) {
                                PaintedDot()
                            }
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(Text(verbatim: render.spokenLabel))
                    .accessibilityAddTraits(paint.sourceRender == render.id ? .isSelected : [])
                }
            } footer: {
                PaintedFooter()
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if model.paintRenders.isEmpty {
                ContentUnavailableView {
                    Label(
                        LocalizedStringResource(
                            "No Other Renders", comment: "Retouch's Renders list empty-state title: the stack has no render but the one retouched."),
                        systemImage: RenderInfo.symbol)
                } description: {
                    Text(
                        "Stack again with other settings to have another render to paint from.",
                        comment: "Retouch's Renders list empty-state text.")
                }
            }
        }
    }
}

/// "• Painted from", under Retouch's lists once something is.
private struct PaintedFooter: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if !model.paint.paintedFrom.isEmpty || !model.paint.paintedFromRenders.isEmpty {
            HStack(spacing: 4) {
                PaintedDot()
                Text(LocalizedStringResource("Painted from", comment: "Marks a frame or render the result has paint from, in the Paint From list."))
            }
        }
    }
}

private struct PaintedDot: View {
    var body: some View {
        Circle().fill(Color.edited).frame(width: 6, height: 6)
            .accessibilityLabel(LocalizedStringResource("Painted from", comment: "Marks a frame or render the result has paint from, in the Paint From list."))
    }
}

extension View {
    /// Locate…: while `shown`, the folder picker for where the first missing
    /// frame's folder moved.
    func locateImporter(_ shown: Binding<Bool>) -> some View {
        modifier(LocateImporter(shown: shown))
    }
}

private struct LocateImporter: ViewModifier {
    @Environment(AppModel.self) private var model
    @Binding var shown: Bool

    func body(content: Content) -> some View {
        content.fileImporter(isPresented: $shown, allowedContentTypes: [.folder]) { picked in
            switch picked {
            case .success(let url):
                if let old = model.firstMissingFolder {
                    Task { await model.relocate(old: old, to: Picked(fromFiles: url)) }
                }
            case .failure(let error): model.report(error)
            }
        }
    }
}
