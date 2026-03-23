import simd
import CoreGraphics

/// Pure Swift 2D pan/zoom state machine.
///
/// All internal state is in **pixels** (drawable resolution).
/// Point-based input (pan deltas, cursor positions) is converted to pixels
/// via `contentsScale` at the call site.
final class ViewTransformController {
    /// Zoom: 1.0 = one image pixel per drawable pixel.
    var scale: Float = 1.0
    /// Pan offset in pixels.
    var offsetX: Float = 0
    var offsetY: Float = 0
    /// Drawable size in pixels.
    var viewportWidth: Float = 0
    var viewportHeight: Float = 0
    /// Image size in pixels.
    var imageWidth: Float = 0
    var imageHeight: Float = 0
    /// Display scale factor (points → pixels). Set once from the layer.
    var contentsScale: Float = 2.0

    /// Set by mutations, read by renderer to skip no-op frames.
    private(set) var isDirty: Bool = true

    /// Deferred fit: set when zoomToFit is called but viewport has no valid size yet.
    private var needsFit: Bool = false

    /// Whether the view is currently at fit-to-window. Re-fits on viewport resize.
    private var isFitted: Bool = false

    // MARK: - Zoom Animation

    private var animTargetScale: Float?
    private var animTargetOffsetX: Float = 0
    private var animTargetOffsetY: Float = 0
    private let animSpeed: Float = 12.0

    /// Advance the zoom animation by `dt` seconds. Returns `true` if animation is still active.
    func tick(dt: Float) -> Bool {
        guard let target = animTargetScale else { return false }

        let t = 1.0 - exp(-animSpeed * dt)

        // Interpolate scale in log-space for perceptually uniform zoom
        let lnCur = log(scale)
        let lnTgt = log(target)
        let newLn = lnCur + (lnTgt - lnCur) * t

        offsetX += (animTargetOffsetX - offsetX) * t
        offsetY += (animTargetOffsetY - offsetY) * t

        if abs(newLn - lnTgt) < 0.001 {
            scale = target
            offsetX = animTargetOffsetX
            offsetY = animTargetOffsetY
            animTargetScale = nil
            clampOffset()
            isDirty = true
            return false
        }

        scale = exp(newLn)
        clampOffset()
        isDirty = true
        return true
    }

    private func animateTo(scale targetScale: Float, offsetX ox: Float, offsetY oy: Float) {
        animTargetScale = targetScale
        animTargetOffsetX = ox
        animTargetOffsetY = oy
        isDirty = true
    }

    private func cancelAnimation() {
        animTargetScale = nil
    }

    // MARK: - Zoom

    var zoomPercent: Int {
        let s = animTargetScale ?? scale
        return Int(round(s * 100))
    }

    func zoomToFit(animated: Bool = true) {
        guard imageWidth > 0, imageHeight > 0 else { return }
        guard viewportWidth > 0, viewportHeight > 0 else {
            // Defer until viewport is sized
            needsFit = true
            return
        }
        needsFit = false
        isFitted = true
        let fitScale = min(viewportWidth / imageWidth, viewportHeight / imageHeight)
        if animated {
            animateTo(scale: fitScale, offsetX: 0, offsetY: 0)
        } else {
            cancelAnimation()
            scale = fitScale
            offsetX = 0
            offsetY = 0
            isDirty = true
        }
    }

    func zoomTo100() {
        isFitted = false
        animateTo(scale: 1.0, offsetX: 0, offsetY: 0)
    }

    func zoomIn() { isFitted = false; zoomByFactor(1.25) }
    func zoomOut() { isFitted = false; zoomByFactor(1.0 / 1.25) }

    func zoomToPercent(_ percent: Int) {
        isFitted = false
        animateTo(scale: Float(percent) / 100.0, offsetX: offsetX, offsetY: offsetY)
    }

    func toggleFit100() {
        if abs(scale - 1.0) < 0.01 {
            zoomToFit()
        } else {
            zoomTo100()
        }
    }

