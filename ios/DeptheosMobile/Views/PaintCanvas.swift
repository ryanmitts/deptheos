import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

/// The render being retouched: pinch and drag to move, paint with a finger
/// or Apple Pencil. On iPhone a finger paints and two fingers move. On iPad
/// a finger paints unless Only Pencil paints is on
/// (`PaintModel.onlyPencilPaints`), when one finger moves, else two do.
/// The Pencil's double-tap toggles the eraser when the system's is set to.
struct PaintCanvas: UIViewRepresentable {
    let paint: PaintModel
    /// Fit is asked for each time this changes.
    var fitRequest = 0
    /// What the controls cover at the bottom past the safe area, which a
    /// zoomed render drags clear of (`ZoomToFit.swift`).
    var below = Below.row(0)
    /// Extra chrome insets past safeArea (e.g., Retouch's side rails or top bar).
    var extraChromeInsets = UIEdgeInsets.zero
    /// Shows the brush ring centered in the clear rect while scrubbing Size or
    /// Hardness in the Retouch tray.
    var previewingBrush = false
    /// All of it, or left of Split's divider.
    var cover = CanvasCover.none
    /// The source's pane, which follows its zoom.
    var sync: ZoomSync?

    func makeUIView(context: Context) -> PaintScrollView {
        let view = PaintScrollView(paint: paint)
        view.accessibilityIdentifier = "paint-canvas"
        paint.attach(view)
        return view
    }

    func updateUIView(_ view: PaintScrollView, context: Context) {
        context.animate {
            view.updateChrome(below: below, extraInsets: extraChromeInsets, controlsHidden: false)
        }
        view.setReady(paint.canvasReady)
        // Until the tiles come, the render's picture in their place.
        view.setStandIn(paint.canvasState?.standIn, size: paint.renderSize)
        view.setRing(radius: CGFloat(paint.radius))
        view.setPreviewingBrush(previewingBrush)
        view.setCover(cover)
        view.setTouch(paint.touch)
        view.updatePanTouches()
        view.join(sync)
        #if DEBUG
        if let picture = paint.layoutPicture { view.showForLayout(picture) }
        #endif
        view.fit(request: fitRequest)
    }
}

/// What covers Retouch's canvas: nothing; the source, whole (peeking) or
/// left of Split's divider (a fraction of the width), nothing until it has
/// a picture; or the depth map lit for the frame painted from, where a drag
/// moves, a tap paints from the frame there, and nothing paints. Each
/// stretched over the render's full size with its own tiles.
enum CanvasCover: Equatable {
    case none
    case source(PanePicture?, split: CGFloat?)
    case map(PanePicture?)
}

/// Retouch's canvas: a `TiledZoomView` of the open render's tiles, with
/// strokes, the brush ring, Sharpest Frame's picks and the Pencil's
/// actions.
final class PaintScrollView: TiledZoomView, PaintCanvasSink, UIPencilInteractionDelegate {
    private let paint: PaintModel
    /// Once the worker has them. Until then the stand-in (the viewer's
    /// picture, stretched to full size) keeps the canvas in place at the
    /// same fit.
    private var tiles: PaintTiles?
    /// While the canvas opens or after its open failed.
    private var standIn: CGImage?
    /// The canvas has its tiles: strokes paint.
    private var ready = false

    /// Light over dark so it shows on any image.
    private let ringDark = CAShapeLayer()
    private let ringLight = CAShapeLayer()
    private var ringRadius: CGFloat = 30
    private var hovering = false
    private var previewingBrush = false
    private let stroke = StrokeRecognizer()

