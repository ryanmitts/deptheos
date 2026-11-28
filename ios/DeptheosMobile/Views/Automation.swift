import CoreGraphics
@preconcurrency import DeptheosEngine
import SwiftUI

private let automationLog = AppLog(category: "automation")

/// Launch arguments that drive the app for simulator tests and screenshots
/// (`-OpenCamera YES`, `-AddPath <path>`). Debug builds only.
enum Automation {
    /// `-<name> YES`.
    static func flag(_ name: String) -> Bool {
        #if DEBUG
        UserDefaults.standard.bool(forKey: name)
        #else
        false
        #endif
    }

    /// `-<name> <text>`.
    static func string(_ name: String) -> String? {
        #if DEBUG
        UserDefaults.standard.string(forKey: name)
        #else
        nil
        #endif
    }

    /// `-<name> <n>`.
    static func integer(_ name: String) -> Int? {
        #if DEBUG
        UserDefaults.standard.object(forKey: name) != nil
            ? UserDefaults.standard.integer(forKey: name) : nil
        #else
        nil
        #endif
    }

    /// `-<name> <number>`; 0 when unset.
    static func number(_ name: String) -> Double {
        #if DEBUG
        UserDefaults.standard.double(forKey: name)
        #else
        0
        #endif
    }

    /// The launch's script, run once the engine has started. Nothing in a
    /// release build.
    static func run(_ model: AppModel) async {
        #if DEBUG
        // `-CrashOnPurpose <kind>` (panic, thread-panic, abort or fault):
        // as Settings ▸ Diagnostics ▸ Crash on Purpose (docs/crash-testing.md).
        if let name = Automation.string("CrashOnPurpose") {
            let kinds: [String: CrashKind] = [
                "panic": .panic, "thread-panic": .threadPanic, "abort": .abort, "fault": .fault,
            ]
            if let kind = kinds[name] {
                try? await Task.sleep(for: .seconds(1))
                crashOnPurpose(kind: kind)
            }
        }
        // `-AddPath <path>` adds a file or folder without the Files picker;
        // `-AutoStack YES` then stacks it.
        if let path = Automation.string("AddPath") {
            await model.add([URL(fileURLWithPath: path)])
            model.setScreen(.stacking)
            if Automation.flag("AutoStack") {
                await model.stack()
            }
        }
        // `-PickPath <path>`: as if picked in Files, the copy sheet shown
        // when it asks.
        if let path = Automation.string("PickPath") {
            model.setScreen(.stacking)
            await model.addPicked([Picked(automation: path)])
            // `-PickConfirm YES`: then Add, in the sheet.
            if Automation.flag("PickConfirm") { await model.confirmAdding() }
        }
        // `-OpenRender <n>`: open the nth render, newest first.
        if let index = Automation.integer("OpenRender"),
           model.history.renders.indices.contains(index) {
            model.setPanel(.closed)
            model.setScreen(.stacking)
            automationLog.debug("automation: opening render \(index)")
            await model.open(model.history.renders.reversed()[index])
            automationLog.debug("automation: render \(index) opened")
        }
        // `-Retouch YES`: retouch the result; `-RetouchStroke YES` then
        // paints one stroke across its middle.
        if Automation.flag("Retouch") {
            // `-RetouchAfter n`: n seconds later, on a result already shown.
            if let wait = Automation.integer("RetouchAfter") {
                try? await Task.sleep(for: .seconds(wait))
            }
            model.startPainting()
            if Automation.flag("RetouchStroke"),
               let result = model.result {
                try? await Task.sleep(for: .seconds(2))
                let (w, h) = (Float(result.stack.width()), Float(result.stack.height()))
                model.paint.radius = w / 20
                let samples = (0...40).map {
                    PaintSample(x: w * (0.2 + 0.6 * Float($0) / 40), y: h / 2, pressure: 1)
                }
                model.paint.beginStroke(samples[0])
                model.paint.addSamples(Array(samples.dropFirst()))
                model.paint.endStroke()
                // `-RetouchPeek YES`: then show the source.
                if Automation.flag("RetouchPeek") {
                    try? await Task.sleep(for: .seconds(1))
                    model.paint.pick(model.paint.frames.last ?? "")
                    try? await Task.sleep(for: .seconds(2))
                    model.paint.setPeeking(true)
                }
                // `-RetouchDone YES`: then back to the result.
                if Automation.flag("RetouchDone") {
                    try? await Task.sleep(for: .seconds(1))
                    model.stopPainting()
                }
            }
        }
        // `-Adjust YES`: adjust the result (`-AdjustAfter n` seconds
        // later); `-AdjustSet <slider>=<value>` then sets one and commits,
        // `-AdjustCrop YES` opens the crop tool, `-AdjustTurn YES` turns it
        // right, `-AdjustBefore YES` holds Before, `-AdjustDone YES` leaves.
        if Automation.flag("Adjust") {
            automationLog.debug("automation: Adjust")
            if let wait = Automation.integer("AdjustAfter") {
                try? await Task.sleep(for: .seconds(wait))
            }
            model.startAdjusting()
            try? await Task.sleep(for: .seconds(3))
            if let set = Automation.string("AdjustSet"), let panel = model.adjust.panel {
                let parts = set.split(separator: "=")
                let all = panel.groups.flatMap(\.sliders)
                if parts.count == 2, let value = Double(parts[1]),
                   let info = all.first(where: { String(describing: $0.slider).lowercased() == parts[0].lowercased() }) {
                    model.adjust.set(info.slider, to: value)
                    model.adjust.commit()
                }
                try? await Task.sleep(for: .seconds(2))
            }
            if Automation.flag("AdjustTurn"), let value = model.adjust.value {
                model.adjust.setReframe(reframeTurned(reframe: value.reframe(), right: true))
                model.adjust.commit()
                try? await Task.sleep(for: .seconds(2))
            }
            // `-AdjustSquare YES`: a square crop, then the sliders.
            if Automation.flag("AdjustSquare"), let value = model.adjust.value, let stack = model.result?.stack {
                let size = stack.storedSize
                let frame = CropFrame(width: size.width, height: size.height, reframe: value.reframe())
                let square = frame.shaped(area: frame.area(), aspect: .ratio(width: 1, height: 1))
                model.adjust.setReframe(frame.reframed(area: square))
                model.adjust.commit()
                try? await Task.sleep(for: .seconds(2))
            }
            if Automation.flag("AdjustCrop") { model.view(.tool(tool: .crop)) }
            if Automation.flag("AdjustBefore") { model.view(.holdBefore(held: true)) }
            if Automation.flag("AdjustDone") {
                try? await Task.sleep(for: .seconds(1))
                model.stopAdjusting()
            }
        }
        // `-ShowLibrary YES`: back to the Library; `-ShowStacking YES`: the
        // stack on screen, as its tile opens it.
        if Automation.flag("ShowLibrary") {
            model.setScreen(.library)
        } else if Automation.flag("ShowStacking") {
            await model.openStack(model.stackID)
        }
        // `-FakeResult YES`: a result from the first frame's preview
        // (`AutomationStackResult`), to see the deck without stacking, the
        // second frame selected. `-FakeFrameCount 150`: that many frames,
        // with made-up names, to see a long stack's rail.
        if Automation.flag("FakeResult"), let first = model.rows.first {
            model.setScreen(.stacking)
            if let image = await ImagePreview.load(first.url, maxPixelSize: 2048, urgent: true) {
                var files = model.rows.map(\.file)
                if let count = Automation.integer("FakeFrameCount"), count > files.count {
                    let folder = (first.file as NSString).deletingLastPathComponent
                    files += (files.count..<count).map {
                        (folder as NSString).appendingPathComponent(String(format: "FAKE_%04d.JPG", $0 + 1))
                    }
                }
                model.setResult(AutomationStackResult.shownResult(image, frames: files))
                if model.rows.count > 1 { await model.select(model.rows[1].file) }
                model.backToResult()
            }
        }
        // `-ScrubTo <n>`: the rail scrubbed to frame n (from 0), alone.
        if let index = Automation.integer("ScrubTo") {
            let frames = model.railFrames
            if frames.indices.contains(index) { model.scrub(to: frames[index].path) }
        }
        // `-ViewLayout split`: that layout (result, split, sideBySide,
        // topBottom, flicker), once the stack is on screen.
        if let name = Automation.string("ViewLayout") {
            if name == "result" {
                model.showResultAlone()
            } else if let layout = CompareLayout.Kind.allCases.first(where: { "\($0)" == name }) {
                model.setLayout(layout)
            }
        }
        // `-ViewContribution YES`: Contribution over the frame in view;
        // `-ViewFlip YES`: Flicker's other side.
        if Automation.flag("ViewContribution") { model.toggleContribution() }
        if Automation.flag("ViewFlip") { model.flip() }
        // `-ViewDepthMap YES`: then the depth map.
        if Automation.flag("ViewDepthMap") { model.toggleDepthMap() }
        // `-LightMap <n>`: then frame n (from 0) lit on the map.
        if let index = Automation.integer("LightMap") {
            let frames = model.railFrames
            if frames.indices.contains(index) { model.scrub(to: frames[index].path) }
        }
        // `-FakeRetouch YES`: Retouch's controls over the fake result.
        // `-RetouchCanvas opening`: the canvas as it opens; `failed`: its
        // failure, with Retry and Done.
        if Automation.flag("FakeRetouch"), let result = model.result {
            model.setPanel(.closed)
            let canvas: PaintCanvasState = switch Automation.string("RetouchCanvas") {
            case "opening": .opening(standIn: result.display)
            case "failed":
                .failed(
                    .verbatim("The render couldn't be opened for retouching. Stand-in reason for checking the layout."),
                    standIn: result.display)
            default: .ready(tiles: nil)
            }
            // With `-FakeFrameCount`, the fake stack's frames.
            let paintFrames = Automation.integer("FakeFrameCount") != nil
                ? result.stack.frames() : model.rows.map(\.file)
            model.paint.openForLayout(
                result.stack, renderID: result.renderID, frames: paintFrames,
                picture: result.display, source: model.sourcePreview?.image, canvas: canvas)
            // `-RetouchPicker YES`: Sharpest Frame on; `-RetouchPickAt YES`:
            // then a tap on the photo. `-RetouchLayout sideBySide`: that
            // layout (canvas, split, sideBySide, topBottom);
            // `-RetouchSource YES`: the source in the canvas's place;
            // `-RetouchPainted 2,5`: those frames (from 0) painted from.
            if let name = Automation.string("RetouchLayout"),
               let kind = PaintLayout.Kind.allCases.first(where: { "\($0)" == name }) {
                model.paint.setLayout(kind)
            }
            if Automation.flag("RetouchSource") { model.paint.setPeeking(true) }
            // `-RetouchDepthMap YES`: the depth map.
            if Automation.flag("RetouchDepthMap") { model.paint.toggleDepthMap() }
            if let marks = Automation.string("RetouchPainted") {
                let frames = model.paint.frames
                model.paint.markPaintedForLayout(marks.split(separator: ",").compactMap { Int($0) }
                    .filter { frames.indices.contains($0) }.map { frames[$0] })
            }
            if Automation.flag("RetouchPicker") { model.paint.toggleFramePicker() }
            if Automation.flag("RetouchPickAt") { model.paint.pickFrame(at: CGPoint(x: 10, y: 10)) }
        }
        // `-StackAfter n`: Stack n seconds after the rest; `-CancelAfter n`:
        // then Cancel n seconds in.
        if let wait = Automation.integer("StackAfter") {
            try? await Task.sleep(for: .seconds(wait))
            if let cancel = Automation.integer("CancelAfter") {
                Task {
                    try? await Task.sleep(for: .seconds(cancel))
                    model.cancelStack()
                }
            }
            await model.stack()
        }
        #endif
    }
}

