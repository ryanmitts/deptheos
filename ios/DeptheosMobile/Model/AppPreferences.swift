import Foundation
import Observation
@preconcurrency import DeptheosEngine

/// The preferences iOS shares with the desktop (ADR-150): exports and Grid
/// search. Defaults, ranges and words are the engine's; values live in
/// `UserDefaults`, clamped as read (`sanitizedPreferences`). Also the
/// Export sheet's format per kind of stack.
@Observable
final class AppPreferences {
    var export: ExportPreferences {
        didSet { if export != oldValue { save() } }
    }
    /// Grid search's limits and exposure compensation.
    var alignment: AlignmentPreferences {
        didSet { if alignment != oldValue { save() } }
    }

    /// The kinds of stack the Export sheet remembers a format for.
    enum ExportKind: String {
        /// JPEG, PNG and TIFF frames.
        case standard
        /// RAW and DNG frames: DNG too.
        case dng
    }

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let stored = Self.read(defaults)
        export = stored.export
        alignment = stored.alignment
    }

    func lastFormat(for kind: ExportKind) -> ExportFormat? {
        defaults.string(forKey: Key.format(kind)).flatMap(ExportFormat.init(stored:))
    }

    func remember(_ format: ExportFormat, for kind: ExportKind) {
        defaults.set(format.stored, forKey: Key.format(kind))
    }

    // MARK: Storage

    private enum Key {
        static let fileName = "ExportFileName"
        static let jpegQuality = "ExportJpegQuality"
        static let sixteenBit = "ExportSixteenBit"
        static let tiffCompression = "ExportTiffCompression"
        static let space = "ExportSpace"
        static let gridHorizontal = "GridHorizontalPercent"
        static let gridVertical = "GridVerticalPercent"
        static let gridRotation = "GridRotationDegrees"
        static let gridMagnification = "GridMagnificationPercent"
        static let exposureCompensation = "ExposureCompensation"

        static func format(_ kind: ExportKind) -> String { "ExportFormat-\(kind.rawValue)" }
    }

    /// What is stored, over the defaults, in its ranges.
    private static func read(_ defaults: UserDefaults) -> SharedPreferences {
        var shared = defaultPreferences()
        func number(_ key: String) -> Double? { defaults.object(forKey: key) as? Double }
        if let name = defaults.string(forKey: Key.fileName) { shared.export.fileNamePattern = name }
        if let quality = number(Key.jpegQuality) {
            // Clamped below; a stored value can be anything.
            shared.export.jpegQuality = UInt8(clamping: Int(quality.isFinite ? quality : 0))
        }
        if let sixteen = defaults.object(forKey: Key.sixteenBit) as? Bool { shared.export.sixteenBit = sixteen }
        if let tiff = defaults.string(forKey: Key.tiffCompression).flatMap(TiffCompression.init(stored:)) {
            shared.export.tiffCompression = tiff
        }
        if let space = defaults.string(forKey: Key.space).flatMap(ExportSpace.init(stored:)) {
            shared.export.space = space
        }
        if let value = number(Key.gridHorizontal) { shared.alignment.gridHorizontalPct = Float(value) }
        if let value = number(Key.gridVertical) { shared.alignment.gridVerticalPct = Float(value) }
        if let value = number(Key.gridRotation) { shared.alignment.gridRotationDeg = Float(value) }
        if let value = number(Key.gridMagnification) { shared.alignment.gridMagnificationPct = Float(value) }
        if let on = defaults.object(forKey: Key.exposureCompensation) as? Bool {
            shared.alignment.exposureCompensation = on
        }
        return sanitizedPreferences(preferences: shared)
    }

    private func save() {
        defaults.set(export.fileNamePattern, forKey: Key.fileName)
        defaults.set(Double(export.jpegQuality), forKey: Key.jpegQuality)
        defaults.set(export.sixteenBit, forKey: Key.sixteenBit)
        defaults.set(export.tiffCompression.stored, forKey: Key.tiffCompression)
        defaults.set(export.space.stored, forKey: Key.space)
        defaults.set(Double(alignment.gridHorizontalPct), forKey: Key.gridHorizontal)
        defaults.set(Double(alignment.gridVerticalPct), forKey: Key.gridVertical)
        defaults.set(Double(alignment.gridRotationDeg), forKey: Key.gridRotation)
        defaults.set(Double(alignment.gridMagnificationPct), forKey: Key.gridMagnification)
        defaults.set(alignment.exposureCompensation, forKey: Key.exposureCompensation)
    }
}

/// "2026-11-14", in the device's time zone.
nonisolated func exportDate(_ date: Date = .now) -> String {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.dateFormat = "yyyy-MM-dd"
    return formatter.string(from: date)
}

extension ExportFormat {
    /// What `UserDefaults` keeps: never changes.
    nonisolated var stored: String {
        switch self {
        case .jpeg: "jpeg"
        case .png: "png"
        case .tiff: "tiff"
        case .dng: "dng"
        }
    }

    nonisolated init?(stored: String) {
        switch stored {
        case "jpeg": self = .jpeg
        case "png": self = .png
        case "tiff": self = .tiff
        case "dng": self = .dng
        default: return nil
        }
    }
}

extension TiffCompression: @retroactive CaseIterable {
    public static var allCases: [TiffCompression] { [.none, .lzw, .zip] }

    /// What `UserDefaults` keeps: never changes.
    nonisolated var stored: String {
        switch self {
        case .none: "none"
        case .lzw: "lzw"
        case .zip: "zip"
        }
    }

    nonisolated init?(stored: String) {
        switch stored {
        case "none": self = .none
        case "lzw": self = .lzw
        case "zip": self = .zip
        default: return nil
        }
    }
}

extension ExportSpace: @retroactive CaseIterable {
    public static var allCases: [ExportSpace] { [.srgb, .displayP3, .adobeRgb, .proPhoto] }

    /// What `UserDefaults` keeps, the desktop's key: never changes.
    nonisolated var stored: String {
        switch self {
        case .srgb: "srgb"
        case .displayP3: "display_p3"
        case .adobeRgb: "adobe_rgb"
        case .proPhoto: "pro_photo"
        }
    }

    nonisolated init?(stored: String) {
        switch stored {
        case "srgb": self = .srgb
        case "display_p3": self = .displayP3
        case "adobe_rgb": self = .adobeRgb
        case "pro_photo": self = .proPhoto
        default: return nil
        }
    }
}
