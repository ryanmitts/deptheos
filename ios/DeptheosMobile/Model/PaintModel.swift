import CoreGraphics
import Foundation
import Observation
import UIKit
import os
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// What the paint canvas is told: a render opened, or tiles of it changed.
protocol PaintCanvasSink: AnyObject {
    func paintOpened(_ tiles: PaintTiles)
    func paintTilesChanged(_ tiles: [PaintTile])
    /// The full-size tiles its view wants at once.
    var fullTilesWanted: Int { get }
}

/// Retouching: brushing a stacked frame (or the original, the eraser) onto
/// a render, as on the desktop. The engine's paint session does the rest;
/// this holds the brush, what is on screen and what a touch does, and reads
/// the session's status after each call and report.
@Observable
final class PaintModel {
    /// Longest side of the painted display outside paint mode.
    static let displaySide = AppModel.displaySide
    /// How long going to the background waits for the paint to be saved.
    static let saveTimeoutMs: UInt32 = 5_000

    /// Retouch on one render: `AppModel.mode`'s retouching case
    /// (`RenderMode`), entered by `open` and left by `close`; inside it only
    /// `PaintModel`'s methods change anything, through `whileOpen`
    /// (ADR-163, ADR-149).
    ///
    /// Lifecycle: the case's own states, changed only by `PaintModel`:
    ///
    ///     canvas:    opening ──the session's tiles──▶ ready
    ///                opening ──the session's status says it failed──▶ failed ──Retry──▶ opening (the session opens it again)
    ///     view:      layout ──another layout──▶ layout
    ///                layout ──the eye, \, touch and hold, a squeeze──▶ peeking(back: it) ──again──▶ layout
    ///                layout, peeking ──Depth Map──▶ depthMap(back: the layout, light: the frame painted from) ──again, the label's close──▶ layout
    ///                depthMap ──another source painted from (a scrub, a tap in the list or on the map, Sharpest Frame, the eraser, a render)──▶ depthMap(light: it, its picture loading; whole for the original or a render)
    ///                depthMap(light: f) ──f's lit picture lands, or one overtaken──▶ depthMap(light: f)
    ///     touch:     paints ──Sharpest Frame──▶ picksFrame ──a tap on the photo, Sharpest Frame again──▶ paints
    ///
    /// What the brush paints from is the session's (`status`). The source's
    /// pane, Split and the peek show the frame as it is: Contribution is the
    /// viewer's alone.
    struct Open {
        let renderID: String
        /// Its size, where its detail came from, its depth map.
        let stack: StackResult
        /// In stacking order: what the brush paints from.
        let frames: [String]
        /// Their scopes, held while it is painted.
        let lease: Lease
        var canvas: PaintCanvasState
        /// What the brush paints from, the frame painted from last, the
        /// light, undo and redo.
        var status: PaintStatus?
        /// The column's side at regular width: the frames as each opening
        /// starts, or the stack's other renders.
        var sourcesTab = SourcesTab.frames
        /// The last source the worker delivered, lined up with the render,
        /// held while the next loads.
        var loadedSource: (source: SourceKind, image: CGImage, picture: Picture?)?
        var view: PaintView
        var touch = PaintTouch.paints
        /// Where the last zoom or drag settled, as fractions of the render;
        /// nil at fit. A canvas made again shows the same place.
        var zoom: CGRect?
    }

    let ranges = brushRanges()
    /// Whose mode holds Retouch's `Open`: set once at launch.
    @ObservationIgnored weak var app: AppModel?

    private var status: PaintStatus? { painting?.status }
    /// Strokes that can be undone, and redone.
    var undoCount: UInt32 { status?.undo ?? 0 }
    var redoCount: UInt32 { status?.redo ?? 0 }
    var painted: Bool { status?.painted ?? false }

    var active: Bool { painting != nil }
    var renderID: String? { painting?.renderID }
    /// In stacking order.
    var frames: [String] { painting?.frames ?? [] }
    /// Why nothing painted can be saved, as the worker says, for a notice
    /// that stays.
    var unsaveable: String? { status?.unsaveable }

    // The brush. Size is the radius in image pixels.
    var radius: Float
    var hardness: Float
    var opacity: Float
    var flow: Float
    var toleranceOn = false
    var tolerance: Float

