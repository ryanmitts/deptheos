import Combine
import SwiftUI

@main
struct DeptheosMobileApp: App {
    @State private var model = AppModel()
    @State private var alertHosts = ErrorAlertHosts()
    @Environment(\.scenePhase) private var scenePhase

    private var textEditing: AnyPublisher<Bool, Never> {
        let center = NotificationCenter.default
        return Publishers.MergeMany(
            center.publisher(for: UITextField.textDidBeginEditingNotification).map { _ in true }.eraseToAnyPublisher(),
            center.publisher(for: UITextView.textDidBeginEditingNotification).map { _ in true }.eraseToAnyPublisher(),
            center.publisher(for: UITextField.textDidEndEditingNotification).map { _ in false }.eraseToAnyPublisher(),
            center.publisher(for: UITextView.textDidEndEditingNotification).map { _ in false }.eraseToAnyPublisher()
        )
        .eraseToAnyPublisher()
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(model)
                .environment(alertHosts)
                // While a rename or search is edited, single-key shortcuts
                // wait.
                .onReceive(textEditing) { editing in
                    if model.editingText != editing { model.editingText = editing }
                }
                .task {
                    await model.start()
                    await Automation.run(model)
                }
        }
        .commands { AppCommands(model: model) }
        .onChange(of: scenePhase) { _, phase in
            if phase == .background {
                model.enteredBackground()
            } else if phase == .active {
                model.enteredForeground()
            }
        }
    }
}
