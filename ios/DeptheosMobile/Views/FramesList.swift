import SwiftUI
@preconcurrency import DeptheosEngine
import UniformTypeIdentifiers

/// A running batch: which stack of how many, and the whole run's progress.
/// Tap to see it in the viewer.
struct BatchProgressBar: View {
    let run: BatchRun
    let stacking: StackingState?
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            VStack(alignment: .leading, spacing: 6) {
                // The engine's words.
                Text(verbatim: run.progress)
                    .font(.footnote.monospacedDigit())
                    .lineLimit(1)
                    .truncationMode(.middle)
                ProgressView(value: fraction)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 20))
        .padding(.horizontal)
        .padding(.bottom, 8)
        .accessibilityHint(LocalizedStringResource(
            "Shows the stack in the viewer", comment: "VoiceOver hint of the iPhone bar with a batch's progress."))
    }

    /// Stacks finished, plus the current one's share, of all.
    private var fraction: Double {
        run.queue.fraction(current: stacking?.fraction ?? 0)
    }
}

/// Every image added, included or excluded, in stacking order. Tap one to
/// view it; Select to act on a few.
struct ListScreen: View {
    static let frames = LocalizedStringResource(
        "Frames", comment: "The side of the Frames | Renders switch at the head of the stack's sources that lists its frames, and the title of the iPhone's sheet of them.")
    static let locate = LocalizedStringResource(
        "Locate…", comment: "Frames list banner button, and the Frames head's menu while their drive isn't connected: points the list and renders at a folder that moved.")

    @Environment(AppModel.self) private var model
    @Environment(\.framePickers) private var pickers
    @State private var locating = false

    var body: some View {
        @Bindable var model = model
        List(selection: selectionBinding) {
            if let added = model.addedToExisting {
                Section {
                    AddedToStackBanner(added: added)
                        .disabled(busy)
                }
            }
            // Frames on a disconnected drive or server: the head says so,
            // with Locate….
            if !model.missing.isEmpty, model.offlinePlace == nil {
                Section {
                    MissingBanner { locating = true }
                        .disabled(busy)
                }
            }
            if !model.rows.isEmpty {
                Section {
                    if model.hasPairs {
                        Picker(
                            LocalizedStringResource(
                                "RAW + JPEG pairs", comment: "Frames list: the picker for shots that have both a RAW and a JPEG file."),
                            selection: Binding(
                                get: { model.list.source },
                                set: { source in Task { await model.setPairSource(source) } })
                        ) {
                            Text("Use RAW", comment: "Pairs picker: stack the RAW file of each shot that has both.").tag(Source.raw)
                            Text("Use JPEG", comment: "Pairs picker: stack the JPEG file of each shot that has both.").tag(Source.standard)
                        }
                        .pickerStyle(.segmented)
                        .disabled(fixed)
                    }
                    ForEach(Array(model.rows.enumerated()), id: \.element.id) { index, row in
                        FrameRow(
                            number: index + 1, row: row, editing: editing,
                            viewed: model.selectedFile == row.file)
                        .swipeActions {
                            if !fixed {
                                Button(LocalizedStringResource(
                                    "Remove", comment: "Swipe action on a frame: takes it out of the list."), role: .destructive) { model.remove([row.file]) }
                                Button(row.included ? FrameRow.exclude : FrameRow.include) {
                                    model.toggleIncluded(row)
                                }
                            }
                        }
                    }
                }
            }
            // Frames on their way in, no sheet up: a folder read in place,
            // or a copy on the device.
            if let names = model.adding.inList {
                Section {
                    ReadingRow(names: names)
                }
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.editMode, editMode)
        .overlay {
            if model.rows.isEmpty, model.adding.inList == nil {
                ContentUnavailableView {
                    Label(
                        LocalizedStringResource("No Frames", comment: "Frames list empty-state title."),
                        systemImage: "photo.stack")
                } description: {
                    Text(
                        "Take a burst, or add frames from Photos or Files.",
                        comment: "Frames list empty-state text, over its Take Burst, Add from Photos…, Add from Files… and Add Folder… buttons.")
                } actions: {
                    Button(CameraView.takeBurst, systemImage: "camera") { model.present(.camera) }
                        .buttonStyle(.glassProminent)
                        .disabled(busy)
                    if let pickers {
                        Button(FramePickers.addFromPhotos, systemImage: FramePickers.photosSymbol, action: pickers.photos)
                            .buttonStyle(.glass)
                            .disabled(busy || model.adding.isActive)
                        // Frames still being read into the list come first,
                        // as in the + menu.
                        Button(FramePickers.addFromFiles, systemImage: FramePickers.filesSymbol, action: pickers.files)
                            .buttonStyle(.glass)
                            .disabled(busy || model.adding.isActive)
                        Button(FramePickers.addFolder, systemImage: FramePickers.folderSymbol, action: pickers.folder)
                            .buttonStyle(.glass)
                            .disabled(busy || model.adding.isActive)
                    }
                }
            }
        }
        .safeAreaBar(edge: .bottom) {
            if editing {
                FrameActions()
            }
        }
        .locateImporter($locating)
    }

    /// A stack or batch is running: browsable, not changeable.
    private var busy: Bool { model.held }

    /// Nothing in the list changes while a stack runs or another opens.
    private var fixed: Bool { model.held }

    private var editing: Bool { model.frameSelecting.isOn }

    private var editMode: Binding<EditMode> {
        Binding(
            get: { editing ? .active : .inactive },
            set: { mode in
                if mode.isEditing != editing { model.setFrameSelecting(mode.isEditing ? .on([]) : .off) }
            })
    }

    private var selectionBinding: Binding<Set<String>> {
        Binding(
            get: { model.frameSelecting.selected },
            set: { if editing { model.setFrameSelecting(.on($0)) } })
    }
}

/// The last row while frames are read in (`Adding.inList`), named when one.
private struct ReadingRow: View {
    let names: [String]

