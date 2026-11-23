import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// About Deptheos: version, license, third-party notices and source. The
/// texts are the repository's files, bundled through Licenses/'s symlinks.
struct AboutView: View {
    static let title = LocalizedStringResource(
        "About Deptheos", comment: "Settings: the row that opens the app's version, license and notices, and that page's title.")

    private var version: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        return "\(short) (\(build))"
    }

    var body: some View {
        List {
            Section {
                LabeledContent {
                    Text(verbatim: version)
                } label: {
                    Text("Version", comment: "About Deptheos: the row showing the app's version and build.")
                }
            } footer: {
                Text(
                    "Deptheos is free software under the GNU General Public License version 3, with an additional permission for the Adobe DNG SDK and XMP Toolkit. It includes software by others under their own licenses, listed in the third-party notices.",
                    comment: "About Deptheos: the footer under the version, on the app's license.")
            }
            Section {
                NavigationLink {
                    BundledText(title: Self.license, resource: "LICENSE", ext: "md")
                } label: {
                    Text(Self.license)
                }
                NavigationLink {
                    BundledText(title: Self.notices, resource: "THIRD_PARTY_NOTICES", ext: "txt")
                } label: {
                    Text(Self.notices)
                }
                Link(destination: URL(string: "https://github.com/ryanmitts/deptheos")!) {
                    Text("Source Code", comment: "About Deptheos: the link to the app's source code on GitHub.")
                }
                Link(destination: URL(string: "https://deptheos.com/privacy/")!) {
                    Text("Privacy Policy", comment: "About Deptheos: the link to the privacy policy on the website.")
                }
            }
        }
        .navigationTitle(Text(Self.title))
        .navigationBarTitleDisplayMode(.inline)
    }

    private static let license = LocalizedStringResource(
        "License", comment: "About Deptheos: the row that opens the app's license, and that page's title.")
    private static let notices = LocalizedStringResource(
        "Third-Party Notices",
        comment: "About Deptheos: the row that opens the licenses of the software Deptheos includes, and that page's title.")
}

/// Settings ▸ Diagnostics: Verbose logging, and Send Diagnostics…, which
/// zips the log and shares it.
struct DiagnosticsSection: View {
    /// Not kept: every launch starts without it.
    @State private var verbose = verboseLogging()
    @State private var diagnostics = Diagnostics.idle {
        didSet { stateLog.debug("diagnostics: \(diagnostics.name, privacy: .public)") }
    }

    /// Lifecycle: idle, writing, sharing (the zip) or failed (why). Held in
    /// `diagnostics`, changed only by Send Diagnostics… and the share sheet
    /// closing.
    ///
    ///     idle ──Send Diagnostics…──▶ writing ──written──▶ sharing(file)
    ///     writing ──failed──▶ failed(message) ──OK──▶ idle
    ///     sharing ──shared or not──▶ idle (the file deleted)
    private enum Diagnostics {
        case idle
        case writing
        case sharing(URL)
        case failed(UserText)

        var name: String {
            switch self {
            case .idle: "idle"
            case .writing: "writing"
            case .sharing: "sharing"
            case .failed: "failed"
            }
        }
    }

    var body: some View {
        Section {
            Toggle(isOn: $verbose) {
                Text("Verbose logging", comment: "Settings ▸ Diagnostics: the switch that adds detail to the log for bug reports.")
            }
            .onChange(of: verbose) { _, on in setVerboseLogging(on: on) }
            Button {
                Task { await writeDiagnostics() }
            } label: {
                HStack {
                    Text(
                        "Send Diagnostics…",
                        comment: "Settings ▸ Diagnostics: the button that zips the log and opens the share sheet.")
                    if case .writing = diagnostics {
                        Spacer()
                        ProgressView()
                    }
                }
            }
            .disabled(!isIdle)
            .background(ShareSheet(file: sharingFile) { sharing.wrappedValue = false })
            #if DEBUG
            CrashOnPurposeMenu()
            #endif
        } footer: {
            Text(
                "Verbose logging adds detail to the log until Deptheos quits. Send Diagnostics… shares the log of Deptheos's recent launches, as a file to email to support@deptheos.com.",
                comment: "Settings ▸ Diagnostics: the footer under Verbose logging and Send Diagnostics….")
        }
        .alert(
            Text(
                "The diagnostics file couldn't be made",
                comment: "Alert title when Send Diagnostics… couldn't zip the log."),
            isPresented: failed, presenting: failure
        ) { _ in
            Button(LocalizedStringResource("OK", comment: "Button that closes an error alert.")) {}
        } message: { message in
            Text(message)
        }
    }

    private var isIdle: Bool {
        if case .idle = diagnostics { true } else { false }
    }

    private var sharingFile: URL? {
        if case .sharing(let file) = diagnostics { file } else { nil }
    }

