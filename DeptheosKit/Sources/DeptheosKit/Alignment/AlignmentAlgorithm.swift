public struct AlignmentResult: Sendable {
    /// The affine transform that maps the reference frame to the source frame.
    public let transform: AffineTransform2D

    /// Alignment quality metric. Correlation coefficient (rho, 0-1).
    public let confidence: Float

    /// Whether the algorithm converged within the iteration limit.
    public let converged: Bool

    /// Number of iterations performed (for diagnostics).
    public let iterations: Int

    public init(transform: AffineTransform2D, confidence: Float, converged: Bool, iterations: Int = 0) {
        self.transform = transform
        self.confidence = confidence
        self.converged = converged
        self.iterations = iterations
    }
}