    var body: some View {
        HStack(spacing: 12) {
            ProgressView()
            VStack(alignment: .leading, spacing: 1) {
                Text("Reading Frames…", comment: "Frames list: the last row while frames added are being read into the list.")
                if names.count == 1, let name = names.first {
                    Text(verbatim: name)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// Images not where the list says (moved, or on an unnamed disconnected
/// drive). A named place is the head's badge (`StorageBadge`).
private struct MissingBanner: View {
    @Environment(AppModel.self) private var model
    let locate: () -> Void

    var body: some View {
        let count = model.missing.count
        VStack(alignment: .leading, spacing: 8) {
            Label(LocalizedStringResource(
                "\(count) frames can't be found",
                comment: "Frames list banner heading: frames whose files aren't where the list says. The placeholder is how many."),
                  systemImage: "exclamationmark.triangle")
            if let folder = model.firstMissingFolder {
                Text(
                    "\(count) frames were in \"\((folder as NSString).lastPathComponent)\".",
                    comment: "Frames list banner, under how many frames can't be found. The placeholders are how many frames and the name of the folder the first was in; in English the number only picks 'It was' or 'They were' and isn't shown.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Button(ListScreen.locate, action: locate)
                .buttonStyle(.glass)
        }
    }
}

/// Frame number, thumbnail, file name, and the eye that includes or
/// excludes it; excluded frames dim.
struct FrameRow: View {
    static let include = LocalizedStringResource(
        "Include", comment: "Includes a frame, or the frames selected, in the stack.")
    static let exclude = LocalizedStringResource(
        "Exclude", comment: "Excludes a frame, or the frames selected, from the stack; it stays in the list.")

    @Environment(AppModel.self) private var model
    let number: Int
    let row: ListRow
    let editing: Bool
    let viewed: Bool

    var body: some View {
        let state = model.unavailable[row.file]
        HStack(spacing: 10) {
            // A button, as a render's row, so a tap always shows the frame;
            // while selecting, taps go to the list.
            Button {
                model.revealViewer()
                Task { await model.select(row.file) }
            } label: {
                HStack(spacing: 10) {
                    Text(number, format: .number.grouping(.never))
                        .font(.caption)
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .frame(minWidth: 22, alignment: .trailing)
                    Thumbnail(image: model.frameThumbnail(row.file), side: 36)
                        .opacity(row.included ? 1 : 0.4)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(verbatim: row.name)
                            .font(.callout)
                            .foregroundStyle(row.included ? .primary : .secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        if let state {
                            Text(state.label)
                                .font(.caption)
                                .foregroundStyle(state == .inCloud ? Color.secondary : Color.orange)
                        } else if let why = model.leftOutReason(for: row) {
                            Label {
                                Text(verbatim: why)
                                    .lineLimit(2)
                            } icon: {
                                Image(systemName: "exclamationmark.triangle.fill")
                            }
                            .font(.caption)
                            .foregroundStyle(.orange)
                        }
                    }
                    Spacer(minLength: 8)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .allowsHitTesting(!editing)
            if !editing {
                Button {
                    model.toggleIncluded(row)
                } label: {
                    Image(systemName: row.included ? "eye" : "eye.slash")
                        .frame(width: 32, height: 32)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .disabled(model.held)
                .accessibilityLabel(row.included ? Self.exclude : Self.include)
            }
        }
        .listRowBackground(viewed && !editing ? Color.accentColor.opacity(0.15) : nil)
        .accessibilityLabel(LocalizedStringResource(
            "Frame \(number), \(row.name)",
            comment: "VoiceOver label of a frame in the list. The placeholders are its number and its file's name."))
        .accessibilityValue(row.included ? .empty : LocalizedStringResource(
            "Excluded", comment: "VoiceOver value of a frame in the list that is excluded from the stack."))
    }
}

/// Frames added to a stack that had some: offers New Stack Instead in case
/// they're another subject.
struct AddedToStackBanner: View {
    @Environment(AppModel.self) private var model
    let added: AddedToExisting

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(verbatim: added.text)
                    .font(.subheadline)
                Spacer()
                Button {
                    model.addedToExisting = nil
                } label: {
                    Image(systemName: "xmark")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(LocalizedStringResource(
                    "Dismiss", comment: "Closes the Added frames notice."))
            }
            Button(LocalizedStringResource(
                "New Stack Instead",
                comment: "Button on the Added frames notice: moves the newly added frames into a new stack of their own.")
            ) {
                Task { await model.moveAddedToNewStack() }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .padding(.vertical, 2)
    }
}

/// While selecting: "2 selected", Stack or Remove, Done; then include,
/// exclude or include only these.
struct FrameActions: View {
    static let more = LocalizedStringResource(
        "More", comment: "The Frames list's menu of its other commands (Select, Include All, Exclude All, Remove All Frames, New Stack), before iOS 27.")

    @Environment(AppModel.self) private var model

    private var selection: Set<String> { model.frameSelecting.selected }

    var body: some View {
        let stackable = model.rows.filter { selection.contains($0.file) && $0.included }
        VStack(spacing: 8) {
            HStack {
                Text(selection.isEmpty
                     ? LocalizedStringResource("Select frames", comment: "Bar while selecting frames, before any is selected.")
                     : LocalizedStringResource(
                        "\(selection.count) selected", comment: "Bar while selecting frames. The placeholder is how many are selected."))
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Spacer()
                Group {
                    Button(
                        LocalizedStringResource(
                            "Stack These", comment: "Bar while selecting frames: stacks the included frames among those selected."),
                        systemImage: "square.3.layers.3d"
                    ) {
                        let files = selection
                        model.setFrameSelecting(.off)
                        model.setPanel(.closed)
                        Task { await model.stack(selected: files) }
                    }
                    .disabled(stackable.count < 2 || model.engine == nil)
                    Button(
                        LocalizedStringResource(
                            "Remove from List", comment: "Bar while selecting frames: takes the frames selected out of the list."),
                        systemImage: "minus.circle", role: .destructive
                    ) {
                        let files = Array(selection)
                        model.setFrameSelecting(.on([]))
                        model.remove(files)
                    }
                    .disabled(selection.isEmpty)
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.glass)
                Button(LocalizedStringResource("Done", comment: "Bar while selecting frames: stops selecting.")) {
                    withAnimation { model.setFrameSelecting(.off) }
                }
                    .fontWeight(.semibold)
            }
            HStack {
                Button(FrameRow.include) { model.setIncluded(true, files: Array(selection)) }
                Button(FrameRow.exclude) { model.setIncluded(false, files: Array(selection)) }
                Button(LocalizedStringResource(
                    "Only These", comment: "Bar while selecting frames: includes the frames selected and excludes the rest.")) {
                    model.includeOnly(selection)
                }
            }
            .buttonStyle(.glass)
            .disabled(selection.isEmpty)
        }
        .padding(.horizontal)
        .padding(.vertical, 10)
    }
}

struct Thumbnail: View {
    let image: CGImage?
    let side: CGFloat

    var body: some View {
        Group {
            if let image {
                Image(decorative: image, scale: 1).resizable().scaledToFill()
            } else {
                Rectangle().fill(.quaternary)
                    .overlay(Image(systemName: "photo").foregroundStyle(.secondary))
            }
        }
        .frame(width: side, height: side)
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}
