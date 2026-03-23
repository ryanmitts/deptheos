import CoreImage
import Foundation
import os

/// Unified image loader: detects format and loads accordingly.
///
/// Single entry point for loading any supported image format.
/// - DNG files: loaded via Adobe DNG SDK with full metadata and render params
/// - Camera RAW: loaded via LibRaw, Adobe DNG Converter, or Apple CIRAWFilter
/// - Standard formats: loaded via CIImage in linear space with sRGB identity params
public struct UnifiedImageLoader: Sendable {
    private static let logger = Logger(subsystem: "com.deptheos", category: "ImageLoader")
    private static let signposter = OSSignposter(subsystem: "com.deptheos", category: .pointsOfInterest)

    private let imageReader: ImageReader

    /// Which backend to use for camera RAW files (CR3, NEF, ARW, etc.)
    public var cameraRAWBackend: CameraRAWBackend = .libRaw

    /// Cache for converted DNG files (shared across loads to avoid re-converting).
    public var dngConverterCache: DNGConverterCache?

    /// Which DCP profile to use for camera RAW files loaded via LibRaw.
    public var dcpProfilePreference: DcpProfilePreference = .adobeStandard

    /// Additional directories to search for DCP profiles before the default Adobe paths.
    /// Supports flat layout: any `.dcp` file whose name contains the camera model.
    public var customProfileDirectories: [String] = []

    public init(ciContext: CIContext) {
        self.imageReader = ImageReader(ciContext: ciContext)
    }

    /// Load an image from a URL, returning a DeptheosImage.
    ///
    /// All images are returned in linear space with non-optional `dngRenderParams`:
    /// - DNG: already linear, real DNG color science params
    /// - Camera RAW: linear via LibRaw, render params from LibRaw color matrices
    /// - Standard formats: linearized by CIImage, sRGB identity params
    public func load(url: URL, metalContext: MetalContext) throws -> DeptheosImage {
        let loadStart = CFAbsoluteTimeGetCurrent()
        defer {
            let elapsed = CFAbsoluteTimeGetCurrent() - loadStart
            print("[ImageLoader] \(url.lastPathComponent): \(String(format: "%.3f", elapsed))s")
        }
        let format = ImageFormat.detect(from: url)

        let filename = url.lastPathComponent

        switch format {
        case .linearDNG:
            let state = Self.signposter.beginInterval("Load DNG", "\(filename, privacy: .public)")
            let result = try loadDNG(url: url)
            Self.signposter.endInterval("Load DNG", state)
            return result

        case .cameraRAW:
            switch cameraRAWBackend {
            case .adobeDNGConverter(let converterPath):
                #if os(macOS)
                let state = Self.signposter.beginInterval("Load CameraRAW (Adobe DNG)", "\(filename, privacy: .public)")
                let dngURL: URL
                if let cache = dngConverterCache {
                    dngURL = try cache.dngURL(for: url, executablePath: converterPath)
                } else {
                    let tempDir = FileManager.default.temporaryDirectory
                        .appendingPathComponent("deptheos-dng-\(UUID().uuidString)")
                    let outputName = url.deletingPathExtension().lastPathComponent + ".dng"
                    dngURL = try DNGConverter.convert(
                        inputURL: url,
                        outputDirectory: tempDir,
                        outputName: outputName,
                        executablePath: converterPath
                    )
                }
                let result = try loadDNG(url: dngURL)
                Self.signposter.endInterval("Load CameraRAW (Adobe DNG)", state)
                return result
                #else
                throw ImageError.readFailed("Adobe DNG Converter is not available on this platform")
                #endif

            case .libRaw:
                let state = Self.signposter.beginInterval("Load CameraRAW (LibRaw)", "\(filename, privacy: .public)")
                let result = try loadCameraRAW(url: url)
                Self.signposter.endInterval("Load CameraRAW (LibRaw)", state)
                return result

            case .apple:
                let state = Self.signposter.beginInterval("Load CameraRAW (CIRAWFilter)", "\(filename, privacy: .public)")
                let buffer = try imageReader.loadRAW(url: url)
                let result = DeptheosImage(
                    pixelBuffer: buffer,
                    dngRenderParams: .srgbIdentity(),
                    sourceBitDepth: 16,
                    sourceFormat: format
                )
                Self.signposter.endInterval("Load CameraRAW (CIRAWFilter)", state)
                return result
            }

        case .standard:
            let state = Self.signposter.beginInterval("Load Standard", "\(filename, privacy: .public)")
            let (buffer, bitDepth) = try imageReader.loadWithBitDepth(url: url)
            let result = DeptheosImage(
                pixelBuffer: buffer,
                dngRenderParams: .srgbIdentity(),
                sourceBitDepth: bitDepth,
                sourceFormat: format
            )
            Self.signposter.endInterval("Load Standard", state)
            return result
        }
    }

