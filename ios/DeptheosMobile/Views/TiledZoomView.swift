import SwiftUI
import UIKit

/// A picture in a scroll view, a point per full-size pixel (`imageSize`),
/// so 100% is a zoom scale of one over the screen scale: the viewer's panes
/// (`ZoomingScrollView`) and Retouch's canvas (`PaintScrollView`). It holds
/// the picture and its cover (whole or left of Split's divider), each tiled
/// at the zoom (`TiledPictureView`, ADR-164), keeps its zoom with the
/// others in `sync`, and tells a subclass where it settled
/// (`reportOwnZoom`).
class TiledZoomView: UIScrollView, UIScrollViewDelegate {
    /// A point per image pixel: touches in it are image coordinates.
    let pictureView = TiledPictureView()
    /// Whole or left of Split's divider: its own picture, stretched over
    /// the same full size, tiled the same.
    let coverView = TiledPictureView()
    private let splitMask = CALayer()
    /// A fraction of the width; `nil` when the cover, if any, is whole.
    private var split: CGFloat?
    /// In image pixels: the content size at zoom 1; zero before one.
    private(set) var imageSize = CGSize.zero
    private(set) weak var sync: ZoomSync?
    /// What the controls cover at the bottom past the safe area.
    private(set) var below = Below.row(0)
    /// Extra chrome depth past the safe area (e.g., Retouch's top bar or
    /// landscape side rails).
    private(set) var extraChromeInsets = UIEdgeInsets.zero
    /// True in photo-only mode (controls hidden): fit expands edge to edge.
    private(set) var controlsHidden = false
    private var lastLayoutBounds = CGSize.zero
    private var lastLayoutSafeArea = UIEdgeInsets.zero

    /// The clear-rect insets that fit frames inside: `safeAreaInsets` plus
    /// non-sheet chrome, or `.zero` when controls are hidden.
    var fitInsets: UIEdgeInsets {
        guard !controlsHidden else { return .zero }
        var insets = safeAreaInsets
        insets.top += extraChromeInsets.top
        insets.left += extraChromeInsets.left
        insets.bottom += extraChromeInsets.bottom
        insets.right += extraChromeInsets.right
        if case .row(let height) = below {
            insets.bottom += height
        }
        return insets
    }

    /// Full pan clearance when zoomed in (includes a low sheet's depth).
    var panMargins: UIEdgeInsets {
        var margins = safeAreaInsets
        margins.top += extraChromeInsets.top
        margins.left += extraChromeInsets.left
        margins.bottom += extraChromeInsets.bottom
        margins.right += extraChromeInsets.right
        switch below {
        case .row(let height): margins.bottom += height
        case .sheet(let depth): margins.bottom = max(margins.bottom, depth)
        }
        return margins
    }

