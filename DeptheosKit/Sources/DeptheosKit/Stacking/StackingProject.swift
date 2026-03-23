//
//  StackingProject.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-24.
//

import Foundation
import Metal

/// A persistent stacking project combining result textures with stacking configuration.
///
/// Created from a `StackingResult` after stacking completes.
/// Saved as a `.deptheos` package directory containing:
/// - `result.jxl` — lossless linear RGBA u16 result texture
/// - `metadata.json` — project metadata (config, source paths, transforms, etc.)
/// - `thumbnail.heic` — 512px sRGB preview for Finder/QuickLook
public struct StackingProject: @unchecked Sendable {

    // MARK: - Result Textures (GPU-resident when loaded)

    /// Stacked result texture (rgba16Unorm, linear).
    public let resultTexture: MTLTexture

    /// Raw focus metric texture (R32Float). Algorithm-specific.
    public let focusMapTexture: MTLTexture?

    /// Rendered focus map visualization (rgba8Unorm, sRGB). Persisted as depthmap.jpg.
    public var bakedFocusMapTexture: MTLTexture?

    // MARK: - Stacking Configuration

    /// Algorithm choice and parameters used for this stack.
    public let config: StackingConfig

    /// Camera RAW backend used during stacking.
    public let cameraRAWBackend: CameraRAWBackend

    /// Absolute paths to source images.
    public let sourceImagePaths: [String]

    /// Per-frame alignment transforms (first frame is nil/identity).
    public let transforms: [AffineTransform2D?]

    /// Valid crop area after alignment. Nil if no alignment was used or auto-crop was off.
    public let validArea: Rect?

    /// Per-image inclusion state (true = participated in the stack).
    public let imageIncluded: [Bool]

    // MARK: - Source Metadata

    /// Source image bit depth (8 or 16).
    public let sourceBitDepth: Int

    /// DNG color pipeline parameters.
    public let dngRenderParams: DeptheosDng.RenderParams

    /// Source DNG metadata (if available).
    public let dngMetadata: DeptheosDng.ImageMetadata?

    /// When this project was created.
    public let createdAt: Date

    // MARK: - Paint Data (mutable, set by app layer)

    /// Paint canvas texture (rgba16Unorm, matching result texture format). Set when paint mode edits have been applied.
    public var paintTexture: MTLTexture?

    // MARK: - Init from StackingResult

    public init(
        result: StackingResult,
        config: StackingConfig,
        cameraRAWBackend: CameraRAWBackend = .libRaw,
        sourceImagePaths: [String],
        transforms: [AffineTransform2D?],
        validArea: Rect?,
        imageIncluded: [Bool],
        createdAt: Date = Date()
    ) {
        self.resultTexture = result.resultTexture
        self.focusMapTexture = result.focusMapTexture
        self.bakedFocusMapTexture = result.bakedFocusMapTexture
        self.config = config
        self.cameraRAWBackend = cameraRAWBackend
        self.sourceImagePaths = sourceImagePaths
        self.transforms = transforms
        self.validArea = validArea
        self.imageIncluded = imageIncluded
        self.sourceBitDepth = result.sourceBitDepth
        self.dngRenderParams = result.dngRenderParams
        self.dngMetadata = result.dngMetadata
        self.createdAt = createdAt
    }

    // MARK: - Export

    /// Export the result to a standard image format.
    /// If paint data exists, exports the painted result; otherwise the stacking result.
    public func export(to url: URL, format: ExportFormat, metalContext: MetalContext, quality: Double = 0.95) async throws {
        let exportTexture = paintTexture ?? resultTexture
        let result = StackingResult(
            resultTexture: exportTexture,
            focusMapTexture: nil,
            validArea: nil,
            metalContext: metalContext,
            sourceBitDepth: sourceBitDepth,
            dngRenderParams: dngRenderParams,
            dngMetadata: dngMetadata
        )
        try await result.save(to: url, format: format, quality: quality)
    }
}

