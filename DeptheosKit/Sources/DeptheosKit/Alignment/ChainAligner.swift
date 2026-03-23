import CoreVideo
import Metal

/// Chain alignment for image sequences. Tracks cumulative transforms
/// and valid crop area as images are processed sequentially.
///
/// Each image is aligned against the previous image (pairwise), and
/// transforms are accumulated so every frame is expressed relative to
/// the first frame (the reference).
public struct ChainAligner {
    private var cumulativeTransform: AffineTransform2D?
    private var validArea: Rect?
    private var imageWidth: Int = 0
    private var imageHeight: Int = 0
    private var imageTransforms: [AffineTransform2D] = []
    private var previousTexture: MTLTexture?
    let aligner: ECCAligner

    public init(aligner: ECCAligner) {
        self.aligner = aligner
    }

    /// The previous texture for pairwise alignment (nil for the first image).
    var previousTextureForAlignment: MTLTexture? { previousTexture }

    /// Add an image to the chain using a pre-computed pairwise alignment result.
    /// For the first image, pass `nil` for `pairwiseResult`.
    public mutating func addImage(_ texture: MTLTexture, pairwiseResult: AlignmentResult?) -> AffineTransform2D {
        let width = texture.width
        let height = texture.height

        if cumulativeTransform == nil {
            cumulativeTransform = .identity
            validArea = Rect(x: 0, y: 0, width: width, height: height)
            imageWidth = width
            imageHeight = height
            imageTransforms.append(.identity)
            previousTexture = texture
            return .identity
        }

        guard let result = pairwiseResult else {
            imageTransforms.append(.identity)
            previousTexture = texture
            return .identity
        }

        let newCumulative = cumulativeTransform!.composed(with: result.transform)
        cumulativeTransform = newCumulative
        imageTransforms.append(newCumulative)

        if let currentValid = validArea {
            if let inverseTransform = newCumulative.inverse() {
                let imageBounds = Rect(x: 0, y: 0, width: width, height: height)
                let transformedBounds = imageBounds.validBoundsAfterTransform(inverseTransform)
                validArea = currentValid.intersection(transformedBounds)
            }
        }

        previousTexture = texture
        return newCumulative
    }

    /// Get the valid crop area that is fully covered by all aligned images.
    public func getValidArea() -> Rect? { validArea }

    /// Get all cumulative transforms (one per image, first is identity).
    public func getAllTransforms() -> [AffineTransform2D] { imageTransforms }

    /// Get the number of images added.
    public func imageCount() -> Int { imageTransforms.count }

    /// Get the dimensions of the reference image.
    public func getImageDimensions() -> (width: Int, height: Int) { (imageWidth, imageHeight) }
}
