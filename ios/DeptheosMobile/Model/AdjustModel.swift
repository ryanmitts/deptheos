import CoreGraphics
import Foundation
import Observation
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// A drag's picture over the settled one, and the shown pixels it covers.
struct AdjustOverlay {
    let image: CGImage
    /// In the shown picture's pixels: turned and flipped, uncropped.
    let covers: CGRect
}

/// Before, asked under a turn and flip.
struct BeforeAsked: LoadRequest {
    let ticket: Ticket
    let turn: Turn
    let flip: Flip
}

/// Before drawn: the render as stored with its paint, turned and flipped.
struct BeforeMade: LoadMade {
    let request: Ticket
    let picture: PanePicture
    let turn: Turn
    let flip: Flip
}

/// Adjust: the sliders and the reframe on a render, as on the desktop. The
/// engine's session (`AdjustSession`, ADR-153) holds the value, its undo,
/// the drawing and the writes; this shows what it says and sends it each
/// change. No adjust logic is here.
@Observable
final class AdjustModel {
    /// Adjust on one render: `AppModel.mode`'s adjusting case
    /// (`RenderMode`), entered by `open` and left by `close`; inside it only
    /// `AdjustModel`'s methods change anything, through `whileOpen`
    /// (ADR-153, ADR-171).
    ///
    /// Lifecycle: the case's own data. The session's status (opening,
    /// ready, away or failed) and picture are the engine's, read as it
    /// tells; Before is a `Latest`:
    ///
    ///     before: none ──opened, or a turn or flip──▶ loading ──drawn──▶ loaded
    ///     overlay: none ──a tick, or the view on screen──▶ shown ──settled──▶ none
    struct Open {
        let renderID: String
        let result: StackResult
        /// The reframe the viewer's pictures came under as Adjust opened.
        let opened: Reframe
        var status: AdjustStatus?
        var panel: AdjustPanel?
        var overlay: AdjustOverlay?
        var histogram: AdjustHistogram?
        var before = Latest<BeforeAsked, BeforeMade>.none
    }

    /// Whose mode holds Adjust's `Open`: set once at launch.
    @ObservationIgnored weak var app: AppModel?
    @ObservationIgnored private var session: AdjustSession?
    @ObservationIgnored private let beforeLoad = LatestLoad(name: "adjust before")
    /// The last view told, so a scroll that moves nothing tells nothing.
    @ObservationIgnored private var toldView: (zoom: Float, visible: PictureRegion)?
    /// Renders whose thumbnail rewrite hasn't landed: asked again once
    /// back in front (a rewrite in the background is refused).
    @ObservationIgnored private var unrewritten: Set<String> = []
    /// The reframe Adjust last showed, kept past its close, for the viewer
    /// to tell whether its pictures still match.
    @ObservationIgnored private(set) var lastReframe: Reframe?

    /// The settled picture of `id`, its image and picture.
    var onSettled: ((String, CGImage, Picture) -> Void)?
    /// Adjust left `id`; `redraw`: what is on screen isn't its saved value.
    var onClosed: ((String, Bool) -> Void)?
    var onRewritten: ((String) -> Void)?
    var onNotice: ((UserText) -> Void)?

    private var adjusting: Open? {
        if case .adjusting(let open)? = app?.mode { open } else { nil }
    }

    var renderID: String? { adjusting?.renderID }
    var status: AdjustStatus? { adjusting?.status }
    var panel: AdjustPanel? { adjusting?.panel }
    var histogram: AdjustHistogram? { adjusting?.histogram }
    var overlay: AdjustOverlay? { adjusting?.overlay }
    var before: PanePicture? { adjusting?.before.shown?.picture }
    var value: AdjustValue? { adjusting?.status?.value }

    private func whileOpen(_ change: (inout Open) -> Void) {
        app?.whileAdjusting(change)
    }

    /// The session, its updates read on the main actor in order
    /// (`AdjustObserverBridge.reports`).
    func start(_ engine: Engine) {
        guard session == nil else { return }
        let observer = AdjustObserverBridge()
        Task { [weak self] in
            for await report in observer.reports {
                guard let self else { return }
                self.take(report)
            }
        }
        session = engine.adjustSession(observer: observer, displaySide: AppModel.displaySide)
        Task { [weak self] in
            for await _ in NotificationCenter.default.notifications(named: ProcessInfo.thermalStateDidChangeNotification) {
                self?.thermalChanged()
            }
        }
        thermalChanged()
    }