    init(paint: PaintModel) {
        self.paint = paint
        super.init(frame: .zero)
        pictureView.onFullTilesWanted = { [paint] in paint.fullTilesWantedChanged() }

        // The Pencil paints; fingers (and the trackpad) move the view.
        panGestureRecognizer.allowedTouchTypes = [
            UITouch.TouchType.direct, .indirect, .indirectPointer,
        ].map { NSNumber(value: $0.rawValue) }
        pinchGestureRecognizer?.allowedTouchTypes = [UITouch.TouchType.direct, .indirectPointer]
            .map { NSNumber(value: $0.rawValue) }

        stroke.fingerPaints = { [unowned self] in !self.paint.onlyPencilPaints }
        stroke.sample = { [unowned self] touch in self.sample(touch) }
        stroke.onBegin = { [unowned self] sample in
            paint.beginStroke(sample)
            showRing(at: sample)
        }
        stroke.onMove = { [unowned self] samples in
            paint.addSamples(samples)
            if let last = samples.last { showRing(at: last) }
        }
        stroke.onEnd = { [unowned self] in
            paint.endStroke()
            if !hovering { hideRing() }
        }
        addGestureRecognizer(stroke)

        let hover = UIHoverGestureRecognizer(target: self, action: #selector(hovered))
        addGestureRecognizer(hover)
        let pencil = UIPencilInteraction(delegate: self)
        addInteraction(pencil)

        // Sharpest Frame: a tap picks the frame sharpest there; a stroke
        // waits for it.
        pick.addTarget(self, action: #selector(picked))
        pick.isEnabled = false
        addGestureRecognizer(pick)
        // Two fingers tapped together pick it too, where the render keeps
        // the record; a pinch or pan never taps.
        let twoFingers = UITapGestureRecognizer(target: self, action: #selector(picked))
        twoFingers.numberOfTouchesRequired = 2
        twoFingers.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.direct.rawValue)]
        twoFingers.delegate = alongside
        addGestureRecognizer(twoFingers)

        // A finger held shows the source, while fingers don't paint.
        let hold = UILongPressGestureRecognizer(target: self, action: #selector(held))
        hold.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.direct.rawValue)]
        hold.minimumPressDuration = 0.35
        addGestureRecognizer(hold)

        for (layer, color, width) in [(ringDark, UIColor.black, 3.0), (ringLight, .white, 1.0)] {
            layer.fillColor = nil
            layer.strokeColor = color.withAlphaComponent(0.8).cgColor
            layer.lineWidth = width
            layer.isHidden = true
            layer.zPosition = 1
            pictureView.layer.addSublayer(layer)
        }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    // MARK: Paint

    func paintOpened(_ tiles: PaintTiles) {
        guard let full = tiles.tileLevels.first else { return }
        self.tiles = tiles
        // Already laid out at this size on the stand-in: the zoom stays,
        // the tiles cover it in place.
        resize(to: CGSize(width: Int(full.width), height: Int(full.height)))
        standIn = nil
        pictureView.show(base: nil, source: tiles)
        setNeedsLayout()
    }

    /// `picture` stretched over `size` (full size, image pixels) until the
    /// tiles come or after a failed open. Nothing once there are tiles.
    func setStandIn(_ picture: CGImage?, size: CGSize?) {
        guard tiles == nil, picture !== standIn else { return }
        if picture != nil, let size { resize(to: size) }
        standIn = picture
        pictureView.show(base: picture, source: nil)
    }

    /// Lay out at `size`, at fit or at the zoom last settled on this render
    /// (a canvas remade on a size-class change), once there are bounds.
    /// Nothing when already that size.
    private func resize(to size: CGSize) {
        guard size != imageSize, size.width > 0, size.height > 0 else { return }
        layOut(at: size)
        updateRingPath()
        pendingZoom = paint.zoom
        setNeedsLayout()
    }

    /// No stroke starts until ready. Opening again, the old tiles go and
    /// the stand-in shows until the new ones come.
    func setReady(_ ready: Bool) {
        guard ready != self.ready else { return }
        self.ready = ready
        if !ready {
            tiles = nil
            pictureView.show(base: standIn, source: nil)
        }
        updateStroke()
    }

    var fullTilesWanted: Int { pictureView.fullTilesWanted }

    /// Changed tiles, or full-size ones the engine lacked and has now: a
    /// tile showing the half-size one under it is refetched.
    func paintTilesChanged(_ changed: [PaintTile]) {
        pictureView.tilesChanged(changed.map { TileKey(level: $0.level, tx: $0.tx, ty: $0.ty) })
    }

    /// In image pixels.
    private func sample(_ touch: UITouch) -> PaintSample {
        let point = touch.preciseLocation(in: pictureView)
        var pressure: Float = 1
        if touch.type == .pencil, PencilPressure.sensed(touch.force) {
            pressure = PencilPressure.curve(touch.force)
        }
        return PaintSample(x: Float(point.x), y: Float(point.y), pressure: pressure)
    }

    /// The source, whole or left of Split's divider, or nothing.
    func setCover(_ cover: CanvasCover) {
        let mapped = if case .map = cover { true } else { false }
        if mapped != showsMap {
            showsMap = mapped
            updateStroke()
            if mapped { hideRing() }
        }
        switch cover {
        case .none: self.cover(with: nil, split: nil)
        case .source(let source, let fraction): self.cover(with: source, split: fraction)
        case .map(let map): self.cover(with: map, split: nil)
        }
    }

    private let pick = UITapGestureRecognizer()
    /// Lets the two-finger tap be seen alongside the pinch and the pan.
    private let alongside = Alongside()

    /// Paint, or pick the frame sharpest where it lands (then a single tap
    /// picks and no stroke starts).
    func setTouch(_ touch: PaintTouch) {
        let picking = touch == .picksFrame
        guard picksFrame != picking else { return }
        picksFrame = picking
        updateStroke()
    }

    private var picksFrame = false
    private var showsMap = false

    /// Strokes only on a canvas with tiles, while a touch paints and the
    /// map isn't over it; else one finger moves. A single tap picks with
    /// Sharpest Frame on, and over the depth map, which it lights.
    private func updateStroke() {
        pick.isEnabled = picksFrame || showsMap
        stroke.isEnabled = ready && !pick.isEnabled
        updatePanTouches()
    }

    func updatePanTouches() {
        panGestureRecognizer.minimumNumberOfTouches = stroke.isEnabled && stroke.fingerPaints() ? 2 : 1
    }

    @objc private func picked(_ tap: UITapGestureRecognizer) {
        guard tap.state == .ended else { return }
        // Two fingers, and a tap on the map, pick only where the render
        // keeps the record; else they say nothing.
        if tap !== pick || !picksFrame, !paint.canPickAtPoint { return }
        paint.pickFrame(at: tap.location(in: pictureView))
    }

    @objc private func held(_ hold: UILongPressGestureRecognizer) {
        guard !stroke.fingerPaints() else { return }
        switch hold.state {
        case .began: paint.setPeeking(true)
        case .ended, .cancelled, .failed: paint.setPeeking(false)
        default: break
        }
    }

    func pencilInteraction(
        _ interaction: UIPencilInteraction, didReceiveSqueeze squeeze: UIPencilInteraction.Squeeze
    ) {
        switch paint.squeezeAction {
        case .switchEraser:
            if squeeze.phase == .ended { paint.toggleEraser() }
        case .showSource:
            // One action per squeeze, as the eraser's: the source in the
            // canvas's place, or the layout again.
            if squeeze.phase == .ended { paint.togglePeek() }
        case .nothing:
            break
        }
    }

    func pencilInteraction(
        _ interaction: UIPencilInteraction, didReceiveTap tap: UIPencilInteraction.Tap
    ) {
        switch UIPencilInteraction.preferredTapAction {
        case .switchEraser, .switchPrevious: paint.toggleEraser()
        default: break
        }
    }

    // MARK: The ring

    func setRing(radius: CGFloat) {
        guard radius != ringRadius else { return }
        ringRadius = radius
        updateRingPath()
        if previewingBrush { showCenteredPreviewRing() }
    }

    func setPreviewingBrush(_ previewing: Bool) {
        guard previewing != previewingBrush else { return }
        previewingBrush = previewing
        if previewing {
            showCenteredPreviewRing()
        } else if !hovering && !stroke.isStroking {
            hideRing()
        }
    }

    private func showCenteredPreviewRing() {
        guard !showsMap, imageSize.width > 0, imageSize.height > 0, bounds.width > 0 else { return }
        let clear = bounds.inset(by: effectiveFitInsets(fitInsets))
        let viewCenter = CGPoint(x: clear.midX, y: clear.midY)
        let imageCenter = convert(viewCenter, to: pictureView)
        let clamped = CGPoint(
            x: min(max(imageCenter.x, 0), imageSize.width),
            y: min(max(imageCenter.y, 0), imageSize.height))
        updateRingPath()
        showRing(at: PaintSample(x: Float(clamped.x), y: Float(clamped.y), pressure: 1))
    }

    override func zoomChanged() {
        updateRingPath()
        if previewingBrush { showCenteredPreviewRing() }
    }

    private func updateRingPath() {
        let r = ringRadius
        let path = CGPath(ellipseIn: CGRect(x: -r, y: -r, width: 2 * r, height: 2 * r), transform: nil)
        // The same width on screen at any zoom.
        let scale = max(zoomScale, 0.0001)
        withoutAnimation {
            for (layer, width) in [(ringDark, 3.0), (ringLight, 1.0)] {
                layer.path = path
                layer.lineWidth = width / scale
            }
        }
    }

    private func showRing(at sample: PaintSample) {
        withoutAnimation {
            for layer in [ringDark, ringLight] {
                layer.position = CGPoint(x: CGFloat(sample.x), y: CGFloat(sample.y))
                layer.isHidden = false
            }
        }
        // On the source's pane too, at the same image point.
        guard imageSize.width > 0, imageSize.height > 0 else { return }
        sync?.ghost(
            from: self,
            center: CGPoint(x: CGFloat(sample.x) / imageSize.width, y: CGFloat(sample.y) / imageSize.height),
            radius: ringRadius / imageSize.width)
    }

    private func hideRing() {
        withoutAnimation {
            for layer in [ringDark, ringLight] { layer.isHidden = true }
        }
        sync?.ghost(from: self, center: nil, radius: 0)
    }

    // MARK: The source's pane

    /// The source's pane moved: the canvas's place is kept.
    override func peerSettled() { reportOwnZoom() }

    #if DEBUG
    /// Automation: `picture` in place of the tiles a fake result lacks.
    func showForLayout(_ picture: CGImage) {
        let size = CGSize(width: picture.width, height: picture.height)
        guard size != imageSize else { return }
        resize(to: size)
        pictureView.show(base: picture, source: nil)
    }
    #endif

    @objc private func hovered(_ hover: UIHoverGestureRecognizer) {
        switch hover.state {
        case .began, .changed:
            guard !showsMap else { return }
            hovering = true
            let point = hover.location(in: pictureView)
            showRing(at: PaintSample(x: Float(point.x), y: Float(point.y), pressure: 1))
        default:
            hovering = false
            if !stroke.isStroking { hideRing() }
        }
    }

    // MARK: Layout

    override func layoutSubviews() {
        // A finger paints, so the view moves with two.
        panGestureRecognizer.minimumNumberOfTouches = stroke.isEnabled && stroke.fingerPaints() ? 2 : 1
        super.layoutSubviews()
        // A pane resized (a layout chosen, the window changed) follows.
        if bounds.size != syncedBounds {
            syncedBounds = bounds.size
            sync?.moved(self)
        }
    }

    private var syncedBounds = CGSize.zero

    private var fitRequest: Int?
    /// To show once there are bounds.
    private var pendingZoom: CGRect?

    /// When `request` is new.
    func fit(request: Int) {
        defer { fitRequest = request }
        guard let fitRequest, fitRequest != request else { return }
        pendingZoom = nil
        zoomToFit(imageSize, fitInsets: fitInsets, panMargins: panMargins)
        paint.zoom = nil
        sync?.moved(self)
    }

    override func settleZoom() {
        guard let pending = pendingZoom, bounds.width > 0, imageSize != .zero else { return }
        pendingZoom = nil
        show(fraction: pending, of: pictureView, size: imageSize)
        sync?.moved(self)
    }

    /// In the model.
    override func reportOwnZoom() {
        guard imageSize != .zero else { return }
        paint.zoom = visibleFraction(of: pictureView, size: imageSize)
    }
}

/// Lets a recognizer work alongside any other: the two-finger tap beside
/// the pinch and pan.
private final class Alongside: NSObject, UIGestureRecognizerDelegate {
    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
    ) -> Bool { true }
}