    /// [ and ] scale the size by these; { and } step hardness by this.
    static let smallerStep: Float = 0.8
    static let largerStep: Float = 1.25
    static let hardnessStep: Float = 0.1

    /// Within the Size slider's range.
    func scaleRadius(by factor: Float) {
        radius = min(max(radius * factor, ranges.radius.min), ranges.radius.max)
    }

    /// Within the Hardness slider's range.
    func nudgeHardness(by delta: Float) {
        hardness = min(max(hardness + delta, ranges.hardness.min), ranges.hardness.max)
    }

    /// "40 px".
    var radiusLabel: LocalizedStringResource {
        LocalizedStringResource(
            "\(Int(radius.rounded())) px",
            comment: "The brush's size, beside the Size slider and in the iPhone's brush bar. The placeholder is its radius in pixels.")
    }

    /// The frames while closed.
    var sourcesTab: SourcesTab {
        get { painting?.sourcesTab ?? .frames }
        set {
            guard newValue != painting?.sourcesTab else { return }
            whileOpen { $0.sourcesTab = newValue }
            stateLog.debug("paint: sources \(String(describing: newValue), privacy: .public)")
        }
    }

    /// `nil` while closed.
    var selection: SourceKind? { status?.selection }

    /// The canvas alone while closed.
    var view: PaintView { painting?.view ?? .layout(.canvas) }

    var touch: PaintTouch { painting?.touch ?? .paints }

    var canvasState: PaintCanvasState? { painting?.canvas }

    /// In image pixels, while open.
    var renderSize: CGSize? {
        painting.map { CGSize(width: Int($0.stack.width()), height: Int($0.stack.height())) }
    }

    /// The canvas has the worker's tiles, so strokes paint.
    var canvasReady: Bool {
        if case .ready? = painting?.canvas { true } else { false }
    }

    var peeking: Bool {
        if case .peeking = view { true } else { false }
    }

    var showsDepthMap: Bool {
        if case .depthMap = view { true } else { false }
    }

    /// The layout when Retouch was last left: the next opening starts in it.
    private var lastLayout = PaintLayout.canvas
    /// One at a time.
    @ObservationIgnored private let mapLighter = MapLighter(name: "retouch lit map")

    /// As fractions of the render; nil at fit. Kept by the canvas.
    var zoom: CGRect? {
        get { painting?.zoom }
        set {
            guard newValue != painting?.zoom else { return }
            whileOpen { $0.zoom = newValue }
        }
    }

    /// Fingers move the view. Starts as the system's Only Draw with Apple
    /// Pencil.
    var onlyPencilPaints: Bool {
        didSet { UserDefaults.standard.set(onlyPencilPaints, forKey: Self.onlyPencilKey) }
    }
    var squeezeAction: SqueezeAction {
        didSet { UserDefaults.standard.set(squeezeAction.rawValue, forKey: Self.squeezeKey) }
    }

    enum SqueezeAction: String, CaseIterable, Identifiable {
        case switchEraser
        case showSource
        case nothing
        var id: Self { self }

        var label: LocalizedStringResource {
            switch self {
            case .switchEraser:
                LocalizedStringResource(
                    "Switch to Eraser", comment: "Choice of what squeezing Apple Pencil Pro does: switch between the brush and the eraser.")
            case .showSource:
                LocalizedStringResource(
                    "Show the Source", comment: "Choice of what squeezing Apple Pencil Pro does: show what the brush paints from in place of the painted result, or the painted result again, one squeeze each.")
            case .nothing:
                LocalizedStringResource("Off", comment: "Choice of what squeezing Apple Pencil Pro does: nothing.")
            }
        }
    }

    /// The system's Only Draw with Apple Pencil on iPad, off elsewhere.
    private static var defaultOnlyPencilPaints: Bool {
        UIDevice.current.userInterfaceIdiom == .pad && UIPencilInteraction.prefersPencilOnlyDrawing
    }
    private static let defaultSqueezeAction = SqueezeAction.switchEraser

    /// For Settings ▸ Retouch's Reset to Defaults.
    var pencilOptionsAreDefault: Bool {
        onlyPencilPaints == Self.defaultOnlyPencilPaints && squeezeAction == Self.defaultSqueezeAction
    }

