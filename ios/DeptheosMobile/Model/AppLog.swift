import Foundation
import os
@preconcurrency import DeptheosEngine

/// The app's logger: each line goes to `os.Logger` and into the engine's
/// log file (what Send Diagnostics zips), under its category.
///
/// Interpolations follow `os.Logger`'s privacy: numbers are written, a
/// string only when `privacy: .public`, else `<private>` in the file. The
/// engine's own lines name files and folders, and a public string can too.
/// The container is written `~` in every line. In `os.Logger` a line with
/// a private part is private as a whole.
nonisolated struct AppLog: Sendable {
    private let logger: Logger
    private let category: String

    /// Start the log file in `Library/Caches/Logs` (not backed up; iOS
    /// empties it only when storage runs very low), headed with the version,
    /// and build. Once at launch, before the engine; earlier lines reach
    /// only `os.Logger`.
    static func install() async {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let directory = caches.appending(path: "Logs", directoryHint: .isDirectory)
        let info = Bundle.main.infoDictionary
        await installLogging(
            directory: directory.path,
            version: info?["CFBundleShortVersionString"] as? String ?? "?",
            build: info?["CFBundleVersion"] as? String ?? "")
    }

    init(category: String) {
        logger = Logger(subsystem: "Deptheos", category: category)
        self.category = category
    }

    func debug(_ message: LogMessage) { write(.debug, message) }
    func info(_ message: LogMessage) { write(.info, message) }
    func warning(_ message: LogMessage) { write(.warning, message) }
    func error(_ message: LogMessage) { write(.error, message) }

    private func write(_ level: LogLevel, _ message: LogMessage) {
        let type: OSLogType =
            switch level {
            case .debug: .debug
            case .info: .info
            case .warning: .default
            case .error: .error
            }
        if message.isPublic {
            logger.log(level: type, "\(message.full, privacy: .public)")
        } else {
            logger.log(level: type, "\(message.full, privacy: .private)")
        }
        logLine(level: level, category: category, message: message.redacted)
    }
}

/// Whether an interpolated string may go in the log file.
nonisolated enum LogPrivacy: Sendable {
    case `public`
    case `private`
}

/// How an interpolated number is written.
nonisolated enum LogFloatFormat: Sendable {
    case fixed(precision: Int)
}

/// A line for `AppLog`, written as an `os.Logger` message is.
nonisolated struct LogMessage: ExpressibleByStringInterpolation, Sendable {
    /// For `os.Logger`.
    let full: String
    /// Each private part `<private>`, for the file.
    let redacted: String
    let isPublic: Bool

    init(stringLiteral value: String) {
        (full, redacted, isPublic) = (value, value, true)
    }

    init(stringInterpolation: StringInterpolation) {
        full = stringInterpolation.full
        redacted = stringInterpolation.redacted
        isPublic = stringInterpolation.isPublic
    }

    struct StringInterpolation: StringInterpolationProtocol {
        var full = ""
        var redacted = ""
        var isPublic = true

        init(literalCapacity: Int, interpolationCount: Int) {
            full.reserveCapacity(literalCapacity)
            redacted.reserveCapacity(literalCapacity)
        }

        mutating func appendLiteral(_ literal: String) {
            append(literal, privacy: .public)
        }

        mutating func appendInterpolation<T: BinaryInteger>(_ value: T) {
            append(String(value), privacy: .public)
        }

        mutating func appendInterpolation<T: BinaryFloatingPoint>(
            _ value: T, format: LogFloatFormat? = nil
        ) {
            let text =
                switch format {
                case .fixed(let precision): String(format: "%.\(precision)f", Double(value))
                case nil: String(describing: value)
                }
            append(text, privacy: .public)
        }

        mutating func appendInterpolation(_ value: String, privacy: LogPrivacy = .private) {
            append(value, privacy: privacy)
        }

        private mutating func append(_ text: String, privacy: LogPrivacy) {
            full += text
            switch privacy {
            case .public:
                redacted += text
            case .private:
                redacted += "<private>"
                isPublic = false
            }
        }
    }
}
