import SwiftUI
#if canImport(AppKit)
import AppKit
#endif
import DeptheosKit

struct ExportView: View {
    @Bindable var appState: AppState
    @State private var selectedFormat: ExportFormat = .tiff
    @State private var jpegQuality: Double = 0.95
    @State private var isExporting = false
    @State private var statusMessage: String = ""
    #if !os(macOS)
    @State private var exportedFileURL: URL?
    @State private var showShareSheet = false
    #endif

    private var isDngSource: Bool {
        appState.currentProject?.dngMetadata != nil
    }

    var body: some View {
        VStack(spacing: 16) {
            HStack {
                Text("Export").font(.headline)
                Spacer()
                Button {
                    appState.showExportSheet = false
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
            }

            Picker("Format", selection: $selectedFormat) {
                Text("TIFF").tag(ExportFormat.tiff)
                Text("PNG").tag(ExportFormat.png)
                Text("JPEG").tag(ExportFormat.jpeg)
                if isDngSource {
                    Text("DNG").tag(ExportFormat.dng)
                }
            }
            .pickerStyle(.segmented)

            if selectedFormat == .jpeg {
                HStack {
                    Text("Quality")
                    Slider(value: $jpegQuality, in: 0.5...1.0)
                    Text(String(format: "%.0f%%", jpegQuality * 100))
                        .monospacedDigit()
                        .frame(width: 40)
                }
            }

            if selectedFormat == .tiff || selectedFormat == .png {
                let depth = appState.currentProject?.sourceBitDepth ?? 8
                Text("\(depth)-bit output")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            HStack {
                Button("Cancel") {
                    appState.showExportSheet = false
                }

                Button {
                    performExport()
                } label: {
                    Label("Export...", systemImage: "square.and.arrow.down")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .disabled(isExporting || appState.currentProject == nil)
            }

            if !statusMessage.isEmpty {
                Text(statusMessage)
                    .font(.caption)
                    .foregroundStyle(statusMessage.starts(with: "Error") ? .red : .secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .transition(.opacity)
            }
        }
        .padding()
        .frame(width: 300)
        #if !os(macOS)
        .sheet(isPresented: $showShareSheet) {
            if let url = exportedFileURL {
                ShareSheet(url: url)
            }
        }
        #endif
    }

    private func performExport() {
        guard let project = appState.currentProject else { return }

        #if os(macOS)
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "result.\(selectedFormat.rawValue)"
        panel.message = "Export stacking result"

        guard panel.runModal() == .OK, let url = panel.url else { return }
        #else
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("result.\(selectedFormat.rawValue)")
        #endif

        isExporting = true
        statusMessage = "Exporting..."

        let format = selectedFormat
        let quality = jpegQuality
        let ctx = appState.metalContext

        Task {
            do {
                try await project.export(to: url, format: format, metalContext: ctx, quality: quality)
                #if os(macOS)
                statusMessage = "Exported!"
                try? await Task.sleep(for: .seconds(2))
                appState.showExportSheet = false
                #else
                exportedFileURL = url
                showShareSheet = true
                statusMessage = ""
                isExporting = false
                #endif
            } catch {
                statusMessage = "Error: \(error.localizedDescription)"
                isExporting = false
            }
        }
    }
}

#if !os(macOS)
import UIKit

struct ShareSheet: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [url], applicationActivities: nil)
    }

    func updateUIViewController(_ uiViewController: UIActivityViewController, context: Context) {}
}
#endif