    /// Only Pencil paints follows the system's setting again.
    func resetPencilOptions() {
        onlyPencilPaints = Self.defaultOnlyPencilPaints
        squeezeAction = Self.defaultSqueezeAction
        UserDefaults.standard.removeObject(forKey: Self.onlyPencilKey)
        UserDefaults.standard.removeObject(forKey: Self.squeezeKey)
    }

    nonisolated private static let onlyPencilKey = "OnlyPencilPaints"
    nonisolated private static let squeezeKey = "PencilSqueeze"

    /// The answer to `present`: the render, and its painted image with its
    /// picture, or `nil` when it shows as stacked.
    var onPresented: ((String, (image: CGImage, picture: Picture)?) -> Void)?
    /// Why, in the engine's or the app's words.
    var onFailed: ((UserText) -> Void)?
    var onNotice: ((UserText) -> Void)?

    /// The canvases, held weakly. SwiftUI may make a canvas twice and keep
    /// either, so the last made is not always the one shown.
    @ObservationIgnored private let canvases = NSHashTable<AnyObject>.weakObjects()

    private var liveCanvases: [PaintCanvasSink] {
        canvases.allObjects.compactMap { $0 as? PaintCanvasSink }
    }

    /// The session keeps as many full-size tiles as the canvases want, so
    /// tiles in view are never rendered twice, and none while each shows a
    /// smaller level.
    func fullTilesWantedChanged() {
        session?.keepFullTiles(count: UInt32(liveCanvases.map(\.fullTilesWanted).max() ?? 0))
    }

    /// One made after the render opened catches up.
    func attach(_ canvas: PaintCanvasSink) {
        canvases.add(canvas)
        if case .ready(let tiles?)? = painting?.canvas, !tiles.tileLevels.isEmpty {
            canvas.paintOpened(tiles)
        }
    }
    /// Also for work off the main actor.
    @ObservationIgnored private(set) var session: PaintSession?

    init() {
        radius = ranges.radius.defaultValue
        hardness = ranges.hardness.defaultValue
        opacity = ranges.opacity.defaultValue
        flow = ranges.flow.defaultValue
        tolerance = ranges.tolerance.defaultValue
        let defaults = UserDefaults.standard
        if UIDevice.current.userInterfaceIdiom == .pad {
            onlyPencilPaints = defaults.object(forKey: Self.onlyPencilKey) as? Bool
                ?? Self.defaultOnlyPencilPaints
        } else {
            onlyPencilPaints = false
        }
        squeezeAction = defaults.string(forKey: Self.squeezeKey).flatMap(SqueezeAction.init)
            ?? Self.defaultSqueezeAction
    }

    /// Start the paint session, its reports read here in order
    /// (`PaintObserverBridge.reports`); one for a render no longer open is
    /// dropped.
    func start(_ engine: Engine) {
        guard session == nil else { return }
        let observer = PaintObserverBridge()
        Task { [weak self] in
            for await report in observer.reports {
                guard let self else { return }
                self.take(report)
            }
        }
        session = engine.paintSession(observer: observer, displaySide: Self.displaySide)
    }

    private func take(_ report: PaintObserverBridge.Report) {
        switch report {
        case .opened(let id, let levels):
            opened(id, levels: levels)
        case .tiles(let id, let tiles):
            guard id == renderID else { return }
            for canvas in liveCanvases { canvas.paintTilesChanged(tiles) }
        case .state:
            refresh()
        case .presented(let id, let painted):
            onPresented?(id, painted)
        case .sourceShown(let id, let source, let image, let picture):
            sourceShown(id, source, image: image, picture: picture)
        case .failed(let message):
            onFailed?(.verbatim(message))
        case .notice(let id, let message):
            guard id == renderID else { return }
            onNotice?(.verbatim(message))
        }
    }

    var brush: Brush {
        Brush(
            radius: radius, hardness: hardness, opacity: opacity, flow: flow,
            tolerance: toleranceOn ? tolerance : nil)
    }

    var erasing: Bool {
        if case .original? = selection { true } else { false }
    }

    var sourceFrame: String? {
        if case .frame(let path)? = selection { path } else { nil }
    }

