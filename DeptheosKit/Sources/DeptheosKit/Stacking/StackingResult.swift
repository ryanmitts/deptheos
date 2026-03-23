//
//  StackingResult.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-23.
//
import Metal
import CoreVideo
import Foundation
import os

/// Export format for saving stacking results.
public enum ExportFormat: String, Sendable {
    case jpeg
    case png
    case tiff
    case dng
    case jxl
}

/// The output of a stacking session. All textures are in linear color space.
private let exportSignposter = OSSignposter(subsystem: "com.deptheos", category: .pointsOfInterest)

public struct StackingResult: @unchecked Sendable {
    /// Stacked result texture (rgba16Unorm, linear).
    public let resultTexture: MTLTexture

    /// Raw focus metric texture (R32Float). Algorithm-specific:
    /// Pyramid = best focus L0, Depth = depth index, Wavelet = merged luma.
    public let focusMapTexture: MTLTexture?

    /// Rendered focus map visualization (rgba8Unorm, sRGB). Algorithm-specific:
    /// Depth = viridis colormap with sidebar, Pyramid = sqrt grayscale.
    public let bakedFocusMapTexture: MTLTexture?

    /// Valid crop area after alignment. Nil if no alignment was used.
    public let validArea: Rect?

    /// Source metadata for export.
    public let sourceBitDepth: Int
    public let dngRenderParams: DeptheosDng.RenderParams
    public let dngMetadata: DeptheosDng.ImageMetadata?

    let metalContext: MetalContext

    init(
        resultTexture: MTLTexture,
        focusMapTexture: MTLTexture?,
        bakedFocusMapTexture: MTLTexture? = nil,
        validArea: Rect?,
        metalContext: MetalContext,
        sourceBitDepth: Int,
        dngRenderParams: DeptheosDng.RenderParams,
        dngMetadata: DeptheosDng.ImageMetadata?
    ) {
        self.resultTexture = resultTexture
        self.focusMapTexture = focusMapTexture
        self.bakedFocusMapTexture = bakedFocusMapTexture
        self.validArea = validArea
        self.metalContext = metalContext
        self.sourceBitDepth = sourceBitDepth
        self.dngRenderParams = dngRenderParams
        self.dngMetadata = dngMetadata
    }

    /// Whether alignment produced a crop region smaller than the result.
    public var needsCrop: Bool {
        guard let area = validArea else { return false }
        return area.width < resultTexture.width || area.height < resultTexture.height
    }

