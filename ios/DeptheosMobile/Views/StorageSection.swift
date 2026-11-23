import SwiftUI

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Settings ▸ Storage: the folders read in place (`FoldersView`), Delete
/// Library… (`DeleteLibrarySection`), then the Imports copies no stack or
/// render uses, with Delete, recounted whenever the history changes.
struct StorageSection: View {
    @Environment(AppModel.self) private var model
    @State private var unused = Unused.counting {
        didSet { stateLog.debug("unused copies: \(unused.name, privacy: .public)") }
    }
    @State private var confirming = false

    /// The unused copies.
    ///
    /// Lifecycle: counting, counted (folders, bytes) or deleting. Held in
    /// `unused`, changed only by its count and its Delete.
    ///
    ///     counting ──counted──▶ counted(folders, bytes) ──Delete──▶ deleting ──▶ counting
    private enum Unused {
        case counting
        case counted(folders: Int, bytes: Int64)
        case deleting

        var name: String {
            switch self {
            case .counting: "counting"
            case .counted: "counted"
            case .deleting: "deleting"
            }
        }
    }

    var body: some View {
        Section {
            NavigationLink(value: SettingsPage.folders) {
                LabeledContent {
                    Text(folderCount)
                } label: {
                    Text(FoldersView.title)
                }
            }
        } header: {
            Text("Frames", comment: "Settings ▸ Storage: the header of the sections on where frames are read from and the copies Deptheos keeps.")
        } footer: {
            Text(
                "The folders whose frames Deptheos reads where they are, on this device, a drive or a server.",
                comment: "Settings ▸ Storage: the footer under the Folders row.")
        }
        DeleteLibrarySection()
        Section {
            LabeledContent {
                Text(value)
            } label: {
                Text(
                    "Unused copies",
                    comment: "Settings ▸ Storage: the row showing how many copied folders no stack or render uses, and their size.")
            }
            Button(
                LocalizedStringResource(
                    "Delete Unused Copies…",
                    comment: "Settings ▸ Storage: the button that asks before deleting the unused copies."),
                role: .destructive
            ) { confirming = true }
                .disabled(!canDelete)
                // On the button, so its popover points at it.
                .confirmationDialog(
                    Text(
                        "Delete the unused copies?",
                        comment: "Settings ▸ Storage: the title of the dialog that confirms Delete Unused Copies…."),
                    isPresented: $confirming, titleVisibility: .visible
                ) {
                    Button(
                        LocalizedStringResource(
                            "Delete", comment: "Settings ▸ Storage: confirms deleting the unused copies."),
                        role: .destructive
                    ) { Task { await delete() } }
                } message: {
                    Text(
                        "The folders in Imports that no stack or render uses are deleted. This can't be undone.",
                        comment: "Settings ▸ Storage: the message of the dialog that confirms Delete Unused Copies….")
                }
        } footer: {
            Text(footer)
        }
        .task(id: model.history.revision) { await count() }
    }

    /// Where Files shows the copies, named as Files names the device.
    private var footer: LocalizedStringResource {
        UIDevice.current.userInterfaceIdiom == .pad
            ? LocalizedStringResource(
                "These are frames copied into Deptheos from Photos or Files that no stack or render uses any more. They are in Files, under On My iPad ▸ Deptheos ▸ Imports.",
                comment: "Settings ▸ Storage: the footer on an iPad. 'On My iPad' is the location as the Files app names it.")
            : LocalizedStringResource(
                "These are frames copied into Deptheos from Photos or Files that no stack or render uses any more. They are in Files, under On My iPhone ▸ Deptheos ▸ Imports.",
                comment: "Settings ▸ Storage: the footer on an iPhone. 'On My iPhone' is the location as the Files app names it.")
    }

    /// "None", or the number.
    private var folderCount: UserText {
        let count = model.keptFolderCount
        return count == 0
            ? .app(LocalizedStringResource(
                "None", comment: "Settings ▸ Storage: the Folders row when no folder is read where it is."))
            : .verbatim(count.formatted())
    }

    private var value: UserText {
        switch unused {
        // Still counting: a mark, not a word.
        case .counting, .deleting: .verbatim("…")
        case .counted(0, _):
            .app(LocalizedStringResource(
                "None", comment: "Settings ▸ Storage: the Unused copies row when there are none."))
        case .counted(let folders, let bytes):
            .app(LocalizedStringResource(
                "\(folders) folders · \(AppModel.size(bytes))",
                comment: "Settings ▸ Storage: the Unused copies row's value. The placeholders are how many folders and their size, such as '1.2 GB'."))
        }
    }

    private var canDelete: Bool {
        if case .counted(let folders, _) = unused { folders > 0 && !model.busy } else { false }
    }

    private func count() async {
        unused = .counting
        let found = await model.unusedImports()
        unused = .counted(folders: found.folders.count, bytes: found.bytes)
    }

    private func delete() async {
        unused = .deleting
        _ = await model.deleteUnusedImports()
        await count()
    }
}