    var sourceRender: String? {
        if case .render(let id)? = selection { id } else { nil }
    }

    /// The frames painted pixels came from in this retouch.
    var paintedFrom: Set<String> {
        Set((status?.paintedFrom ?? []).compactMap { if case .frame(let path) = $0 { path } else { nil } })
    }

    /// The renders painted pixels came from in this retouch.
    var paintedFromRenders: Set<String> {
        Set((status?.paintedFrom ?? []).compactMap { if case .render(let id) = $0 { id } else { nil } })
    }

    /// The source the worker delivered last, with tiles at the render's
    /// full size: what the pane, Split and the peek show.
    var sourcePane: PanePicture? {
        painting?.loadedSource.map { PanePicture($0.image, picture: $0.picture) }
    }

    private var mapLighting: MapLighting? {
        if case .depthMap(_, let light)? = painting?.view { light } else { nil }
    }

    var litFrame: String? {
        mapLighting?.light?.file
    }

    /// The frame painted from's lit map, or while it loads the one before;
    /// `nil` for the map whole.
    var shownLitMap: LitMap? {
        guard let painting, let image = mapLighting?.light?.picture.shown,
            image.result == ObjectIdentifier(painting.stack)
        else { return nil }
        return image
    }

    /// The render keeps where its detail came from, so the sharpest frame
    /// at a point can be found.
    var canPickAtPoint: Bool {
        painting?.stack.hasContribution() ?? false
    }

    /// "Frame 23", "#3", "Original".
    func label(of source: SourceKind?, renders: [RenderInfo] = []) -> UserText {
        switch source {
        case .original:
            .app(LocalizedStringResource(
                "Original", comment: "What the brush paints from: the render as stacked, which the eraser and Original in the Paint From menu under the photo paint back. After 'Source · ' over the source's picture."))
        case .frame(let path):
            frames.firstIndex(of: path).map { .app(frameLabel($0 + 1)) } ?? .verbatim((path as NSString).lastPathComponent)
        case .render(let id):
            renders.first { $0.id == id }?.numberLabel ?? .app(LocalizedStringResource(
                "Another render", comment: "What the brush paints from: another render of the stack, whose number isn't known."))
        case nil:
            .app(LocalizedStringResource("None", comment: "What the brush paints from: nothing is picked yet."))
        }
    }

    // MARK: Opening

    /// Paint `result`, saved as `renderID`, from `frame` (else the session
    /// picks), in the last layout. `standIn` shows until the session's
    /// tiles come; `lease` holds the frames' scopes.
    func open(_ result: StackResult, renderID: String, frame: String?, standIn: CGImage?, lease: Lease) {
        guard let session else { return }
        close()
        do {
            try session.open(result: result, renderId: renderID, firstSource: frame)
        } catch {
            onFailed?(AppModel.message(error))
            return
        }
        begin(Open(
            renderID: renderID, stack: result, frames: result.frames(), lease: lease,
            canvas: .opening(standIn: standIn), status: session.status(), view: .layout(lastLayout)))
    }

    /// Retry on the canvas's card: the session opens it again.
    func retryOpen() {
        guard case .failed? = painting?.canvas else { return }
        stateLog.debug("paint: retrying the open")
        session?.retry()
        refresh()
    }

    private func begin(_ open: Open) {
        app?.setMode(.retouching(open))
        let (canvas, view) = (open.canvas.logName, open.view.logName)
        stateLog.debug("paint: open, canvas \(canvas, privacy: .public), \(view, privacy: .public)")
    }

    #if DEBUG
    /// Automation only: paint mode's controls over a fake result
    /// (`-FakeRetouch YES`), with no worker. The canvas shows `picture`, the
    /// pane `source`; `canvas` is ready with `picture` in place of tiles, or
    /// opening or failed with it as stand-in.
    func openForLayout(
        _ result: StackResult, renderID: String, frames: [String], picture: CGImage?, source: CGImage?,
        canvas: PaintCanvasState = .ready(tiles: nil)
    ) {
        close()
        let first = frames.first.map { SourceKind.frame(path: $0) } ?? .original
        let said: DeptheosEngine.PaintCanvas = switch canvas {
        case .opening: .opening
        case .ready: .ready
        case .failed(let reason, _): .failed(message: String(reason.string))
        }
        let status = PaintStatus(
            renderId: renderID, canvas: said, selection: first, ready: true, lastFrame: frames.first,
            litFrame: nil, undo: 0, redo: 0, painted: false, unsaveable: nil, paintedFrom: [])
        var open = Open(
            renderID: renderID, stack: result, frames: frames, lease: .none, canvas: canvas, status: status,
            view: .layout(lastLayout))
        if let first = frames.first, let source { open.loadedSource = (.frame(path: first), source, nil) }
        // Opening or failed, the stand-in is the canvas's own.
        if case .ready = canvas { layoutPicture = picture }
        begin(open)
    }

