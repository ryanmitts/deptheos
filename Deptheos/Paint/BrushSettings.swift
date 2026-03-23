import Foundation

/// Active paint tool type.
enum PaintTool: String, CaseIterable {
    case brush
    case eraser
}

/// Brush parameter model for the paint system.
@Observable
@MainActor
final class BrushSettings {
    var activeTool: PaintTool = .brush
    var size: Float = 30         // 1–1024 px
    var hardness: Float = 0.8    // 0–1
    var opacity: Float = 1.0     // 0.01–1
    var colorTolerance: Float = 1.0 // 0–1
    var currentPressure: Float = 1.0 // 0.05–1, from NSEvent.pressure
}

/// A single brush dab to be dispatched on the GPU.
struct Dab {
    var x: Float
    var y: Float
    var radius: Float
    var hardness: Float
    var opacity: Float
    var colorTolerance: Float
    var isEraser: Bool
}

/// Spacing factor — dabs placed at this fraction of brush radius apart.
private let dabSpacing: Float = 0.25

/// Interpolate dabs between two image-space positions at 0.25x radius spacing.
/// Pressure modulates hardness and opacity per dab.
func interpolateDabs(
    fromX: Float, fromY: Float,
    toX: Float, toY: Float,
    settings: BrushSettings
) -> [Dab] {
    let dx = toX - fromX
    let dy = toY - fromY
    let dist = (dx * dx + dy * dy).squareRoot()
    let spacing = max(settings.size * dabSpacing, 1.0)
    let steps = Int(dist / spacing)
    guard steps > 0 else { return [] }

    let isEraser = settings.activeTool == .eraser
    let pressure = settings.currentPressure
    let h = settings.hardness * pressure
    let o = settings.opacity * pressure
    let tol = isEraser ? Float(0) : settings.colorTolerance

    var dabs = [Dab]()
    dabs.reserveCapacity(steps)
    for i in 1...steps {
        let t = Float(i) * spacing / dist
        dabs.append(Dab(
            x: fromX + dx * t,
            y: fromY + dy * t,
            radius: settings.size,
            hardness: h,
            opacity: o,
            colorTolerance: tol,
            isEraser: isEraser
        ))
    }
    return dabs
}

/// Create a single dab at the given position (for mouseDown — first touch).
func makeSingleDab(x: Float, y: Float, settings: BrushSettings) -> Dab {
    let isEraser = settings.activeTool == .eraser
    let pressure = settings.currentPressure
    return Dab(
        x: x, y: y,
        radius: settings.size,
        hardness: settings.hardness * pressure,
        opacity: settings.opacity * pressure,
        colorTolerance: isEraser ? 0 : settings.colorTolerance,
        isEraser: isEraser
    )
}
