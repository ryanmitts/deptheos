import SwiftUI
import DeptheosKit

struct StackingSettingsView: View {
    @Bindable var appState: AppState
    @Environment(AppSettings.self) private var settings

    private var ws: Bindable<WorkspaceState> { Bindable(appState.workspace) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Algorithm
            VStack(alignment: .leading, spacing: 6) {
                Text("Algorithm").font(.caption).foregroundStyle(.secondary)
                Picker("Algorithm", selection: ws.selectedAlgorithm) {
                    ForEach(AlgorithmChoice.allCases) { choice in
                        Text(choice.rawValue).tag(choice)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()

                AlgorithmSettingsView(ws: ws)
            }

            // RAW Backend (only for camera RAW images, not DNG)
            if appState.workspace.hasCameraRAWImages {
                DisclosureGroup("RAW Backend") {
                    Picker("RAW Backend", selection: rawBackendBinding) {
                        Text("LibRaw").tag(RawBackendChoice.libRaw)
                        Text("Apple").tag(RawBackendChoice.apple)
                        #if os(macOS)
                        if DNGConverter.isAvailable {
                            Text("Adobe DNG Converter").tag(RawBackendChoice.adobeDNG)
                        }
                        #endif
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }

            // Stack / Cancel
            if appState.stacking.coordinator.isStacking {
                StackingProgressView(appState: appState, settings: settings)
            } else {
                HStack {
                    Button {
                        appState.workspace.resetSettingsToDefaults()
                    } label: {
                        Text("Reset")
                    }

                    Button {
                        appState.stacking.startStacking(appSettings: settings)
                    } label: {
                        Text("Stack")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(appState.workspace.includedURLs.count < 2)
                }

                if let error = appState.stacking.coordinator.error {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            }
        }
    }

    private var rawBackendBinding: Binding<RawBackendChoice> {
        Binding(
            get: {
                switch appState.workspace.cameraRAWBackend {
                case .libRaw: .libRaw
                case .apple: .apple
                case .adobeDNGConverter: .adobeDNG
                }
            },
            set: {
                switch $0 {
                case .libRaw: appState.workspace.cameraRAWBackend = .libRaw
                case .apple: appState.workspace.cameraRAWBackend = .apple
                case .adobeDNG: appState.workspace.cameraRAWBackend = .adobeDNGConverter(path: settings.dngConverterPath)
                }
            }
        )
    }

}

// MARK: - Algorithm Settings

private struct AlgorithmSettingsView: View {
    var ws: Bindable<WorkspaceState>

    private var workspace: WorkspaceState { ws.wrappedValue }

    var body: some View {
        switch workspace.selectedAlgorithm {
        case .pyramid:
            HStack {
                Text("Smoothness")
                Slider(value: ws.pyramidSmoothness, in: 1...10, step: 1)
                Text(String(format: "%.0f", workspace.pyramidSmoothness))
                    .monospacedDigit()
                    .frame(width: 30)
            }
        case .depth:
            HStack {
                Text("Radius")
                Slider(value: ws.depthRadius, in: 1...25)
                Text(String(format: "%.0f", workspace.depthRadius))
                    .monospacedDigit()
                    .frame(width: 30)
            }
            HStack {
                Text("Smoothing")
                Slider(value: ws.depthSmoothing, in: 0...50)
                Text(String(format: "%.0f", workspace.depthSmoothing))
                    .monospacedDigit()
                    .frame(width: 30)
            }
            HStack {
                Text("Blend Range")
                Slider(value: ws.depthBlendRange, in: 0...10)
                Text(String(format: "%.0f", workspace.depthBlendRange))
                    .monospacedDigit()
                    .frame(width: 30)
            }
        case .wavelet:
            HStack {
                Text("Denoise")
                Slider(value: ws.waveletDenoiseLevel, in: 0...10)
                Text(String(format: "%.1f", workspace.waveletDenoiseLevel))
                    .monospacedDigit()
                    .frame(width: 30)
            }
            HStack {
                Text("Consistency")
                Slider(value: .init(
                    get: { Float(workspace.waveletConsistency) },
                    set: { workspace.waveletConsistency = Int($0) }
                ), in: 0...2, step: 1)
                Text("\(workspace.waveletConsistency)")
                    .monospacedDigit()
                    .frame(width: 30)
            }
        }
    }
}

// MARK: - Stacking Progress

struct StackingProgressView: View {
    @Bindable var appState: AppState
    let settings: AppSettings

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ProgressView(value: appState.stacking.coordinator.progress)
            Text(appState.stacking.coordinator.progressMessage)
                .font(.caption)
                .foregroundStyle(.secondary)

            Button("Cancel") {
                appState.stacking.coordinator.cancel()
            }
            .frame(maxWidth: .infinity)
        }
    }
}

private enum RawBackendChoice: Hashable {
    case libRaw, apple, adobeDNG
}
