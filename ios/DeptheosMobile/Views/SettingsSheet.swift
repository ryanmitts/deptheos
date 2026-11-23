import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

/// Settings, from the Library's gear: the app's options as pages, a first
/// page of rows each opening one. Export and Alignment are preferences the
/// desktop shares, named and ranged by the engine (ADR-150); then Retouch's
/// Pencil options (iPad), Storage, Diagnostics and About. A preferences
/// page ends with Reset to Defaults. A stack's own settings are never here.
struct SettingsSheet: View {
    static let title = LocalizedStringResource(
        "Settings", comment: "The Library's gear button, and the title of the sheet it opens: the app's options.")

    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var path: [SettingsPage] = []

    var body: some View {
        NavigationStack(path: $path) {
            Form {
                Section {
                    row(.export)
                    row(.alignment)
                    if UIDevice.current.userInterfaceIdiom == .pad {
                        row(.retouch)
                    }
                }
                Section {
                    row(.storage)
                }
                Section {
                    row(.benchmark)
                    row(.diagnostics)
                    row(.about)
                }
            }
            .navigationDestination(for: SettingsPage.self) { page in
                destination(page)
            }
            // Automation: `-SettingsPage export`, `alignment`, `retouch`,
            // `storage`, `diagnostics`, `about`, or `folders` (Storage's).
            .automation(Automation.string("SettingsPage") != nil, after: .seconds(1)) {
                switch Automation.string("SettingsPage").flatMap(SettingsPage.init(rawValue:)) {
                case .folders: path = [.storage, .folders]
                case let page?: path = [page]
                case nil: break
                }
            }
            .navigationTitle(Text(Self.title))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(LocalizedStringResource("Done", comment: "Closes the Settings sheet.")) { dismiss() }
                }
            }
        }
        .singleItemSheetBar()
    }

    private func row(_ page: SettingsPage) -> some View {
        NavigationLink(value: page) {
            Label {
                Text(page.title)
            } icon: {
                Image(systemName: page.symbol)
            }
        }
    }

    @ViewBuilder
    private func destination(_ page: SettingsPage) -> some View {
        switch page {
        case .export:
            SettingsPageForm(page) {
                ExportSection(preferences: model.preferences)
                ResetSection(isDefault: model.preferences.export == defaultPreferences().export) {
                    model.preferences.export = defaultPreferences().export
                }
            }
        case .alignment:
            SettingsPageForm(page) {
                AlignmentSection(preferences: model.preferences)
                ResetSection(isDefault: model.preferences.alignment == defaultPreferences().alignment) {
                    model.preferences.alignment = defaultPreferences().alignment
                }
            }
        case .retouch:
            SettingsPageForm(page) {
                PencilSection(paint: model.paint)
                ResetSection(isDefault: model.paint.pencilOptionsAreDefault) {
                    model.paint.resetPencilOptions()
                }
            }
        case .storage:
            SettingsPageForm(page) {
                StorageSection()
            }
        case .benchmark:
            BenchmarkView()
        case .diagnostics:
            SettingsPageForm(page) {
                DiagnosticsSection()
            }
        case .about:
            AboutView()
        case .folders:
            FoldersView()
        }
    }
}

/// Pushed from the first page; `folders` is pushed from Storage.
enum SettingsPage: String, Hashable {
    case export, alignment, retouch, storage, benchmark, diagnostics, about, folders

    /// Its row's name, and its title.
    var title: LocalizedStringResource {
        switch self {
        case .export:
            LocalizedStringResource("Export", comment: "Settings: the page on how exports are written and named, its row and title.")
        case .alignment:
            LocalizedStringResource("Alignment", comment: "Settings: the page on Grid search's limits and exposure compensation, its row and title.")
        case .retouch:
            LocalizedStringResource("Retouch", comment: "Settings: the page on Apple Pencil in Retouch (iPad), its row and title.")
        case .storage:
            LocalizedStringResource("Storage", comment: "Settings: the page on the folders read where they are, the copies Deptheos keeps and Delete Library…, its row and title.")
        case .benchmark:
            LocalizedStringResource("Benchmark", comment: "Settings: the page that times focus stacking on this device, its row and title.")
        case .diagnostics:
            LocalizedStringResource("Diagnostics", comment: "Settings: the page with Verbose logging and Send Diagnostics…, its row and title.")
        case .about: AboutView.title
        case .folders: FoldersView.title
        }
    }

    var symbol: String {
        switch self {
        case .export: "square.and.arrow.up"
        case .alignment: "square.3.layers.3d"
        case .retouch: "applepencil"
        case .storage: "internaldrive"
        case .benchmark: "gauge.with.dots.needle.67percent"
        case .diagnostics: "stethoscope"
        case .about: "info.circle"
        case .folders: "folder"
        }
    }
}

private struct SettingsPageForm<Content: View>: View {
    let page: SettingsPage
    @ViewBuilder let content: Content

    init(_ page: SettingsPage, @ViewBuilder content: () -> Content) {
        self.page = page
        self.content = content()
    }

    var body: some View {
        Form { content }
            .navigationTitle(Text(page.title))
            .navigationBarTitleDisplayMode(.inline)
    }
}

/// Reset to Defaults for the page's preferences, once one differs.
private struct ResetSection: View {
    let isDefault: Bool
    let reset: () -> Void

    var body: some View {
        Section {
            Button(
                LocalizedStringResource(
                    "Reset to Defaults",
                    comment: "Settings: the button at the end of a page that sets every option on it back to its default."),
                action: reset
            )
            .disabled(isDefault)
        }
    }
}

/// The engine's words for a shared preference.
private func preferenceText(_ preference: Preference) -> String {
    AppWords.words.preferenceLabel(preference: preference)
}

