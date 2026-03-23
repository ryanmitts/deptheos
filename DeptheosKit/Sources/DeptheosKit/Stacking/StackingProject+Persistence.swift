//
//  StackingProject+Persistence.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-24.
//

import Foundation
import Metal
import MetalPerformanceShaders
import CoreImage
import os

extension StackingProject {

    private static let signposter = OSSignposter(subsystem: "com.deptheos", category: .pointsOfInterest)

    // MARK: - Save

    /// Save as a `.deptheos` package directory.
    ///
    /// Creates:
    /// - `result.jxl` — lossless linear RGBA u16
    /// - `metadata.json` — project metadata
    /// - `thumbnail.jpg` — 512px sRGB preview
    public func save(to url: URL, metalContext: MetalContext) async throws {
        let fm = FileManager.default
        try fm.createDirectory(at: url, withIntermediateDirectories: true)

        // 1. Save result texture as JXL
        let s1 = Self.signposter.beginInterval("GPU Readback (result)")
        guard let pixelData = await resultTexture.readPixelData(
            commandQueue: metalContext.commandQueue
        ) else {
            Self.signposter.endInterval("GPU Readback (result)", s1)
            throw ProjectError.readbackFailed
        }
        Self.signposter.endInterval("GPU Readback (result)", s1)

        let s2 = Self.signposter.beginInterval("JXL Encode (result)")
        let resultWidth = resultTexture.width
        let resultHeight = resultTexture.height
        let jxlData = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    cont.resume(returning: try JXLCodec.encode(
                        pixels: pixelData,
                        width: resultWidth,
                        height: resultHeight,
                        effort: 1
                    ))
                } catch {
                    cont.resume(throwing: error)
                }
            }
        }
        try jxlData.write(to: url.appendingPathComponent("result.jxl"))
        Self.signposter.endInterval("JXL Encode (result)", s2)

        // 2. Save paint texture if present
        if let paintTex = paintTexture {
            let s3 = Self.signposter.beginInterval("GPU Readback (paint)")
            if let paintPixelData = await paintTex.readPixelData(
                commandQueue: metalContext.commandQueue
            ) {
                Self.signposter.endInterval("GPU Readback (paint)", s3)

                let s4 = Self.signposter.beginInterval("JXL Encode (paint)")
                let paintWidth = paintTex.width
                let paintHeight = paintTex.height
                let paintJxl = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
                    DispatchQueue.global(qos: .userInitiated).async {
                        do {
                            cont.resume(returning: try JXLCodec.encode(
                                pixels: paintPixelData,
                                width: paintWidth,
                                height: paintHeight,
                                effort: 1
                            ))
                        } catch {
                            cont.resume(throwing: error)
                        }
                    }
                }
                try paintJxl.write(to: url.appendingPathComponent("paint.jxl"))
                Self.signposter.endInterval("JXL Encode (paint)", s4)
            } else {
                Self.signposter.endInterval("GPU Readback (paint)", s3)
            }
        }

        // 3. Save baked focus map if present
        if let depthMapTex = bakedFocusMapTexture {
            if let jpegData = await Self.encodeTextureAsJPEG(
                depthMapTex, commandQueue: metalContext.commandQueue, quality: 0.92
            ) {
                try? jpegData.write(to: url.appendingPathComponent("depthmap.jpg"))
            }
        }

        // 4. Save metadata
        let metadata = ProjectMetadata(
            config: config,
            cameraRAWBackend: cameraRAWBackend,
            sourceImagePaths: sourceImagePaths,
            transforms: transforms,
            validArea: validArea,
            imageIncluded: imageIncluded,
            sourceBitDepth: sourceBitDepth,
            dngRenderParams: dngRenderParams,
            dngMetadata: dngMetadata,
            resultWidth: resultTexture.width,
            resultHeight: resultTexture.height,
            createdAt: createdAt,
            hasPaintData: paintTexture != nil,
            hasDepthMap: bakedFocusMapTexture != nil
        )
        let jsonData = try JSONEncoder().encode(metadata)
        try jsonData.write(to: url.appendingPathComponent("metadata.json"))

        // 5. Thumbnail
        let s5 = Self.signposter.beginInterval("Thumbnail Generation")
        if let thumbData = try? await generateThumbnail(metalContext: metalContext) {
            try? thumbData.write(to: url.appendingPathComponent("thumbnail.jpg"))
        }
        Self.signposter.endInterval("Thumbnail Generation", s5)
    }

    // MARK: - Load

    /// Load from a `.deptheos` package directory.
    public static func load(from url: URL, metalContext: MetalContext) async throws -> StackingProject {
        // 1. Load metadata
        let jsonData = try Data(contentsOf: url.appendingPathComponent("metadata.json"))
        let metadata = try JSONDecoder().decode(ProjectMetadata.self, from: jsonData)

        // 2. Load result texture from JXL
        let jxlData = try Data(contentsOf: url.appendingPathComponent("result.jxl"))
        let decoded = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<JXLCodec.DecodeResult, Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    cont.resume(returning: try JXLCodec.decode(data: jxlData))
                } catch {
                    cont.resume(throwing: error)
                }
            }
        }

        guard let resultTexture = await metalContext.device.uploadPixelData(
            decoded.pixels,
            width: decoded.width,
            height: decoded.height,
            pixelFormat: .rgba16Unorm,
            commandQueue: metalContext.commandQueue
        ) else {
            throw ProjectError.uploadFailed
        }
        resultTexture.label = "project_result"

        // 3. Load paint texture if present
        var paintTexture: MTLTexture?
        let paintURL = url.appendingPathComponent("paint.jxl")
        if FileManager.default.fileExists(atPath: paintURL.path) {
            let paintJxlData = try Data(contentsOf: paintURL)
            let paintDecoded = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<JXLCodec.DecodeResult, Error>) in
                DispatchQueue.global(qos: .userInitiated).async {
                    do {
                        cont.resume(returning: try JXLCodec.decode(data: paintJxlData))
                    } catch {
                        cont.resume(throwing: error)
                    }
                }
            }
            paintTexture = await metalContext.device.uploadPixelData(
                paintDecoded.pixels,
                width: paintDecoded.width,
                height: paintDecoded.height,
                pixelFormat: .rgba16Unorm,
                commandQueue: metalContext.commandQueue
            )
            paintTexture?.label = "project_paint"
        }

        // 4. Load baked depth map if present
        var depthMapTexture: MTLTexture?
        let depthMapURL = url.appendingPathComponent("depthmap.jpg")
        if FileManager.default.fileExists(atPath: depthMapURL.path) {
            depthMapTexture = await Self.loadJPEGAsTexture(
                depthMapURL, device: metalContext.device, commandQueue: metalContext.commandQueue
            )
        }

        var project = StackingProject(
            result: StackingResult(
                resultTexture: resultTexture,
                focusMapTexture: nil,
                validArea: metadata.validArea,
                metalContext: metalContext,
                sourceBitDepth: metadata.sourceBitDepth,
                dngRenderParams: metadata.dngRenderParams,
                dngMetadata: metadata.dngMetadata
            ),
            config: metadata.config,
            cameraRAWBackend: metadata.cameraRAWBackend,
            sourceImagePaths: metadata.sourceImagePaths,
            transforms: metadata.transforms,
            validArea: metadata.validArea,
            imageIncluded: metadata.imageIncluded,
            createdAt: metadata.createdAt
        )
        project.paintTexture = paintTexture
        project.bakedFocusMapTexture = depthMapTexture
        return project
    }

    // MARK: - JPEG Encode/Decode Helpers

    /// Read back an rgba8Unorm GPU texture and encode as JPEG.
    private static func encodeTextureAsJPEG(
        _ texture: MTLTexture, commandQueue: MTLCommandQueue, quality: Double
    ) async -> Data? {
        let w = texture.width
        let h = texture.height

        // Read back to CPU — handles private storage mode
        guard let pixelData = await texture.readPixelData(commandQueue: commandQueue) else { return nil }

        // Create CGImage from raw RGBA8 data
        let bytesPerRow = w * 4
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        guard let provider = CGDataProvider(data: pixelData as CFData),
              let cgImage = CGImage(
                width: w, height: h,
                bitsPerComponent: 8, bitsPerPixel: 32,
                bytesPerRow: bytesPerRow,
                space: colorSpace,
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                provider: provider,
                decode: nil, shouldInterpolate: false,
                intent: .defaultIntent
              ) else { return nil }

        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, "public.jpeg" as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(dest, cgImage, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
        CGImageDestinationFinalize(dest)
        return data as Data
    }

    /// Load a JPEG file and upload to GPU as rgba8Unorm texture.
    private static func loadJPEGAsTexture(
        _ url: URL, device: MTLDevice, commandQueue: MTLCommandQueue
    ) async -> MTLTexture? {
        guard let data = try? Data(contentsOf: url),
              let provider = CGDataProvider(data: data as CFData),
              let cgImage = CGImage(
                jpegDataProviderSource: provider,
                decode: nil, shouldInterpolate: false,
                intent: .defaultIntent
              ) else { return nil }

        let w = cgImage.width
        let h = cgImage.height
        let bytesPerRow = w * 4
        var pixels = [UInt8](repeating: 0, count: bytesPerRow * h)

        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(
            data: &pixels, width: w, height: h,
            bitsPerComponent: 8, bytesPerRow: bytesPerRow,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ) else { return nil }
        ctx.draw(cgImage, in: CGRect(x: 0, y: 0, width: w, height: h))

        return await device.uploadPixelData(
            Data(pixels),
            width: w,
            height: h,
            pixelFormat: .rgba8Unorm,
            commandQueue: commandQueue
        )
    }

    // MARK: - Thumbnail

    /// Generate a 512px sRGB JPEG thumbnail.
    /// Renders to sRGB at full res, downscales on GPU via MPS, then reads back only the small texture.
    private func generateThumbnail(metalContext: MetalContext, maxSize: Int = 512) async throws -> Data? {
        let w = resultTexture.width
        let h = resultTexture.height
        let scale = Float(maxSize) / Float(max(w, h))
        let thumbW = max(1, scale < 1.0 ? Int(Float(w) * scale) : w)
        let thumbH = max(1, scale < 1.0 ? Int(Float(h) * scale) : h)

        guard let commandBuffer = metalContext.commandQueue.makeCommandBuffer() else { return nil }

        // 1. Render linear → sRGB at full resolution (GPU)
        let fullDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba16Unorm,
            width: w, height: h, mipmapped: false
        )
        fullDesc.usage = [.shaderRead, .shaderWrite]
        fullDesc.storageMode = .private
        guard let srgbTexture = metalContext.device.makeTexture(descriptor: fullDesc) else { return nil }

        let dngPipeline = try DngRenderPipeline(context: metalContext)
        let resources = dngPipeline.makeResources(for: dngRenderParams)
        dngPipeline.encode(
            commandBuffer: commandBuffer,
            input: resultTexture,
            output: srgbTexture,
            resources: resources
        )

        // 2. Bilinear downscale to thumbnail size (GPU)
        let thumbDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba8Unorm,
            width: thumbW, height: thumbH, mipmapped: false
        )
        thumbDesc.usage = [.shaderRead, .shaderWrite]
        thumbDesc.storageMode = .shared
        guard let thumbTexture = metalContext.device.makeTexture(descriptor: thumbDesc) else { return nil }

        let scaler = MPSImageBilinearScale(device: metalContext.device)
        scaler.encode(commandBuffer: commandBuffer, sourceTexture: srgbTexture, destinationTexture: thumbTexture)

        let sGpu = Self.signposter.beginInterval("Thumbnail GPU")
        commandBuffer.commit()
        await commandBuffer.completed()
        Self.signposter.endInterval("Thumbnail GPU", sGpu)

        // 3. Read back the small thumbnail directly into a CIImage and encode as JPEG
        let sEncode = Self.signposter.beginInterval("Thumbnail JPEG Encode")
        let bytesPerRow = thumbW * 4
        var pixels = [UInt8](repeating: 0, count: bytesPerRow * thumbH)
        thumbTexture.getBytes(&pixels, bytesPerRow: bytesPerRow, from: MTLRegion(
            origin: MTLOrigin(x: 0, y: 0, z: 0),
            size: MTLSize(width: thumbW, height: thumbH, depth: 1)
        ), mipmapLevel: 0)

        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: &pixels, width: thumbW, height: thumbH,
            bitsPerComponent: 8, bytesPerRow: bytesPerRow,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ), let cgImage = context.makeImage() else {
            Self.signposter.endInterval("Thumbnail JPEG Encode", sEncode)
            return nil
        }

        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, "public.jpeg" as CFString, 1, nil) else {
            Self.signposter.endInterval("Thumbnail JPEG Encode", sEncode)
            return nil
        }
        CGImageDestinationAddImage(dest, cgImage, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
        CGImageDestinationFinalize(dest)
        Self.signposter.endInterval("Thumbnail JPEG Encode", sEncode)
        return data as Data
    }
}
