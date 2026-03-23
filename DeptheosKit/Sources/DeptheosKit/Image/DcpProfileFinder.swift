import Foundation

/// Which DCP profile to prefer when loading camera raw files.
public enum DcpProfilePreference: Sendable, Hashable {
    /// Use "Adobe Standard" — matches the profile embedded by Adobe DNG Converter.
    case adobeStandard
    /// Use "Camera Standard" (camera maker's profile, matches in-camera JPEG look).
    case cameraStandard
    /// Skip DCP entirely — use only LibRaw's basic color matrices.
    case none
}

/// Searches for installed Adobe DCP (DNG Camera Profile) files on macOS.
public enum DcpProfileFinder {

    /// macOS directories where Adobe installs camera profiles.
    private static let basePaths = [
        "/Library/Application Support/Adobe/CameraRaw/CameraProfiles",
        "/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app/Contents/Resources/CameraProfiles",
        "/Applications/Adobe Lightroom CC/Adobe Lightroom.app/Contents/Resources/CameraProfiles",
    ]

    /// Find a DCP profile file for the given camera model.
    ///
    /// Searches `customDirectories` first (flat layout: any `.dcp` whose filename
    /// contains the model name), then falls back to Adobe's standard install paths.
    public static func findProfile(
        forCamera model: String,
        preference: DcpProfilePreference = .adobeStandard,
        customDirectories: [String] = []
    ) -> URL? {
        guard preference != .none else { return nil }

        // Search custom directories first (flat: any .dcp matching the model name)
        for dir in customDirectories {
            let dirURL = URL(fileURLWithPath: dir)
            if let url = findDcp(in: dirURL, matching: model) {
                return url
            }
        }

        let preferCamera = (preference == .cameraStandard)

        if preferCamera {
            if let url = findCameraProfile(model: model, profileName: "Camera Standard") {
                return url
            }
            if let url = findAdobeStandard(model: model) {
                return url
            }
        } else {
            if let url = findAdobeStandard(model: model) {
                return url
            }
            if let url = findCameraProfile(model: model, profileName: "Camera Standard") {
                return url
            }
        }

        // Last resort: any .dcp in the camera's directory
        for base in basePaths {
            let cameraDir = URL(fileURLWithPath: base).appendingPathComponent("Camera")
            let exactDir = cameraDir.appendingPathComponent(model)
            if let url = findAnyDcp(in: exactDir) {
                return url
            }
            if let modelDir = findSubdirectory(in: cameraDir, named: model), modelDir != exactDir {
                if let url = findAnyDcp(in: modelDir) {
                    return url
                }
            }
        }

        return nil
    }

    /// Returns true if any Adobe camera profiles are installed.
    public static var isAvailable: Bool {
        basePaths.contains { FileManager.default.fileExists(atPath: $0) }
    }

    private static func findAdobeStandard(model: String) -> URL? {
        let filename = "\(model) Adobe Standard.dcp"
        for base in basePaths {
            let dir = URL(fileURLWithPath: base).appendingPathComponent("Adobe Standard")
            // Try exact path first (fast), then fall back to case-insensitive scan
            let exact = dir.appendingPathComponent(filename)
            if FileManager.default.fileExists(atPath: exact.path) {
                return exact
            }
            if let url = findFile(in: dir, named: filename) {
                return url
            }
        }
        return nil
    }

    private static func findCameraProfile(model: String, profileName: String) -> URL? {
        for base in basePaths {
            let cameraDir = URL(fileURLWithPath: base).appendingPathComponent("Camera")
            // Try exact model directory first, then case-insensitive match
            let exactDir = cameraDir.appendingPathComponent(model)
            if let url = findDcp(in: exactDir, matching: profileName) {
                return url
            }
            if let modelDir = findSubdirectory(in: cameraDir, named: model), modelDir != exactDir {
                if let url = findDcp(in: modelDir, matching: profileName) {
                    return url
                }
            }
        }
        return nil
    }

    private static func findDcp(in directory: URL, matching profileName: String) -> URL? {
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil
        ) else { return nil }

        let lowerName = profileName.lowercased()
        for entry in entries {
            guard entry.pathExtension.lowercased() == "dcp" else { continue }
            let stem = entry.deletingPathExtension().lastPathComponent
            if stem.localizedCaseInsensitiveContains(lowerName) {
                return entry
            }
        }
        return nil
    }

    /// Case-insensitive file lookup in a directory.
    private static func findFile(in directory: URL, named filename: String) -> URL? {
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil
        ) else { return nil }
        let lower = filename.lowercased()
        return entries.first { $0.lastPathComponent.lowercased() == lower }
    }

    /// Case-insensitive subdirectory lookup.
    private static func findSubdirectory(in parent: URL, named name: String) -> URL? {
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: parent, includingPropertiesForKeys: [.isDirectoryKey]
        ) else { return nil }
        let lower = name.lowercased()
        return entries.first {
            $0.lastPathComponent.lowercased() == lower
            && (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
        }
    }

    private static func findAnyDcp(in directory: URL) -> URL? {
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil
        ) else { return nil }

        return entries.first { $0.pathExtension.lowercased() == "dcp" }
    }
}