    /// Automation: what the canvas shows in place of the worker's tiles.
    private(set) var layoutPicture: CGImage?

    /// Automation: `frames` marked as painted from.
    func markPaintedForLayout(_ frames: [String]) {
        whileOpen { $0.status?.paintedFrom = frames.map { .frame(path: $0) } }
    }
    #endif

    /// The render as the canvas shows it now, at the viewer's size, from
    /// the session's display: shown on leaving until `present`'s answer.
    /// `nil` with nothing painted or nothing to make it from.
    func paintedImage() -> CGImage? {
        guard painted else { return nil }
        return session?.canvasImage()?.makeCGImage()
    }

    /// Leave paint mode, the paint saved; the next opening starts in the
    /// layout on screen.
    func close() {
        guard let painting else { return }
        lastLayout = painting.view.layout
        // Tiles after this go nowhere: leaving shows `paintedImage`, then
        // `present`'s answer, which has every stroke.
        session?.close()
        app?.setMode(.viewing)
        stateLog.debug("paint: closed")
    }

    private var painting: Open? {
        if case .retouching(let open)? = app?.mode { open } else { nil }
    }

    /// Nothing when no render is open.
    private func whileOpen(_ change: (inout Open) -> Void) {
        app?.whileRetouching(change)
    }

    private func opened(_ id: String, levels: [PaintLevel]) {
        guard id == renderID, let session else { return }
        let tiles = PaintTiles(session: session, levels: levels)
        setCanvas(.ready(tiles: tiles))
        for canvas in liveCanvases { canvas.paintOpened(tiles) }
    }

    /// Read the session's status again after a call or report: reports only
    /// say it changed, so a late one never restores an older status. A
    /// failed open shows on the canvas with Retry and Done, not an alert a
    /// dismissing sheet could hide.
    private func refresh() {
        guard let painting, let new = session?.status(), new.renderId == painting.renderID else { return }
        guard new != painting.status else { return }
        let lit = painting.status?.litFrame
        whileOpen { $0.status = new }
        switch (new.canvas, painting.canvas) {
        case (.failed(let message), _):
            setCanvas(.failed(.verbatim(message), standIn: painting.canvas.standIn))
        case (.opening, .failed(_, let standIn)):
            setCanvas(.opening(standIn: standIn))
        case (.opening, _), (.ready, _):
            break
        }
        if new.litFrame != lit { refreshMapLight() }
    }

    /// Shown in the pane only while `id` is open.
    private func sourceShown(_ id: String, _ source: SourceKind, image: CGImage, picture: Picture) {
        guard id == renderID else { return }
        whileOpen { $0.loadedSource = (source, image, picture) }
    }

    // MARK: The state's changes

    private func setCanvas(_ new: PaintCanvasState) {
        whileOpen { open in
            stateLog.debug("paint: canvas \(open.canvas.logName, privacy: .public) → \(new.logName, privacy: .public)")
            open.canvas = new
        }
    }

    private func setView(_ new: PaintView) {
        whileOpen { open in
            if open.view.logName != new.logName {
                stateLog.debug("paint: view \(open.view.logName, privacy: .public) → \(new.logName, privacy: .public)")
            }
            open.view = new
        }
    }

    private func setTouch(_ new: PaintTouch) {
        guard let painting, painting.touch != new else { return }
        whileOpen { open in
            stateLog.debug("paint: touch \(String(describing: open.touch), privacy: .public) → \(String(describing: new), privacy: .public)")
            open.touch = new
        }
    }

    // MARK: What the brush paints from

