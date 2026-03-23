import SwiftUI

struct InspectorView: View {
    @Bindable var appState: AppState

    var body: some View {
        VStack(spacing: 0) {
            ImageListView(appState: appState)

            Divider()

            ScrollView {
                if appState.isPaintMode {
                    PaintSettingsView(appState: appState)
                        .padding(8)
                } else {
                    StackingSettingsView(appState: appState)
                        .padding(8)
                }
            }
            .frame(minHeight: 200)
        }
    }
}