    /// Update chrome insets and photo-only state. Call inside `context.animate`
    /// so fit-framing transitions smoothly with SwiftUI chrome animations.
    func updateChrome(
        below: Below, extraInsets: UIEdgeInsets = .zero, controlsHidden: Bool = false
    ) {
        guard below != self.below || extraInsets != extraChromeInsets || controlsHidden != self.controlsHidden
        else { return }
        self.below = below
        self.extraChromeInsets = extraInsets
        self.controlsHidden = controlsHidden
        guard imageSize != .zero, bounds.width > 0 else { return }
        keepZoomRange(fitting: imageSize, fitInsets: fitInsets, panMargins: panMargins)
        updateSplitMask()
        updateTiles()
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        showsVerticalScrollIndicator = false
        showsHorizontalScrollIndicator = false
        decelerationRate = .fast
        contentInsetAdjustmentBehavior = .never
        hideEdgeEffects()
        addSubview(pictureView)
        // Stretched over full size: magnified pixels stay sharp, reduced
        // ones smoothed.
        coverView.layer.magnificationFilter = .nearest
        coverView.layer.minificationFilter = .trilinear
        coverView.isHidden = true
        coverView.isUserInteractionEnabled = false
        coverView.layer.zPosition = 0.5
        splitMask.backgroundColor = UIColor.black.cgColor
        pictureView.addSubview(coverView)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// Once.
    func join(_ sync: ZoomSync?) {
        guard let sync, self.sync !== sync else { return }
        self.sync = sync
        sync.add(self)
    }

    /// Lay out and fit at `size`, the cover over the same.
    func layOut(at size: CGSize) {
        imageSize = size
        layOutZoomed(pictureView, at: size)
        withoutAnimation { coverView.frame = CGRect(origin: .zero, size: size) }
        zoomToFit(size, fitInsets: fitInsets, panMargins: panMargins)
    }

    /// Left of `split` (a fraction of the width) or whole; `nil` uncovers.
    func cover(with picture: PanePicture?, split: CGFloat?) {
        self.split = split
        withoutAnimation {
            coverView.frame = CGRect(origin: .zero, size: imageSize)
            coverView.isHidden = picture == nil
            coverView.layer.mask = split == nil ? nil : splitMask
        }
        coverView.show(base: picture?.base, source: picture?.source)
        updateSplitMask()
        updateTiles()
    }

    // MARK: For subclasses

    /// Take the zoom to show once there are bounds: each layout, after the
    /// zoom range is kept.
    func settleZoom() {}

    /// A pinch, or set: what follows it on screen.
    func zoomChanged() {}

    func reportOwnZoom() {}

    /// Another view's brush ring at `center` (fractions of the image),
    /// `radius` a fraction of its width; `nil` hides it.
    func showGhost(center: CGPoint?, radius: CGFloat) {}

    func peerSettled() {}

    // MARK: Zoom

    /// Sized, so it has a zoom to share.
    var hasImage: Bool { imageSize != .zero && bounds.width > 0 }

    /// Take `other`'s zoom relative to each one's fit: the same share of
    /// the image in view, at the same place. The images may differ in size
    /// (a frame and a cropped result, a preview), so scale and offset
    /// aren't copied.
    func follow(_ other: TiledZoomView) {
        let otherSize = other.imageSize
        guard imageSize != .zero, otherSize != .zero, bounds.width > 0,
            other.bounds.width > 0, other.minimumZoomScale > 0
        else { return }
        let relative = other.zoomScale / other.fitZoomScale
        // Within this view's range: a smaller image's 100% is further in.
        let target = min(max(fitZoomScale * relative, minimumZoomScale), maximumZoomScale)
        if relative <= 1.0001 {
            // At fit or under it: centered, at the same share of fit.
            if abs(zoomScale - target) > 0.0001 { zoomScale = target }
            centerContent(fitInsets: fitInsets, panMargins: panMargins)
            contentOffset = CGPoint(x: -contentInset.left, y: -contentInset.top)
            return
        }
        if abs(zoomScale - target) > 0.0001 { zoomScale = target }
        // The image point at the other's center, as fractions of its image.
        let center = other.convert(CGPoint(x: other.bounds.midX, y: other.bounds.midY), to: other.pictureView)
        let fx = center.x / otherSize.width
        let fy = center.y / otherSize.height
        let offset = CGPoint(
            x: fx * imageSize.width * zoomScale - bounds.width / 2,
            y: fy * imageSize.height * zoomScale - bounds.height / 2)
        let clamped = clampedOffset(offset)
        if abs(clamped.x - contentOffset.x) > 0.5 || abs(clamped.y - contentOffset.y) > 0.5 {
            contentOffset = clamped
        }
    }

    /// Kept inside the content and its insets, as a drag would.
    func clampedOffset(_ offset: CGPoint) -> CGPoint {
        CGPoint(
            x: min(max(offset.x, -contentInset.left), max(contentSize.width - bounds.width + contentInset.right, -contentInset.left)),
            y: min(max(offset.y, -contentInset.top), max(contentSize.height - bounds.height + contentInset.bottom, -contentInset.top)))
    }

    override func safeAreaInsetsDidChange() {
        super.safeAreaInsetsDidChange()
        if safeAreaInsets != lastLayoutSafeArea {
            lastLayoutSafeArea = safeAreaInsets
            keepZoomRange(fitting: imageSize, fitInsets: fitInsets, panMargins: panMargins)
            updateSplitMask()
            updateTiles()
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        if bounds.size != lastLayoutBounds || safeAreaInsets != lastLayoutSafeArea {
            lastLayoutBounds = bounds.size
            lastLayoutSafeArea = safeAreaInsets
            keepZoomRange(fitting: imageSize, fitInsets: fitInsets, panMargins: panMargins)
        }
        settleZoom()
        updateSplitMask()
        updateTiles()
    }

    /// At the zoom's level: the picture's and the cover's.
    func updateTiles() {
        guard bounds.width > 0, imageSize != .zero else { return }
        let visible = convert(bounds, to: pictureView)
        let scale = zoomScale * screenPixelScale
        pictureView.update(visible: visible, scale: scale)
        // The cover lies over the whole of the picture, in its points.
        if !coverView.isHidden { coverView.update(visible: visible, scale: scale) }
    }

    /// In the picture's own coordinates.
    private func updateSplitMask() {
        guard let split, !coverView.isHidden else { return }
        let x = convert(CGPoint(x: contentOffset.x + bounds.width * split, y: 0), to: pictureView).x
        withoutAnimation {
            splitMask.frame = CGRect(x: 0, y: 0, width: min(max(x, 0), imageSize.width), height: imageSize.height)
        }
    }

    /// Told to the others in its sync too.
    private func reportZoom() {
        reportOwnZoom()
        sync?.settled(self)
    }

    func withoutAnimation(_ body: () -> Void) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        body()
        CATransaction.commit()
    }

    // MARK: UIScrollViewDelegate

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { pictureView }

    func scrollViewDidZoom(_ scrollView: UIScrollView) {
        centerContent(fitInsets: fitInsets, panMargins: panMargins)
        updateSplitMask()
        updateTiles()
        zoomChanged()
        sync?.moved(self)
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        updateSplitMask()
        updateTiles()
        sync?.moved(self)
    }

    func scrollViewDidEndZooming(_ scrollView: UIScrollView, with view: UIView?, atScale scale: CGFloat) {
        reportZoom()
    }

    func scrollViewDidEndDragging(_ scrollView: UIScrollView, willDecelerate decelerate: Bool) {
        if !decelerate { reportZoom() }
    }

    func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) {
        reportZoom()
    }
}

