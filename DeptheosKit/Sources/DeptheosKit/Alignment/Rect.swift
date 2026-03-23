import Foundation
import simd

/// Geometry utilities for valid area tracking in chain alignment.
public struct Rect: Equatable, Sendable, Codable {
    public var x: Int
    public var y: Int
    public var width: Int
    public var height: Int

    public init(x: Int, y: Int, width: Int, height: Int) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }

    /// Compute the intersection of two rectangles.
    public func intersection(_ other: Rect) -> Rect {
        let x0 = max(x, other.x)
        let y0 = max(y, other.y)
        let x1 = min(x + width, other.x + other.width)
        let y1 = min(y + height, other.y + other.height)
        return Rect(x: x0, y: y0, width: max(0, x1 - x0), height: max(0, y1 - y0))
    }

    /// Compute the valid bounds after applying an affine transform.
    /// Transforms the four corners and returns the tight axis-aligned bounding box
    /// intersected with the original rect.
    public func validBoundsAfterTransform(_ transform: AffineTransform2D) -> Rect {
        let corners = [
            (Float(x), Float(y)),
            (Float(x + width), Float(y)),
            (Float(x), Float(y + height)),
            (Float(x + width), Float(y + height))
        ]

        let transformed = corners.map { (px, py) -> (Float, Float) in
            let result = transform.matrix * simd_float3(px, py, 1)
            return (result.x, result.y)
        }

        let top = Int(ceil(max(transformed[0].1, transformed[1].1)))
        let left = Int(ceil(max(transformed[0].0, transformed[2].0)))
        let bottom = Int(floor(min(transformed[2].1, transformed[3].1)))
        let right = Int(floor(min(transformed[1].0, transformed[3].0)))

        let clampedLeft = max(0, left)
        let clampedTop = max(0, top)
        let resultWidth = max(0, max(0, right) - clampedLeft)
        let resultHeight = max(0, max(0, bottom) - clampedTop)

        return self.intersection(Rect(x: clampedLeft, y: clampedTop, width: resultWidth, height: resultHeight))
    }
}
