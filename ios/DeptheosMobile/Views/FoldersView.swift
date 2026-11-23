import SwiftUI
import UniformTypeIdentifiers
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Settings ▸ Storage ▸ Folders: the folders outside Deptheos read in place
/// (`KeptFolder`), the Library's top roots (ADR-145), each with its place,
/// whether it is reachable, and how many stacks use it. A folder's menu
/// copies its frames in, or while unreachable, Locate…. Nothing to remove
/// by hand: a folder goes with the last stack or render using it.
struct FoldersView: View {
    static let title = LocalizedStringResource(
        "Folders",
        comment: "Settings ▸ Storage: the row that opens the list of folders Deptheos reads frames from where they are, and that list's title.")

    @Environment(AppModel.self) private var model
    @State private var folders = Folders.reading(last: []) {
        didSet { stateLog.debug("folders: \(folders.name, privacy: .public)") }
    }
    /// The folder Locate…'s picker in Files is up for.
    @State private var locating: KeptFolder?

    /// Lifecycle: reading (with the last list) or listed. Held in `folders`,
    /// changed only by its reads.
    ///
    ///     reading(last) ──read──▶ listed(folders)
    ///     listed ──Copy into Deptheos or Locate…, done──▶ reading(last) ──read──▶ listed
    ///
    /// A reread keeps the last list on screen. A copy's progress is
    /// `AppModel.adding`.
    private enum Folders {
        case reading(last: [KeptFolder])
        case listed([KeptFolder])

        var shown: [KeptFolder] {
            switch self {
            case .reading(let last): last
            case .listed(let folders): folders
            }
        }

        var name: String {
            switch self {
            case .reading: "reading"
            case .listed(let folders): "listed \(folders.count)"
            }
        }
    }

    var body: some View {
        List {
            if case .fetching(let progress, _, _) = model.adding {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        ProgressView(value: Double(progress.done), total: Double(max(progress.total, 1)))
                        Text(progress.label)
                            .font(.callout)
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                    }
                    Button(
                        LocalizedStringResource(
                            "Cancel", comment: "Settings ▸ Storage ▸ Folders: stops copying a folder's frames into Deptheos."),
                        role: .cancel
                    ) { model.cancelAdding() }
                }
            }
            if !folders.shown.isEmpty {
                Section {
                    ForEach(folders.shown) { folder in
                        row(folder)
                    }
                } footer: {
                    Text(
                        "Deptheos reads the frames in these folders where they are. A folder stays here while a stack or render uses its frames, and goes when the last of them is deleted. After a drive or a server was disconnected, pick its top folder once with Locate… and every folder on it comes back.",
                        comment: "Settings ▸ Storage ▸ Folders: the footer under the list of folders. Locate… is a folder's command here.")
                }
            }
        }
        .overlay {
            if folders.shown.isEmpty {
                if case .listed = folders {
                    ContentUnavailableView {
                        Label(
                            LocalizedStringResource(
                                "No Folders", comment: "Settings ▸ Storage ▸ Folders: the empty-state title."),
                            systemImage: "folder")
                    } description: {
                        Text(
                            "A folder added with Add Folder…, or stacked in a batch, is read where it is and listed here.",
                            comment: "Settings ▸ Storage ▸ Folders: the empty-state text. Add Folder… is the Frames list's command.")
                    }
                } else {
                    ProgressView()
                }
            }
        }
        .navigationTitle(Text(Self.title))
        .navigationBarTitleDisplayMode(.inline)
        .task { await read() }
        .fileImporter(
            isPresented: Binding(get: { locating != nil }, set: { if !$0 { locating = nil } }),
            allowedContentTypes: [.folder]
        ) { [locating] picked in
            switch picked {
            case .success(let url):
                guard let locating else { return }
                Task {
                    await model.locate(locating, at: Picked(fromFiles: url))
                    await read()
                }
            case .failure(let error): model.report(error)
            }
        }
    }

    private func row(_ folder: KeptFolder) -> some View {
        HStack(spacing: 12) {
            Image(systemName: folder.place?.symbol ?? "folder")
                .foregroundStyle(.secondary)
                .frame(width: 28)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(verbatim: folder.name)
                Text(line(folder))
                    .font(.caption)
                    .foregroundStyle(folder.reach == .there ? AnyShapeStyle(.secondary) : AnyShapeStyle(.orange))
            }
            Spacer(minLength: 8)
            Menu {
                if folder.reach != .there {
                    Button(
                        LocalizedStringResource(
                            "Locate…",
                            comment: "Settings ▸ Storage ▸ Folders, a folder's menu while it can't be reached: picks where the folder is now, and its stacks and renders point there."),
                        systemImage: "folder.badge.questionmark"
                    ) { locating = folder }
                } else {
                    Button(
                        LocalizedStringResource(
                            "Copy into Deptheos",
                            comment: "Settings ▸ Storage ▸ Folders, a folder's menu: copies the frames in use from the folder into Deptheos, so they are no longer read from it."),
                        systemImage: "square.and.arrow.down.on.square"
                    ) {
                        Task {
                            folders = .reading(last: folders.shown)
                            await model.copyIn(folder)
                            await read()
                        }
                    }
                }
            } label: {
                Image(systemName: "ellipsis")
                    .frame(width: 44, height: 44)
                    .contentShape(.rect)
                    .accessibilityLabel(LocalizedStringResource(
                        "Folder Actions",
                        comment: "Settings ▸ Storage ▸ Folders: VoiceOver label of the menu button on a folder's row."))
            }
            .disabled(model.busy || model.adding.isActive)
        }
    }

    /// "On T7 SSD · 3 stacks", "T7 SSD Not Connected · 3 stacks", or
    /// "Missing · 3 stacks" for one gone from a connected drive.
    private func line(_ folder: KeptFolder) -> UserText {
        var parts: [UserText] = []
        switch folder.reach {
        case .notConnected:
            parts.append(.app(folder.place?.notConnected ?? LocalizedStringResource(
                "Not Connected",
                comment: "Settings ▸ Storage ▸ Folders: under a folder that can't be reached, when what it is on isn't known.")))
        case .missing:
            parts.append(.app(LocalizedStringResource(
                "Missing",
                comment: "Settings ▸ Storage ▸ Folders: under a folder that isn't where it was, though what it is on is there.")))
        case .there:
            if let place = folder.place { parts.append(.app(place.onLine)) }
        }
        parts.append(.app(stacksCount(folder.stacks)))
        return UserText.line(parts)
    }

    private func read() async {
        folders = .reading(last: folders.shown)
        folders = .listed(await model.keptFolders() ?? [])
    }
}
