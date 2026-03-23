import SwiftUI
import DeptheosKit

@main
struct DeptheosApp: App {
    @State private var appSettings = AppSettings()
    @State private var appState: AppState? = nil
    @State private var initError: String? = nil

    var body: some Scene {
        WindowGroup {
            if let appState {
                ContentView(appState: appState)
                    .environment(appSettings)
            } else if let initError {
                Text("Failed to initialize: \(initError)")
                    .padding()
            } else {
                ProgressView("Initializing Metal...")
                    .task {
                        do {
                            let state = try AppState(appSettings: appSettings)
                            appState = state
                        } catch {
                            initError = error.localizedDescription
                        }
                    }
            }
        }
        .defaultSize(width: 1100, height: 700)
        .windowResizability(.contentMinSize)
        .commands {
            CommandGroup(after: .newItem) {
                if let appState {
                    Button("Open Project...") {
                        appState.openProject()
                    }
                    .keyboardShortcut("o", modifiers: [.command, .shift])

                    Button("Save Project...") {
                        appState.saveProject()
                    }
                    .keyboardShortcut("s", modifiers: .command)
                    .disabled(appState.currentProject == nil)
                }
            }
        }

        #if os(macOS)
        Settings {
            SettingsView()
                .environment(appSettings)
        }
        #endif
    }
}
