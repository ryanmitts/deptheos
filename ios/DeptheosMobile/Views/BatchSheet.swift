import SwiftUI
import UniformTypeIdentifiers
import os
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Batch…: folders from Add Folders…, Add Subfolders Of… (optionally
/// deeper) and the list; one stack per folder or each auto sorted. A
/// summary of what it will make, then Start. Settings are the inspector's.
struct BatchSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    /// The engine's (`BatchSources`, as on the desktop), changed only
    /// through `step`.
    @State private var sources = BatchSources()
    /// As the engine last answered.
    @State private var now = BatchSourcesNow(entries: [], parents: [], deep: false)
    @State private var useList = false
    @State private var split = Split.perFolder
    @State private var planning = Planning.none {
        didSet { stateLog.debug("batch plan: \(planning.name, privacy: .public)") }
    }
    @State private var picking: Picking?
    /// The sheet's own alert: one from under a sheet is never shown.
    @State private var problem: UserText?

    /// What the run would make, for the folders and choices as they are.
    ///
    /// Lifecycle: none, planning (with the last plan), planned or failed
    /// (why). Held in `planning`, changed as folders and choices change and
    /// the engine answers.
    ///
    ///     none ──folders or choices──▶ planning(nil) ──planned──▶ planned(plan)
    ///     planned(plan) ──a change──▶ planning(plan) ──planned──▶ planned(new)
    ///     planning ──the engine failed──▶ failed(why) ──a change──▶ planning(nil)
    ///     any ──nothing left to plan──▶ none
    ///
    /// Replanning keeps the last plan on screen; a failure says why. Start
    /// is off for both.
    private enum Planning {
        case none
        case planning(last: BatchPlan?)
        case planned(BatchPlan)
        case failed(UserText)

        /// The one made, or the last while another is.
        var shown: BatchPlan? {
            switch self {
            case .none, .failed: nil
            case .planning(let last): last
            case .planned(let plan): plan
            }
        }

        var name: String {
            switch self {
            case .none: "none"
            case .planning: "planning"
            case .planned: "planned"
            case .failed: "failed"
            }
        }
    }

    enum Picking: Identifiable {
        case folders
        case parent
        var id: Self { self }
    }

    /// In order, each once.
    private var entries: [BatchSource] { now.entries }

    private func step(_ change: (BatchSources) -> BatchSourcesNow) {
        now = change(sources)
    }

    private var listUsable: Bool { model.includedRows.count >= 2 }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ForEach(entries, id: \.folder) { entry in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(verbatim: (entry.folder as NSString).lastPathComponent)
                            if let parent = entry.parent {
                                Text(
                                    "in \((parent as NSString).lastPathComponent)",
                                    comment: "New Batch: under a folder found with Add Subfolders of…. The placeholder is the name of the folder it was found in.")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .swipeActions {
                            Button(
                                LocalizedStringResource(
                                    "Remove", comment: "New Batch: the swipe action that takes a folder out of the batch."),
                                role: .destructive
                            ) { step { $0.remove(folder: entry.folder) } }
                        }
                    }
                    Button {
                        picking = .folders
                    } label: {
                        Label {
                            Text(
                                "Add Folders…",
                                comment: "New Batch: the button that picks folders in Files, each stacked.")
                        } icon: {
                            Image(systemName: "folder.badge.plus")
                        }
                    }
                    Button {
                        picking = .parent
                    } label: {
                        Label {
                            Text(
                                "Add Subfolders of…",
                                comment: "New Batch: the button that picks one folder in Files and stacks each folder inside it.")
                        } icon: {
                            Image(systemName: "folder")
                        }
                    }
                    if !now.parents.isEmpty {
                        Toggle(isOn: Binding(get: { now.deep }, set: { deep in step { $0.setDeep(deep: deep) } })) {
                            Text(
                                "Include deeper folders",
                                comment: "New Batch: the switch that also stacks the folders inside the subfolders.")
                        }
                    }
                    Toggle(isOn: $useList) {
                        Text(
                            "Use the list (\(model.includedRows.count))",
                            comment: "New Batch: the switch that adds the frames in the list to the batch. The placeholder is how many frames the list has.")
                    }
                    .disabled(!listUsable)
                } header: {
                    Text("Folders", comment: "New Batch: the header of the section of folders to stack.")
                } footer: {
                    Text(
                        "The frames directly inside each folder are stacked; folders inside it are not. Add Subfolders of… stacks each folder inside the one you pick.",
                        comment: "New Batch: the footer under the folders.")
                }
                Section {
                    Picker(
                        LocalizedStringResource(
                            "How to split", comment: "New Batch: the choice of how folders become stacks (read by VoiceOver)."),
                        selection: $split
                    ) {
                        Text(
                            "One stack per folder",
                            comment: "New Batch, How to Split: each folder becomes one stack.")
                            .tag(Split.perFolder)
                        Text(
                            "Auto sort",
                            comment: "New Batch, How to Split: each folder is split into stacks by capture time.")
                            .tag(Split.autoSort)
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                } header: {
                    Text("How to Split", comment: "New Batch: the header of the section of how folders become stacks.")
                } footer: {
                    Text(
                        "Auto sort splits each folder into stacks by capture time. It never mixes folders, and skips frames it can't place.",
                        comment: "New Batch: the footer under How to Split.")
                }
                Section {
                    HStack {
                        Text(summary).foregroundStyle(.secondary)
                        if case .planning = planning { Spacer(); ProgressView() }
                    }
                    Text(settingsLine).font(.caption).foregroundStyle(.secondary)
                } footer: {
                    Text(
                        "Every stack uses the method and alignment set in Settings. Where iOS gives Deptheos time in the background, the batch goes on when you leave the app; otherwise it pauses until you come back.",
                        comment: "New Batch: the footer under the summary of what the batch will make.")
                }
            }
            .navigationTitle(Text("New Batch", comment: "The title of the sheet that sets up a batch."))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(
                        LocalizedStringResource("Cancel", comment: "New Batch: closes the sheet without starting a batch."),
                        role: .cancel
                    ) {
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(LocalizedStringResource("Start", comment: "New Batch: starts the batch.")) { start() }
                        .disabled(!canStart)
                }
            }
            .fileImporter(
                isPresented: Binding(
                    get: { picking != nil }, set: { if !$0 { picking = nil } }),
                allowedContentTypes: [.folder],
                allowsMultipleSelection: picking == .folders
            ) { [picking] picked in
                switch picked {
                case .success(let urls): Task { await add(urls, asParents: picking == .parent) }
                case .failure(let error): model.report(error)
                }
            }
            .alert(
                Text(verbatim: "Deptheos"),
                isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } }),
                presenting: problem
            ) { _ in
                Button(LocalizedStringResource("OK", comment: "Button that closes an error alert.")) {}
            } message: { problem in
                Text(problem)
            }
            .task(id: parentsKey) { await findChildren() }
            .task(id: planKey) { await replan() }
        }
    }

    /// "Pyramid · S4 · aligned · cropped": the settings every stack uses.
    private var settingsLine: UserText {
        // The method and its values, as the engine words them.
        var parts = [UserText.verbatim(AppWords.words.algorithmSummary(algorithm: model.settings.algorithm))]
        let settings = model.stackSettings
        parts.append(.app(
            settings.align
                ? LocalizedStringResource(
                    "aligned", comment: "New Batch: in the line of settings every stack uses ('Pyramid · S4 · aligned · cropped'). Lowercase.")
                : LocalizedStringResource(
                    "not aligned", comment: "New Batch: in the line of settings every stack uses ('Pyramid · S4 · not aligned'). Lowercase.")))
        if settings.align {
            parts.append(.app(
                settings.crop
                    ? LocalizedStringResource(
                        "cropped", comment: "New Batch: in the line of settings every stack uses ('Pyramid · S4 · aligned · cropped'). Lowercase.")
                    : LocalizedStringResource(
                        "uncropped", comment: "New Batch: in the line of settings every stack uses ('Pyramid · S4 · aligned · uncropped'). Lowercase.")))
        }
        return UserText.line(parts)
    }

    private var canStart: Bool {
        guard case .planned(let plan) = planning else { return false }
        return !plan.jobs.isEmpty && model.engine != nil
            && !model.busy
    }

    private var summary: UserText {
        if entries.isEmpty, !useList {
            return .app(LocalizedStringResource(
                "Add the folders to stack", comment: "New Batch: the summary line before any folder is added."))
        }
        if case .failed(let why) = planning { return why }
        // The plan's summary is the engine's own sentence.
        if let plan = planning.shown { return .verbatim(plan.summary) }
        return .app(LocalizedStringResource(
            "Working out the stacks…", comment: "New Batch: the summary line while the first plan is made."))
    }

    private func add(_ urls: [URL], asParents: Bool) async {
        // A folder whose access can't be kept is left out, and said.
        var paths: [String] = []
        for picked in urls.map(Picked.init(fromFiles:)) {
            if await model.keep(picked) {
                paths.append(picked.path)
            } else {
                problem = .app(AppModel.accessNotKept(picked.name))
            }
        }
        if asParents {
            for path in paths { step { $0.addParent(parent: path) } }
        } else {
            step { $0.addFolders(folders: paths) }
        }
    }

    // MARK: Planning

    private var parentsKey: String { "\(now.parents) \(now.deep)" }

    /// The engine ignores a listing at another depth or of a parent gone.
    private func findChildren() async {
        let (parents, deep) = (now.parents, now.deep)
        // Their scopes, held while they are listed.
        let lease = await model.access(parents)
        defer { withExtendedLifetime(lease) {} }
        // A newer scan (Include Deeper Folders flipped back) cancels this
        // one, and wins.
        for parent in parents {
            guard let found = try? await Request().run({
                try await foldersInside(request: $0, parent: parent, deep: deep)
            }), !Task.isCancelled else { return }
            step { $0.listed(parent: parent, deep: deep, found: found) }
        }
    }

    private var planKey: String {
        "\(entries.map(\.folder)) \(useList) \(split) \(model.list.files) \(model.list.excluded) \(model.stackSettings.rawDecoder)"
    }

    private func replan() async {
        var inputs = entries.map { BatchInput.folder(path: $0.folder) }
        if useList, listUsable {
            // The engine names the list's stacks with this.
            let label = String(
                localized: "The list",
                comment: "New Batch: the name a batch gives the stacks made from the frames in the list, alone or before a folder's name ('The list · bee').")
            inputs.insert(.list(list: model.list, label: label), at: 0)
        }
        guard !inputs.isEmpty else {
            planning = .none
            return
        }
        planning = .planning(last: planning.shown)
        let (split, source, decoder) = (split, model.list.source, model.stackSettings.rawDecoder)
        // The folders' scopes (and the list's), held while they are read.
        let lease = await model.access(entries.map(\.folder) + (useList ? model.list.files : []))
        guard !Task.isCancelled else { return }
        defer { withExtendedLifetime(lease) {} }
        // A newer key cancels this task and its plan.
        let made: BatchPlan
        do {
            made = try await Request().run {
                try await planBatch(
                    request: $0, inputs: inputs, split: split, source: source, rawDecoder: decoder, words: AppWords.words)
            }
        } catch {
            guard !Task.isCancelled, !(error is CancellationError) else { return }
            if case .Canceled? = error as? EngineError { return }
            // Said in the summary, not left on its spinner.
            planning = .failed(AppModel.message(error))
            return
        }
        guard !Task.isCancelled else { return }
        planning = .planned(made)
    }

    private func start() {
        guard case .planned(let plan) = planning else { return }
        dismiss()
        model.setPanel(.closed)
        Task { await model.startBatch(plan) }
    }
}