    /// A tap in the list or on the depth map.
    func pick(_ frame: String) {
        guard active, frames.contains(frame) else { return }
        session?.pickFrame(path: frame)
        refresh()
    }

    /// Painted from on screen at once, its pixels fetched at the session's
    /// pace.
    func scrub(to frame: String) {
        guard active, frames.contains(frame) else { return }
        session?.scrubTo(path: frame)
        refresh()
    }

    /// Paint from another of the stack's renders, shown as picked at once;
    /// one that can't be read is said, and the selection goes back.
    func pickRender(_ id: String) {
        guard active else { return }
        do {
            try session?.pickRender(renderId: id)
        } catch {
            onFailed?(AppModel.message(error))
        }
        refresh()
    }

    /// The Pencil's double-tap, set to switch tools.
    func toggleEraser() {
        if erasing { useBrush() } else { useEraser() }
    }

    /// Paint the original back (Original in Paint From too).
    func useEraser() {
        guard active, !erasing else { return }
        session?.pickOriginal()
        refresh()
    }

    /// Back to what was painted from before the eraser, or the first frame.
    func useBrush() {
        guard active, erasing else { return }
        session?.useBrush()
        refresh()
    }

    /// Frames, in Paint From: the frame painted from last, or the first.
    func useFrames() {
        guard active, sourceFrame == nil else { return }
        stateLog.debug("paint: back to the frames")
        session?.useFrames()
        refresh()
    }

    /// ← and →: the frame `step` places on, from the original the first or
    /// last. From a render, `AppModel.stepPaintSource` steps renders.
    func stepFrame(by step: Int) {
        guard active else { return }
        session?.step(by: Int32(clamping: step))
        refresh()
    }

    /// Pick the frame sharpest at `point` (image coordinates), then paint
    /// again. Off the image picks nothing; a render with no record of where
    /// its detail came from says so (`onNotice`).
    func pickFrame(at point: CGPoint) {
        guard active else { return }
        setTouch(.paints)
        session?.pickAt(x: Float(point.x), y: Float(point.y))
        refresh()
    }

    func toggleFramePicker() {
        if touch == .picksFrame {
            setTouch(.paints)
        } else if canPickAtPoint {
            setTouch(.picksFrame)
        }
    }

    // MARK: What is on screen

    func setLayout(_ kind: PaintLayout.Kind) {
        let layout: PaintLayout = switch kind {
        case .canvas: .canvas
        case .split: .split(divider: lastDivider)
        case .sideBySide: .sideBySide
        case .topBottom: .topBottom
        }
        setView(.layout(layout))
    }

    /// As a fraction of the photo's width.
    func setDivider(_ fraction: CGFloat) {
        guard case .layout(.split) = view else { return }
        let fraction = min(max(fraction, 0.02), 0.98)
        lastDivider = fraction
        setView(.layout(.split(divider: fraction)))
    }

    private var lastDivider: CGFloat = 0.5

    func setPeeking(_ on: Bool) {
        switch view {
        case .layout(let layout) where on: setView(.peeking(back: layout))
        case .depthMap(let layout, _) where on: setView(.peeking(back: layout))
        case .peeking(let layout) where !on: setView(.layout(layout))
        case .layout, .peeking, .depthMap: break
        }
    }

    func togglePeek() { setPeeking(!peeking) }

    /// Depth Map over the canvas, lit for the frame painted from: a drag
    /// pans, a tap paints from the frame there, nothing paints.
    func toggleDepthMap() {
        switch view {
        case .depthMap(let back, _): setView(.layout(back))
        case .layout(let layout), .peeking(let layout):
            setView(.depthMap(back: layout, light: .whole))
            refreshMapLight()
        }
    }

    /// Light the map as the session says: for the frame painted from where
    /// the render keeps its detail's sources, else whole. One picture at a
    /// time through `mapLighter`, each applied only while still awaited.
    private func refreshMapLight() {
        guard let painting, case .depthMap(_, let light) = painting.view else { return }
        guard let file = painting.status?.litFrame else {
            if light.light != nil { setMapLighting(.whole) }
            return
        }
        mapLighter.light(
            file, stacked: painting.frames.contains(file), stack: painting.stack,
            current: { [weak self] in self?.mapLighting },
            set: { [weak self] light in self?.setMapLighting(light) },
            failed: { [weak self] _ in self?.setMapLighting(.whole) })
    }