/// One stroke: from the Pencil at once, with every sample between frames;
/// from a finger, when fingers paint, once it is clearly not a pan or
/// pinch starting (it moves, or a moment passes).
final class StrokeRecognizer: UIGestureRecognizer {
    /// The system's Only Draw with Apple Pencil is off.
    var fingerPaints: () -> Bool = { false }
    var sample: (UITouch) -> PaintSample = { _ in PaintSample(x: 0, y: 0, pressure: 1) }
    var onBegin: (PaintSample) -> Void = { _ in }
    var onMove: ([PaintSample]) -> Void = { _ in }
    var onEnd: () -> Void = {}

    /// Points on screen, or time, before a finger is a stroke.
    private static let fingerTravel: CGFloat = 10
    /// Long enough for a second finger to make a pinch; after, one landing
    /// is ignored. The held samples are painted when it commits.
    private static let fingerWait: TimeInterval = 0.25

    private var touch: UITouch?
    /// Held until it is clearly a stroke.
    private var held: [PaintSample]?
    private var start: (point: CGPoint, time: TimeInterval) = (.zero, 0)

    var isStroking: Bool { touch != nil && held == nil }

    override init(target: Any?, action: Selector?) {
        super.init(target: target, action: action)
        allowedTouchTypes = [UITouch.TouchType.pencil, .direct].map { NSNumber(value: $0.rawValue) }
    }

