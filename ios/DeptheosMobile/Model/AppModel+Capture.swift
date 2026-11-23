import Foundation
@preconcurrency import DeptheosEngine

/// A burst from the camera, made a stack.
extension AppModel {
    /// Named for when it was taken.
    func newCaptureFolder() throws -> URL {
        try AppFiles.newFolder(in: "Captures", named: "Capture")
    }

    enum CaptureOpen: Equatable {
        case opened
        /// Not stacked, and why: the camera, still open, says so itself (an
        /// alert under its cover would never show).
        case declined(ErrorMessage)
        /// Already reported.
        case failed
    }

    /// A burst too short to stack is thrown away; one that finished while
    /// the app was busy is kept.
    func openCapture(_ folder: URL) async -> CaptureOpen {
        // Kept in Captures, to add from Files later.
        let busyNow = CaptureOpen.declined(ErrorMessage(.app(LocalizedStringResource(
            "The burst is saved in Captures but wasn't stacked, because Deptheos was busy. Add it from Files when it finishes.",
            comment: "Alert text: a camera burst finished while a stack or batch ran. Captures is the name of the app's folder and Files the system app."))))
        guard !busy else { return busyNow }
        let path = folder.standardizedFileURL.path
        let empty = listClear(list: ListInfo(files: [], excluded: [], source: .raw))
        let captured: ListInfo
        do {
            captured = try await Request().run { try await listAdd(request: $0, list: empty, paths: [path]) }
        } catch {
            // Kept, as when busy: the camera says so, and why.
            return .declined(ErrorMessage(.app(LocalizedStringResource(
                "The burst is saved in Captures but couldn't be read to stack it. \(Self.message(error).string) Add it from Files.",
                comment: "Alert text: a camera burst's frames couldn't be listed to make a stack. The placeholder is the engine's sentence saying why. Captures is the name of the app's folder and Files the system app."))))
        }
        // A stack, a batch or an open may have started while it listed.
        guard !busy else { return busyNow }
        guard listRows(list: captured).count >= 2 else {
            discardCapture(folder)
            return .declined(ErrorMessage(.app(LocalizedStringResource(
                "The burst had fewer than 2 frames, so it wasn't stacked. In Sweep, keep holding the shutter while you move.",
                comment: "Alert text: a camera burst too short to stack. Sweep is the name of a camera mode."))))
        }
        let burst = newWorking(captured.files, settings: settings)
        let switched = await switchStack(to: burst.stack, opening: .capture(burst.stack), fallback: burst) { _ in
            setScreen(.stacking)
            setPanel(.closed)
            await loadSourcePreview()
        }
        return switched ? .opened : .failed
    }

    func discardCapture(_ folder: URL) {
        try? FileManager.default.removeItem(at: folder)
    }
}
