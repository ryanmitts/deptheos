#if DEBUG
import Foundation
import SwiftUI
@preconcurrency import DeptheosEngine

private nonisolated let log = AppLog(category: "files")

/// The end-to-end test's hooks (`ios/DeptheosMobileUITests`,
/// `script/e2e-ios.sh`): debug builds only, when the test's launch
/// environment names them. Everything else is the app as a user drives it.
///
/// - `DEPTHEOS_E2E_FRAMES`: a folder of frames on the Mac, copied into
///   Imports at launch and opened as a new stack, standing in for Add from
///   Files…, whose picker is the system's.
/// - `DEPTHEOS_E2E_CASE`: the case, as `e2e-check case` gives it (JSON):
///   its method and settings are the new stack's. The test taps the rest.
/// - `DEPTHEOS_E2E_EXPORTS`: a folder on the Mac. The Export sheet gains
///   Save to Test Folder, writing the export there with a note of what was
///   asked (`e2e-check notes` reads them), standing in for Save to Files….
/// - `DEPTHEOS_PROBE_LOCAL_TONE`: `without`, `whole` or `bands`. At launch
///   the engine measures that draw of a synthetic 45 MP stack, the memory
///   probe of the sliders that read neighbors (`deptheos_app::runs::probe`),
///   and logs its line; `DEPTHEOS_PROBE_BAND` sets the band's rows (512).
nonisolated enum E2E {
    static let frames: URL? = folder("DEPTHEOS_E2E_FRAMES")
    static let exports: URL? = folder("DEPTHEOS_E2E_EXPORTS")

    /// Nil without a case.
    static let stacking: Stacking? = {
        guard let json = ProcessInfo.processInfo.environment["DEPTHEOS_E2E_CASE"],
              let data = json.data(using: .utf8)
        else { return nil }
        do {
            return try JSONDecoder().decode(Stacking.self, from: data)
        } catch {
            log.error("e2e: the case doesn't read: \(error.localizedDescription, privacy: .public)")
            return nil
        }
    }()

    /// The method and its settings, each nil for its default.
    struct Stacking: Decodable {
        let method: String
        let smoothness: Float?
        let radius: Float?
        let smoothing: Float?
        let blendRange: Float?
        let denoise: Float?

        enum CodingKeys: String, CodingKey {
            case method, smoothness, radius, smoothing, denoise
            case blendRange = "blend_range"
        }

        /// The defaults from `ranges`.
        func algorithm(_ ranges: AlgorithmRanges) -> Algorithm {
            switch method {
            case "depth":
                .depth(
                    radius: radius ?? ranges.radius.defaultValue,
                    smoothing: smoothing ?? ranges.smoothing.defaultValue,
                    blendRange: blendRange ?? ranges.blendRange.defaultValue)
            case "wavelet":
                .wavelet(denoise: denoise ?? ranges.denoise.defaultValue, consistency: 0)
            default:
                .pyramid(smoothness: smoothness ?? ranges.smoothness.defaultValue)
            }
        }
    }

    private static func folder(_ name: String) -> URL? {
        ProcessInfo.processInfo.environment[name].map { URL(fileURLWithPath: $0, isDirectory: true) }
    }

    /// Copy `file` into the exports folder as the next, numbered export,
    /// with its note: `{"file": "1 Spider.tif", "format": "tiff", "bits": 16,
    /// "paintedFrom": "spider-46.jpg"}` (`null` when not retouched).
    static func keep(_ file: URL, format: ExportFormat, bits: Int, paintedFrom: String?) throws {
        guard let exports else { return }
        let fileManager = FileManager.default
        try fileManager.createDirectory(at: exports, withIntermediateDirectories: true)
        let notes = try fileManager.contentsOfDirectory(atPath: exports.path).filter { $0.hasSuffix(".json") }
        let name = "\(notes.count + 1) \(file.lastPathComponent)"
        let kept = exports.appending(path: name)
        try? fileManager.removeItem(at: kept)
        try fileManager.copyItem(at: file, to: kept)
        let note: [String: Any] = [
            "file": name,
            "format": formatName(format),
            "bits": bits,
            "paintedFrom": paintedFrom ?? NSNull(),
        ]
        let data = try JSONSerialization.data(withJSONObject: note, options: [.sortedKeys])
        try data.write(to: exports.appending(path: "\(name).json"))
        log.info("e2e: kept export \(name, privacy: .public)")
    }

    private static func formatName(_ format: ExportFormat) -> String {
        switch format {
        case .tiff: "tiff"
        case .jpeg: "jpeg"
        case .png: "png"
        case .dng: "dng"
        }
    }
}

