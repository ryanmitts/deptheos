import CommonCrypto
import Foundation
import Synchronization
import os

/// Strategy for loading camera RAW files (CR3, NEF, ARW, etc.)
public enum CameraRAWBackend: Sendable, Codable {
    /// Apple's CIRAWFilter (sRGB output)
    case apple
    /// LibRaw linear + DNG GPU pipeline (default)
    case libRaw
    /// Adobe DNG Converter → linear DNG → DNG SDK pipeline.
    /// Uses the standard macOS install path unless overridden.
    case adobeDNGConverter(path: String = DNGConverter.defaultExecutablePath)

    // Custom Codable — encode as a simple string tag (the converter path is
    // not persisted since it's always the default or overridden at runtime).
    private enum Tag: String, Codable {
        case apple, libRaw, adobeDNGConverter
    }

    public init(from decoder: Decoder) throws {
        let tag = try Tag(from: decoder)
        switch tag {
        case .apple: self = .apple
        case .libRaw: self = .libRaw
        case .adobeDNGConverter: self = .adobeDNGConverter()
        }
    }

    public func encode(to encoder: Encoder) throws {
        let tag: Tag = switch self {
        case .apple: .apple
        case .libRaw: .libRaw
        case .adobeDNGConverter: .adobeDNGConverter
        }
        try tag.encode(to: encoder)
    }
}

/// Converts camera RAW files to linear DNG using the Adobe DNG Converter application.
public struct DNGConverter: Sendable {

    public static let defaultExecutablePath =
        "/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"

    /// Whether the Adobe DNG Converter is installed at the default path.
    public static var isAvailable: Bool {
        #if os(macOS)
        FileManager.default.isExecutableFile(atPath: defaultExecutablePath)
        #else
        false
        #endif
    }

    #if os(macOS)
    /// Convert a camera RAW file to linear DNG, writing output into `outputDirectory`.
    ///
    /// - Parameters:
    ///   - inputURL: URL of the camera RAW file to convert.
    ///   - outputDirectory: Directory where the converted DNG will be written.
    ///   - executablePath: Path to the Adobe DNG Converter executable.
    /// - Returns: The URL of the output `.dng` file.
    public static func convert(
        inputURL: URL,
        outputDirectory: URL,
        outputName: String,
        executablePath: String = defaultExecutablePath
    ) throws -> URL {
        guard FileManager.default.isExecutableFile(atPath: executablePath) else {
            throw ConversionError.converterNotFound(executablePath)
        }

        try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)

        let process = Process()
        process.executableURL = URL(fileURLWithPath: executablePath)
        process.arguments = [
            "-l",                           // linear DNG output
            "-d", outputDirectory.path,     // output directory
            "-o", outputName,               // output filename
            inputURL.path,                  // input file
        ]

        let stderrPipe = Pipe()
        process.standardError = stderrPipe

        try process.run()
        let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        guard process.terminationStatus == 0 else {
            let stderr = String(data: stderrData, encoding: .utf8) ?? ""
            throw ConversionError.conversionFailed(
                status: process.terminationStatus, stderr: stderr
            )
        }

        let dngURL = outputDirectory.appendingPathComponent(outputName)
        guard FileManager.default.fileExists(atPath: dngURL.path) else {
            throw ConversionError.outputNotFound
        }

        return dngURL
    }
    #endif

    public enum ConversionError: LocalizedError {
        case converterNotFound(String)
        case conversionFailed(status: Int32, stderr: String)
        case outputNotFound

        public var errorDescription: String? {
            switch self {
            case .converterNotFound(let path):
                return "Adobe DNG Converter not found at \(path)"
            case .conversionFailed(let status, let stderr):
                return "DNG conversion failed (exit \(status)): \(stderr)"
            case .outputNotFound:
                return "DNG conversion produced no output file"
            }
        }
    }
}

/// Caches converted DNG files in Application Support so each RAW file is only
/// converted once per session. The cache directory is cleaned up on `purge()`.
public final class DNGConverterCache: Sendable {

    private let cacheDirectory: URL
    /// One in-memory entry per source path. The `key` is a composite hash of
    /// `(path, size, mtime)`; if the source file is edited in place its key
    /// changes and we treat the cached entry as stale.
    private struct Entry: Sendable {
        let key: String
        let url: URL
    }
    private let entries = Mutex<[String: Entry]>([:])

    public init() {
        let appSupport = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first!
        cacheDirectory = appSupport
            .appendingPathComponent("Deptheos", isDirectory: true)
            .appendingPathComponent("DNGCache", isDirectory: true)
    }

    /// Create a cache rooted at a caller-provided directory. Useful for
    /// process-scoped caches (e.g. CLI invocations) that should not share
    /// the default `Application Support` location with the app.
    public init(cacheDirectory: URL) {
        self.cacheDirectory = cacheDirectory
    }

    #if os(macOS)
    /// Return a cached DNG for the source URL, converting if needed.
    public func dngURL(
        for sourceURL: URL,
        executablePath: String = DNGConverter.defaultExecutablePath
    ) throws -> URL {
        let path = sourceURL.path
        let key = try cacheKey(for: sourceURL)

        // Fast path: still-valid entry whose on-disk file exists.
        if let entry = entries.withLock({ $0[path] }),
           entry.key == key,
           FileManager.default.fileExists(atPath: entry.url.path) {
            return entry.url
        }

        // Stale entry (file was edited in place): delete the orphaned DNG so
        // the cache directory doesn't accumulate dead files across edits.
        if let stale = entries.withLock({ $0[path] }) {
            try? FileManager.default.removeItem(at: stale.url)
        }

        // Write into the flat cache directory with a hash-derived filename.
        let dngURL = try DNGConverter.convert(
            inputURL: sourceURL,
            outputDirectory: cacheDirectory,
            outputName: "\(key).dng",
            executablePath: executablePath
        )

        entries.withLock { $0[path] = Entry(key: key, url: dngURL) }
        return dngURL
    }
    #endif

    /// Delete all cached DNG files.
    public func purge() {
        entries.withLock { $0.removeAll() }
        try? FileManager.default.removeItem(at: cacheDirectory)
    }

    /// Total size of cached files in bytes.
    public var sizeOnDisk: UInt64 {
        guard let enumerator = FileManager.default.enumerator(
            at: cacheDirectory,
            includingPropertiesForKeys: [.fileSizeKey],
            options: [.skipsHiddenFiles]
        ) else { return 0 }

        var total: UInt64 = 0
        for case let url as URL in enumerator {
            if let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize {
                total += UInt64(size)
            }
        }
        return total
    }

    /// Stable filename stem derived from `(path, size, mtime)`. Editing the
    /// source file in place changes its size or mtime, which produces a fresh
    /// key and forces re-conversion.
    private func cacheKey(for url: URL) throws -> String {
        let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
        let size = (attrs[.size] as? NSNumber)?.uint64Value ?? 0
        let mtime = (attrs[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
        let composite = "\(url.path)|\(size)|\(mtime)"
        let data = Data(composite.utf8)
        var hash = [UInt8](repeating: 0, count: Int(CC_SHA256_DIGEST_LENGTH))
        data.withUnsafeBytes { buffer in
            _ = CC_SHA256(buffer.baseAddress, CC_LONG(buffer.count), &hash)
        }
        return hash.prefix(16).map { String(format: "%02x", $0) }.joined()
    }
}