// MARK: - Metadata (Codable, persisted as metadata.json)

/// Internal Codable representation of project metadata.
public struct ProjectMetadata: Codable, Sendable {
    public let config: StackingConfig
    public let cameraRAWBackend: CameraRAWBackend
    public let sourceImagePaths: [String]
    public let transforms: [AffineTransform2D?]
    public let validArea: Rect?
    public let imageIncluded: [Bool]
    public let sourceBitDepth: Int
    public let dngRenderParams: DeptheosDng.RenderParams
    public let dngMetadata: DeptheosDng.ImageMetadata?
    public let resultWidth: Int
    public let resultHeight: Int
    public let createdAt: Date
    public let hasPaintData: Bool
    public let hasDepthMap: Bool

    // Backward-compatible decoding — old projects won't have hasPaintData or hasDepthMap
    enum CodingKeys: String, CodingKey {
        case config, cameraRAWBackend, sourceImagePaths, transforms, validArea
        case imageIncluded, sourceBitDepth, dngRenderParams, dngMetadata
        case resultWidth, resultHeight, createdAt, hasPaintData, hasDepthMap
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        config = try container.decode(StackingConfig.self, forKey: .config)
        cameraRAWBackend = try container.decodeIfPresent(CameraRAWBackend.self, forKey: .cameraRAWBackend) ?? .libRaw
        sourceImagePaths = try container.decode([String].self, forKey: .sourceImagePaths)
        transforms = try container.decode([AffineTransform2D?].self, forKey: .transforms)
        validArea = try container.decodeIfPresent(Rect.self, forKey: .validArea)
        imageIncluded = try container.decode([Bool].self, forKey: .imageIncluded)
        sourceBitDepth = try container.decode(Int.self, forKey: .sourceBitDepth)
        dngRenderParams = try container.decode(DeptheosDng.RenderParams.self, forKey: .dngRenderParams)
        dngMetadata = try container.decodeIfPresent(DeptheosDng.ImageMetadata.self, forKey: .dngMetadata)
        resultWidth = try container.decode(Int.self, forKey: .resultWidth)
        resultHeight = try container.decode(Int.self, forKey: .resultHeight)
        createdAt = try container.decode(Date.self, forKey: .createdAt)
        hasPaintData = try container.decodeIfPresent(Bool.self, forKey: .hasPaintData) ?? false
        hasDepthMap = try container.decodeIfPresent(Bool.self, forKey: .hasDepthMap) ?? false
    }

    public init(
        config: StackingConfig,
        cameraRAWBackend: CameraRAWBackend = .libRaw,
        sourceImagePaths: [String], transforms: [AffineTransform2D?],
        validArea: Rect?, imageIncluded: [Bool],
        sourceBitDepth: Int, dngRenderParams: DeptheosDng.RenderParams,
        dngMetadata: DeptheosDng.ImageMetadata?, resultWidth: Int,
        resultHeight: Int, createdAt: Date, hasPaintData: Bool = false,
        hasDepthMap: Bool = false
    ) {
        self.config = config
        self.cameraRAWBackend = cameraRAWBackend
        self.sourceImagePaths = sourceImagePaths
        self.transforms = transforms
        self.validArea = validArea
        self.imageIncluded = imageIncluded
        self.sourceBitDepth = sourceBitDepth
        self.dngRenderParams = dngRenderParams
        self.dngMetadata = dngMetadata
        self.resultWidth = resultWidth
        self.resultHeight = resultHeight
        self.createdAt = createdAt
        self.hasPaintData = hasPaintData
        self.hasDepthMap = hasDepthMap
    }
}

// MARK: - Errors

public enum ProjectError: LocalizedError {
    case readbackFailed
    case uploadFailed
    case metadataCorrupted

    public var errorDescription: String? {
        switch self {
        case .readbackFailed: "Failed to read result texture from GPU"
        case .uploadFailed: "Failed to upload texture to GPU"
        case .metadataCorrupted: "Project metadata is missing or corrupted"
        }
    }
}