/// What the viewer's loads have landed, for `EndToEndTests.testViewLoads`:
/// an invisible element valued
/// `selected=…;frame=…;full=…;compare=…;contribution=…;lighting=…;lit=…`,
/// each a file name (or `-`): compare and contribution as
/// `kind:name:aligned:picture` (`base` until the picture is in), the lit
/// picture as `name:picture`. Only while the test runs.
struct E2EViewProbe: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Color.clear
            .frame(width: 1, height: 1)
            .allowsHitTesting(false)
            .accessibilityElement()
            .accessibilityLabel(Text(verbatim: "e2e view"))
            .accessibilityIdentifier("e2e-view")
            .accessibilityValue(Text(verbatim: value))
    }

    private var value: String {
        func name(_ path: String?) -> String {
            path.map { ($0 as NSString).lastPathComponent } ?? "-"
        }
        func compared(_ image: CompareImage?) -> String {
            guard let image else { return "-" }
            let kind = switch image.source {
            case .frame(_, contribution: true): "contribution"
            case .frame: "frame"
            case .render: "render"
            }
            let path = switch image.source {
            case .frame(let path, _): path
            case .render(let id): id
            }
            return "\(kind):\(name(path)):\(image.aligned ? "aligned" : "unaligned"):\(image.picture != nil ? "picture" : "base")"
        }
        let lit = model.shownLitMap.map { "\(name($0.file)):\($0.picture != nil ? "picture" : "base")" } ?? "-"
        return [
            "selected=\(name(model.selectedFile))",
            "frame=\(name(model.shownFrameFile))",
            "full=\(model.sourcePreview?.full.logName ?? "-")",
            "compare=\(compared(model.shownCompare))",
            "contribution=\(compared(model.shownContributionFrame))",
            "lighting=\(name(model.litFrame))",
            "lit=\(lit)",
        ].joined(separator: ";")
    }
}

extension AppModel {
    /// Opened as a new, unstacked stack: Stack is the test's to tap.
    func openTestFrames() async {
        guard let source = E2E.frames, engine != nil else { return }
        let copied: URL
        do {
            copied = try await Blocking.run(.background) { try Self.copyTestFrames(from: source) }
        } catch {
            log.error("e2e: the frames weren't copied: \(error.localizedDescription, privacy: .public)")
            errorMessage = ErrorMessage(.verbatim("The test's frames weren't copied: \(error.localizedDescription)"))
            return
        }
        let empty = listClear(list: ListInfo(files: [], excluded: [], source: .raw))
        let listed = await ask { _, request in
            try await listAdd(request: request, list: empty, paths: [copied.standardizedFileURL.path])
        }
        guard let listed else { return }
        if let stacking = E2E.stacking {
            stackSettings.use(algorithm: stacking.algorithm(stackSettings.ranges))
            log.info("e2e: stacking with \(stacking.method, privacy: .public)")
        }
        let working = newWorking(listed.files, settings: settings)
        await switchStack(to: working.stack, opening: .sample(working.stack), fallback: working) { _ in
            slideIn(then: .closed)
            await loadSourcePreview()
        }
    }

    /// The memory probe the launch environment names, logged.
    func probeLocalTone() async {
        let environment = ProcessInfo.processInfo.environment
        guard let draw = environment["DEPTHEOS_PROBE_LOCAL_TONE"] else { return }
        let band = environment["DEPTHEOS_PROBE_BAND"].flatMap { UInt32($0) } ?? 512
        let line = await ask { engine, request in
            try await engine.probeLocalTone(
                draw: draw, width: 8192, height: 5464, band: band, request: request)
        }
        log.info("\(line ?? "local tone probe failed", privacy: .public)")
    }

    /// Blocks: off the main actor.
    nonisolated static func copyTestFrames(from source: URL) throws -> URL {
        let fileManager = FileManager.default
        let frames = try fileManager.contentsOfDirectory(
            at: source, includingPropertiesForKeys: nil, options: .skipsHiddenFiles)
        let folder = try AppFiles.newImportFolder(named: source.lastPathComponent)
        for frame in frames {
            try fileManager.copyItem(at: frame, to: folder.appending(path: frame.lastPathComponent))
        }
        return folder
    }
}
#endif
