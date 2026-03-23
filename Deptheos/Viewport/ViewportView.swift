import SwiftUI
import QuartzCore

#if os(macOS)
import AppKit

/// NSViewRepresentable wrapping a CAMetalLayer-backed NSView for Metal rendering.
struct ViewportView: NSViewRepresentable {
    let renderer: ViewportRenderer
    var onScrollWheel: ((CGFloat, CGFloat, CGPoint) -> Void)?
    var onMagnify: ((CGFloat, CGPoint) -> Void)?
    var onMouseMoved: ((CGPoint) -> Void)?
    var onMouseExited: (() -> Void)?
    var onLeftMouseDown: ((CGPoint) -> Void)?
    var onLeftMouseDragged: ((CGPoint, CGSize) -> Void)?
    var onLeftMouseUp: (() -> Void)?
    var onRightMouseDown: ((CGPoint) -> Void)?
    var onRightMouseDragged: ((CGPoint, CGSize) -> Void)?
    var onRightMouseUp: (() -> Void)?
    var onKeyDown: ((String) -> Void)?
    var onKeyUp: ((String) -> Void)?
    var onPressureChanged: ((Float) -> Void)?

    func makeNSView(context: Context) -> ViewportNSView {
        let view = ViewportNSView(frame: .zero)
        view.wantsLayer = true
        view.layer?.isOpaque = true
        view.layer?.backgroundColor = NSColor(red: 0.1, green: 0.1, blue: 0.1, alpha: 1).cgColor

        if let screen = NSScreen.main {
            view.layer?.contentsScale = screen.backingScaleFactor
        }

        if let metalLayer = view.layer as? CAMetalLayer {
            renderer.configureLayer(metalLayer)
        }

        view.renderer = renderer
        view.coordinator = context.coordinator
        view.onLayoutChanged = { [renderer] in renderer.markDirty() }

        return view
    }

    func updateNSView(_ nsView: ViewportNSView, context: Context) {
        context.coordinator.parent = self

        if let scale = nsView.window?.backingScaleFactor {
            nsView.layer?.contentsScale = scale
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    class Coordinator {
        var parent: ViewportView

        init(parent: ViewportView) {
            self.parent = parent
        }
    }
}

/// Custom NSView that uses CAMetalLayer and captures input events.
class ViewportNSView: NSView {
    var coordinator: ViewportView.Coordinator?
    weak var renderer: ViewportRenderer?
    var onLayoutChanged: (() -> Void)?

    private var trackingArea: NSTrackingArea?
    private var keyDownMonitor: Any?
    private var keyUpMonitor: Any?

    override var acceptsFirstResponder: Bool { true }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil {
            if let metalLayer = layer as? CAMetalLayer {
                renderer?.startDisplayLink(metalLayer)
            }
            installKeyMonitors()
        } else {
            renderer?.stop()
            removeKeyMonitors()
        }
    }

    override func makeBackingLayer() -> CALayer {
        CAMetalLayer()
    }

    override var wantsUpdateLayer: Bool { true }

    override func layout() {
        super.layout()
        guard let metalLayer = layer as? CAMetalLayer else { return }
        let scale = metalLayer.contentsScale
        let w = bounds.width * scale
        let h = bounds.height * scale
        guard w > 0, h > 0 else { return }
        let target = CGSize(width: w, height: h)
        if metalLayer.drawableSize != target {
            metalLayer.drawableSize = target
            onLayoutChanged?()
        }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let existing = trackingArea { removeTrackingArea(existing) }
        let area = NSTrackingArea(
            rect: bounds,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self, userInfo: nil
        )
        trackingArea = area
        addTrackingArea(area)
    }

    private func flipped(_ event: NSEvent) -> CGPoint {
        let loc = convert(event.locationInWindow, from: nil)
        return CGPoint(x: loc.x, y: bounds.height - loc.y)
    }

    override func scrollWheel(with event: NSEvent) {
        coordinator?.parent.onScrollWheel?(event.scrollingDeltaX, event.scrollingDeltaY, flipped(event))
    }

    override func magnify(with event: NSEvent) {
        coordinator?.parent.onMagnify?(event.magnification, flipped(event))
    }

    override func mouseMoved(with event: NSEvent) {
        coordinator?.parent.onMouseMoved?(flipped(event))
    }

    override func mouseExited(with event: NSEvent) {
        coordinator?.parent.onMouseExited?()
    }

    override func mouseDown(with event: NSEvent) {
        coordinator?.parent.onLeftMouseDown?(flipped(event))
        emitPressure(event)
    }

    override func mouseDragged(with event: NSEvent) {
        let delta = CGSize(width: event.deltaX, height: event.deltaY)
        coordinator?.parent.onLeftMouseDragged?(flipped(event), delta)
        emitPressure(event)
    }

    private func emitPressure(_ event: NSEvent) {
        let raw = event.pressure
        let adjusted = max(pow(raw, 0.4), 0.05)
        coordinator?.parent.onPressureChanged?(adjusted)
    }

    override func mouseUp(with event: NSEvent) {
        coordinator?.parent.onLeftMouseUp?()
    }

    override func rightMouseDown(with event: NSEvent) {
        coordinator?.parent.onRightMouseDown?(flipped(event))
    }

    override func rightMouseDragged(with event: NSEvent) {
        let delta = CGSize(width: event.deltaX, height: event.deltaY)
        coordinator?.parent.onRightMouseDragged?(flipped(event), delta)
    }

    override func rightMouseUp(with event: NSEvent) {
        coordinator?.parent.onRightMouseUp?()
    }

    override func keyDown(with event: NSEvent) {
        // Handled by local event monitor — override to prevent system beep
    }

    private func installKeyMonitors() {
        keyDownMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            // Don't intercept when a text field has focus
            if let responder = event.window?.firstResponder, responder is NSTextView {
                return event
            }
            guard !event.isARepeat, let chars = event.charactersIgnoringModifiers else { return event }
            self?.coordinator?.parent.onKeyDown?(chars)
            return nil
        }
        keyUpMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyUp) { [weak self] event in
            if let responder = event.window?.firstResponder, responder is NSTextView {
                return event
            }
            guard let chars = event.charactersIgnoringModifiers else { return event }
            self?.coordinator?.parent.onKeyUp?(chars)
            return nil
        }
    }

    private func removeKeyMonitors() {
        if let monitor = keyDownMonitor { NSEvent.removeMonitor(monitor) }
        if let monitor = keyUpMonitor { NSEvent.removeMonitor(monitor) }
        keyDownMonitor = nil
        keyUpMonitor = nil
    }

    deinit {
        if let monitor = keyDownMonitor { NSEvent.removeMonitor(monitor) }
        if let monitor = keyUpMonitor { NSEvent.removeMonitor(monitor) }
    }
}