    private func zoomByFactor(_ factor: Float) {
        let currentBase = animTargetScale ?? scale
        let newScale = max(0.01, min(currentBase * factor, 64.0))
        animateTo(scale: newScale, offsetX: offsetX, offsetY: offsetY)
    }

    // MARK: - Pan / Drag (input in points, converted to pixels)

    func drag(to point: CGPoint, delta: CGSize) {
        guard !isFitted else { return }
        offsetX += Float(delta.width) * contentsScale
        offsetY += Float(delta.height) * contentsScale
        clampOffset()
        isDirty = true
    }

    func onPanScroll(dx: CGFloat, dy: CGFloat) {
        guard !isFitted else { return }
        offsetX += Float(dx) * contentsScale
        offsetY += Float(dy) * contentsScale
        clampOffset()
        isDirty = true
    }

    func onMagnify(at point: CGPoint, magnification: CGFloat, paneOriginX: Float = 0, paneOriginY: Float = 0, paneWidth: Float? = nil, paneHeight: Float? = nil) {
        isFitted = false
        cancelAnimation()
        let oldScale = scale
        let newScale = max(0.01, min(scale * (1.0 + Float(magnification)), 64.0))
        let ratio = newScale / oldScale

        // Zoom towards cursor (convert point position to pixels, relative to the active pane)
        let pw = paneWidth ?? viewportWidth
        let ph = paneHeight ?? viewportHeight
        let cx = (Float(point.x) * contentsScale - paneOriginX) - pw * 0.5
        let cy = (Float(point.y) * contentsScale - paneOriginY) - ph * 0.5
        offsetX = (offsetX - cx) * ratio + cx
        offsetY = (offsetY - cy) * ratio + cy

        scale = newScale
        clampOffset()
        isDirty = true
    }

    // MARK: - Size Updates

    func setViewportSize(width: Float, height: Float) {
        guard width != viewportWidth || height != viewportHeight else { return }
        viewportWidth = width
        viewportHeight = height
        isDirty = true

        // Fulfill deferred fit, or maintain fit on resize
        if needsFit || isFitted {
            zoomToFit(animated: false)
        } else {
            clampOffset()
        }
    }

    func setImageSize(_ size: CGSize) {
        cancelAnimation()
        imageWidth = Float(size.width)
        imageHeight = Float(size.height)
        isDirty = true
    }

    // MARK: - Offset Clamping

    /// Clamp pan offset so the image can never be panned outside the viewport.
    /// When the image is larger than the viewport (zoomed in), the image edge
    /// stops at the viewport edge. When smaller, the axis is centered.
    private func clampOffset() {
        guard imageWidth > 0, imageHeight > 0,
              viewportWidth > 0, viewportHeight > 0 else { return }

        let imgW = imageWidth * scale
        let imgH = imageHeight * scale

        if imgW <= viewportWidth {
            offsetX = 0
        } else {
            let maxOff = (imgW - viewportWidth) / 2
            offsetX = max(-maxOff, min(offsetX, maxOff))
        }

        if imgH <= viewportHeight {
            offsetY = 0
        } else {
            let maxOff = (imgH - viewportHeight) / 2
            offsetY = max(-maxOff, min(offsetY, maxOff))
        }
    }

    // MARK: - Transform Matrix

    /// 4x4 transform for the vertex shader (NDC space).
    func transformMatrix() -> simd_float4x4 {
        guard viewportWidth > 0, viewportHeight > 0,
              imageWidth > 0, imageHeight > 0 else {
            return matrix_identity_float4x4
        }

        let sx = (imageWidth * scale) / viewportWidth
        let sy = (imageHeight * scale) / viewportHeight
        let tx = (offsetX * 2.0) / viewportWidth
        let ty = -(offsetY * 2.0) / viewportHeight

        return simd_float4x4(columns: (
            simd_float4(sx, 0, 0, 0),
            simd_float4(0, sy, 0, 0),
            simd_float4(0, 0, 1, 0),
            simd_float4(tx, ty, 0, 1)
        ))
    }