private func preferenceHelpText(_ preference: Preference) -> String {
    AppWords.words.preferenceHelp(preference: preference)
}

/// How exports are written and named, as the desktop's Export pane; the
/// Export sheet starts from them.
private struct ExportSection: View {
    @Bindable var preferences: AppPreferences

    var body: some View {
        Section {
            Toggle(isOn: $preferences.export.sixteenBit) {
                Text(
                    "16 bits per channel",
                    comment: "Export sheet and Settings ▸ Export switch for PNG and TIFF: write 16 bits per color channel rather than 8.")
                Text(
                    "For PNG and TIFF. Off writes 8 bits per channel, in smaller files.",
                    comment: "Settings ▸ Export: the help under the 16 bits per channel switch.")
            }
            // The help under the row, as a slider's: a menu picker's subtitle
            // would push its value onto its own line.
            VStack(alignment: .leading, spacing: 4) {
                Picker(selection: $preferences.export.tiffCompression) {
                    ForEach(TiffCompression.allCases, id: \.self) { compression in
                        // The engine's name for it.
                        Text(verbatim: AppWords.words.tiffCompressionLabel(compression: compression))
                            .tag(compression)
                    }
                } label: {
                    Text(verbatim: preferenceText(.tiffCompression))
                }
                Text(verbatim: preferenceHelpText(.tiffCompression))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            PreferenceSlider(
                preference: .jpegQuality,
                value: Binding(
                    get: { Float(preferences.export.jpegQuality) },
                    set: { preferences.export.jpegQuality = UInt8($0.rounded()) }),
                help: true)
        } header: {
            Text(
                "Defaults for new exports",
                comment: "Settings ▸ Export: the header over 16-bit, TIFF compression and JPEG quality, which the Export sheet starts from.")
        }
        Section {
            LabeledContent {
                TextField(
                    text: $preferences.export.fileNamePattern,
                    prompt: Text(verbatim: defaultPreferences().export.fileNamePattern)
                ) {
                    Text(verbatim: preferenceText(.fileName))
                }
                .multilineTextAlignment(.trailing)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .font(.body.monospaced())
            } label: {
                Text(verbatim: preferenceText(.fileName))
            }
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: preferenceHelpText(.fileName))
                // "For example: bee-4.jpg", live as the pattern is typed.
                Text(verbatim: AppWords.words.exportFileNameExample(
                    export: preferences.export, format: .jpeg, date: exportDate()))
            }
        }
    }
}

/// Grid search's reach and exposure compensation, for every stack and batch
/// aligning with it.
private struct AlignmentSection: View {
    @Bindable var preferences: AppPreferences

    var body: some View {
        Section {
            PreferenceSlider(preference: .gridHorizontal, value: $preferences.alignment.gridHorizontalPct, help: true)
            PreferenceSlider(preference: .gridVertical, value: $preferences.alignment.gridVerticalPct, help: true)
            PreferenceSlider(preference: .gridRotation, value: $preferences.alignment.gridRotationDeg, help: true)
            PreferenceSlider(
                preference: .gridMagnification, value: $preferences.alignment.gridMagnificationPct, help: true)
        } header: {
            Text(verbatim: AppWords.words.gridLimitsLabel())
        } footer: {
            Text(verbatim: AppWords.words.gridLimitsHelp())
        }
        Section {
            Toggle(isOn: $preferences.alignment.exposureCompensation) {
                Text(verbatim: preferenceText(.exposureCompensation))
                Text(verbatim: preferenceHelpText(.exposureCompensation))
            }
        }
    }
}

/// Its name and value over a slider in the engine's range and step; with
/// `help`, the engine's line under it.
private struct PreferenceSlider: View {
    let preference: Preference
    @Binding var value: Float
    var help = false

    var body: some View {
        if let range = preferenceRange(preference: preference) {
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(verbatim: preferenceText(preference))
                    Spacer()
                    Text(verbatim: shown(range))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                Slider(value: $value, in: range.min...range.max, step: range.step)
                    .accessibilityLabel(Text(verbatim: preferenceText(preference)))
                    .accessibilityValue(Text(verbatim: shown(range)))
                if help {
                    Text(verbatim: AppWords.words.preferenceHelp(preference: preference))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    /// As the step shows it, in the user's locale: "95", "2.5".
    private func shown(_ range: SettingRange) -> String {
        let digits = range.step >= 1 ? 0 : (range.step >= 0.1 ? 1 : 2)
        return value.formatted(.number.precision(.fractionLength(digits)).grouping(.never))
    }
}

/// On iPad: whether a finger paints too, and what an Apple Pencil Pro
/// squeeze does, whenever Retouch is open.
private struct PencilSection: View {
    @Bindable var paint: PaintModel

    var body: some View {
        Section {
            OnlyPencilToggle(paint: paint)
            Picker(
                LocalizedStringResource("Squeeze", comment: "Settings ▸ Retouch: the picker of what squeezing Apple Pencil Pro does."),
                selection: $paint.squeezeAction
            ) {
                ForEach(PaintModel.SqueezeAction.allCases) { action in
                    Text(action.label).tag(action)
                }
            }
        } footer: {
            Text(paint.onlyPencilPaints
                 ? LocalizedStringResource(
                    "Fingers pan and zoom; touch and hold shows the source. Pencil pressure sets how much each stroke paints.",
                    comment: "Settings ▸ Retouch: footer under the Apple Pencil options, Only Pencil paints on.")
                 : LocalizedStringResource(
                    "A finger paints too; two fingers pan and zoom. Pencil pressure sets how much each stroke paints.",
                    comment: "Settings ▸ Retouch: footer under the Apple Pencil options, Only Pencil paints off."))
        }
    }
}