/// Zoom and position shared across Side by Side and Top and Bottom panes,
/// and Retouch's canvas and source; in Retouch the brush ring shows on the
/// source at the same image point.
final class ZoomSync {
    private var views: [WeakView] = []
    private var following = false

    private struct WeakView {
        weak var view: TiledZoomView?
    }

    func add(_ view: TiledZoomView) {
        views.removeAll { $0.view == nil || $0.view === view }
        views.append(WeakView(view: view))
    }

    private func others(than view: TiledZoomView) -> [TiledZoomView] {
        views.compactMap(\.view).filter { $0 !== view }
    }

    /// From the first other view with an image.
    func catchUp(_ view: TiledZoomView) {
        guard !following, let other = others(than: view).first(where: { $0.hasImage }) else { return }
        following = true
        defer { following = false }
        view.follow(other)
    }

    func moved(_ source: TiledZoomView) {
        guard !following else { return }
        following = true
        defer { following = false }
        for other in others(than: source) {
            other.follow(source)
        }
    }

    func settled(_ source: TiledZoomView) {
        for other in others(than: source) { other.peerSettled() }
    }

    /// `center` in fractions of the image, or `nil` when it went.
    func ghost(from source: TiledZoomView, center: CGPoint?, radius: CGFloat) {
        for other in others(than: source) { other.showGhost(center: center, radius: radius) }
    }
}

/// A picture in a scroll view (`PanePicture`): fits on open, on a size
/// change and on `fitRequest`; pinch or double-tap to zoom. With
/// `keepsFrame`, a new image of another size keeps a zoomed-in view's
/// relative zoom and place (a frame in the result's place, Flicker).
/// `onHold` reports a hold. With `split`, `overlay` covers left of it; with
/// `sync`, zoom follows the others. `below` is what the controls cover
/// at the bottom past the safe area (`ZoomToFit.swift`). A new view starts at `remembered` for the same
/// image and reports to `onZoom` as it settles.
struct ZoomableImage: UIViewRepresentable {
    let picture: PanePicture
    var keepsFrame = false
    var onHold: ((Bool) -> Void)?
    var overlay: PanePicture?
    var split: CGFloat?
    var fitRequest = 0
    var sync: ZoomSync?
    var below = Below.row(0)
    var extraChromeInsets = UIEdgeInsets.zero
    var controlsHidden = false
    var remembered: ViewerZoom?
    var onZoom: ((ViewerZoom?) -> Void)?
    /// At fractions of the picture: `true` when it did something, else the
    /// tap toggles the controls.
    var onImageTap: ((CGPoint) -> Bool)?