    private func setMapLighting(_ light: MapLighting) {
        guard case .depthMap(let back, _) = view else { return }
        setView(.depthMap(back: back, light: light))
    }

    // MARK: Strokes

    /// Only on a ready canvas: one begun on the stand-in would be painted,
    /// unseen, once the open lands. The worker ignores samples of a stroke
    /// never begun.
    func beginStroke(_ sample: PaintSample) {
        guard canvasReady else { return }
        session?.beginStroke(brush: brush, sample: sample)
        refresh()
    }

    func addSamples(_ samples: [PaintSample]) {
        session?.addSamples(samples: samples)
    }

    func endStroke() {
        session?.endStroke()
    }

    func undo() { session?.undo() }
    func redo() { session?.redo() }

    /// Every pixel back to the original, history cleared: not undoable, so
    /// the UI asks first.
    func reset() { session?.reset() }

    // MARK: Outside paint mode

    /// `onPresented` answers, `nil` if it has no paint.
    func present(_ result: StackResult, renderID: String) {
        try? session?.present(result: result, renderId: renderID)
    }

    /// Write `result` with its paint as `format` with `options`, under
    /// `request` (`AppModel.runExport`'s, lasting).
    static func export(
        _ session: PaintSession?, request: Request, result: StackResult, renderID: String,
        directory: String, name: String, format: ExportFormat, options: ExportPreferences
    ) async throws -> String {
        try await request.run { request in
            guard let session else {
                return try await result.export(
                    request: request, directory: directory, name: name, format: format,
                    preferences: options)
            }
            return try await session.export(
                request: request, result: result, renderId: renderID, directory: directory,
                name: name, format: format, preferences: options)
        }
    }

    /// Retouch on a deleted render closes. The engine had the worker drop
    /// their paint and close its files first.
    func remove(_ renderIDs: [String]) {
        if let renderID, renderIDs.contains(renderID) { close() }
    }

    /// The worker's sources follow the relink (paint names frames by root,
    /// so none of it changes). The render open follows as the app reopens
    /// it (`AppModel.reopenShown`).
    func relink(_ moved: [FolderMove]) {
        session?.relink(moved: moved)
    }

    /// Write unsaved paint now and close its files (the app is going to the
    /// background). `false` when a commit failed (said already), paint is
    /// in RAM only, or the save ran out of time.
    @discardableResult
    func save() async -> Bool {
        guard let session else { return true }
        let saved = (try? await session.save(timeoutMs: Self.saveTimeoutMs)) ?? false
        if !saved { stateLog.error("leaving: the retouching isn't all saved") }
        return saved
    }

    /// Back in front: a stopped lit map, if still wanted, loads.
    func askStoppedAgain() {
        mapLighter.askAgain()
    }
}

/// How Retouch lays out the canvas: alone, split with the source left, the
/// source beside, or above. Only the canvas paints; the pane follows its
/// zoom.
enum PaintLayout: Equatable {
    case canvas
    case split(divider: CGFloat)
    case sideBySide
    case topBottom

    /// As the layout menu names them.
    enum Kind: CaseIterable {
        case canvas
        case split
        case sideBySide
        case topBottom

        var label: LocalizedStringResource {
            switch self {
            case .canvas:
                LocalizedStringResource(
                    "Painted", comment: "Retouch: the label over the painted result beside or under its source, and the layout menu's item for the painted result alone.")
            case .split: CompareLayout.Kind.split.label
            case .sideBySide: CompareLayout.Kind.sideBySide.label
            case .topBottom: CompareLayout.Kind.topBottom.label
            }
        }

        var systemImage: String {
            switch self {
            case .canvas: "square"
            case .split: CompareLayout.Kind.split.systemImage
            case .sideBySide: CompareLayout.Kind.sideBySide.systemImage
            case .topBottom: CompareLayout.Kind.topBottom.systemImage
            }
        }
    }

    var kind: Kind {
        switch self {
        case .canvas: .canvas
        case .split: .split
        case .sideBySide: .sideBySide
        case .topBottom: .topBottom
        }
    }
}

