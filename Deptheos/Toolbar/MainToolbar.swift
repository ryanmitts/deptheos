import SwiftUI
#if canImport(AppKit)
import AppKit
#endif
import UniformTypeIdentifiers
import DeptheosKit

/// All toolbar items for the main content view.
struct MainToolbar: ToolbarContent {
    @Bindable var appState: AppState
    @Binding var columnVisibility: NavigationSplitViewVisibility
    @Binding var isSettingsPresented: Bool
    @Binding var isInspectorPresented: Bool

    var body: some ToolbarContent {
        #if os(macOS)
        ToolbarItemGroup(placement: .accessoryBar(id: "viewControls")) {
            viewControlsContent
        }
        #else
        ToolbarItem(placement: .topBarLeading) {
            if columnVisibility == .detailOnly {
                Button {
                    columnVisibility = .all
                } label: {
                    Image(systemName: "sidebar.leading")
                }
            }
        }

        ToolbarItemGroup(placement: .bottomBar) {
            viewControlsContent
        }
        #endif

        ToolbarItem {
            Button {
                appState.showExportSheet = true
            } label: {
                Image(systemName: "square.and.arrow.up")
            }
            .help("Export Result")
            .disabled(appState.currentProject == nil)
        }

        ToolbarSpacer(.flexible)

        ToolbarItem {
            Menu {
                Button("Open Files...") { openFiles() }
                Button("Open Folder...") { openFolder() }
            } label: {
                Image(systemName: "photo.badge.plus")
            }
            #if os(macOS)
            .help("Add Images")
            #endif
        }

        ToolbarItem {
            Button {
                if appState.isPaintMode { appState.exitPaintMode() } else { appState.enterPaintMode() }
            } label: {
                Image(systemName: "paintbrush.pointed")
                    .foregroundStyle(appState.isPaintMode ? Color.accentColor : .primary)
            }
            #if os(macOS)
            .help("Paint Mode (E)")
            #endif
            .disabled(appState.currentProject == nil)
        }

        ToolbarSpacer(.fixed)

        ToolbarItem {
            Button {
                isSettingsPresented = true
            } label: {
                Image(systemName: "gear")
            }
            .help("Settings")
        }

        ToolbarItem {
            Button {
                isInspectorPresented.toggle()
            } label: {
                Image(systemName: "sidebar.trailing")
            }
            .help("Toggle Inspector")
        }
    }

    // MARK: - View Controls

    @ViewBuilder
    private var viewControlsContent: some View {
        Button { appState.viewport.zoomToFit() } label: {
            Image(systemName: "arrow.up.left.and.arrow.down.right")
        }
        .help("Zoom to Fit (F)")

        Button { appState.viewport.zoomOut() } label: {
            Image(systemName: "minus.magnifyingglass")
        }
        .help("Zoom Out (-)")

        Button { appState.viewport.zoomIn() } label: {
            Image(systemName: "plus.magnifyingglass")
        }
        .help("Zoom In (+)")

        Menu {
            ForEach([25, 50, 75, 100, 150, 200, 400], id: \.self) { percent in
                Button("\(percent)%") { appState.viewport.zoomToPercent(percent) }
            }
        } label: {
            Text("\(appState.viewport.zoomPercent)%")
                .monospacedDigit()
                .frame(minWidth: 45)
        }
        #if os(macOS)
        .menuStyle(.borderlessButton)
        #endif
        .help("Zoom Level")

        Spacer()

        if appState.viewport.displayState.mainTexture != nil {
            let viewMode = appState.viewMode

            // Result/Preview button — deselects image to show result or preview
            Button {
                appState.deselectImage()
            } label: {
                Image(systemName: "rectangle")
                    .foregroundStyle(viewMode == .result || viewMode == .preview ? Color.accentColor : .primary)
                    .opacity(appState.isPaintMode ? 0.3 : 1.0)
            }
            .help("\(appState.currentProject != nil ? "Result" : "Preview") (1)")
            .disabled(appState.isPaintMode)

            // Compare button
            Button {
                appState.comparisonStyle = .compare
                if !appState.isPaintMode { appState.ensureImageSelected() }
            } label: {
                Image(systemName: ComparisonStyle.compare.icon)
                    .foregroundStyle(viewMode == .compare ? Color.accentColor : .primary)
            }
            .help("Compare (2)")
            .disabled(appState.currentProject == nil)

            // Side by Side button
            Button {
                appState.comparisonStyle = .sideBySide
                if !appState.isPaintMode { appState.ensureImageSelected() }
            } label: {
                Image(systemName: ComparisonStyle.sideBySide.icon)
                    .foregroundStyle(viewMode == .sideBySide ? Color.accentColor : .primary)
            }
            .help("Side by Side (3)")
            .disabled(appState.currentProject == nil)

            // Side by Side Vertical button
            Button {
                appState.comparisonStyle = .sideBySideVertical
                if !appState.isPaintMode { appState.ensureImageSelected() }
            } label: {
                Image(systemName: ComparisonStyle.sideBySideVertical.icon)
                    .foregroundStyle(viewMode == .sideBySideVertical ? Color.accentColor : .primary)
            }
            .help("Side by Side Vertical (4)")
            .disabled(appState.currentProject == nil)
        }
    }

    // MARK: - File Open Actions

    private func openFolder() {
        #if os(macOS)
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.message = "Select a folder of images to stack"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let urls = ImageFormat.discoverImages(in: url)
        appState.workspace.removeAllImages()
        appState.workspace.addImages(urls: urls)
        #else
        DocumentPickerHelper.presentPicker(
            types: [.folder],
            allowsMultiple: false
        ) { urls in
            guard let url = urls.first else { return }
            let imageURLs = ImageFormat.discoverImages(in: url)
            appState.workspace.removeAllImages()
            appState.workspace.addImages(urls: imageURLs)
        }
        #endif
    }

    private func openFiles() {
        #if os(macOS)
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = true
        panel.message = "Select images to stack"
        guard panel.runModal() == .OK else { return }
        appState.workspace.removeAllImages()
        appState.workspace.addImages(urls: panel.urls)
        #else
        DocumentPickerHelper.presentPicker(
            types: [.image],
            allowsMultiple: true
        ) { urls in
            appState.workspace.removeAllImages()
            appState.workspace.addImages(urls: urls)
        }
        #endif
    }
}