#else // iOS
import UIKit

/// UIPanGestureRecognizer subclass that captures Apple Pencil pressure.
/// Applies a power curve (exponent 0.4) so light-to-medium pressure
/// maps to a usable range — matching the feel of Procreate and similar apps.
class PencilPanGestureRecognizer: UIPanGestureRecognizer {
    private(set) var pressure: CGFloat = 0

    private func updatePressure(from touches: Set<UITouch>) {
        if let touch = touches.first, touch.maximumPossibleForce > 0 {
            let raw = touch.force / touch.maximumPossibleForce
            pressure = pow(raw, 0.4)
        }
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        super.touchesBegan(touches, with: event)
        updatePressure(from: touches)
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        super.touchesMoved(touches, with: event)
        updatePressure(from: touches)
    }
}

/// UIViewRepresentable wrapping a CAMetalLayer-backed UIView with touch gestures.
struct ViewportView: UIViewRepresentable {
    let renderer: ViewportRenderer
    var onScrollWheel: ((CGFloat, CGFloat, CGPoint) -> Void)?
    var onMagnify: ((CGFloat, CGPoint) -> Void)?
    var onMouseMoved: ((CGPoint) -> Void)?
    var onMouseExited: (() -> Void)?
    var onLeftMouseDown: ((CGPoint) -> Void)?
    var onLeftMouseDragged: ((CGPoint, CGSize) -> Void)?
    var onLeftMouseUp: (() -> Void)?
    var onRightMouseDown: ((CGPoint) -> Void)?
    var onRightMouseDragged: ((CGPoint, CGSize) -> Void)?
    var onRightMouseUp: (() -> Void)?
    var onKeyDown: ((String) -> Void)?
    var onKeyUp: ((String) -> Void)?
    var onPressureChanged: ((Float) -> Void)?