    private var failure: UserText? {
        if case .failed(let message) = diagnostics { message } else { nil }
    }

    /// Open while sharing; closing it deletes the file.
    private var sharing: Binding<Bool> {
        Binding {
            if case .sharing = diagnostics { true } else { false }
        } set: { open in
            guard !open, case .sharing(let file) = diagnostics else { return }
            try? FileManager.default.removeItem(at: file.deletingLastPathComponent())
            diagnostics = .idle
        }
    }

    private var failed: Binding<Bool> {
        Binding {
            failure != nil
        } set: { open in
            if !open, failure != nil { diagnostics = .idle }
        }
    }

    /// Into a temporary `Deptheos Diagnostics <date>.zip`, on the engine's
    /// threads.
    private func writeDiagnostics() async {
        guard isIdle else { return }
        diagnostics = .writing
        let name = "Deptheos Diagnostics \(exportDate()).zip"
        do {
            let folder = try await Blocking.run { try AppFiles.newTemporaryFolder(prefix: "Diagnostics") }
            let file = folder.appending(path: name)
            do {
                try await Request().run {
                    try await DeptheosEngine.writeDiagnostics(request: $0, path: file.path, words: AppWords.words)
                }
            } catch {
                try? FileManager.default.removeItem(at: folder)
                throw error
            }
            diagnostics = .sharing(file)
        } catch {
            diagnostics = .failed(AppModel.message(error))
        }
    }
}

#if DEBUG
/// Crash on Purpose, Debug builds only: each way the engine can crash
/// (`crash_on_purpose`), to test the log, the next launch's alert and the
/// crash report (`docs/crash-testing.md`). For developers, so not in the
/// String Catalog.
private struct CrashOnPurposeMenu: View {
    private static let kinds: [(CrashKind, String)] = [
        (.panic, "Rust Panic"),
        (.threadPanic, "Rust Panic on an Engine Thread"),
        (.abort, "Abort"),
        (.fault, "Bad Access"),
    ]

    var body: some View {
        Menu {
            ForEach(Self.kinds, id: \.1) { kind, name in
                Button(role: .destructive) {
                    stateLog.warning("crashing on purpose: \(name, privacy: .public)")
                    crashOnPurpose(kind: kind)
                } label: {
                    Text(verbatim: name)
                }
            }
        } label: {
            Text(verbatim: "Crash on Purpose")
        }
    }
}
#endif

/// The share sheet for `file` while it is set, presented by UIKit from the
/// view this backs (a popover pointing at it on iPad); `done` when it
/// closes, shared or not. As a SwiftUI sheet's content, the share sheet
/// dismissed itself and the sheet under it.
struct ShareSheet: UIViewControllerRepresentable {
    let file: URL?
    let done: () -> Void

    func makeUIViewController(context: Context) -> Anchor { Anchor() }

    func updateUIViewController(_ anchor: Anchor, context: Context) {
        anchor.done = done
        anchor.show(file)
    }

    final class Anchor: UIViewController {
        var done: () -> Void = {}
        private var shown: URL?

        func show(_ file: URL?) {
            guard file != shown else { return }
            if shown != nil, presentedViewController is UIActivityViewController {
                dismiss(animated: true)
            }
            shown = file
            guard let file else { return }
            let controller = UIActivityViewController(activityItems: [file], applicationActivities: nil)
            controller.popoverPresentationController?.sourceView = view
            controller.popoverPresentationController?.sourceRect = view.bounds
            controller.completionWithItemsHandler = { [weak self] _, _, _, _ in
                guard let self, shown == file else { return }
                shown = nil
                done()
            }
            present(controller, animated: true)
        }
    }
}

/// The notices run to about a megabyte, which a UITextView scrolls smoothly
/// and a SwiftUI Text does not.
private struct BundledText: View {
    let title: LocalizedStringResource
    let resource: String
    let ext: String

    var body: some View {
        PlainTextView(text: text)
            .ignoresSafeArea(edges: .bottom)
            .navigationTitle(Text(title))
            .navigationBarTitleDisplayMode(.inline)
    }

    private var text: String {
        guard let url = Bundle.main.url(forResource: resource, withExtension: ext),
            let text = try? String(contentsOf: url, encoding: .utf8)
        else {
            return String(
                localized: "This copy of Deptheos is missing \"\(resource).\(ext)\". Reinstall Deptheos to read it.",
                comment: "Shown in place of the license or the notices when the app's copy of the file is missing. The placeholders are the file's name and its extension, such as LICENSE and md.")
        }
        return text
    }
}

private struct PlainTextView: UIViewRepresentable {
    let text: String

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false
        view.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        view.textContainerInset = UIEdgeInsets(top: 12, left: 8, bottom: 12, right: 8)
        view.text = text
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {}
}
