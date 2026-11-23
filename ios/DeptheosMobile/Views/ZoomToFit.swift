import UIKit

/// Zoom to fit for the viewer's and canvas's scroll views, laid out a point
/// per image pixel: at fit the image fills the unoccluded clear area between
/// the bars and trays (`fitInsets`), centered cleanly, while the scroll view
/// stays full bleed so pinching and panning glide smoothly underneath the
/// translucent chrome. It zooms in to 800% or 16 times fit, whichever is
/// further (`maximumZoom`), and out to a quarter of fit (`belowFit`),
/// centered. Fit is the range's minimum times that (`fitZoomScale`).
///
/// What the controls cover at a scroll view's bottom that its safe area
/// doesn't: the bottom row's height over the safe area, or a low sheet's
/// depth from the view's lower edge.
enum Below: Equatable {
    case row(CGFloat)
    case sheet(depth: CGFloat)
}

extension UIScrollView {
    /// Returns `raw` when the inset clear rect still leaves comfortable room
    /// for the photo (>= 52% of bounds along both axes); otherwise falls back
    /// to `.zero` (full-bleed fit) on very short screens like Duo outer display
    /// or compact landscape.
    func effectiveFitInsets(_ raw: UIEdgeInsets) -> UIEdgeInsets {
        guard bounds.width > 0, bounds.height > 0 else { return .zero }
        let clear = bounds.inset(by: raw)
        guard clear.width >= bounds.width * 0.52, clear.height >= bounds.height * 0.52 else {
            return .zero
        }
        return raw
    }

    /// 1 until both have a size: a view with a width and no height yet
    /// would fit at zero, a scale the scroll view doesn't take, and stop
    /// counting as fitted.
    func fittingScale(for size: CGSize, fitInsets: UIEdgeInsets = .zero) -> CGFloat {
        let clear = bounds.inset(by: effectiveFitInsets(fitInsets))
        guard size.width > 0, size.height > 0, clear.width > 0, clear.height > 0 else { return 1 }
        return min(clear.width / size.width, clear.height / size.height)
    }

    /// Physical pixels per point: on a screen that renders at a larger
    /// scale and reduces to its panel (`nativeScale` under `displayScale`),
    /// fewer than the rendering's.
    var screenPixelScale: CGFloat {
        max(window?.windowScene?.screen.nativeScale ?? traitCollection.displayScale, 1)
    }

    /// The scale at 100%: one image pixel per physical screen pixel.
    var actualPixelsScale: CGFloat {
        1 / screenPixelScale
    }

    /// 800% (eight screen pixels per image pixel) or 16 times fit,
    /// whichever is further.
    func maximumZoom(fitting fitScale: CGFloat) -> CGFloat {
        max(actualPixelsScale * 8, fitScale * 16)
    }

    /// A pinch goes down to fit divided by this.
    static let belowFit: CGFloat = 4

    /// The range runs down from it to a quarter.
    var fitZoomScale: CGFloat { minimumZoomScale * Self.belowFit }

    /// Give or take rounding.
    var isFitted: Bool { abs(zoomScale - fitZoomScale) <= fitZoomScale * 0.002 }

    func setZoomRange(fitting fitScale: CGFloat) {
        minimumZoomScale = fitScale / Self.belowFit
        maximumZoomScale = maximumZoom(fitting: fitScale)
    }

    /// For `layoutSubviews`: the range follows the view's size and clear rect,
    /// and a fitted view stays fitted and centered in the clear rect.
    func keepZoomRange(fitting size: CGSize, fitInsets: UIEdgeInsets, panMargins: UIEdgeInsets) {
        let fitScale = fittingScale(for: size, fitInsets: fitInsets)
        let wasFitted = isFitted
        if abs(fitZoomScale - fitScale) > 0.0001 {
            setZoomRange(fitting: fitScale)
            if wasFitted { zoomScale = fitScale }
        }
        centerContent(fitInsets: fitInsets, panMargins: panMargins)
        if wasFitted, size != .zero, bounds.width > 0 {
            contentOffset = CGPoint(x: -contentInset.left, y: -contentInset.top)
        }
    }

