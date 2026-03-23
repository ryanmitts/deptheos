import SwiftUI
import UniformTypeIdentifiers

struct ImageListView: View {
    @Bindable var appState: AppState
    @State private var isDropTargeted = false

    private var selectionBinding: Binding<UUID?> {
        Binding(
            get: {
                guard let index = appState.workspace.selectedImageIndex,
                      index < appState.workspace.imageItems.count else { return nil }
                return appState.workspace.imageItems[index].id
            },
            set: { newID in
                if let newID, let index = appState.workspace.imageItems.firstIndex(where: { $0.id == newID }) {
                    appState.workspace.selectedImageIndex = index
                    appState.loadSelectedImage()
                } else {
                    appState.workspace.selectedImageIndex = nil
                    appState.viewport.displayState.setCompareTexture(nil)
                }
            }
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            if appState.workspace.imageItems.isEmpty {
                Spacer()
                Text("No images")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity)
                Spacer()
            } else {
                HStack {
                    let included = appState.workspace.imageItems.filter(\.isIncluded).count
                    Text("\(appState.workspace.imageItems.count) images (\(included) selected)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Spacer()
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)

                let paintSourceID: UUID? = {
                    guard appState.isPaintMode,
                          let idx = appState.paintCoordinator.paintState.selectedSourceIndex,
                          idx < appState.workspace.imageItems.count else { return nil }
                    return appState.workspace.imageItems[idx].id
                }()

                List(selection: selectionBinding) {
                    ForEach(appState.workspace.imageItems) { item in
                        HStack(spacing: 6) {
                            if let thumb = item.thumbnail {
                                Image(decorative: thumb, scale: 1.0)
                                    .resizable()
                                    .aspectRatio(contentMode: .fill)
                                    .frame(width: 32, height: 32)
                                    .clipShape(RoundedRectangle(cornerRadius: 3))
                            } else {
                                RoundedRectangle(cornerRadius: 3)
                                    .fill(.quaternary)
                                    .frame(width: 32, height: 32)
                            }

                            Text(item.filename)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .font(.caption)

                            if item.id == paintSourceID {
                                Image(systemName: "paintbrush.pointed.fill")
                                    .font(.caption2)
                                    .foregroundStyle(Color.accentColor)
                            }
                        }
                        .tag(item.id)
                        .listRowBackground(
                            item.isIncluded ? nil : Color.red.opacity(0.15)
                        )
                        .opacity(item.isIncluded ? 1.0 : 0.6)
                        .contextMenu {
                            if item.isIncluded {
                                Button("Exclude") {
                                    if let idx = appState.workspace.imageItems.firstIndex(where: { $0.id == item.id }) {
                                        appState.workspace.imageItems[idx].isIncluded = false
                                    }
                                }
                            } else {
                                Button("Include") {
                                    if let idx = appState.workspace.imageItems.firstIndex(where: { $0.id == item.id }) {
                                        appState.workspace.imageItems[idx].isIncluded = true
                                    }
                                }
                            }
                            Button("Include All") { appState.workspace.setAllIncluded(true) }
                            Button("Exclude All") { appState.workspace.setAllIncluded(false) }
                            Divider()
                            Button("Remove Image") {
                                appState.workspace.removeImage(id: item.id)
                                appState.viewport.displayState.setCompareTexture(nil)
                            }
                            Button("Remove All Images", role: .destructive) {
                                appState.workspace.removeAllImages()
                                appState.viewport.displayState.setCompareTexture(nil)
                            }
                        }
                    }
                    .onMove { appState.workspace.moveImages(fromOffsets: $0, toOffset: $1) }
                }
                .listStyle(.sidebar)
            }
        }
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(Color.accentColor, lineWidth: 2)
                .opacity(isDropTargeted ? 1 : 0)
        )
        .onDrop(of: [.fileURL], isTargeted: $isDropTargeted) { providers in
            Task { @MainActor in
                var urls: [URL] = []
                for provider in providers {
                    if let url = await provider.loadFileURL() {
                        urls.append(url)
                    }
                }
                if !urls.isEmpty {
                    appState.workspace.addImages(urls: urls)
                }
            }
            return true
        }
    }
}

extension NSItemProvider {
    func loadFileURL() async -> URL? {
        await withCheckedContinuation { continuation in
            loadItem(forTypeIdentifier: UTType.fileURL.identifier) { data, _ in
                let url = (data as? Data).flatMap { URL(dataRepresentation: $0, relativeTo: nil) }
                continuation.resume(returning: url)
            }
        }
    }
}
