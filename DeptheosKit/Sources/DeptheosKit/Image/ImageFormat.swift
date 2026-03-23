import Foundation

/// Supported image formats, partitioned by which loader path handles them.
///
/// Cases reflect real branches in `UnifiedImageLoader`, not container variety.
/// Per-format display metadata (e.g. "JPEG 8-bit sRGB") should come from the
/// system codec / `UTType`, not from this enum.
public enum ImageFormat: String, Sendable, Codable {
    case linearDNG
    case cameraRAW
    case standard

    public static let dngExtensions: Set<String> = ["dng"]

    public static let rawExtensions: Set<String> = [
        "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf",
        "pef", "srw", "x3f", "3fr", "mrw", "nrw", "rwl",
    ]

    /// Rendered formats validated end-to-end through the standard codec path.
    /// Adding a new entry here is a deliberate decision — discovery is an
    /// explicit allowlist, not a UTType.image inheritance.
    public static let standardExtensions: Set<String> = [
        "jpg", "jpeg", "png", "tiff", "tif", "heif", "heic",
    ]

    public static let allSupportedExtensions: Set<String> =
        dngExtensions.union(rawExtensions).union(standardExtensions)

    /// Detect which loader path should handle a given URL.
    ///
    /// Returns `.standard` for any extension that isn't DNG or camera RAW —
    /// the standard codec path will throw if the file isn't actually decodable.
    public static func detect(from url: URL) -> ImageFormat {
        let ext = url.pathExtension.lowercased()
        if dngExtensions.contains(ext) { return .linearDNG }
        if rawExtensions.contains(ext) { return .cameraRAW }
        return .standard
    }

    /// Discover all supported image files in a directory, sorted alphabetically.
    /// Focus stacking relies on alphabetical order matching focus distance order.
    public static func discoverImages(in directory: URL) -> [URL] {
        guard let contents = try? FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]
        ) else {
            return []
        }

        return contents
            .filter { allSupportedExtensions.contains($0.pathExtension.lowercased()) }
            .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
    }
}
