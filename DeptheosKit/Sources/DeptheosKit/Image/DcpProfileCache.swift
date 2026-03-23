import os
import Foundation

/// Thread-safe cache for parsed DCP (DNG Camera Profile) data.
/// Profiles are loaded once per camera model and reused for subsequent images.
public final class DcpProfileCache: Sendable {
    private static let logger = Logger(subsystem: "com.deptheos", category: "DcpProfileCache")
    public static let shared = DcpProfileCache()

    /// Maps camera model → parsed profile (nil value means "searched but not found").
    private let cache = OSAllocatedUnfairLock<[String: DeptheosDng.DcpProfileData?]>(initialState: [:])

    private init() {}

    /// Look up (or load) a DCP profile for the given camera model.
    /// Returns nil if no matching profile is installed.
    public func profile(
        forCamera model: String,
        preference: DcpProfilePreference = .adobeStandard,
        customDirectories: [String] = []
    ) -> DeptheosDng.DcpProfileData? {
        let key = "\(model)|\(preference)"

        if let cached = cache.withLock({ $0[key] }) {
            return cached
        }

        // Search and load outside the lock
        guard let url = DcpProfileFinder.findProfile(
            forCamera: model, preference: preference,
            customDirectories: customDirectories
        ) else {
            cache.withLock { $0[key] = .some(nil) }
            return nil
        }

        let result = DeptheosDng.loadDcpProfile(at: url.path)
        if let result {
            Self.logger.info("Loaded '\(result.colorScience.profileName)' for \(model)")
        } else {
            Self.logger.warning("Failed to parse DCP at \(url.path)")
        }

        cache.withLock { $0[key] = result }
        return result
    }
}