    /// Crop alignment borders, returning a new result with the cropped texture.
    /// Returns self unchanged if no crop is needed.
    public func cropped() async throws -> StackingResult {
        guard let area = validArea,
              area.width > 0, area.height > 0,
              area.width < resultTexture.width || area.height < resultTexture.height
        else { return self }

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer for crop")
        }

        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: resultTexture.pixelFormat,
            width: area.width, height: area.height, mipmapped: false
        )
        desc.usage = [.shaderRead, .shaderWrite]
        desc.storageMode = .private
        guard let croppedTexture = metalContext.device.makeTexture(descriptor: desc) else {
            throw StackingError.metalError("Failed to allocate cropped texture")
        }
        croppedTexture.label = "cropped_result"

        guard let blit = commandBuffer.makeBlitCommandEncoder() else {
            throw StackingError.metalError("Failed to create blit encoder")
        }
        blit.copy(
            from: resultTexture,
            sourceSlice: 0, sourceLevel: 0,
            sourceOrigin: MTLOrigin(x: area.x, y: area.y, z: 0),
            sourceSize: MTLSize(width: area.width, height: area.height, depth: 1),
            to: croppedTexture,
            destinationSlice: 0, destinationLevel: 0,
            destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0)
        )
        blit.endEncoding()

        commandBuffer.commit()
        await commandBuffer.completed()

        return StackingResult(
            resultTexture: croppedTexture,
            focusMapTexture: focusMapTexture,
            bakedFocusMapTexture: bakedFocusMapTexture,
            validArea: nil,
            metalContext: metalContext,
            sourceBitDepth: sourceBitDepth,
            dngRenderParams: dngRenderParams,
            dngMetadata: dngMetadata
        )
    }

    // MARK: - Export

    /// Run the DNG color pipeline on the result (linear → sRGB).
    /// Returns a new rgba16Unorm texture in sRGB color space.
    public func rendered() async throws -> MTLTexture {
        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else {
            throw StackingError.metalError("Failed to create command buffer for render")
        }

        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba16Unorm,
            width: resultTexture.width, height: resultTexture.height, mipmapped: false
        )
        desc.usage = [.shaderRead, .shaderWrite]
        desc.storageMode = .private
        guard let outputTexture = metalContext.device.makeTexture(descriptor: desc) else {
            throw StackingError.metalError("Failed to allocate render output texture")
        }
        outputTexture.label = "rendered_srgb"

        let dngPipeline = try DngRenderPipeline(context: metalContext)
        let resources = dngPipeline.makeResources(for: dngRenderParams)
        dngPipeline.encode(
            commandBuffer: commandBuffer,
            input: resultTexture,
            output: outputTexture,
            resources: resources
        )

        commandBuffer.commit()
        await commandBuffer.completed()
        return outputTexture
    }

    /// Export the raw linear result texture to a CVPixelBuffer (rgba16Unorm).
    public func pixelBuffer() async -> CVPixelBuffer? {
        await resultTexture.pixelBuffer(
            cache: metalContext.textureCache,
            commandQueue: metalContext.commandQueue,
            pixelFormat: .rgba16Unorm
        )
    }

    /// Save the result to a file.
    ///
    /// For JPEG/PNG/TIFF: applies the DNG color pipeline (linear → sRGB), then writes.
    /// For DNG: writes the raw linear result with source metadata.
    /// Save the result to a file.
    ///
    /// - Parameters:
    ///   - quality: JPEG compression quality 0.0-1.0 (default 0.95). Ignored for other formats.
    ///   - jxlEffort: JXL compression effort 1 (fastest) to 9 (smallest). Default 3.
    public func save(to url: URL, format: ExportFormat, quality: Double = 0.95, jxlEffort: Int = 3) async throws {
        let filename = url.lastPathComponent

        if format == .dng {
            let state = exportSignposter.beginInterval("Save DNG", "\(filename, privacy: .public)")
            try await saveDNG(to: url)
            exportSignposter.endInterval("Save DNG", state)
            return
        }

        if format == .jxl {
            let state = exportSignposter.beginInterval("Save JXL", "\(filename, privacy: .public)")
            try await saveJXL(to: url, effort: jxlEffort)
            exportSignposter.endInterval("Save JXL", state)
            return
        }

        let state = exportSignposter.beginInterval("Save Image", "\(filename, privacy: .public)")

        // Apply color pipeline (linear → sRGB)
        let srgbTexture = try await rendered()

        guard let pixelBuffer = await srgbTexture.pixelBuffer(
            cache: metalContext.textureCache,
            commandQueue: metalContext.commandQueue,
            pixelFormat: .rgba16Unorm
        ) else {
            throw ExportError.readbackFailed
        }

        let writerFormat: ImageWriter.Format = switch format {
        case .jpeg: .jpeg
        case .png: .png
        case .tiff: .tiff
        case .dng, .jxl: fatalError("unreachable")
        }

        try ImageWriter().write(
            pixelBuffer,
            to: url,
            format: writerFormat,
            quality: quality,
            bitDepth: sourceBitDepth
        )

        exportSignposter.endInterval("Save Image", state)
    }

    private func saveDNG(to url: URL) async throws {
        guard let metadata = dngMetadata else {
            throw ExportError.dngRequiresDngInput
        }

        guard let pb = await pixelBuffer() else {
            throw ExportError.readbackFailed
        }

        guard let (pixels, w, h, channels) = pb.toRGB16Pixels() else {
            throw ExportError.readbackFailed
        }

        guard DeptheosDng.writeDng(
            to: url.path, metadata: metadata,
            pixels: pixels, width: w, height: h, channels: channels
        ) else {
            throw ExportError.dngWriteFailed
        }
    }

    /// Save as lossless JPEG XL (raw linear u16 RGBA).
    private func saveJXL(to url: URL, effort: Int = 3) async throws {
        guard let pixelData = await resultTexture.readPixelData(
            commandQueue: metalContext.commandQueue
        ) else {
            throw ExportError.readbackFailed
        }

        let width = resultTexture.width
        let height = resultTexture.height
        let jxlData = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    cont.resume(returning: try JXLCodec.encode(
                        pixels: pixelData,
                        width: width,
                        height: height,
                        effort: effort
                    ))
                } catch {
                    cont.resume(throwing: error)
                }
            }
        }
        try jxlData.write(to: url)
    }
}

public enum ExportError: LocalizedError {
    case readbackFailed
    case dngRequiresDngInput
    case dngWriteFailed

    public var errorDescription: String? {
        switch self {
        case .readbackFailed: "Failed to read texture back from GPU"
        case .dngRequiresDngInput: "DNG output requires DNG source images"
        case .dngWriteFailed: "Failed to write DNG file"
        }
    }
}
