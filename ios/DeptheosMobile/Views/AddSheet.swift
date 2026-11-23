import SwiftUI
import UniformTypeIdentifiers

/// Add from Files as a sheet, from the Frames column at regular width (at
/// compact width `AddFrames` is pushed onto the Frames sheet).
struct AddSheet: View {
    var body: some View {
        NavigationStack {
            AddFrames()
        }
        .interactiveDismissDisabled()
        .presentationDetents([.medium, .large])
    }
}

/// What was picked in Files, copied in or used in place, then the copy's
/// progress, with Cancel and Add. Shown while `AppModel.adding` asks or
/// fetches, and with a failure said here, since an alert from under a sheet
/// or as one closes never shows.
struct AddFrames: View {
    @Environment(AppModel.self) private var model
    /// Use Them Where They Are…'s folder picker is up.
    @State private var choosingFolder = false
    /// The sheet's own alert; the plan stays as it was.
    @State private var folderProblem: UserText?

    var body: some View {
        Group {
            switch model.adding {
            case .asking(let plan, _): asking(plan)
            case .fetching(let progress, _, _): fetching(progress)
            case .failed(let text, _): failed(text)
            case .none, .planning, .reading: ProgressView()
            }
        }
        .alert(
            Text(verbatim: "Deptheos"),
            isPresented: Binding(get: { folderProblem != nil }, set: { if !$0 { folderProblem = nil } }),
            presenting: folderProblem
        ) { _ in
            Button(LocalizedStringResource("OK", comment: "Button that closes an error alert.")) {}
        } message: { problem in
            Text(problem)
        }
        .navigationTitle(Text("Add Frames", comment: "The title of the sheet for frames picked with Add from Files, and of the page it is in the Frames sheet."))
        .navigationBarTitleDisplayMode(.inline)
        // Cancel takes its place.
        .navigationBarBackButtonHidden()
        .toolbar {
            if case .failed = model.adding {
                ToolbarItem(placement: .confirmationAction) {
                    Button(
                        LocalizedStringResource("OK", comment: "Add Frames sheet: closes it after it said what went wrong.")
                    ) { Task { await model.acknowledgeAddFailure() } }
                }
            } else {
                ToolbarItem(placement: .cancellationAction) {
                    Button(
                        LocalizedStringResource("Cancel", comment: "Add Frames sheet: stops adding the frames picked in Files."),
                        role: .cancel
                    ) { model.cancelAdding() }
                }
            }
            if case .asking(let plan, _) = model.adding {
                ToolbarItem(placement: .confirmationAction) {
                    Button(
                        LocalizedStringResource("Add", comment: "Add Frames sheet: adds the frames picked in Files.")
                    ) { Task { await model.confirmAdding() } }
                        .disabled(plan.short)
                }
            }
        }
    }