    private func take(_ report: AdjustObserverBridge.Report) {
        switch report {
        case .state:
            refresh()
        case .preview(let overlay, let histogram):
            guard adjusting != nil else { return }
            whileOpen {
                $0.overlay = overlay
                $0.histogram = histogram
            }
            app?.view(.previewed)
        case .settled(let id, let image, let picture, let histogram):
            guard adjusting?.renderID == id else { return }
            whileOpen {
                $0.histogram = histogram
                $0.overlay = nil
            }
            if let image { onSettled?(id, image, picture) }
            app?.view(.settled)
        case .counted(let id, let histogram):
            guard adjusting?.renderID == id else { return }
            whileOpen { $0.histogram = histogram }
        case .closed(let id, let redraw):
            onClosed?(id, redraw)
        case .rewritten(let id):
            unrewritten.remove(id)
            onRewritten?(id)
        case .notice(let message):
            onNotice?(.verbatim(message))
        }
    }

    /// The session's status, Before drawn again on a turn or flip.
    private func refresh() {
        guard let session, let open = adjusting else { return }
        let status = session.status()
        if let status, status.renderId != open.renderID { return }
        whileOpen {
            $0.status = status
            if $0.panel == nil { $0.panel = session.panel() }
        }
        if let status { lastReframe = status.value.reframe() }
        if case .failed(let message) = status?.phase {
            stateLog.debug("adjust: failed: \(message, privacy: .public)")
        }
        askBefore()
    }

    // MARK: Open and close

    /// Adjust render `renderID` of `result`, its pictures handed over under
    /// `reframe`. Another mode must be closed first.
    func open(_ result: StackResult, renderID: String) {
        guard let session, let app, app.viewing else {
            stateLog.debug("adjust: not opened: \(self.session == nil ? "no session" : "another mode", privacy: .public)")
            return
        }
        do {
            try session.open(result: result, renderId: renderID)
        } catch {
            stateLog.info("adjust: open refused: \(error.localizedDescription, privacy: .public)")
            app.report(error)
            return
        }
        toldView = nil
        lastReframe = nil
        app.setMode(.adjusting(Open(renderID: renderID, result: result, opened: result.reframe())))
        refresh()
    }

    /// Leave Adjust, a change in progress committed: the mode goes at once,
    /// `onClosed` once its write is in.
    func close() {
        guard let open = adjusting else { return }
        // Changed: its thumbnail is drawn again, asked again if refused.
        if (open.status?.undo ?? 0) > 0 { unrewritten.insert(open.renderID) }
        session?.leave()
        app?.setMode(.viewing)
    }

    // MARK: Changes

    func set(_ slider: Slider, to value: Double) { session?.setSlider(slider: slider, value: value) }
    func setReframe(_ reframe: Reframe) { session?.setReframe(reframe: reframe) }
    /// The change ends: written and undoable if the value moved.
    func commit() { session?.commit() }
    func undo() { session?.undo() }
    func redo() { session?.redo() }
    func reset(_ slider: Slider) { session?.reset(slider: slider) }
    func resetAll() { session?.resetAll() }
    func paste(_ copied: AdjustValue) { session?.paste(copied: copied) }
    func retry() { session?.retry() }

    /// White balance from the stored pixel under `point`.
    func eyedrop(at pixel: PicturePixel) {
        session?.eyedrop(x: Double(pixel.x) + 0.5, y: Double(pixel.y) + 0.5)
    }

    /// The viewer's zoom (screen pixels per shown pixel) over `visible`, in
    /// the shown picture's pixels, told in stored pixels.
    func viewChanged(zoom: CGFloat, visible: CGRect) {
        guard let session, let open = adjusting, let value = open.status?.value else { return }
        let size = open.result.storedSize
        let framing = PaneFraming(width: size.width, height: size.height, reframe: value.reframe(), pane: .uncropped)
        let corners = [
            CGPoint(x: visible.minX, y: visible.minY), CGPoint(x: visible.maxX, y: visible.minY),
            CGPoint(x: visible.minX, y: visible.maxY), CGPoint(x: visible.maxX, y: visible.maxY),
        ].map { framing.toStored(p: PicturePoint(x: Double($0.x), y: Double($0.y))) }
        let x0 = max(corners.map(\.x).min() ?? 0, 0)
        let y0 = max(corners.map(\.y).min() ?? 0, 0)
        let x1 = min(corners.map(\.x).max() ?? 0, Double(size.width))
        let y1 = min(corners.map(\.y).max() ?? 0, Double(size.height))
        guard x1 > x0, y1 > y0 else { return }
        let region = PictureRegion(
            x: UInt32(x0.rounded(.down)), y: UInt32(y0.rounded(.down)),
            width: UInt32((x1 - x0.rounded(.down)).rounded(.up)), height: UInt32((y1 - y0.rounded(.down)).rounded(.up)))
        let told = (zoom: Float(zoom), visible: region)
        if let last = toldView, last.visible == region, abs(last.zoom - told.zoom) < 0.001 { return }
        toldView = told
        session.view(zoom: told.zoom, visible: region)
    }