    func makeUIView(context: Context) -> ViewportUIView {
        let view = ViewportUIView(frame: .zero)
        view.isOpaque = true
        view.backgroundColor = UIColor(red: 0.1, green: 0.1, blue: 0.1, alpha: 1)
        view.isMultipleTouchEnabled = true

        if let metalLayer = view.layer as? CAMetalLayer {
            metalLayer.contentsScale = UIScreen.main.scale
            renderer.configureLayer(metalLayer)
        }

        view.renderer = renderer

        // Pinch to zoom
        let pinch = UIPinchGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.handlePinch(_:)))
        pinch.delegate = context.coordinator
        view.addGestureRecognizer(pinch)

        // Single finger pan → drag
        let fingerPan = UIPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.handleOneFingerPan(_:)))
        fingerPan.minimumNumberOfTouches = 1
        fingerPan.maximumNumberOfTouches = 1
        fingerPan.allowedTouchTypes = [UITouch.TouchType.direct.rawValue as NSNumber]
        view.addGestureRecognizer(fingerPan)


        // Apple Pencil pan → painting (pencil-only, with pressure)
        let pencilPan = PencilPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.handlePencilPan(_:)))
        pencilPan.minimumNumberOfTouches = 1
        pencilPan.maximumNumberOfTouches = 1
        pencilPan.allowedTouchTypes = [UITouch.TouchType.pencil.rawValue as NSNumber]
        pencilPan.delegate = context.coordinator
        view.addGestureRecognizer(pencilPan)

        // Apple Pencil hover → brush cursor (iPadOS 16.1+, M-chip iPads)
        let pencilHover = UIHoverGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.handlePencilHover(_:)))
        view.addGestureRecognizer(pencilHover)

        view.onLayoutChanged = { [renderer] in renderer.markDirty() }

        return view
    }

    func updateUIView(_ uiView: ViewportUIView, context: Context) {
        context.coordinator.parent = self
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    class Coordinator: NSObject, UIGestureRecognizerDelegate {
        var parent: ViewportView

        init(parent: ViewportView) {
            self.parent = parent
        }

        func gestureRecognizer(
            _ gestureRecognizer: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
        ) -> Bool { true }

        @objc func handlePinch(_ gesture: UIPinchGestureRecognizer) {
            guard let view = gesture.view else { return }
            let location = gesture.location(in: view)
            if gesture.state == .changed {
                let magnification = gesture.scale - 1.0
                parent.onMagnify?(magnification, location)
                gesture.scale = 1.0
            }
        }

        @objc func handleOneFingerPan(_ gesture: UIPanGestureRecognizer) {
            guard let view = gesture.view else { return }
            let location = gesture.location(in: view)
            switch gesture.state {
            case .began:
                parent.onLeftMouseDown?(location)
            case .changed:
                let translation = gesture.translation(in: view)
                let delta = CGSize(width: translation.x, height: translation.y)
                gesture.setTranslation(.zero, in: view)
                parent.onLeftMouseDragged?(location, delta)
            case .ended, .cancelled:
                parent.onLeftMouseUp?()
            default: break
            }
        }


        // MARK: - Apple Pencil

        @objc func handlePencilPan(_ gesture: PencilPanGestureRecognizer) {
            guard let view = gesture.view else { return }
            let location = gesture.location(in: view)
            parent.onPressureChanged?(max(Float(gesture.pressure), 0.05))
            switch gesture.state {
            case .began:
                parent.onLeftMouseDown?(location)
            case .changed:
                let translation = gesture.translation(in: view)
                let delta = CGSize(width: translation.x, height: translation.y)
                gesture.setTranslation(.zero, in: view)
                parent.onLeftMouseDragged?(location, delta)
            case .ended, .cancelled:
                parent.onLeftMouseUp?()
            default: break
            }
        }

        @objc func handlePencilHover(_ gesture: UIHoverGestureRecognizer) {
            guard let view = gesture.view else { return }
            let location = gesture.location(in: view)

            switch gesture.state {
            case .began, .changed:
                parent.onMouseMoved?(location)
            case .ended, .cancelled:
                parent.onMouseExited?()
            default: break
            }
        }
    }
}

class ViewportUIView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }

    weak var renderer: ViewportRenderer?
    var onLayoutChanged: (() -> Void)?

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil {
            if let metalLayer = layer as? CAMetalLayer {
                renderer?.startDisplayLink(metalLayer)
            }
        } else {
            renderer?.stop()
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard let metalLayer = layer as? CAMetalLayer else { return }
        let scale = metalLayer.contentsScale
        let w = bounds.width * scale
        let h = bounds.height * scale
        guard w > 0, h > 0 else { return }
        let target = CGSize(width: w, height: h)
        if metalLayer.drawableSize != target {
            metalLayer.drawableSize = target
            onLayoutChanged?()
        }
    }
}

#endif
