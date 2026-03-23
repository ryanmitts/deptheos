import SwiftUI

/// The main content area: viewport with optional comparison divider overlay.
struct DetailPane: View {
    @Bindable var appState: AppState

    var body: some View {
        GeometryReader { geo in
            ZStack {
                ViewportContainer(appState: appState)

                if appState.workspace.imageItems.isEmpty && appState.currentProject == nil {
                    ContentUnavailableView(
                        "No Images",
                        systemImage: "photo.on.rectangle.angled",
                        description: Text("Click \(Image(systemName: "photo.badge.plus")) to add images")
                    )
                }

                if (appState.viewMode == .compare || appState.viewMode == .sideBySide || appState.viewMode == .sideBySideVertical) && appState.viewport.displayState.compareTexture != nil {
                    ComparisonDividerOverlay(
                        dividerPosition: Binding(
                            get: { appState.viewport.displayState.splitPosition },
                            set: { appState.viewport.displayState.splitPosition = $0 }
                        ),
                        containerSize: geo.size,
                        viewMode: appState.viewMode
                    )
                }
            }
            .overlay(alignment: .bottomLeading) {
                if appState.isPaintMode, let sourceIndex = appState.paintCoordinator.paintState.selectedSourceIndex,
                   sourceIndex < appState.workspace.imageItems.count {
                    PaintSourceHUD(item: appState.workspace.imageItems[sourceIndex])
                }
            }
        }
    }
}

/// Floating HUD showing the current paint source image.
private struct PaintSourceHUD: View {
    let item: ImageItem

    var body: some View {
        HStack(spacing: 6) {
            if let thumb = item.thumbnail {
                Image(decorative: thumb, scale: 1.0)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: 28, height: 28)
                    .clipShape(RoundedRectangle(cornerRadius: 3))
            }
            Text(item.filename)
                .font(.caption)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 6))
        .padding(8)
        .allowsHitTesting(false)
    }
}