    /// Before held up: drawn as Adjust opens and on each turn or flip.
    private func askBefore() {
        guard let open = adjusting, let value = open.status?.value else { return }
        let reframe = value.reframe()
        let current = open.before.asked ?? open.before.shown.map {
            BeforeAsked(ticket: $0.request, turn: $0.turn, flip: $0.flip)
        }
        if let current, current.turn == reframe.turn, current.flip == reframe.flip { return }
        guard let session else { return }
        let request = BeforeAsked(ticket: beforeLoad.ticket(), turn: reframe.turn, flip: reframe.flip)
        let (result, id) = (open.result, open.renderID)
        let asked = Reframe(crop: nil, flip: reframe.flip, turn: reframe.turn)
        whileOpen { $0.before = $0.before.asking(request, keeping: { _ in false }) }
        stateLog.debug("adjust: Before asked \(request.ticket.number)")
        beforeLoad.load(
            request,
            latest: { [weak self] in
                guard let open = self?.adjusting, open.renderID == id else { return nil }
                return open.before
            },
            set: { [weak self] state in self?.whileOpen { $0.before = state } },
            make: {
                let pictured = try await Request().load {
                    try await session.before(
                        request: $0, result: result, renderId: id, reframe: asked, baseSide: AppModel.displaySide)
                }
                guard let pictured, let base = pictured.base else { return nil }
                let image = try await Blocking.run { base.makeCGImage().map(AppModel.preparedForDisplay) }
                guard let image else { return nil }
                return BeforeMade(
                    request: request.ticket, picture: PanePicture(image, picture: pictured.picture),
                    turn: request.turn, flip: request.flip)
            })
    }

    // MARK: Leaving and coming back

    /// To the background: a drag ends and keeps its write; the GPU copies
    /// go until `back`.
    func away() { session?.away() }

    /// In front again: the copies are made again, and thumbnails the
    /// background refused are drawn again.
    func back(_ engine: Engine?) {
        session?.back()
        guard let engine else { return }
        for id in unrewritten {
            Task { [weak self] in
                guard (try? await engine.rewriteThumbnail(renderId: id)) == true else { return }
                self?.unrewritten.remove(id)
                self?.onRewritten?(id)
            }
        }
    }

    func shed(_ level: MemoryShed) { session?.shed(level: level) }

    /// The render's stored pixels changed (its paint): drawn again.
    func storedChanged() { session?.storedChanged() }

    /// Serious or critical: a drag's ticks at fit draw smaller.
    private func thermalChanged() {
        let state = ProcessInfo.processInfo.thermalState
        session?.setHot(hot: state == .serious || state == .critical)
    }
}

extension StackResult {
    /// The stored picture's size, before any turn.
    var storedSize: (width: UInt32, height: UInt32) { (width(), height()) }
}

/// The session's updates, off the main actor, each into one stream read on
/// it in order. A preview's image is made here, reading the engine's pixels
/// in place (ADR-139).
nonisolated final class AdjustObserverBridge: AdjustObserver, @unchecked Sendable {
    enum Report: @unchecked Sendable {
        /// Only that it changed: the model reads the status then.
        case state
        case preview(AdjustOverlay, AdjustHistogram)
        /// `nil` image: one that can't be drawn.
        case settled(String, CGImage?, Picture, AdjustHistogram)
        case counted(String, AdjustHistogram)
        case closed(String, Bool)
        case rewritten(String)
        case notice(String)
    }

    let reports: AsyncStream<Report>
    private let continuation: AsyncStream<Report>.Continuation

    init() {
        (reports, continuation) = AsyncStream.makeStream(of: Report.self, bufferingPolicy: .unbounded)
    }

    func stateChanged() { continuation.yield(.state) }

    func previewed(preview: AdjustPreview, histogram: AdjustHistogram) {
        overlay(preview, histogram)
    }

    func onScreen(preview: AdjustPreview, histogram: AdjustHistogram) {
        overlay(preview, histogram)
    }

    private func overlay(_ preview: AdjustPreview, _ histogram: AdjustHistogram) {
        guard let image = preview.makeCGImage() else { return }
        let c = preview.covers()
        let covers = CGRect(x: Int(c.x), y: Int(c.y), width: Int(c.width), height: Int(c.height))
        continuation.yield(.preview(AdjustOverlay(image: image, covers: covers), histogram))
    }

    func settled(renderId: String, image: DisplayImage, picture: Picture, histogram: AdjustHistogram) {
        let made = image.makeCGImage().map(AppModel.preparedForDisplay)
        continuation.yield(.settled(renderId, made, picture, histogram))
    }

    func counted(renderId: String, histogram: AdjustHistogram) {
        continuation.yield(.counted(renderId, histogram))
    }

    func closed(renderId: String, redraw: Bool) { continuation.yield(.closed(renderId, redraw)) }
    func rewritten(renderId: String) { continuation.yield(.rewritten(renderId)) }
    func notice(message: String) { continuation.yield(.notice(message)) }
}
