import CoreVideo

/// Errors from image loading and processing operations.
public enum ImageError: Error, Sendable {
    case decodeFailed(String)
    case readFailed(String)
    case contextCreationFailed
    case textureCacheFailed(CVReturn)
}