    private func asking(_ plan: AddPlan) -> some View {
        Form {
            if !plan.folders.isEmpty {
                Section {
                    ForEach(plan.folders) { folder in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(verbatim: folder.name)
                            Text(line(folder))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Toggle(
                        isOn: Binding(
                            get: { plan.copyFolders }, set: { model.setCopyFolders($0) })
                    ) {
                        Text(
                            "Copy into Deptheos",
                            comment: "Add Frames sheet: the switch that copies the picked folders into Deptheos instead of reading them where they are.")
                    }
                } footer: {
                    Text(folderFooter(plan))
                }
            }
            if !plan.loose.isEmpty {
                Section {
                    Text(looseLine(plan))
                    if plan.looseFolder != nil {
                        Button(LocalizedStringResource(
                            "Use Them Where They Are…",
                            comment: "Add Frames sheet: the button under the line about single frames picked in Files, all from one folder. It opens Files to pick that folder, so the frames are read from it and not copied.")
                        ) { choosingFolder = true }
                    }
                } footer: {
                    Text(looseFooter(plan))
                }
            }
            if !plan.covered.isEmpty {
                Section {
                    Text(
                        "\(plan.covered.count) frames are in a folder Deptheos already reads, and are used where they are.",
                        comment: "Add Frames sheet: about frames picked one by one in Files from inside a folder whose frames Deptheos already uses where they are. The placeholder is how many.")
                }
            }
            if plan.short {
                Section {
                    Label {
                        Text(
                            "Not enough space: the copy needs \(AppModel.size(plan.copyBytes)), and \(AppModel.size(plan.free ?? 0)) is free.",
                            comment: "Add Frames sheet: the warning when the device hasn't room for the copy. The placeholders are sizes, such as '14.2 GB': what the copy needs and what is free.")
                    } icon: {
                        Image(systemName: "exclamationmark.triangle")
                    }
                    .foregroundStyle(.orange)
                }
            }
        }
        // Opened at the folder the frames are in: Open picks it.
        .fileImporter(isPresented: $choosingFolder, allowedContentTypes: [.folder]) { picked in
            switch picked {
            case .success(let url):
                Task { folderProblem = await model.useFolder(Picked(fromFiles: url)) }
            case .failure(let error): folderProblem = AppModel.message(error)
            }
        }
        .fileDialogDefaultDirectory(plan.looseFolder)
    }

    /// Until OK.
    private func failed(_ text: UserText) -> some View {
        ContentUnavailableView {
            Label {
                Text("Couldn't Add Frames", comment: "Add Frames sheet: the heading when adding frames picked in Files failed, over the sentence saying why.")
            } icon: {
                Image(systemName: "exclamationmark.triangle")
            }
        } description: {
            Text(text)
        }
    }

    /// Why single frames are copied, and with all in one folder, the way to
    /// use them in place.
    private func looseFooter(_ plan: AddPlan) -> LocalizedStringResource {
        guard let folder = plan.looseFolder else {
            return LocalizedStringResource(
                "Frames picked one by one are always copied. To use them where they are, add their folder instead.",
                comment: "Add Frames sheet: the footer under the line about single frames picked in Files, from more than one folder.")
        }
        return LocalizedStringResource(
            "Frames picked one by one are copied. To use these where they are, choose their folder, \"\(folder.lastPathComponent)\": Deptheos then reads them from it each time.",
            comment: "Add Frames sheet: the footer under the line about single frames picked in Files, all from one folder, and the Use Them Where They Are… button. The placeholder is the folder's name.")
    }

    private func fetching(_ progress: FetchProgress) -> some View {
        VStack(spacing: 12) {
            ProgressView(value: Double(progress.done), total: Double(max(progress.total, 1)))
            Text(progress.label)
                .font(.callout)
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
        .padding(24)
        .frame(maxHeight: .infinity)
    }

    /// "214 frames · 14.2 GB · on T7 SSD".
    private func line(_ folder: AddPlan.Item) -> UserText {
        let count = folder.images.count
        var parts: [UserText] = [.app(framesCount(count))]
        if folder.bytes > 0 { parts.append(.verbatim(AppModel.size(folder.bytes))) }
        if let place = folder.source.place { parts.append(.app(place.onInLine)) }
        return UserText.line(parts)
    }

    private func folderFooter(_ plan: AddPlan) -> LocalizedStringResource {
        if plan.copyFolders {
            let size = plan.folders.reduce(0) { $0 + $1.bytes }
            guard size > 0 else {
                return LocalizedStringResource(
                    "The frames are copied, so they stay available whatever happens to the photos.",
                    comment: "Add Frames sheet: the footer under the folders while Copy into Deptheos is on.")
            }
            return LocalizedStringResource(
                "The frames are copied, so they stay available whatever happens to the photos. Uses \(AppModel.size(size)) on this device.",
                comment: "Add Frames sheet: the footer under the folders while Copy into Deptheos is on. The placeholder is the copy's size, such as '14.2 GB'.")
        }
        guard let place = plan.place else {
            return LocalizedStringResource(
                "Deptheos reads the frames from where they are each time. Stacking again and retouching need them there.",
                comment: "Add Frames sheet: the footer under the folders while Copy into Deptheos is off, for folders on the device.")
        }
        return place.readsFrom
    }

    private func looseLine(_ plan: AddPlan) -> LocalizedStringResource {
        let count = plan.loose.count
        let size = plan.loose.reduce(0) { $0 + $1.bytes }
        guard size > 0 else {
            return LocalizedStringResource(
                "\(count) frames will be copied into Deptheos.",
                comment: "Add Frames sheet: about the frames picked one by one in Files. The placeholder is how many.")
        }
        let sizeText = AppModel.size(size)
        return LocalizedStringResource(
            "\(count) frames will be copied into Deptheos (\(sizeText)).",
            comment: "Add Frames sheet: about the frames picked one by one in Files. The placeholders are how many and their size, such as '1.2 GB'.")
    }
}
