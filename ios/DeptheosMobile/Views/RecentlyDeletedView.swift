import SwiftUI
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Recently Deleted (ADR-140), pushed from the Library: what each delete
/// took, newest first, kept 30 days, with thumbnail, name, days left and
/// size. Restore puts it back whole; Empty Recently Deleted… asks, then
/// deletes everything for good. The rows are `HistoryModel.deleted`,
/// reread each time the list opens.
struct RecentlyDeletedView: View {
    static let title = LocalizedStringResource(
        "Recently Deleted",
        comment: "The Library's row (iPad: its sidebar's) that opens the list of stacks and renders deleted in the last 30 days, and that list's title.")
    static let symbol = "trash"

    @Environment(AppModel.self) private var model
    @Environment(\.libraryLocked) private var locked
    @State private var state = ListState.browsing {
        didSet { stateLog.debug("recently deleted: \(oldValue.name, privacy: .public) → \(state.name, privacy: .public)") }
    }

    /// Lifecycle: browsing, selecting (the ids), restoring (the ids),
    /// asking (Empty's question), emptying or failed (why). Held in
    /// `state`, changed only by its buttons and the writes they start.
    ///
    ///     browsing ──Select──▶ selecting(ids) ──Done──▶ browsing
    ///     browsing, selecting ──Restore──▶ restoring(ids) ──restored──▶ browsing
    ///     browsing ──Empty Recently Deleted…──▶ asking(question) ──Cancel──▶ browsing
    ///     asking ──Empty (in the tap itself)──▶ emptying ──emptied──▶ browsing
    ///     restoring, emptying ──part stayed──▶ failed(message) ──OK──▶ browsing
    ///
    /// Restore and Empty are Library writes, never canceled: leaving the
    /// list doesn't stop them, and their result lands nowhere.
    private enum ListState {
        case browsing
        case selecting(Set<String>)
        case restoring(Set<String>)
        case asking(Question)
        case emptying
        case failed(ErrorMessage)

        var name: String {
            switch self {
            case .browsing: "browsing"
            case .selecting(let ids): "selecting \(ids.count)"
            case .restoring(let ids): "restoring \(ids.count)"
            case .asking: "asking"
            case .emptying: "emptying"
            case .failed: "failed"
            }
        }
    }

    /// In the engine's words.
    private struct Question {
        let title: String
        let message: String
        let button: String
    }