extension View {
    /// `action`, `delay` after the view appears, if `when` holds then.
    /// Nothing in a release build.
    @ViewBuilder
    func automation(
        _ when: @escaping @autoclosure () -> Bool, after delay: Duration,
        perform action: @escaping @MainActor () -> Void
    ) -> some View {
        #if DEBUG
        task {
            guard when() else { return }
            try? await Task.sleep(for: delay)
            action()
        }
        #else
        self
        #endif
    }
}

extension View {
    /// Stacking's automation once its view appears: `columnTab` is the
    /// Frames column's side, `compact` whether the window is compact as each
    /// runs. Nothing in a release build.
    func stackingAutomation(
        _ model: AppModel, compact: @escaping @autoclosure () -> Bool, columnTab: Binding<SourcesTab>
    ) -> some View {
        // `-ShowPanel Settings`, `Frames` (`FramesTall`: the whole screen),
        // `Renders`, `Brush` (in Retouch) open that compact sheet;
        // `-ShowPanel Closed` closes the one a never-stacked stack opens.
        automation(Automation.string("ShowPanel") != nil, after: .seconds(1)) {
            // At regular width, `Renders` turns the Frames column to them.
            guard compact() else {
                if Automation.string("ShowPanel") == "Renders" { columnTab.wrappedValue = .renders }
                return
            }
            switch Automation.string("ShowPanel") {
            case "Settings": model.setPanel(.settings(.low))
            case "Frames": model.setPanel(.sources(.frames, .low))
            case "FramesTall": model.setPanel(.sources(.frames, .tall))
            case "Renders": model.setPanel(.sources(.renders, .low))
            case "Closed": model.setPanel(.closed)
            default: break
            }
        }
        // `-ShowRenderInfo YES`: the render shown's Render Info.
        .automation(Automation.flag("ShowRenderInfo"), after: .seconds(3)) {
            if let id = model.result?.renderID { model.present(.renderInfo(id)) }
        }
        // `-HideControls YES`: as a tap on the photo.
        .automation(Automation.flag("HideControls"), after: .seconds(2)) {
            model.toggleControls(voiceOver: false)
        }
        // Automation: `-ShowExport YES` opens the Export sheet.
        .automation(Automation.flag("ShowExport"), after: .seconds(2)) {
            if model.result != nil { model.present(.export) }
        }
    }
}

