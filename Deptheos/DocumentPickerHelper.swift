#if os(iOS)
import UIKit
import UniformTypeIdentifiers

/// Presents a UIDocumentPickerViewController directly from the key window,
/// bypassing SwiftUI's .fileImporter which can fail in certain view hierarchies.
enum DocumentPickerHelper {
    static func presentPicker(
        types: [UTType],
        allowsMultiple: Bool,
        completion: @escaping ([URL]) -> Void
    ) {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: false)
        picker.allowsMultipleSelection = allowsMultiple

        let delegate = PickerDelegate(completion: completion)
        picker.delegate = delegate
        // Store delegate to prevent deallocation
        objc_setAssociatedObject(picker, &PickerDelegate.key, delegate, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)

        guard let scene = UIApplication.shared.connectedScenes
            .compactMap({ $0 as? UIWindowScene })
            .first(where: { $0.activationState == .foregroundActive }),
              let root = scene.keyWindow?.rootViewController else {
            return
        }

        // Find the topmost presented view controller
        var presenter = root
        while let presented = presenter.presentedViewController {
            presenter = presented
        }

        presenter.present(picker, animated: true)
    }
}

private class PickerDelegate: NSObject, UIDocumentPickerDelegate {
    static var key: UInt8 = 0
    let completion: ([URL]) -> Void

    init(completion: @escaping ([URL]) -> Void) {
        self.completion = completion
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        // Start security-scoped access for each URL
        for url in urls {
            _ = url.startAccessingSecurityScopedResource()
        }
        Task { @MainActor in
            self.completion(urls)
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        // Nothing to do
    }
}
#endif
