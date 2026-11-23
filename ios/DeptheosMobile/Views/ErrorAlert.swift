import SwiftUI

/// The presentations that can show `AppModel.errorMessage`, in the order
/// they appeared. Only the topmost shows it, so an error raised inside a
/// sheet is seen over the sheet, and shown once.
@Observable
final class ErrorAlertHosts {
    private var hosts: [UUID] = []

    var top: UUID? { hosts.last }

    func appeared(_ id: UUID) {
        hosts.removeAll { $0 == id }
        hosts.append(id)
    }

    func disappeared(_ id: UUID) {
        hosts.removeAll { $0 == id }
    }
}

extension View {
    /// The error alert, on the window's root and each presentation's.
    func errorAlert() -> some View {
        modifier(ErrorAlert())
    }
}

private struct ErrorAlert: ViewModifier {
    @Environment(AppModel.self) private var model
    @Environment(ErrorAlertHosts.self) private var hosts
    @State private var id = UUID()

    private var shown: Bool {
        model.errorMessage != nil && hosts.top == id
    }

    func body(content: Content) -> some View {
        content
            .onAppear { hosts.appeared(id) }
            .onDisappear { hosts.disappeared(id) }
            .alert(
                Text(model.errorMessage?.title ?? .verbatim("Deptheos")),
                isPresented: Binding(
                    get: { shown },
                    set: { if !$0 { model.errorMessage = nil } }),
                presenting: model.errorMessage
            ) { _ in
                Button(LocalizedStringResource("OK", comment: "Button that closes an error alert.")) {}
            } message: { message in
                Text(message.text)
            }
    }
}
