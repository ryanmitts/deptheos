import SwiftUI

struct ContentView: View {
    @Bindable var appState: AppState
    @State private var columnVisibility: NavigationSplitViewVisibility = .automatic
    @State private var preferredColumn: NavigationSplitViewColumn = .detail
    @State private var isInspectorPresented: Bool = true
    @State private var isSettingsPresented: Bool = false

    var body: some View {
        NavigationSplitView(columnVisibility: $columnVisibility, preferredCompactColumn: $preferredColumn) {
            ProjectListView(appState: appState)
                #if os(macOS)
                .navigationSplitViewColumnWidth(min: 220, ideal: 260, max: 320)
                #endif
        } detail: {
                NavigationStack {
                    DetailPane(appState: appState)
                        .toolbar {
                            MainToolbar(
                                appState: appState,
                                columnVisibility: $columnVisibility,
                                isSettingsPresented: $isSettingsPresented,
                                isInspectorPresented: $isInspectorPresented
                            )
                        }
                }
                .inspector(isPresented: $isInspectorPresented) {
                    InspectorView(appState: appState)
                        .inspectorColumnWidth(min: 260, ideal: 320, max: 400)
                }
        }
        .sheet(isPresented: $isSettingsPresented) {
            SettingsView()
        }
        .sheet(isPresented: $appState.showExportSheet) {
            ExportView(appState: appState)
        }
    }
}