    func makeUIView(context: Context) -> ZoomingScrollView {
        let view = ZoomingScrollView()
        view.pendingZoom = remembered
        return view
    }

    func updateUIView(_ view: ZoomingScrollView, context: Context) {
        context.animate {
            view.updateChrome(
                below: below, extraInsets: extraChromeInsets, controlsHidden: controlsHidden)
        }
        view.onZoom = onZoom
        view.onHold = onHold
        view.onTap = context.environment.viewerTap?.perform
        view.onImageTap = onImageTap
        view.show(picture, keepingFrame: keepsFrame)
        // The overlay only with a divider to show it left of.
        view.cover(with: split == nil ? nil : overlay, split: split)
        view.requestFit(fitRequest)
        view.join(sync)
    }
}

/// The viewer's pane: a `TiledZoomView` with a hold, taps, and Retouch's
/// brush ring as the source's pane. The cover is the compare (or the result,
/// swapped), left of the divider.
final class ZoomingScrollView: TiledZoomView {
    /// Light over dark (Retouch's source).
    private let ghostDark = CAShapeLayer()
    private let ghostLight = CAShapeLayer()
    private var ghost: (center: CGPoint, radius: CGFloat)?
    private var fitRequest = 0
    /// Names the picture for the zoom kept in the model (`ViewerZoom`).
    private var shownImage: CGImage?
    private var shownSource: (any TileSource)?
    /// Took its sync's zoom since its image was fitted (`catchUp`).
    private var caughtUp = false
    private let hold = UILongPressGestureRecognizer()
    private let tap = UITapGestureRecognizer()
    /// Taken once the image and view have sizes.
    var pendingZoom: ViewerZoom?
    /// `nil` at fit.
    var onZoom: ((ViewerZoom?) -> Void)?
    /// The controls over the photo hide or show.
    var onTap: (() -> Void)? {
        didSet { tap.isEnabled = onTap != nil || onImageTap != nil }
    }
    /// At fractions of the picture, asked first: `true` when it did
    /// something, else `onTap`.
    var onImageTap: ((CGPoint) -> Bool)? {
        didSet { tap.isEnabled = onTap != nil || onImageTap != nil }
    }
    var onHold: ((Bool) -> Void)? {
        didSet { hold.isEnabled = onHold != nil }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        // Stretched over full size: magnified pixels stay sharp, reduced
        // ones smoothed.
        pictureView.layer.magnificationFilter = .nearest
        pictureView.layer.minificationFilter = .trilinear
        for (layer, color) in [(ghostDark, UIColor.black), (ghostLight, .white)] {
            layer.fillColor = nil
            layer.strokeColor = color.withAlphaComponent(0.6).cgColor
            layer.lineDashPattern = nil
            layer.isHidden = true
            layer.zPosition = 1
            pictureView.layer.addSublayer(layer)
        }
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(toggleZoom))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)
        tap.addTarget(self, action: #selector(tapped))
        tap.require(toFail: doubleTap)
        tap.isEnabled = false
        addGestureRecognizer(tap)
        hold.minimumPressDuration = 0.3
        hold.addTarget(self, action: #selector(held))
        hold.isEnabled = false
        addGestureRecognizer(hold)
        NotificationCenter.default.addObserver(
            self, selector: #selector(enteredBackground),
            name: UIApplication.didEnterBackgroundNotification, object: nil)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// In the background the pane lets go of its tiles and their pictures,
    /// keeping the flat ones, whether or not the model's change reaches it
    /// (`AppModel.fitPictures`).
    @objc private func enteredBackground() {
        shownSource = nil
        pictureView.dropSource()
        coverView.dropSource()
    }

    /// Show `picture`. One of another size is laid out at its own size and
    /// fitted by the same rule, so switching images never changes framing
    /// at fit. With `keepingFrame`, a zoomed view stays as far in relative
    /// to each fit, on the same part of the picture, as
    /// `TiledZoomView.follow` does.
    func show(_ picture: PanePicture, keepingFrame: Bool = false) {
        let image = picture.base
        guard image !== shownImage || picture.source !== shownSource || picture.size != imageSize else { return }
        shownImage = image
        shownSource = picture.source
        let size = picture.size
        if size != imageSize {
            // Where the view is in the image shown before, when zoomed in.
            var kept: (relative: CGFloat, center: CGPoint)?
            if keepingFrame, imageSize != .zero, bounds.width > 0, minimumZoomScale > 0,
               zoomScale > fitZoomScale * 1.0001 {
                let center = convert(CGPoint(x: bounds.midX, y: bounds.midY), to: pictureView)
                kept = (
                    zoomScale / fitZoomScale,
                    CGPoint(x: center.x / imageSize.width, y: center.y / imageSize.height))
            }
            // The base alone until laid out: tiles are asked only once in
            // their geometry.
            pictureView.show(base: image, source: nil)
            layOut(at: size)
            if let kept {
                zoomScale = min(fitZoomScale * kept.relative, maximumZoomScale)
                centerContent(fitInsets: fitInsets, panMargins: panMargins)
                let offset = CGPoint(
                    x: kept.center.x * size.width * zoomScale - bounds.width / 2,
                    y: kept.center.y * size.height * zoomScale - bounds.height / 2)
                contentOffset = clampedOffset(offset)
                reportOwnZoom()
            }
            caughtUp = false
        }
        pictureView.show(base: image, source: picture.source)
        updateTiles()
    }

    /// When `request` is new.
    func requestFit(_ request: Int) {
        guard request != fitRequest else { return }
        fitRequest = request
        setZoomScale(fitZoomScale, animated: true)
        onZoom?(nil)
    }

    /// The model's zoom for the image, once sized; a pane just fitted to a
    /// new image takes its sync's zoom, so one isn't left at fit.
    override func settleZoom() {
        if let pending = pendingZoom, let shownImage, bounds.width > 0, imageSize != .zero {
            pendingZoom = nil
            if pending.image == ObjectIdentifier(shownImage) {
                show(fraction: pending.visible, of: pictureView, size: imageSize)
                sync?.moved(self)
            }
        }
        if !caughtUp, hasImage {
            caughtUp = true
            sync?.catchUp(self)
        }
    }

    override func reportOwnZoom() {
        guard let shownImage else { return }
        onZoom?(visibleFraction(of: pictureView, size: imageSize).map {
            ViewerZoom(image: ObjectIdentifier(shownImage), visible: $0)
        })
    }

    override func showGhost(center: CGPoint?, radius: CGFloat) {
        ghost = center.map { ($0, radius) }
        updateGhost()
    }

    override func zoomChanged() {
        if ghost != nil { updateGhost() }
    }

    /// The same width on screen at any zoom.
    private func updateGhost() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        guard let ghost, imageSize != .zero else {
            ghostDark.isHidden = true
            ghostLight.isHidden = true
            return
        }
        let r = ghost.radius * imageSize.width
        let path = CGPath(ellipseIn: CGRect(x: -r, y: -r, width: 2 * r, height: 2 * r), transform: nil)
        let scale = max(zoomScale, 0.0001)
        for (layer, width) in [(ghostDark, 3.0), (ghostLight, 1.0)] {
            layer.path = path
            layer.lineWidth = width / scale
            layer.position = CGPoint(x: ghost.center.x * imageSize.width, y: ghost.center.y * imageSize.height)
            layer.isHidden = false
        }
    }

    @objc private func held(_ press: UILongPressGestureRecognizer) {
        switch press.state {
        case .began: onHold?(true)
        case .ended, .cancelled, .failed: onHold?(false)
        default: break
        }
    }

    @objc private func tapped(_ tap: UITapGestureRecognizer) {
        let point = tap.location(in: pictureView)
        if let onImageTap, imageSize.width > 0, imageSize.height > 0,
           pictureView.bounds.contains(point),
           onImageTap(CGPoint(x: point.x / imageSize.width, y: point.y / imageSize.height))
        {
            return
        }
        onTap?()
    }

    /// Fit ↔ 100% at the point tapped (`toggleActualPixels`).
    @objc private func toggleZoom(_ tap: UITapGestureRecognizer) {
        toggleActualPixels(at: tap.location(in: pictureView))
    }
}
