import Foundation
import UIKit
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// An export in flight: its lasting request and iOS's background GPU time
/// for it (`ContinuedWork`). Held in `AppModel.exports` while it runs.
final class ExportRun {
    let request = Request.lasting()
    let continued: ContinuedWork
    /// Why it was stopped in the background, told once it returns.
    var stopped: StackingState.Stop?

    init(_ continued: ContinuedWork) {
        self.continued = continued
    }
}

/// Export of the render shown.
extension AppModel {
    /// Nothing is stacking, and no other render's picture is on its way.
    var canExport: Bool {
        result != nil && stacking == nil && opening == nil
    }

    /// Write `stack` (render `renderID`) with its paint to a temporary file
    /// alone in its folder, as `format` with `options`, named by the
    /// preferences' pattern. The caller deletes the folder when done.
    func export(
        _ stack: StackResult, renderID: String, format: ExportFormat, options: ExportPreferences
    ) async -> URL? {
        let name = Self.exportName(
            stack, render: history.renders.first { $0.id == renderID }, pattern: options.fileNamePattern)
        let session = paint.session
        // Its metadata is read from the frames; lasting, as the export goes
        // on in the background.
        let lease = await access(stack.frames(), under: .lasting())
        defer { withExtendedLifetime(lease) {} }
        var folder: URL?
        do {
            let directory = try AppFiles.newTemporaryFolder(prefix: "Export")
            folder = directory
            let path = try await runExport(name: name) { request in
                try await PaintModel.export(
                    session, request: request, result: stack, renderID: renderID,
                    directory: directory.path, name: name, format: format, options: options)
            }
            return URL(fileURLWithPath: path)
        } catch {
            if let folder { try? FileManager.default.removeItem(at: folder) }
            report(error)
            return nil
        }
    }

    /// Run `write` under an export's request, which lasts under iOS's
    /// background GPU time, with a background task for the rest. In the
    /// background without that time it stops (`stopExports`), and why is
    /// said.
    func runExport(name: String, _ write: (Request) async throws -> String) async throws -> String {
        let id = UUID()
        let title = String(localized: LocalizedStringResource(
            "Exporting", comment: "Title of the system's progress for an export going on after the app leaves the foreground."))
        let run = ExportRun(ContinuedWork(title: title, subtitle: name) { [weak self] in
            self?.exportExpired(id) ?? false
        })
        let background = BackgroundTask("Export")
        exports[id] = run
        stateLog.debug("export: running")
        defer {
            exports[id] = nil
            background.end()
        }
        if UIApplication.shared.applicationState == .background, !run.continued.covers {
            stop(run, .background)
        }
        do {
            let path = try await write(run.request)
            run.continued.end(success: true)
            stateLog.debug("export: written")
            return path
        } catch {
            run.continued.end(success: false)
            if let why = run.stopped {
                errorMessage = ErrorMessage(.app(why.exportMessage))
            }
            throw error
        }
    }

    /// In the background, stop every export iOS gave no time for.
    func stopExports(_ why: StackingState.Stop) {
        for run in exports.values where !run.continued.covers && run.stopped == nil {
            stop(run, why)
        }
    }

    /// iOS took back an export's time: in the background it stops, and the
    /// time goes back once it has returned.
    private func exportExpired(_ id: UUID) -> Bool {
        guard UIApplication.shared.applicationState == .background, let run = exports[id] else { return false }
        stop(run, .expired)
        return true
    }

    private func stop(_ run: ExportRun, _ why: StackingState.Stop) {
        run.stopped = why
        stateLog.debug("export: running → stopped in the background (\(String(describing: why), privacy: .public))")
        run.request.cancel()
    }

    /// "bee-4": a file name without extension from the pattern
    /// (`{stack}-{render}` by default), as on the desktop; the engine
    /// cleans it.
    nonisolated static func exportName(_ stack: StackResult, render: RenderInfo?, pattern: String) -> String {
        stack.exportFileStem(
            pattern: pattern, stack: render?.shownStackName ?? "", render: render?.number,
            date: exportDate())
    }
}