    /// Transform matrix for a given viewport width (side-by-side mode).
    func transformMatrixForWidth(_ width: Float) -> simd_float4x4 {
        guard width > 0, viewportHeight > 0,
              imageWidth > 0, imageHeight > 0 else {
            return matrix_identity_float4x4
        }

        let sx = (imageWidth * scale) / width
        let sy = (imageHeight * scale) / viewportHeight
        let tx = (offsetX * 2.0) / width
        let ty = -(offsetY * 2.0) / viewportHeight

        return simd_float4x4(columns: (
            simd_float4(sx, 0, 0, 0),
            simd_float4(0, sy, 0, 0),
            simd_float4(0, 0, 1, 0),
            simd_float4(tx, ty, 0, 1)
        ))
    }

    /// Transform matrix for a given viewport height (vertical side-by-side mode).
    func transformMatrixForHeight(_ height: Float) -> simd_float4x4 {
        guard viewportWidth > 0, height > 0,
              imageWidth > 0, imageHeight > 0 else {
            return matrix_identity_float4x4
        }

        let sx = (imageWidth * scale) / viewportWidth
        let sy = (imageHeight * scale) / height
        let tx = (offsetX * 2.0) / viewportWidth
        let ty = -(offsetY * 2.0) / height

        return simd_float4x4(columns: (
            simd_float4(sx, 0, 0, 0),
            simd_float4(0, sy, 0, 0),
            simd_float4(0, 0, 1, 0),
            simd_float4(tx, ty, 0, 1)
        ))
    }

    // MARK: - Coordinate Conversion

    /// Convert screen-space point (logical points) to image UV coordinates (0-1).
    func screenToUV(screenX: Float, screenY: Float) -> SIMD2<Float> {
        let px = screenX * contentsScale
        let py = screenY * contentsScale
        return screenToUV(pixelX: px, pixelY: py, viewportWidth: viewportWidth)
    }

    /// Convert drawable pixels to image UV, with a given viewport width (for side-by-side).
    func screenToUV(pixelX: Float, pixelY: Float, viewportWidth vpW: Float) -> SIMD2<Float> {
        guard vpW > 0, viewportHeight > 0,
              imageWidth > 0, imageHeight > 0 else {
            return SIMD2<Float>(0.5, 0.5)
        }

        let cx = vpW * 0.5
        let cy = viewportHeight * 0.5

        let imgX = (pixelX - cx - offsetX) / scale + imageWidth * 0.5
        let imgY = (pixelY - cy - offsetY) / scale + imageHeight * 0.5

        let u = imgX / imageWidth
        let v = imgY / imageHeight

        return SIMD2<Float>(u, v)
    }

    /// Convert drawable pixels to image UV, with a given viewport height (for vertical side-by-side).
    func screenToUV(pixelX: Float, pixelY: Float, viewportHeight vpH: Float) -> SIMD2<Float> {
        guard viewportWidth > 0, vpH > 0,
              imageWidth > 0, imageHeight > 0 else {
            return SIMD2<Float>(0.5, 0.5)
        }

        let cx = viewportWidth * 0.5
        let cy = vpH * 0.5

        let imgX = (pixelX - cx - offsetX) / scale + imageWidth * 0.5
        let imgY = (pixelY - cy - offsetY) / scale + imageHeight * 0.5

        let u = imgX / imageWidth
        let v = imgY / imageHeight

        return SIMD2<Float>(u, v)
    }

    /// Convert screen-space point (logical points) to image pixel coordinates.
    func screenToImagePixel(screenX: Float, screenY: Float) -> SIMD2<Float> {
        let uv = screenToUV(screenX: screenX, screenY: screenY)
        return SIMD2<Float>(uv.x * imageWidth, uv.y * imageHeight)
    }

    /// Clear the dirty flag after rendering.
    func clearDirty() {
        isDirty = false
    }
}