/// What Retouch shows: a layout, the source in the canvas's place, or the
/// depth map over the canvas lit for the frame painted from (whole for the
/// original or a render), the last two with the layout to go back to.
enum PaintView {
    case layout(PaintLayout)
    case peeking(back: PaintLayout)
    case depthMap(back: PaintLayout, light: MapLighting)

    /// The layout on screen, or under the peek or the map.
    var layout: PaintLayout {
        switch self {
        case .layout(let layout), .peeking(let layout), .depthMap(let layout, _): layout
        }
    }

    /// For the log.
    var logName: String {
        switch self {
        case .layout(let layout): "\(layout.kind)"
        case .peeking(let back): "peeking(back: \(back.kind))"
        case .depthMap(let back, let light): "depthMap(back: \(back.kind), \(light.logName))"
        }
    }
}

/// What a touch on Retouch's photo does: paint (a finger where only the
/// Pencil paints moves the view), or pick the frame sharpest there, once.
enum PaintTouch: Equatable {
    case paints
    case picksFrame
}

/// Retouch's canvas, changed only by `PaintModel` as the session reports:
/// opening, the viewer's picture standing in at the same fit, no strokes;
/// ready, with the session's tiles (none for automation); or failed, with
/// why, Retry and Done. Retry (`retryOpen`) opens it again.
enum PaintCanvasState {
    case opening(standIn: CGImage?)
    case ready(tiles: PaintTiles?)
    case failed(UserText, standIn: CGImage?)

    var standIn: CGImage? {
        switch self {
        case .opening(let standIn), .failed(_, let standIn): standIn
        case .ready: nil
        }
    }

    var failure: UserText? {
        if case .failed(let reason, _) = self { reason } else { nil }
    }

    var logName: String {
        switch self {
        case .opening: "opening"
        case .ready: "ready"
        case .failed: "failed"
        }
    }
}

/// The open render's tiles for the canvas's `TiledPictureView`, one per
/// open so a tile never lands in another's canvas. A full-size tile not
/// yet rendered is nothing until the worker reports it changed.
nonisolated final class PaintTiles: TileSource {
    private let session: PaintSession
    let tileLevels: [TileLevel]

    init(session: PaintSession, levels: [PaintLevel]) {
        self.session = session
        tileLevels = levels.map(TileLevel.init)
    }

    func fetchTiles(_ asked: [TileAsk], request: Request) async throws -> [TileFetched] {
        try await session.tiles(request: request, asked: asked)
    }
}

/// The paint session's reports as one stream read on the main actor
/// (`PaintModel.start`), in the order sent. Unbounded: none may be dropped.
nonisolated final class PaintObserverBridge: PaintObserver, @unchecked Sendable {
    enum Report: Sendable {
        case opened(String, [PaintLevel])
        case tiles(String, [PaintTile])
        /// Only that it changed: the model reads the status then.
        case state
        /// `nil`: an image that can't be drawn shows as stacked.
        case presented(String, (image: CGImage, picture: Picture)?)
        case sourceShown(String, SourceKind, CGImage, Picture)
        case failed(String)
        case notice(String, String)
    }

    let reports: AsyncStream<Report>
    private let continuation: AsyncStream<Report>.Continuation

    init() {
        (reports, continuation) = AsyncStream.makeStream(of: Report.self, bufferingPolicy: .unbounded)
    }

    func opened(renderId: String, levels: [PaintLevel]) { continuation.yield(.opened(renderId, levels)) }
    func tilesChanged(renderId: String, tiles: [PaintTile]) { continuation.yield(.tiles(renderId, tiles)) }
    func stateChanged() { continuation.yield(.state) }
    func closed(renderId: String) {}

    func presented(renderId: String, painted: PaintedDisplay?) {
        let image = painted?.image.makeCGImage()
        continuation.yield(.presented(renderId, image.flatMap { image in painted.map { (image, $0.picture) } }))
    }

    func sourceShown(renderId: String, source: SourceKind, image: DisplayImage, picture: Picture) {
        if let cgImage = image.makeCGImage() { continuation.yield(.sourceShown(renderId, source, cgImage, picture)) }
    }

    func failed(renderId: String?, message: String) { continuation.yield(.failed(message)) }
    func notice(renderId: String, message: String) { continuation.yield(.notice(renderId, message)) }
}
