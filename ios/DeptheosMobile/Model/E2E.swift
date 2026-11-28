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
///   Its adjustments and reframe are set once Adjust is first ready
///   (`E2EAdjustHook`), standing in for setting each slider by hand.
/// - `DEPTHEOS_PROBE_LOCAL_TONE`: `without`, `whole` or `bands`. At launch
///   the engine measures that draw of a synthetic 45 MP stack, the memory
///   probe of the sliders that read neighbors (`deptheos_app::runs::probe`),
///   and logs its line; `DEPTHEOS_PROBE_BAND` sets the band's rows (512).
/// - `DEPTHEOS_PROBE_ADJUST_TICK`: `fit`, `region`, `fit-local` or
///   `region-local`. At launch 40 of Adjust's drag ticks on a synthetic
///   45 MP stack, each timed from the engine's draw through its pixels'
///   copy into Swift to a drawn `CGImage`, and read in place, the
///   median and 95th percentile of each step logged; then three settles'
///   full pictures with the footprint's peak.
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

    /// The case's adjustments and reframe; nil without either.
    static let adjusting: Adjusting? = {
        guard let json = ProcessInfo.processInfo.environment["DEPTHEOS_E2E_CASE"],
              let data = json.data(using: .utf8),
              let read = try? JSONDecoder().decode(Adjusting.self, from: data),
              read.adjust != nil || read.reframe != nil
        else { return nil }
        return read
    }()

    /// As `e2e.toml` writes them: each slider by its key (unset ones null),
    /// the crop in the stored picture's pixels, the turn in degrees.
    struct Adjusting: Decodable {
        let adjust: [String: Double?]?
        let reframe: CaseReframe?

        struct CaseReframe: Decodable {
            let crop: [UInt32]?
            let flip: String?
            let turn: UInt32?
        }

        /// As `CaseReframe::reframe` in `crates/deptheos_e2e/src/cases.rs`.
        var engineReframe: Reframe? {
            guard let reframe else { return nil }
            let crop = reframe.crop.flatMap { c in
                c.count == 4 ? Rect(x: c[0], y: c[1], width: c[2], height: c[3]) : nil
            }
            let turns: [Turn] = [.none, .quarter, .half, .threeQuarters]
            var quarter = Int(reframe.turn ?? 0) / 90 % 4
            var flip = Flip.none
            switch reframe.flip {
            case "horizontal": flip = .horizontal
            case "vertical":
                flip = .horizontal
                quarter = (quarter + 2) % 4
            default: break
            }
            return Reframe(crop: crop, flip: flip, turn: turns[quarter])
        }

        /// Each slider set and committed, then the reframe.
        @MainActor func apply(to adjust: AdjustModel) {
            log.info("e2e: adjusting as the case says")
            if let values = adjust.panel.map({ $0.groups.flatMap(\.sliders) }) {
                for (key, value) in self.adjust ?? [:] {
                    guard let value else { continue }
                    let name = key == "kelvin" ? "temperature" : key.replacingOccurrences(of: "_", with: "")
                    guard let info = values.first(where: { String(describing: $0.slider).lowercased() == name })
                    else {
                        log.error("e2e: no slider \(key, privacy: .public)")
                        continue
                    }
                    adjust.set(info.slider, to: value)
                    adjust.commit()
                }
            }
            if let reframe = engineReframe {
                adjust.setReframe(reframe)
                adjust.commit()
            }
        }
    }

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

/// Sets the case's adjustments and reframe the first time Adjust is ready.
struct E2EAdjustHook: View {
    @Environment(AppModel.self) private var model
    @State private var applied = false

    private var ready: Bool {
        guard let status = model.adjust.status else { return false }
        return status.phase == AdjustPhase.ready
    }

    var body: some View {
        Color.clear.task(id: ready) {
            guard !applied, ready, let adjusting = E2E.adjusting else { return }
            applied = true
            adjusting.apply(to: model.adjust)
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

    /// Adjust's tick probe the launch environment names, logged.
    func probeAdjustTick() async {
        guard let kind = ProcessInfo.processInfo.environment["DEPTHEOS_PROBE_ADJUST_TICK"] else { return }
        let region = kind.hasPrefix("region")
        let local = kind.hasSuffix("local")
        let screen = UIScreen.main.nativeBounds.size
        let (sw, sh) = (UInt32(max(screen.width, screen.height)), UInt32(min(screen.width, screen.height)))
        guard let probe = await ask({ engine, request in
            try await engine.probeAdjustTicks(width: 8192, height: 5460, local: local, request: request)
        }) else {
            log.error("adjust tick probe: the copies weren't made")
            return
        }
        let lines = try? await Blocking.run { () -> String in
            var draw: [Double] = [], copy: [Double] = [], image: [Double] = [], shown: [Double] = []
            var total: [Double] = [], inPlace: [Double] = [], size = ""
            for step in 1...40 {
                let start = DispatchTime.now()
                let tick = try probe.tick(step: UInt32(step), screenWidth: sw, screenHeight: sh, region: region)
                let drawn = DispatchTime.now()
                let pixels = DisplayImage(width: tick.width, height: tick.height, bgra: probe.bgra())
                let copied = DispatchTime.now()
                guard let cgImage = pixels.makeCGImage() else { continue }
                let made = DispatchTime.now()
                // Drawn once, as the layer's first display copies it.
                let context = CGContext(
                    data: nil, width: cgImage.width, height: cgImage.height, bitsPerComponent: 8,
                    bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
                context?.draw(cgImage, in: CGRect(x: 0, y: 0, width: cgImage.width, height: cgImage.height))
                let end = DispatchTime.now()
                // The same pixels read where the engine holds them.
                let address = probe.bgraAddress()
                let bytes = Int(tick.width * tick.height * 4)
                if address != 0, let pointer = UnsafeRawPointer(bitPattern: UInt(address)),
                   let provider = CGDataProvider(dataInfo: nil, data: pointer, size: bytes, releaseData: { _, _, _ in }),
                   let held = CGImage(
                       width: Int(tick.width), height: Int(tick.height), bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: Int(tick.width) * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue
                           | CGBitmapInfo.byteOrder32Little.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent) {
                    context?.draw(held, in: CGRect(x: 0, y: 0, width: held.width, height: held.height))
                }
                let placed = DispatchTime.now()
                let ms = { (a: DispatchTime, b: DispatchTime) in Double(b.uptimeNanoseconds - a.uptimeNanoseconds) / 1e6 }
                draw.append(ms(start, drawn)); copy.append(ms(drawn, copied))
                image.append(ms(copied, made)); shown.append(ms(made, end)); total.append(ms(start, end))
                inPlace.append(ms(start, drawn) + ms(end, placed))
                size = "\(tick.width)x\(tick.height)"
            }
            let settles = try (1...3).map { try probe.settle(step: UInt32(40 + $0)) }
            func stats(_ v: [Double]) -> String {
                let s = v.dropFirst(5).sorted()
                guard !s.isEmpty else { return "?" }
                let at = { (q: Double) in s[Int((Double(s.count - 1) * q).rounded())] }
                return String(format: "%.1f / %.1f", at(0.5), at(0.95))
            }
            return "adjust tick probe, \(kind), \(size) on a \(sw)x\(sh) screen, median / p95 ms: "
                + "draw \(stats(draw)), copy into Swift \(stats(copy)), CGImage \(stats(image)), "
                + "drawn \(stats(shown)), total \(stats(total)); read in place, total \(stats(inPlace)); "
                + settles.joined(separator: "; ")
        }
        log.info("\(lines ?? "adjust tick probe failed", privacy: .public)")
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
