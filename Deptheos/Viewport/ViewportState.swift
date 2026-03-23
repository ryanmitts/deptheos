import Metal
import CoreGraphics

/// Groups the viewport rendering infrastructure: display state, transform controller,
/// and renderer. Owns the wiring between them.
@Observable
@MainActor
final class ViewportState {
    let displayState = DisplayState()
    let viewTransform = ViewTransformController()
    let renderer: ViewportRenderer
    private(set) var zoomPercent: Int = 100

    init(device: MTLDevice, commandQueue: MTLCommandQueue) {
        renderer = ViewportRenderer(device: device, commandQueue: commandQueue)
        renderer.displayState = displayState
        renderer.viewTransform = viewTransform
        displayState.onDirty = { [weak renderer] in renderer?.markDirty() }
    }

    // MARK: - Zoom Helpers

    func zoomIn() { viewTransform.zoomIn(); updateZoomPercent() }
    func zoomOut() { viewTransform.zoomOut(); updateZoomPercent() }
    func zoomToFit() { viewTransform.zoomToFit(); updateZoomPercent() }
    func zoomTo100() { viewTransform.zoomTo100(); updateZoomPercent() }
    func zoomToPercent(_ percent: Int) { viewTransform.zoomToPercent(percent); updateZoomPercent() }
    func toggleFit100() { viewTransform.toggleFit100(); updateZoomPercent() }

    func updateZoomPercent() {
        let newValue = viewTransform.zoomPercent
        if newValue != zoomPercent {
            zoomPercent = newValue
        }
    }

    /// Set image size on the transform, zoom to fit, and update the displayed percent.
    /// This pattern repeats after stacking completion, project open, and project store load.
    func fitToTexture(width: Int, height: Int) {
        viewTransform.setImageSize(CGSize(width: width, height: height))
        viewTransform.zoomToFit(animated: false)
        updateZoomPercent()
    }
}