#if DEBUG
/// Automation only: one flat image, no levels, for
/// `AutomationStackResult`'s lit map.
nonisolated final class AutomationPicture: Picture, @unchecked Sendable {
    private let image: DisplayImage

    init(_ image: DisplayImage) {
        self.image = image
        super.init(noHandle: .init())
    }

    required init(unsafeFromHandle handle: UInt64) { fatalError("not used") }

    override func size() -> PictureSize {
        PictureSize(width: image.width, height: image.height)
    }

    override func levels() -> [PictureLevel] { [] }
    override func tiles(request: Request, asked: [TileAsk]) async throws -> [TileFetched] {
        asked.map { _ in .nothing }
    }
}

/// Automation only, compiled out of release builds: a result from a
/// frame's preview, to see the deck's states at once (`-FakeResult YES`).
/// It has no pixels in the engine: a compare shows the frame unaligned,
/// and Retouch, Export and anything else reaching the engine must not be
/// used with it.
nonisolated final class AutomationStackResult: StackResult, @unchecked Sendable {
    private struct NoPixels: Error {}

    /// Flat; the map lit for any frame is the picture darkened, so lighting
    /// along the rail shows.
    override func picture(request: Request, of: PictureOf, baseSide: UInt32) async throws -> Pictured? {
        switch of {
        case .display, .map: nil
        case .litMap: fakeLit.map { Pictured(picture: AutomationPicture($0), base: $0) }
        case .comparedFrame, .comparedRender: throw NoPixels()
        }
    }

    override func pictures(request: Request, baseSide: UInt32) async throws -> ResultPictures {
        ResultPictures(display: nil, map: nil)
    }

    /// So Contribution shows, as for a real Depth result.
    override func hasContribution() -> Bool { true }

    var fakeLit: DisplayImage?

    var fakeFrames: [String] = []

    override func frames() -> [String] { fakeFrames }

    override func leftOut() -> [LeftOutInfo] { [] }

    /// The picture's, as if stacked at that size.
    var fakeSize = (width: UInt32(0), height: UInt32(0))

    override func width() -> UInt32 { fakeSize.width }
    override func height() -> UInt32 { fakeSize.height }

    /// The frames in bands left to right, so a tap on the map lights one.
    override func frameAt(x: Float, y: Float) -> String? {
        guard !fakeFrames.isEmpty, fakeSize.width > 0, x >= 0, x < Float(fakeSize.width) else { return nil }
        return fakeFrames[min(Int(x / Float(fakeSize.width) * Float(fakeFrames.count)), fakeFrames.count - 1)]
    }

    /// A shown result whose picture and map are `image`, legend colors along
    /// a plasma-like ramp, made-up shares peaking a third along (none with
    /// `-FakeNoShares YES`); `-FakeLeftOut 3,7` leaves those frames out.
    @MainActor static func shownResult(_ image: CGImage, frames: [String]) -> ShownResult {
        let out = Set((Automation.string("FakeLeftOut") ?? "").split(separator: ",").compactMap { Int($0) })
        let leftOut = Dictionary(
            frames.enumerated().filter { out.contains($0.offset) }.map { ($0.element, "Left out to test.") },
            uniquingKeysWith: { first, _ in first })
        let weights = frames.indices.map { i -> Float in
            guard !out.contains(i) else { return 0 }
            let x = (Float(i) / Float(max(frames.count - 1, 1)) - 0.35) / 0.18
            return exp(-x * x) + 0.02
        }
        let total = weights.reduce(0, +)
        let contributions = Automation.flag("FakeNoShares") ? nil : frames.enumerated().map {
            FrameContribution(path: $0.element, share: weights[$0.offset] / total, leftOut: out.contains($0.offset))
        }
        let stack = AutomationStackResult(noHandle: .init())
        stack.fakeFrames = frames
        stack.fakeSize = (UInt32(image.width), UInt32(image.height))
        stack.fakeLit = darkened(image)
        let stacked = frames.enumerated().filter { !out.contains($0.offset) }.map(\.element)
        let legend = Dictionary(
            stacked.enumerated().map { i, file in
                let t = Double(i) / Double(max(stacked.count - 1, 1))
                return (file, Color(hue: (0.72 + 0.42 * t).truncatingRemainder(dividingBy: 1), saturation: 0.85, brightness: 0.55 + 0.4 * t))
            },
            uniquingKeysWith: { first, _ in first })
        return ShownResult(
            stack: stack, renderID: "automation-result",
            display: image, plainDisplay: image, map: image,
            fullSize: CGSize(width: image.width, height: image.height), displayPicture: .none, mapPicture: nil,
            legend: legend, leftOut: leftOut,
            stacked: Set(frames).subtracting(leftOut.keys), contributions: contributions, lease: .none)
    }

    /// At a third of its brightness, as BGRA.
    private static func darkened(_ image: CGImage) -> DisplayImage? {
        let (width, height) = (image.width, image.height)
        var bgra = [UInt8](repeating: 0, count: width * height * 4)
        let info = CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        let drawn = bgra.withUnsafeMutableBytes { bytes -> Bool in
            guard let context = CGContext(
                data: bytes.baseAddress, width: width, height: height, bitsPerComponent: 8,
                bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: info)
            else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            context.setFillColor(CGColor(gray: 0, alpha: 0.67))
            context.fill(CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        return drawn ? DisplayImage(width: UInt32(width), height: UInt32(height), bgra: Data(bgra)) : nil
    }
}
#endif