    var body: some View {
        let items = model.history.deleted
        List(selection: selection) {
            if !items.isEmpty {
                Section {
                    ForEach(items, id: \.id) { item in
                        DeletedRow(item: item, restoring: restoring.contains(item.id), selecting: selecting) {
                            restore([item.id])
                        }
                        .disabled(locked || !idle)
                        .swipeActions(edge: .leading) {
                            Button(Self.restore, systemImage: "arrow.uturn.backward") { restore([item.id]) }
                                .tint(.accentColor)
                                .disabled(locked || !idle)
                        }
                        .contextMenu {
                            Button(Self.restore, systemImage: "arrow.uturn.backward") { restore([item.id]) }
                                .disabled(locked || !idle)
                        }
                    }
                } header: {
                    // The engine's words: "4 items · 1.2 GB".
                    Text(verbatim: summary)
                } footer: {
                    Text(
                        "Stacks and renders stay here for 30 days, then are deleted for good. Restore puts one back with its renders and retouching.",
                        comment: "Recently Deleted: the footer under its list, on how long things stay and what Restore does.")
                }
                Section {
                    Button(role: .destructive) {
                        ask()
                    } label: {
                        HStack {
                            Text(
                                "Empty Recently Deleted…",
                                comment: "Recently Deleted: the button that asks before deleting everything in the list for good.")
                            if case .emptying = state {
                                Spacer()
                                ProgressView()
                            }
                        }
                    }
                    .disabled(locked || !idle)
                    // On the button, so its popover points at it.
                    .confirmationDialog(
                        Text(verbatim: question?.title ?? ""), isPresented: asking, titleVisibility: .visible,
                        presenting: question
                    ) { question in
                        Button(role: .destructive) {
                            empty()
                        } label: {
                            Text(verbatim: question.button)
                        }
                    } message: { question in
                        Text(verbatim: question.message)
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.editMode, editMode)
        .navigationTitle(Self.title)
        .navigationBarTitleDisplayMode(.inline)
        .overlay {
            if items.isEmpty {
                ContentUnavailableView {
                    Label(Self.title, systemImage: Self.symbol)
                } description: {
                    // The engine's words: nothing was deleted lately.
                    Text(verbatim: summary)
                }
            }
        }
        .toolbar {
            if !items.isEmpty {
                ToolbarItem(placement: .primaryAction) {
                    if case .selecting = state {
                        Button(
                            LocalizedStringResource("Done", comment: "Recently Deleted's bar while selecting: stops selecting."),
                            role: .confirm
                        ) {
                            state = .browsing
                        }
                    } else {
                        Button(LocalizedStringResource("Select", comment: "Recently Deleted's bar: selects rows to restore several at once.")) {
                            state = .selecting([])
                        }
                        .disabled(locked || !idle)
                    }
                }
            }
        }
        .safeAreaBar(edge: .bottom) {
            if case .selecting(let ids) = state {
                Button(
                    LocalizedStringResource(
                        "Restore \(ids.count) Items",
                        comment: "Recently Deleted's bottom button while selecting: puts the rows selected back. The placeholder is how many."),
                    systemImage: "arrow.uturn.backward"
                ) {
                    restore(Array(ids))
                }
                .buttonStyle(.glassProminent)
                .disabled(ids.isEmpty || locked)
                .padding(.bottom, 8)
            }
        }
        .alert(
            Text(failure?.title ?? .verbatim("")), isPresented: failed, presenting: failure
        ) { _ in
            Button(LocalizedStringResource("OK", comment: "Button that closes an error alert.")) {}
        } message: { failure in
            Text(failure.text)
        }
        // Days left move on, and the engine removes what passed 30 days as
        // it opens: reread each time.
        .task { await model.history.refreshDeleted() }
        .onChange(of: items.map(\.id)) { _, ids in
            if case .selecting(let selected) = state { state = .selecting(selected.intersection(ids)) }
            if ids.isEmpty, case .selecting = state { state = .browsing }
        }
    }

    static let restore = LocalizedStringResource(
        "Restore", comment: "Recently Deleted: a row's button, swipe action and menu item that put it back in the Library.")

    /// "4 items · 1.2 GB", or that nothing was deleted lately.
    private var summary: String {
        let items = model.history.deleted
        return AppWords.words.deletedSummary(
            items: UInt32(clamping: items.count), size: AppModel.size(Int64(clamping: model.history.deletedBytes)))
    }

    private var idle: Bool {
        switch state {
        case .browsing, .selecting: true
        case .restoring, .asking, .emptying, .failed: false
        }
    }

    private var selecting: Bool {
        if case .selecting = state { true } else { false }
    }

    private var restoring: Set<String> {
        if case .restoring(let ids) = state { ids } else { [] }
    }

    private var question: Question? {
        if case .asking(let question) = state { question } else { nil }
    }

    private var failure: ErrorMessage? {
        if case .failed(let message) = state { message } else { nil }
    }

    private var selection: Binding<Set<String>> {
        Binding {
            if case .selecting(let ids) = state { ids } else { [] }
        } set: { ids in
            if case .selecting = state { state = .selecting(ids) }
        }
    }

    private var editMode: Binding<EditMode> {
        Binding {
            selecting ? .active : .inactive
        } set: { mode in
            if !mode.isEditing, selecting { state = .browsing }
        }
    }

    /// Open while asking; Cancel (or a tap outside) closes it.
    private var asking: Binding<Bool> {
        Binding {
            question != nil
        } set: { open in
            if !open, question != nil { state = .browsing }
        }
    }

    private var failed: Binding<Bool> {
        Binding {
            failure != nil
        } set: { open in
            if !open, failure != nil { state = .browsing }
        }
    }

    /// Then browse, or say what stayed.
    private func restore(_ ids: [String]) {
        guard idle, !ids.isEmpty else { return }
        state = .restoring(Set(ids))
        Task {
            let failure = await model.restoreDeleted(ids)
            guard case .restoring = state else { return }
            state = failure.map(ListState.failed) ?? .browsing
        }
    }

    /// With the row count and their total size.
    private func ask() {
        guard idle else { return }
        let items = model.history.deleted
        guard !items.isEmpty else { return }
        let asked = AppWords.words.emptyDeletedQuestion(
            items: UInt32(clamping: items.count), frees: AppModel.size(Int64(clamping: model.history.deletedBytes)))
        state = .asking(Question(title: asked.title, message: asked.message, button: asked.button))
    }

    /// Empty starts in the tap, as `DeleteLibrarySection.delete` does, so
    /// the dialog closing first can't leave it undone.
    private func empty() {
        switch state {
        case .asking, .browsing: break
        case .selecting, .restoring, .emptying, .failed: return
        }
        state = .emptying
        Task {
            let failure = await model.emptyDeleted()
            guard case .emptying = state else { return }
            state = failure.map(ListState.failed) ?? .browsing
        }
    }
}

/// Its thumbnail, "Bee #2", "29 days left · 120 MB", and Restore.
private struct DeletedRow: View {
    @Environment(AppModel.self) private var model
    let item: DeletedInfo
    let restoring: Bool
    let selecting: Bool
    let restore: () -> Void
    /// Its newest render's, once read.
    @State private var thumbnail: CGImage?

    var body: some View {
        HStack(spacing: 12) {
            Group {
                if let thumbnail {
                    Image(decorative: thumbnail, scale: 1).resizable().scaledToFill()
                } else {
                    Rectangle().fill(.quaternary)
                        .overlay {
                            Image(systemName: item.renders.isEmpty ? "xmark.circle" : "photo")
                                .foregroundStyle(.secondary)
                        }
                }
            }
            .frame(width: 72, height: 54)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                // The engine's words: "Bee #2", "3 renders of Bee".
                Text(verbatim: AppWords.words.deletedTitle(item: item, untitled: String(localized: AppModel.untitledStack)))
                    .font(.callout.weight(.medium))
                    .lineLimit(2)
                // The engine's "29 days left", then the size.
                Text(verbatim: UserText.line([
                    .verbatim(AppWords.words.deletedDaysLeft(item: item)),
                    .verbatim(AppModel.size(Int64(clamping: item.size))),
                ]).string)
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            if restoring {
                ProgressView()
            } else if !selecting {
                Button(RecentlyDeletedView.restore, action: restore)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
        .accessibilityAction(named: Text(RecentlyDeletedView.restore), restore)
        // Read while in view; scrolled away it is canceled (`Request.run`),
        // and a late one is dropped.
        .task(id: item.id) {
            guard thumbnail == nil, let engine = model.engine, !item.renders.isEmpty else { return }
            let id = item.id
            let image = try? await Request().run { try await engine.deletedThumbnail(request: $0, id: id) }
                .makeCGImage()
            guard !Task.isCancelled, id == item.id else { return }
            thumbnail = image
        }
    }
}

/// After a delete (`DeleteUndo`): the engine's "Bee #2 moved to Recently
/// Deleted." with Undo, at the foot of the Library and the Renders list.
struct DeletedNoticeBar: View {
    @Environment(AppModel.self) private var model

    static let undo = LocalizedStringResource(
        "Undo", comment: "Button of the notice after a delete: puts back what was just deleted.")

    var body: some View {
        if let offer = model.deleteUndo.offer {
            HStack(spacing: 12) {
                Image(systemName: RecentlyDeletedView.symbol)
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
                // The engine's words.
                Text(verbatim: offer.text)
                    .font(.footnote)
                    .lineLimit(3)
                Spacer(minLength: 8)
                if case .restoring = model.deleteUndo {
                    ProgressView()
                } else {
                    Button(Self.undo) {
                        Task { await model.undoDelete() }
                    }
                    .bold()
                    .disabled(model.busy)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(maxWidth: 520)
            .glassEffect(.regular, in: .rect(cornerRadius: 20))
            .padding(.horizontal)
            .padding(.bottom, 8)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .onAppear {
                AccessibilityNotification.Announcement(offer.text).post()
            }
            .id(offer.token)
        }
    }
}
