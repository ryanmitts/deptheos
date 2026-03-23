import SwiftUI
import UniformTypeIdentifiers
import DeptheosKit

struct SettingsView: View {
    @Environment(AppSettings.self) private var settings
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                #if os(macOS)
                DngConverterSection(settings: settings)
                #endif

                DcpProfileSection(settings: settings)

                AlignmentSection(settings: settings)
            }
            .formStyle(.grouped)
            .navigationTitle("Settings")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

// MARK: - Adobe DNG Converter Path

#if os(macOS)
private struct DngConverterSection: View {
    @Bindable var settings: AppSettings
    @State private var showBrowse = false

    var body: some View {
        Section {
            LabeledContent {
                HStack {
                    TextField("Executable Path", text: $settings.dngConverterPath)
                        .textFieldStyle(.roundedBorder)

                    statusIndicator(isValid: settings.isDngConverterAvailable)
                }
            } label: {
                Text("Path")
            }

            HStack {
                Button("Browse...") {
                    showBrowse = true
                }

                Button("Reset to Default") {
                    settings.dngConverterPath = DNGConverter.defaultExecutablePath
                }
            }
        } header: {
            Text("Adobe DNG Converter")
        } footer: {
            Text("Path to the Adobe DNG Converter executable. Used when the RAW backend is set to Adobe DNG Converter.")
        }
        .fileImporter(isPresented: $showBrowse, allowedContentTypes: [.executable, .unixExecutable]) { result in
            if case .success(let url) = result {
                settings.dngConverterPath = url.path
            }
        }
    }
}
#endif

// MARK: - DCP Profile Directory

private struct DcpProfileSection: View {
    @Bindable var settings: AppSettings
    @State private var showBrowse = false

    var body: some View {
        Section {
            LabeledContent {
                HStack {
                    TextField("Directory Path", text: $settings.dcpProfileDirectory)
                        .textFieldStyle(.roundedBorder)

                    statusIndicator(isValid: settings.isDcpDirectoryValid)
                }
            } label: {
                Text("Path")
            }

            HStack {
                #if os(macOS)
                Button("Browse...") {
                    showBrowse = true
                }
                #endif

                Button("Reset to Default") {
                    settings.dcpProfileDirectory = AppSettings.defaultDcpProfileDirectory
                }
            }
        } header: {
            Text("DCP Profile Directory")
        } footer: {
            Text("Directory containing DCP color profiles for camera RAW processing. Profiles are matched by camera model name. Defaults to Adobe Standard profiles.")
        }
        #if os(macOS)
        .fileImporter(isPresented: $showBrowse, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result {
                settings.dcpProfileDirectory = url.path
            }
        }
        #endif
    }
}

// MARK: - Alignment

private struct AlignmentSection: View {
    @Bindable var settings: AppSettings

    var body: some View {
        Section {
            Toggle("Align Frames", isOn: $settings.alignmentEnabled)

            Toggle("Auto Crop", isOn: $settings.autoCrop)
        } header: {
            Text("Alignment")
        } footer: {
            Text("ECC alignment corrects for slight shifts between frames. Auto crop removes black borders introduced by alignment.")
        }
    }
}

// MARK: - Status Indicator

private func statusIndicator(isValid: Bool) -> some View {
    Image(systemName: "circle.fill")
        .font(.system(size: 8))
        .foregroundStyle(isValid ? .green : .red)
}
