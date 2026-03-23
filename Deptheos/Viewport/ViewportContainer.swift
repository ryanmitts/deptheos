import SwiftUI

/// Wraps ViewportView with all input event handlers (scroll, magnify, mouse, keyboard, pressure).
struct ViewportContainer: View {
    @Bindable var appState: AppState

    var body: some View {
        ViewportView(
            renderer: appState.viewport.renderer,
            onScrollWheel: { dx, dy, _ in
                appState.viewport.viewTransform.onPanScroll(dx: dx, dy: dy)
            },
            onMagnify: { magnification, point in
                let vt = appState.viewport.viewTransform
                let viewMode = appState.viewMode
                if viewMode == .sideBySide {
                    let halfW = vt.viewportWidth / 2
                    let pixelX = Float(point.x) * vt.contentsScale
                    let paneOriginX: Float = pixelX < halfW ? 0 : halfW
                    vt.onMagnify(at: point, magnification: magnification, paneOriginX: paneOriginX, paneWidth: halfW)
                } else if viewMode == .sideBySideVertical {
                    let halfH = vt.viewportHeight / 2
                    let pixelY = Float(point.y) * vt.contentsScale
                    let paneOriginY: Float = pixelY < halfH ? 0 : halfH
                    vt.onMagnify(at: point, magnification: magnification, paneOriginY: paneOriginY, paneHeight: halfH)
                } else {
                    vt.onMagnify(at: point, magnification: magnification)
                }
                appState.viewport.updateZoomPercent()
            },
            onMouseMoved: { point in
                if appState.isPaintMode {
                    updateBrushCursor(at: point)
                }
            },
            onMouseExited: {
                if appState.isPaintMode {
                    appState.viewport.renderer.clearBrushCursor()
                }
            },
            onLeftMouseDown: { point in
                if appState.isPaintMode {
                    let pixel = paintImagePixel(at: point)
                    appState.paintCoordinator.handleMouseDown(imageX: pixel.x, imageY: pixel.y)
                    appState.viewport.renderer.markDirty()
                }
            },
            onLeftMouseDragged: { point, delta in
                if appState.isPaintMode {
                    let pixel = paintImagePixel(at: point)
                    appState.paintCoordinator.handleMouseDragged(imageX: pixel.x, imageY: pixel.y)
                    appState.viewport.renderer.markDirty()
                    updateBrushCursor(at: point)
                } else {
                    appState.viewport.viewTransform.drag(to: .zero, delta: delta)
                }
            },
            onLeftMouseUp: {
                if appState.isPaintMode {
                    appState.paintCoordinator.handleMouseUp()
                    appState.paintStrokeEnded()
                }
            },
            onRightMouseDown: { point in
                appState.viewport.displayState.isLoupeActive = true
                appState.viewport.renderer.updateLoupe(
                    screenX: Float(point.x),
                    screenY: Float(point.y),
                    dpr: appState.viewport.viewTransform.contentsScale
                )
            },
            onRightMouseDragged: { point, _ in
                appState.viewport.renderer.updateLoupe(
                    screenX: Float(point.x),
                    screenY: Float(point.y),
                    dpr: appState.viewport.viewTransform.contentsScale
                )
            },
            onRightMouseUp: {
                appState.viewport.displayState.isLoupeActive = false
                appState.viewport.renderer.clearLoupe()
            },
            onKeyDown: { key in
                handleKeyDown(key)
            },
            onKeyUp: { key in
                handleKeyUp(key)
            },
            onPressureChanged: { pressure in
                if appState.isPaintMode {
                    appState.paintCoordinator.paintState.brushSettings.currentPressure = pressure
                }
            }
        )
    }

    /// Convert a screen point to image pixel coordinates, accounting for side-by-side layout
    /// where the main texture occupies the right half of the viewport.
    private func paintImagePixel(at point: CGPoint) -> SIMD2<Float> {
        let vt = appState.viewport.viewTransform
        let viewMode = appState.viewMode
        if viewMode == .sideBySideVertical {
            let halfHeightLogical = vt.viewportHeight / (2 * vt.contentsScale)
            // Main texture is on the bottom half — remap y relative to the bottom pane
            let localY = Float(point.y) - halfHeightLogical
            let px = Float(point.x) * vt.contentsScale
            let py = localY * vt.contentsScale
            let uv = vt.screenToUV(pixelX: px, pixelY: py, viewportHeight: vt.viewportHeight / 2)
            return SIMD2<Float>(uv.x * vt.imageWidth, uv.y * vt.imageHeight)
        } else if viewMode == .sideBySide {
            let halfWidthLogical = vt.viewportWidth / (2 * vt.contentsScale)
            // Main texture is on the right half — remap x relative to the right pane
            let localX = Float(point.x) - halfWidthLogical
            let px = localX * vt.contentsScale
            let py = Float(point.y) * vt.contentsScale
            let uv = vt.screenToUV(pixelX: px, pixelY: py, viewportWidth: vt.viewportWidth / 2)
            return SIMD2<Float>(uv.x * vt.imageWidth, uv.y * vt.imageHeight)
        } else {
            return vt.screenToImagePixel(screenX: Float(point.x), screenY: Float(point.y))
        }
    }

    private func updateBrushCursor(at point: CGPoint) {
        let brush = appState.paintCoordinator.paintState.brushSettings
        appState.viewport.renderer.updateBrushCursor(
            screenX: Float(point.x),
            screenY: Float(point.y),
            brushRadius: brush.size,
            hardness: brush.hardness,
            opacity: brush.opacity,
            dpr: appState.viewport.viewTransform.contentsScale
        )
    }

    private func handleKeyDown(_ key: String) {
        switch key {
        case "f": appState.viewport.zoomToFit()
        case " ": appState.viewport.toggleFit100()
        case "0": appState.viewport.zoomTo100()
        case "1":
            if !appState.isPaintMode { appState.deselectImage() }
        case "2":
            if appState.currentProject != nil {
                appState.comparisonStyle = .compare
                if !appState.isPaintMode { appState.ensureImageSelected() }
            }
        case "3":
            if appState.currentProject != nil {
                appState.comparisonStyle = .sideBySide
                if !appState.isPaintMode { appState.ensureImageSelected() }
            }
        case "+", "=": appState.viewport.zoomIn()
        case "-": appState.viewport.zoomOut()
        case "e":
            if appState.isPaintMode {
                let brush = appState.paintCoordinator.paintState.brushSettings
                brush.activeTool = brush.activeTool == .eraser ? .brush : .eraser
            } else if appState.currentProject != nil {
                appState.enterPaintMode()
            }
        case "b":
            if appState.isPaintMode {
                appState.paintCoordinator.paintState.brushSettings.activeTool = .brush
            }
        case "z":
            if appState.isPaintMode { appState.paintUndo() }
        case "[":
            if appState.isPaintMode {
                let brush = appState.paintCoordinator.paintState.brushSettings
                brush.size = max(1, brush.size * 0.8)
            }
        case "]":
            if appState.isPaintMode {
                let brush = appState.paintCoordinator.paintState.brushSettings
                brush.size = min(1024, brush.size * 1.25)
            }
        case "\\":
            appState.beginBeforeAfterPreview()
        default: break
        }
    }

    private func handleKeyUp(_ key: String) {
        switch key {
        case "\\":
            appState.endBeforeAfterPreview()
        default: break
        }
    }
}
