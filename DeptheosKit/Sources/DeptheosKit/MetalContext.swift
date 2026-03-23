//
//  MetalContext.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-22.
//
import CoreImage
import Metal
import CoreVideo

/// Shared Metal items
public final class MetalContext: @unchecked Sendable {
    public let device: MTLDevice
    public let commandQueue: MTLCommandQueue
    public let textureCache: CVMetalTextureCache
    public let ciContext: CIContext
    public let defaultLibrary: MTLLibrary

    public init(device: MTLDevice? = nil, debugShaders: Bool = false) throws {
        guard let dev = device ?? MTLCreateSystemDefaultDevice() else {
            throw MetalContextError.noDevice
        }
        self.device = dev

        guard let queue = dev.makeCommandQueue() else {
            throw MetalContextError.noCommandQueue
        }
        self.commandQueue = queue

        var cache: CVMetalTextureCache?
        let status = CVMetalTextureCacheCreate(nil, nil, dev, nil, &cache)
        guard status == kCVReturnSuccess, let textureCache = cache else {
            throw MetalContextError.noTextureCache
        }
        self.textureCache = textureCache

        self.ciContext = CIContext(mtlCommandQueue: queue, options: [
            .cvMetalTextureCache: textureCache
        ])

        // Load the metallib compiled by MetalCompilerPlugin in DeptheosShaders.
        // Bundle.main works in apps; in swift-test Bundle.main is the SPM toolchain,
        // so fall back to locating the bundle near this binary via #dsohandle.
        let bundleName = "DeptheosKit_DeptheosShaders"
        let shadersBundleURL: URL? =
            Bundle.main.url(forResource: bundleName, withExtension: "bundle")
            ?? {
                var info = Dl_info()
                guard dladdr(#dsohandle, &info) != 0, let path = info.dli_fname else { return nil }
                // Walk up from the binary to find the resource bundle in the build products dir.
                var dir = URL(fileURLWithPath: String(cString: path)).deletingLastPathComponent()
                for _ in 0..<5 {
                    let candidate = dir.appendingPathComponent("\(bundleName).bundle")
                    if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
                    dir = dir.deletingLastPathComponent()
                }
                return nil
            }()
        guard let shadersBundleURL,
              let shadersBundle = Bundle(url: shadersBundleURL),
              let metallibURL = shadersBundle.url(forResource: "DeptheosShaders", withExtension: "metallib") else {
            throw MetalContextError.noLibrary
        }
        if debugShaders {
            print("[Metal] Loaded DeptheosShaders.metallib from \(metallibURL.path)")
        }
        self.defaultLibrary = try dev.makeLibrary(URL: metallibURL)
    }

    deinit {
        flushTextureCache()
    }

    public func flushTextureCache() {
        CVMetalTextureCacheFlush(textureCache, 0)
    }

    public func applyWarpAndCrop(
        source: MTLTexture,
        transform: AffineTransform2D?,
        cropArea: Rect?
    ) async throws -> MTLTexture {
        var working = source

        if let transform {
            let warpDesc = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: source.pixelFormat,
                width: source.width, height: source.height, mipmapped: false
            )
            warpDesc.usage = [.shaderRead, .shaderWrite]
            warpDesc.storageMode = .private
            guard let warpedTexture = device.makeTexture(descriptor: warpDesc),
                  let cb = commandQueue.makeCommandBuffer() else {
                throw MetalContextError.metalError("Failed to create warp resources")
            }

            let warp = try GPUWarp(library: defaultLibrary, device: device)
            try warp.encodeWarp(
                commandBuffer: cb,
                src: source, dst: warpedTexture,
                transform: transform, applySrgb: false
            )

            cb.commit()
            await cb.completed()
            working = warpedTexture
        }

        if let area = cropArea,
           area.width < working.width || area.height < working.height {
            let cropDesc = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: working.pixelFormat,
                width: area.width, height: area.height, mipmapped: false
            )
            cropDesc.usage = [.shaderRead, .shaderWrite]
            cropDesc.storageMode = .private
            guard let croppedTexture = device.makeTexture(descriptor: cropDesc),
                  let cb = commandQueue.makeCommandBuffer(),
                  let blit = cb.makeBlitCommandEncoder() else {
                throw MetalContextError.metalError("Failed to create crop resources")
            }

            blit.copy(
                from: working,
                sourceSlice: 0, sourceLevel: 0,
                sourceOrigin: MTLOrigin(x: area.x, y: area.y, z: 0),
                sourceSize: MTLSize(width: area.width, height: area.height, depth: 1),
                to: croppedTexture,
                destinationSlice: 0, destinationLevel: 0,
                destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0)
            )
            blit.endEncoding()
            cb.commit()
            await cb.completed()
            working = croppedTexture
        }

        return working
    }

    /// Start GPU trace capture. Returns true if capture started successfully.
    @discardableResult
    public func startGPUTrace(to url: URL) -> Bool {
        let manager = MTLCaptureManager.shared()
        guard manager.supportsDestination(.gpuTraceDocument) else {
            print("[GPU Trace] Capture not supported. Ensure MTL_CAPTURE_ENABLED=1 is set.")
            return false
        }
        // Remove existing trace file to avoid "file exists" error
        if FileManager.default.fileExists(atPath: url.path) {
            try? FileManager.default.removeItem(at: url)
        }
        let descriptor = MTLCaptureDescriptor()
        descriptor.captureObject = device
        descriptor.destination = .gpuTraceDocument
        descriptor.outputURL = url
        do {
            try manager.startCapture(with: descriptor)
            print("[GPU Trace] Capture started → \(url.path)")
            return true
        } catch {
            print("[GPU Trace] Failed to start: \(error)")
            return false
        }
    }

    /// Stop GPU trace capture. Pass url to log success, nil for error path.
    public func stopGPUTrace(url: URL?) {
        MTLCaptureManager.shared().stopCapture()
        if let url {
            print("[GPU Trace] Capture saved → \(url.path)")
        }
    }
}

public enum MetalContextError: Error {
    case noDevice
    case noCommandQueue
    case noTextureCache
    case noLibrary
    case metalError(String)
}
