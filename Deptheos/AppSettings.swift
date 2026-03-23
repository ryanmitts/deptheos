import Foundation
import DeptheosKit

/// Global application preferences — persisted via UserDefaults.
///
/// These are machine-level or user-level settings that are never saved/restored
/// per-project. Injected into the SwiftUI view hierarchy via `.environment()`.
@Observable
@MainActor
final class AppSettings {
    // MARK: - Persisted Settings

    /// Path to the Adobe DNG Converter executable (macOS only).
    var dngConverterPath: String {
        didSet { UserDefaults.standard.set(dngConverterPath, forKey: Keys.dngConverterPath) }
    }

    /// Directory to search for DCP profiles (flat layout: any `.dcp` matching camera model).
    /// Defaults to the Adobe Standard profile install path.
    var dcpProfileDirectory: String {
        didSet { UserDefaults.standard.set(dcpProfileDirectory, forKey: Keys.dcpProfileDirectory) }
    }

    /// Whether to run ECC alignment on the image stack.
    var alignmentEnabled: Bool {
        didSet { UserDefaults.standard.set(alignmentEnabled, forKey: Keys.alignmentEnabled) }
    }

    /// Whether to auto-crop alignment borders from the stacked result.
    var autoCrop: Bool {
        didSet { UserDefaults.standard.set(autoCrop, forKey: Keys.autoCrop) }
    }

    // MARK: - Defaults

    nonisolated static let defaultDcpProfileDirectory =
        "/Library/Application Support/Adobe/CameraRaw/CameraProfiles/Adobe Standard"

    // MARK: - Init

    init() {
        let converterPath = UserDefaults.standard.string(forKey: Keys.dngConverterPath)
            ?? DNGConverter.defaultExecutablePath
        self.dngConverterPath = converterPath
        self.dcpProfileDirectory = UserDefaults.standard.string(forKey: Keys.dcpProfileDirectory)
            ?? Self.defaultDcpProfileDirectory
        self.alignmentEnabled = UserDefaults.standard.object(forKey: Keys.alignmentEnabled) as? Bool ?? true
        self.autoCrop = UserDefaults.standard.object(forKey: Keys.autoCrop) as? Bool ?? true
    }

    // MARK: - Validation

    var isDngConverterAvailable: Bool {
        FileManager.default.isExecutableFile(atPath: dngConverterPath)
    }

    var isDcpDirectoryValid: Bool {
        var isDir: ObjCBool = false
        return FileManager.default.fileExists(atPath: dcpProfileDirectory, isDirectory: &isDir) && isDir.boolValue
    }

    // MARK: - Keys

    private enum Keys {
        static let dngConverterPath = "settings.dngConverterPath"
        static let dcpProfileDirectory = "settings.dcpProfileDirectory"
        static let alignmentEnabled = "settings.alignmentEnabled"
        static let autoCrop = "settings.autoCrop"
    }
}
