import UIKit
@preconcurrency import DeptheosEngine

/// The crop tool on the photo (ADR-171): the crop over the uncropped
/// picture as shown, dimmed outside, with its handles. Every drag is the
/// engine's (`CropFrame.drag`); the app only draws its answer.
struct CropTool {
    let frame: CropFrame
    let area: CropArea
    let aspect: CropAspect
    /// A drag's reframe as it moves; `ended` at its last.
    let changed: (Reframe, _ ended: Bool) -> Void
}

/// Drawn in the picture's pixels, inside `pictureView`.
final class CropMarkView: UIView {
    private let dim = CAShapeLayer()
    private let border = CAShapeLayer()
    private let handles = CAShapeLayer()
    private var area: CropArea?
    private var zoom: CGFloat = 1

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        dim.fillRule = .evenOdd
        dim.fillColor = UIColor.black.withAlphaComponent(0.55).cgColor
        border.fillColor = nil
        border.strokeColor = UIColor.white.withAlphaComponent(0.9).cgColor
        handles.fillColor = nil
        handles.strokeColor = UIColor.white.cgColor
        for layer in [dim, border, handles] { self.layer.addSublayer(layer) }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func show(_ area: CropArea?, zoom: CGFloat) {
        self.area = area
        self.zoom = max(zoom, 0.0001)
        isHidden = area == nil
        redraw()
    }

    private func redraw() {
        guard let area else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        let rect = CGRect(x: area.left, y: area.top, width: area.right - area.left, height: area.bottom - area.top)
        let outside = UIBezierPath(rect: bounds)
        outside.append(UIBezierPath(rect: rect))
        dim.path = outside.cgPath
        border.path = UIBezierPath(rect: rect).cgPath
        border.lineWidth = 1 / zoom
        // An L at each corner and a bar at each side's middle, 18 points.
        let l = min(18 / zoom, rect.width / 3, rect.height / 3)
        let marks = UIBezierPath()
        for (corner, dx, dy) in [
            (CGPoint(x: rect.minX, y: rect.minY), 1.0, 1.0), (CGPoint(x: rect.maxX, y: rect.minY), -1.0, 1.0),
            (CGPoint(x: rect.maxX, y: rect.maxY), -1.0, -1.0), (CGPoint(x: rect.minX, y: rect.maxY), 1.0, -1.0),
        ] {
            marks.move(to: CGPoint(x: corner.x + dx * l, y: corner.y))
            marks.addLine(to: corner)
            marks.addLine(to: CGPoint(x: corner.x, y: corner.y + dy * l))
        }
        for (a, b) in [
            (CGPoint(x: rect.midX - l / 2, y: rect.minY), CGPoint(x: rect.midX + l / 2, y: rect.minY)),
            (CGPoint(x: rect.midX - l / 2, y: rect.maxY), CGPoint(x: rect.midX + l / 2, y: rect.maxY)),
            (CGPoint(x: rect.minX, y: rect.midY - l / 2), CGPoint(x: rect.minX, y: rect.midY + l / 2)),
            (CGPoint(x: rect.maxX, y: rect.midY - l / 2), CGPoint(x: rect.maxX, y: rect.midY + l / 2)),
        ] {
            marks.move(to: a)
            marks.addLine(to: b)
        }
        handles.path = marks.cgPath
        handles.lineWidth = 3 / zoom
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        redraw()
    }
}

/// A press on a crop handle drags it; anywhere else the photo pans.
final class CropDrag: NSObject, UIGestureRecognizerDelegate {
    let pan = UIPanGestureRecognizer()
    let hover = UIHoverGestureRecognizer()
    weak var view: ZoomingScrollView?
    var tool: CropTool?
    /// The handle held and the crop it started from.
    private var held: (handle: CropHandle, from: CropArea)?
    /// A handle's reach, in screen points.
    private static let reach: CGFloat = 28

    init(_ view: ZoomingScrollView) {
        self.view = view
        super.init()
        pan.maximumNumberOfTouches = 1
        pan.delegate = self
        pan.addTarget(self, action: #selector(panned))
        hover.addTarget(self, action: #selector(hovered))
        view.addGestureRecognizer(pan)
        view.addGestureRecognizer(hover)
        view.panGestureRecognizer.require(toFail: pan)
    }

    private func handle(at point: CGPoint) -> CropHandle? {
        guard let tool, let view else { return nil }
        let reach = Double(Self.reach / max(view.zoomScale, 0.0001))
        return tool.frame.handleAt(area: tool.area, x: Double(point.x), y: Double(point.y), reach: reach)
    }

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        guard gesture === pan, let view else { return true }
        return handle(at: gesture.location(in: view.pictureView)) != nil
    }

    @objc private func panned(_ pan: UIPanGestureRecognizer) {
        guard let view, let tool else { return }
        switch pan.state {
        case .began:
            held = handle(at: pan.location(in: view.pictureView)).map { ($0, tool.area) }
        case .changed, .ended:
            guard let held else { return }
            let by = pan.translation(in: view.pictureView)
            let area = tool.frame.drag(
                from: held.from, handle: held.handle, dx: Double(by.x), dy: Double(by.y), aspect: tool.aspect)
            let ended = pan.state == .ended
            if ended { self.held = nil }
            tool.changed(tool.frame.reframed(area: area), ended)
        case .cancelled, .failed:
            if let held { tool.changed(tool.frame.reframed(area: held.from), true) }
            held = nil
        default:
            break
        }
    }

    /// With a pointer: the cursor says what a press would hold.
    @objc private func hovered(_ hover: UIHoverGestureRecognizer) {
        guard let view else { return }
        let held = hover.state == .ended ? nil : handle(at: hover.location(in: view.pictureView))
        view.cropHover = held
    }
}