    func zoomToFit(_ size: CGSize, fitInsets: UIEdgeInsets, panMargins: UIEdgeInsets) {
        let fitScale = fittingScale(for: size, fitInsets: fitInsets)
        setZoomRange(fitting: fitScale)
        zoomScale = fitScale
        centerContent(fitInsets: fitInsets, panMargins: panMargins)
        contentOffset = CGPoint(x: -contentInset.left, y: -contentInset.top)
    }

    /// Lay `view` out at `size` at a zoom of 1, so the zoom's transform
    /// isn't folded into its frame. The range is opened to take 1 first:
    /// the scroll view clamps zoom to it, and a small stand-in's fit over 1
    /// left the next picture shrunk.
    func layOutZoomed(_ view: UIView, at size: CGSize) {
        minimumZoomScale = min(minimumZoomScale, 1)
        maximumZoomScale = max(maximumZoomScale, 1)
        zoomScale = 1
        view.frame = CGRect(origin: .zero, size: size)
        contentSize = size
    }

    /// Double tap: fit to 100% centered on `point` (in image pixels), any
    /// other zoom back to fit. A picture whose 100% is hardly past fit goes
    /// to twice fit instead.
    func toggleActualPixels(at point: CGPoint) {
        let fit = fitZoomScale
        if !isFitted {
            setZoomScale(fit, animated: true)
            return
        }
        var target = actualPixelsScale
        if target < fit * 1.25 { target = fit * 2 }
        target = min(target, maximumZoomScale)
        let size = CGSize(width: bounds.width / target, height: bounds.height / target)
        zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2,
                        width: size.width, height: size.height), animated: true)
    }

    /// Center content inside the unoccluded clear rect (`fitInsets`) at fit,
    /// while keeping full-bleed scrolling underneath chrome and ramping in any
    /// extra `panMargins` (such as a low sheet or short-screen fallback) as
    /// the user zooms in.
    func centerContent(fitInsets: UIEdgeInsets, panMargins: UIEdgeInsets) {
        let effective = effectiveFitInsets(fitInsets)
        let clear = bounds.inset(by: effective)
        let extraX = max((clear.width - contentSize.width) / 2, 0)
        let extraY = max((clear.height - contentSize.height) / 2, 0)
        let fit = max(fitZoomScale, 0.0001)
        let extraShare = min(max((zoomScale - fit) / (fit * 0.5), 0), 1)
        let extraTop = max(panMargins.top - effective.top, 0) * extraShare
        let extraLeft = max(panMargins.left - effective.left, 0) * extraShare
        let extraBottom = max(panMargins.bottom - effective.bottom, 0) * extraShare
        let extraRight = max(panMargins.right - effective.right, 0) * extraShare
        contentInset = UIEdgeInsets(
            top: effective.top + extraY + extraTop,
            left: effective.left + extraX + extraLeft,
            bottom: effective.bottom + extraY + extraBottom,
            right: effective.right + extraX + extraRight)
    }

    /// As fractions of the image; `nil` at fit or under it.
    func visibleFraction(of view: UIView, size: CGSize) -> CGRect? {
        guard size.width > 0, size.height > 0, zoomScale > fitZoomScale * 1.01 else { return nil }
        let seen = convert(bounds, to: view).intersection(CGRect(origin: .zero, size: size))
        guard !seen.isNull else { return nil }
        return CGRect(
            x: seen.minX / size.width, y: seen.minY / size.height,
            width: seen.width / size.width, height: seen.height / size.height)
    }

    /// `fraction` as `visibleFraction` gave it.
    func show(fraction: CGRect, of view: UIView, size: CGSize) {
        let rect = CGRect(
            x: fraction.minX * size.width, y: fraction.minY * size.height,
            width: fraction.width * size.width, height: fraction.height * size.height)
        zoom(to: rect, animated: false)
    }

    /// The system's blur where the photo meets a bar: none over a photo.
    func hideEdgeEffects() {
        for effect in [topEdgeEffect, leftEdgeEffect, bottomEdgeEffect, rightEdgeEffect] {
            effect.isHidden = true
        }
    }
}
