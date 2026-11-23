import SwiftUI
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Settings ▸ Storage's Delete Library…, as on the desktop: asks with what
/// goes and what it frees (the renders once all are measured, and Recently
/// Deleted) in the engine's words, then deletes (`AppModel.deleteLibrary`).
/// The Imports copies it leaves are Storage's unused copies, below. The
/// footer says what it deletes, or why not now (`AppModel.libraryDeletable`).
struct DeleteLibrarySection: View {
    @Environment(AppModel.self) private var model
    @State private var deleting = DeleteLibrary.idle {
        didSet { stateLog.debug("delete library: \(deleting.name, privacy: .public)") }
    }

    /// Lifecycle: idle, asking (the engine's question), deleting or failed
    /// (why). Held in `deleting`, changed only by its button, alert and the
    /// delete they start.
    ///
    ///     idle ──Delete Library…──▶ asking(question)
    ///     asking ──Cancel──▶ idle
    ///     asking ──Delete Library (in the tap itself)──▶ deleting ──all gone──▶ idle
    ///     deleting ──part stayed──▶ failed(message) ──OK──▶ idle
    private enum DeleteLibrary {
        case idle
        case asking(Question)
        case deleting
        case failed(ErrorMessage)

        var name: String {
            switch self {
            case .idle: "idle"
            case .asking: "asking"
            case .deleting: "deleting"
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
        Section {
            Button(role: .destructive) {
                ask()
            } label: {
                HStack {
                    Text(
                        "Delete Library…",
                        comment: "Settings ▸ Storage: the button that asks before deleting every stack and render in the Library.")
                    if busy {
                        Spacer()
                        ProgressView()
                    }
                }
            }
            .disabled(!canAsk)
            // On the button, so its popover points at it.
            .confirmationDialog(
                Text(verbatim: question?.title ?? ""), isPresented: asking, titleVisibility: .visible,
                presenting: question
            ) { question in
                Button(role: .destructive) {
                    delete()
                } label: {
                    Text(verbatim: question.button)
                }
            } message: { question in
                Text(verbatim: question.message)
            }
        } header: {
            Text("Library", comment: "Settings ▸ Storage: the header of the section with Delete Library….")
        } footer: {
            Text(footer)
        }
        // Automation: `-AskDeleteLibrary YES` asks, as the button does.
        .automation(Automation.flag("AskDeleteLibrary"), after: .seconds(2)) {
            ask()
        }
        .alert(
            Text(failure?.title ?? .verbatim("")), isPresented: failed, presenting: failure
        ) { _ in
            Button(LocalizedStringResource("OK", comment: "Button that closes an error alert.")) {}
        } message: { failure in
            Text(failure.text)
        }
    }

    private var busy: Bool {
        if case .deleting = deleting { true } else { false }
    }

    private var canAsk: Bool {
        guard case .idle = deleting, case .ready = model.libraryDeletable else { return false }
        return true
    }

    private var question: Question? {
        if case .asking(let question) = deleting { question } else { nil }
    }

    private var failure: ErrorMessage? {
        if case .failed(let message) = deleting { message } else { nil }
    }

    /// Open while asking; Cancel (or a tap outside) closes it.
    private var asking: Binding<Bool> {
        Binding {
            question != nil
        } set: { open in
            if !open, question != nil { deleting = .idle }
        }
    }

    private var failed: Binding<Bool> {
        Binding {
            failure != nil
        } set: { open in
            if !open, failure != nil { deleting = .idle }
        }
    }

    /// What Delete Library… deletes, or why it can't now.
    private var footer: LocalizedStringResource {
        switch model.libraryDeletable {
        case .unread:
            LocalizedStringResource(
                "The Library is still being read.",
                comment: "Settings ▸ Storage: the footer under Delete Library… while the Library hasn't been read yet.")
        case .empty:
            LocalizedStringResource(
                "The Library is empty.",
                comment: "Settings ▸ Storage: the footer under Delete Library… when there is nothing to delete.")
        case .busy:
            LocalizedStringResource(
                "Available once nothing is stacking, opening or being retouched.",
                comment: "Settings ▸ Storage: the footer under Delete Library… while a stack, batch, open or Retouch holds the app.")
        case .ready:
            LocalizedStringResource(
                "Deletes every stack, render and failed stack, with their retouching, the working version and everything in Recently Deleted, for good, leaving the Library empty. Your frames and exported files are left alone; copies of frames that nothing uses then are unused copies, below.",
                comment: "Settings ▸ Storage: the footer under Delete Library…, on what it deletes and leaves.")
        }
    }

    /// With what the Library holds and frees: its renders once all are
    /// measured, and Recently Deleted, which goes too.
    private func ask() {
        guard canAsk, case .ready(let stacks, let renders) = model.libraryDeletable else { return }
        let frees = model.history.sizeBytes(model.history.renders.map(\.id))
            .map { AppModel.size(Int64(clamping: $0 + model.history.deletedBytes)) }
        let asked = AppWords.words.deleteLibraryQuestion(
            stacks: UInt32(clamping: stacks), renders: UInt32(clamping: renders), frees: frees)
        deleting = .asking(Question(title: asked.title, message: asked.message, button: asked.button))
    }

    /// The delete starts in the tap, not a task after it: the dialog's
    /// binding returns to idle as it closes, so a later task would find no
    /// question. Either way, it runs once.
    private func delete() {
        switch deleting {
        case .asking, .idle: break
        case .deleting, .failed: return
        }
        deleting = .deleting
        Task {
            if let failure = await model.deleteLibrary() {
                deleting = .failed(failure)
            } else {
                deleting = .idle
            }
        }
    }
}