    convenience init() {
        self.init(target: nil, action: nil)
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        if touch != nil {
            if held != nil {
                // A second finger: a pan or a pinch, not a stroke.
                state = .failed
            } else {
                for other in touches { ignore(other, for: event) }
            }
            return
        }
        guard touches.count == 1, let first = touches.first,
              first.type == .pencil || fingerPaints()
        else {
            state = .failed
            return
        }
        touch = first
        let s = sample(first)
        if first.type == .pencil {
            onBegin(s)
            state = .began
        } else {
            held = [s]
            start = (first.location(in: nil), first.timestamp)
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let touch, touches.contains(touch) else { return }
        // Every sample since the last event (the Pencil reports at 240 Hz).
        let samples = (event.coalescedTouches(for: touch) ?? [touch]).map(sample)
        if var held {
            held += samples
            let moved = touch.location(in: nil).distance(to: start.point)
            if moved > Self.fingerTravel || touch.timestamp - start.time > Self.fingerWait {
                commit(held)
                state = .began
            } else {
                self.held = held
            }
        } else {
            onMove(samples)
            state = .changed
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let touch, touches.contains(touch) else { return }
        if let held {
            // A finger's tap: one dab.
            commit(held)
        }
        onEnd()
        state = .ended
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let touch, touches.contains(touch) else { return }
        if held == nil {
            // What was painted stays, as a stroke undo takes back.
            onEnd()
            state = .cancelled
        } else {
            state = .failed
        }
    }

    override func reset() {
        super.reset()
        touch = nil
        held = nil
    }

    private func commit(_ samples: [PaintSample]) {
        held = nil
        guard let first = samples.first else { return }
        onBegin(first)
        if samples.count > 1 { onMove(Array(samples.dropFirst())) }
    }
}

private extension CGPoint {
    func distance(to other: CGPoint) -> CGFloat {
        hypot(x - other.x, y - other.y)
    }
}

/// Apple Pencil pressure, as how much a sample paints.
///
/// A Pencil without a sensor (USB-C) reports a constant force, so it counts
/// as pressure-sensitive only once its force has varied; until then every
/// sample paints fully.
enum PencilPressure {
    /// The force (1 is an average touch) that paints fully, replacing the
    /// result completely.
    static let full: CGFloat = 2.5
    /// So a feather-light touch still shows.
    static let least: Float = 0.05

    private static var first: CGFloat?
    private static var varies = false

    /// Learned from `force`.
    static func sensed(_ force: CGFloat) -> Bool {
        if !varies, force > 0 {
            if let first {
                varies = abs(force - first) > 0.01
            } else {
                first = force
            }
        }
        return varies
    }

    /// Light and medium pressure count more than linear, as in drawing apps.
    static func curve(_ force: CGFloat) -> Float {
        let share = Float(min(max(force / full, 0), 1))
        return max(share.squareRoot(), least)
    }
}
