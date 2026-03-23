import simd

/// 2D affine transform for image alignment.
///
/// Represents the matrix:
/// ```
/// | a  c  tx |
/// | b  d  ty |
/// | 0  0   1 |
/// ```
/// Backed by `simd_float3x3` where column 0 = (a, b, 0), column 1 = (c, d, 0),
/// column 2 = (tx, ty, 1).
public struct AffineTransform2D: Sendable, Equatable {
    public var matrix: simd_float3x3

    public init(matrix: simd_float3x3) {
        self.matrix = matrix
    }

    public init(a: Float, b: Float, c: Float, d: Float, tx: Float, ty: Float) {
        self.matrix = simd_float3x3(
            simd_float3(a, b, 0),
            simd_float3(c, d, 0),
            simd_float3(tx, ty, 1)
        )
    }

    public var a: Float {
        get { matrix[0][0] }
        set { matrix[0][0] = newValue }
    }
    public var b: Float {
        get { matrix[0][1] }
        set { matrix[0][1] = newValue }
    }
    public var c: Float {
        get { matrix[1][0] }
        set { matrix[1][0] = newValue }
    }
    public var d: Float {
        get { matrix[1][1] }
        set { matrix[1][1] = newValue }
    }
    public var tx: Float {
        get { matrix[2][0] }
        set { matrix[2][0] = newValue }
    }
    public var ty: Float {
        get { matrix[2][1] }
        set { matrix[2][1] = newValue }
    }

    public static let identity = AffineTransform2D(matrix: matrix_identity_float3x3)


    /// Scale translation components when moving between pyramid levels.
    public func scaled(fromScale: Float, toScale: Float) -> AffineTransform2D {
        let ratio = toScale / fromScale
        return AffineTransform2D(
            a: a, b: b, c: c, d: d,
            tx: tx * ratio, ty: ty * ratio
        )
    }

    /// Compose two transforms: result = self * other (apply other first, then self).
    public func composed(with other: AffineTransform2D) -> AffineTransform2D {
        AffineTransform2D(matrix: self.matrix * other.matrix)
    }

    /// Compute the inverse transform. Returns nil if the transform is singular.
    public func inverse() -> AffineTransform2D? {
        let det = simd_determinant(matrix)
        guard abs(det) > 1e-10 else { return nil }
        return AffineTransform2D(matrix: simd_inverse(matrix))
    }

    /// Padded columns for Metal shader parameter structs (SIMD4 with zero padding).
    public var gpuColumns: (SIMD4<Float>, SIMD4<Float>, SIMD4<Float>) {
        (SIMD4<Float>(matrix[0], 0),
         SIMD4<Float>(matrix[1], 0),
         SIMD4<Float>(matrix[2], 0))
    }

}

// MARK: - Codable

extension AffineTransform2D: Codable {
    enum CodingKeys: String, CodingKey {
        case a, b, c, d, tx, ty
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            a: try container.decode(Float.self, forKey: .a),
            b: try container.decode(Float.self, forKey: .b),
            c: try container.decode(Float.self, forKey: .c),
            d: try container.decode(Float.self, forKey: .d),
            tx: try container.decode(Float.self, forKey: .tx),
            ty: try container.decode(Float.self, forKey: .ty)
        )
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(a, forKey: .a)
        try container.encode(b, forKey: .b)
        try container.encode(c, forKey: .c)
        try container.encode(d, forKey: .d)
        try container.encode(tx, forKey: .tx)
        try container.encode(ty, forKey: .ty)
    }
}