    // MARK: - DNG Loading (Adobe DNG SDK)

    private func loadDNG(url: URL) throws -> DeptheosImage {
        guard let dngResult = DeptheosDng.readDngPixelBuffer(at: url.path) else {
            throw ImageError.readFailed(url.lastPathComponent)
        }
        guard let renderParams = DeptheosDng.computeRenderParams(metadata: dngResult.metadata) else {
            throw ImageError.readFailed("Failed to compute render params: \(url.lastPathComponent)")
        }
        return DeptheosImage(
            pixelBuffer: dngResult.pixelBuffer,
            dngMetadata: dngResult.metadata,
            dngRenderParams: renderParams,
            sourceBitDepth: 16,
            sourceFormat: .linearDNG
        )
    }

    // MARK: - Camera RAW Loading (LibRaw → linear)

    private func loadCameraRAW(url: URL) throws -> DeptheosImage {
        guard let rawResult = DeptheosRaw.readRawLinearPixelBuffer(at: url.path) else {
            throw ImageError.readFailed(url.lastPathComponent)
        }

        // Build base DNG metadata from LibRaw's color science
        var dngMetadata = DeptheosDng.ImageMetadata.fromLibRaw(rawResult.metadata)

        // Try to apply an Adobe DCP profile for better color science.
        // Adobe DCP filenames use "Make Model" (e.g. "Canon EOS R5 Mark II"),
        // but LibRaw's uniqueCameraModel often omits the make. Prepend if needed.
        let cameraModel = Self.buildCameraModel(from: rawResult.metadata)
        dngMetadata.camera.uniqueCameraModel = cameraModel

        if dcpProfilePreference != .none,
           let dcpProfile = DcpProfileCache.shared.profile(
               forCamera: cameraModel, preference: dcpProfilePreference,
               customDirectories: customProfileDirectories
           ) {
            applyDcpProfile(dcpProfile, to: &dngMetadata)
        }

        // Compute render params via the DNG SDK (dual-illuminant interpolation, etc.)
        let renderParams: DeptheosDng.RenderParams
        if let params = DeptheosDng.computeRenderParams(metadata: dngMetadata) {
            renderParams = params
        } else {
            Self.logger.warning("computeRenderParams failed for \(url.lastPathComponent), using sRGB identity")
            renderParams = .srgbIdentity()
        }

        return DeptheosImage(
            pixelBuffer: rawResult.pixelBuffer,
            dngMetadata: dngMetadata,
            dngRenderParams: renderParams,
            sourceBitDepth: 16,
            sourceFormat: .cameraRAW
        )
    }

    // MARK: - Camera Model Normalization

    /// Build a normalized "Make Model" string for DCP profile lookup.
    /// Trims whitespace, uses case-insensitive prefix check, and prepends make
    /// only when the model doesn't already start with it.
    static func buildCameraModel(from raw: DeptheosRaw.RawMetadata) -> String {
        let rawModel = raw.uniqueCameraModel.isEmpty ? raw.model : raw.uniqueCameraModel
        let model = rawModel.trimmingCharacters(in: .whitespaces)
        let make = raw.make.trimmingCharacters(in: .whitespaces)

        if !make.isEmpty && !model.lowercased().hasPrefix(make.lowercased()) {
            return "\(make) \(model)"
        }
        return model
    }

    // MARK: - DCP Profile Application

    /// Override LibRaw's color science with a parsed DCP profile.
    /// Replaces matrices, illuminants, LUTs, and tone curve.
    private func applyDcpProfile(
        _ dcp: DeptheosDng.DcpProfileData,
        to metadata: inout DeptheosDng.ImageMetadata
    ) {
        let cs = dcp.colorScience
        metadata.colorScience.colorMatrix1 = cs.colorMatrix1
        metadata.colorScience.colorMatrix2 = cs.colorMatrix2
        metadata.colorScience.forwardMatrix1 = cs.forwardMatrix1
        metadata.colorScience.forwardMatrix2 = cs.forwardMatrix2
        metadata.colorScience.calibrationIlluminant1 = cs.calibrationIlluminant1
        metadata.colorScience.calibrationIlluminant2 = cs.calibrationIlluminant2
        metadata.colorScience.profileCalibrationSig = cs.profileCalibrationSig
        metadata.colorScience.profileName = cs.profileName

        metadata.luts = dcp.luts

        if let offset = dcp.baselineExposureOffset {
            let existing = metadata.colorScience.baselineExposure ?? 0
            metadata.colorScience.baselineExposure = existing + offset
        }
    }
}
