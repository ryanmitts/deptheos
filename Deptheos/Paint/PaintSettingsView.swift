import SwiftUI

struct PaintSettingsView: View {
    @Bindable var appState: AppState

    private var paintState: PaintState {
        appState.paintCoordinator.paintState
    }

    var body: some View {
        @Bindable var brush = appState.paintCoordinator.paintState.brushSettings

        VStack(alignment: .leading, spacing: 12) {
            // Tool picker
            VStack(alignment: .leading, spacing: 6) {
                Text("Paint Tool").font(.caption).foregroundStyle(.secondary)
                Picker("Tool", selection: $brush.activeTool) {
                    Label("Brush", systemImage: "paintbrush.pointed").tag(PaintTool.brush)
                    Label("Eraser", systemImage: "eraser").tag(PaintTool.eraser)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }

            // Brush settings
            VStack(alignment: .leading, spacing: 6) {
                Text("Brush").font(.caption).foregroundStyle(.secondary)

                HStack {
                    Text("Size")
                    Slider(
                        value: Binding<Float>(
                            get: { log2(brush.size) },
                            set: { brush.size = pow(2.0, $0) }
                        ),
                        in: 0...10 // log2(1)...log2(1024)
                    )
                    Text("\(Int(brush.size))px")
                        .monospacedDigit()
                        .frame(width: 45, alignment: .trailing)
                }

                HStack {
                    Text("Hardness")
                    Slider(value: $brush.hardness, in: 0...1)
                    Text("\(Int(brush.hardness * 100))%")
                        .monospacedDigit()
                        .frame(width: 45, alignment: .trailing)
                }

                HStack {
                    Text("Opacity")
                    Slider(value: $brush.opacity, in: 0.01...1)
                    Text("\(Int(brush.opacity * 100))%")
                        .monospacedDigit()
                        .frame(width: 45, alignment: .trailing)
                }

                if brush.activeTool == .brush {
                    HStack {
                        Text("Tolerance")
                        Slider(value: $brush.colorTolerance, in: 0...1)
                        Text("\(Int(brush.colorTolerance * 100))%")
                            .monospacedDigit()
                            .frame(width: 45, alignment: .trailing)
                    }
                }
            }

            Divider()

            // Undo / Redo
            HStack(spacing: 8) {
                Button { appState.paintUndoAll() } label: {
                    Image(systemName: "backward.end.fill")
                }
                .help("Undo All (Reset to Result)")

                Button { appState.paintUndo() } label: {
                    Image(systemName: "arrow.uturn.backward")
                }
                .help("Undo (Z)")

                Spacer()

                Button { appState.paintRedo() } label: {
                    Image(systemName: "arrow.uturn.forward")
                }
                .help("Redo")

                Button { appState.paintRedoAll() } label: {
                    Image(systemName: "forward.end.fill")
                }
                .help("Redo All")
            }
            .buttonStyle(.borderless)

            // Source indicator
            if let sourceIndex = paintState.selectedSourceIndex,
               sourceIndex < appState.workspace.imageItems.count {
                HStack(spacing: 4) {
                    Image(systemName: "paintbrush.pointed.fill")
                        .foregroundStyle(Color.accentColor)
                        .font(.caption)
                    Text("Source: \(appState.workspace.imageItems[sourceIndex].filename)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }

            // Reset to original
            Button(role: .destructive) {
                appState.paintResetToOriginal()
            } label: {
                Label("Reset to Original", systemImage: "arrow.counterclockwise")
            }
            .help("Discard all paint edits and reset to the stacking result")

            Spacer()

            // Save status indicator
            paintSaveIndicator
        }
    }

    @ViewBuilder
    private var paintSaveIndicator: some View {
        HStack(spacing: 4) {
            switch appState.paintSaveStatus {
            case .saved:
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .font(.caption)
                Text("Saved")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            case .unsaved:
                Image(systemName: "circle.fill")
                    .foregroundStyle(.orange)
                    .font(.caption2)
                Text("Unsaved changes")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            case .saving:
                ProgressView()
                    .controlSize(.mini)
                Text("Saving…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .animation(.easeInOut(duration: 0.2), value: appState.paintSaveStatus)
    }
}
