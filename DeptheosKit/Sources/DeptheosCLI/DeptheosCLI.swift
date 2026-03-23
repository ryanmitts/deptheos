//
//  DeptheosCLI.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-22.
//

import ArgumentParser
import Foundation
import CoreVideo
import DeptheosKit

@main
struct DeptheosCLI: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "deptheos",
        abstract: "Deptheos CLI",
        subcommands: [Align.self, Pyramid.self, Depth.self, Wavelet.self, Benchmark.self]
    )
}

/// Per-process DNG converter cache rooted in `$TMPDIR`. Each CLI invocation
/// gets its own directory so `purge()` cannot disturb the app's persistent
/// cache under Application Support.
func makeProcessDNGCache() -> DNGConverterCache {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("deptheos-cli-dng-\(ProcessInfo.processInfo.processIdentifier)")
    return DNGConverterCache(cacheDirectory: dir)
}

// MARK: - Shared output options

struct OutputOptions: ParsableArguments {
    @Option(name: [.short, .long], help: "Output file path")
    var output: String?

    @Option(name: .long, help: "Output format: jpeg, png, tiff, dng")
    var format: String = "jpeg"

    @Option(name: .long, help: "JPEG quality 0.0-1.0")
    var quality: Double = 0.95

    @Option(name: .long, help: "Alignment: ecc, none")
    var align: String = "ecc"

    @Option(name: .long, help: "RAW backend: libraw, apple, adobe-dng, or adobe-dng:/path/to/converter")
    var rawBackend: String = "libraw"

    @Option(name: .long, help: "Capture GPU trace to file path (e.g. /tmp/deptheos.gputrace)")
    var trace: String?

    var traceURL: URL? {
        guard let trace else { return nil }
        let resolved = (trace as NSString).expandingTildeInPath
        if resolved.hasSuffix(".gputrace") {
            return URL(fileURLWithPath: resolved)
        }
        return URL(fileURLWithPath: resolved + ".gputrace")
    }

    func exportFormat() throws -> ExportFormat {
        guard let fmt = ExportFormat(rawValue: format.lowercased()) else {
            throw ValidationError("Unsupported format '\(format)'. Use: jpeg, png, tiff, dng")
        }
        return fmt
    }

    func cameraRAWBackend() throws -> CameraRAWBackend {
        switch rawBackend.lowercased() {
        case "libraw": return .libRaw
        case "apple": return .apple
        case "adobe-dng": return .adobeDNGConverter()
        default:
            if rawBackend.lowercased().hasPrefix("adobe-dng:") {
                let path = String(rawBackend.dropFirst("adobe-dng:".count))
                return .adobeDNGConverter(path: path)
            }
            throw ValidationError("Unknown RAW backend '\(rawBackend)'. Use: libraw, apple, adobe-dng")
        }
    }

    func makeAligner(metalContext: MetalContext) throws -> ECCAligner? {
        switch align.lowercased() {
        case "ecc": return try ECCAligner(context: metalContext)
        case "none": return nil
        default:
            throw ValidationError("Unknown alignment '\(align)'. Use: ecc, none")
        }
    }
}

/// Run the stacking pipeline via processStack, printing progress.
/// Returns the StackingResult from the .complete event.
func runStack(session: StackingSession, urls: [URL], loader: UnifiedImageLoader, prefetch: Int, traceURL: URL? = nil) async throws -> StackingResult {
    await session.setGPUTraceURL(traceURL)
    var result: StackingResult?
    for try await event in await session.processStack(urls: urls, loader: loader, prefetchCount: prefetch) {
        switch event {
        case .accumulated(let current, let total, _):
            print("\r[Accumulate] \(current)/\(total)", terminator: "")
            if current == total { print() }
        case .finalized:
            print("[Finalized]")
        case .blended(let current, let total):
            print("\r[Blend] \(current)/\(total)", terminator: "")
            if current == total { print() }
        case .complete(let r):
            result = r
        }
    }
    guard let result else {
        throw ExitCode.failure
    }
    return result
}

/// Run the stacking pipeline with pre-loaded in-memory images, printing progress.
/// Returns the StackingResult from the .complete event.
func runStack(session: StackingSession, images: [DeptheosImage]) async throws -> StackingResult {
    var result: StackingResult?
    for try await event in await session.processStack(images: images) {
        switch event {
        case .accumulated(let current, let total, _):
            print("\r[Accumulate] \(current)/\(total)", terminator: "")
            if current == total { print() }
        case .finalized:
            print("[Finalized]")
        case .blended(let current, let total):
            print("\r[Blend] \(current)/\(total)", terminator: "")
            if current == total { print() }
        case .complete(let r):
            result = r
        }
    }
    guard let result else {
        throw ExitCode.failure
    }
    return result
}

/// Print a benchmark summary after stacking completes.
func printBenchmark(width: Int, height: Int, imageCount: Int, bitDepth: Int, channels: Int = 3, method: String, elapsed: Double) {
    let megapixels = Double(width) * Double(height) * Double(imageCount) / 1_000_000
    let mpPerSec = megapixels / elapsed
    print("")
    print("Stack: \(imageCount) in-memory images, method \(method).")
    print("Images: \(width)x\(height), \(bitDepth) bits per channel, \(channels) channels.")
    print("Performance: \(String(format: "%.2f", mpPerSec)) Mpx/s (\(String(format: "%.3f", elapsed))s)")
}

/// Save a stacking result using the shared output options.
func saveResult(_ result: StackingResult, options: OutputOptions, folderURL: URL) async throws {
    let cropped = try await result.cropped()
    print("Result: \(cropped.resultTexture.width)×\(cropped.resultTexture.height)")

    let fmt = try options.exportFormat()

    let outputURL: URL
    if let outputPath = options.output {
        let resolved = (outputPath as NSString).expandingTildeInPath
        outputURL = URL(fileURLWithPath: resolved)
    } else {
        let filename = "stacked.\(fmt.rawValue)"
        outputURL = URL(fileURLWithPath: filename)
    }

    let saveStart = CFAbsoluteTimeGetCurrent()
    try await cropped.save(to: outputURL, format: fmt, quality: options.quality)
    let saveElapsed = CFAbsoluteTimeGetCurrent() - saveStart
    print("Saved \(fmt.rawValue) to \(outputURL.path) (\(String(format: "%.3f", saveElapsed))s)")
}

// MARK: - Align

struct Align: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Align, warp, and crop a folder of images without stacking"
    )

    @Argument(help: "Path to folder of images")
    var folder: String

    @Option(name: [.short, .long], help: "Output folder path (default: <folder>/aligned/)")
    var output: String?

    @Option(name: .long, help: "Output format: jpeg, png, tiff")
    var format: String = "png"

    @Option(name: .long, help: "JPEG quality 0.0-1.0")
    var quality: Double = 0.70

    @Option(name: .long, help: "Alignment: ecc, none")
    var align: String = "ecc"

    @Option(name: .long, help: "RAW backend: libraw, apple, adobe-dng, or adobe-dng:/path/to/converter")
    var rawBackend: String = "libraw"

    @Option(name: .long, help: "Number of images to prefetch ahead")
    var prefetch: Int = 4

    func run() async throws {
        DeptheosDng.initialize()

        let resolvedPath = (folder as NSString).expandingTildeInPath
        let folderURL = URL(fileURLWithPath: resolvedPath)
        let urls = ImageFormat.discoverImages(in: folderURL)

        guard !urls.isEmpty else {
            print("No supported images found in \(resolvedPath)")
            throw ExitCode.failure
        }
        print("Found \(urls.count) images in \(folderURL.lastPathComponent)")

        let metalContext = try MetalContext()

        // Parse alignment
        let aligner: ECCAligner?
        switch align.lowercased() {
        case "ecc": aligner = try ECCAligner(context: metalContext)
        case "none": aligner = nil
        default:
            throw ValidationError("Unknown alignment '\(align)'. Use: ecc, none")
        }

        guard let aligner else {
            print("Alignment is required for the align command")
            throw ExitCode.failure
        }

        // Parse RAW backend
        let backend: CameraRAWBackend
        switch rawBackend.lowercased() {
        case "libraw": backend = .libRaw
        case "apple": backend = .apple
        case "adobe-dng": backend = .adobeDNGConverter()
        default:
            if rawBackend.lowercased().hasPrefix("adobe-dng:") {
                let path = String(rawBackend.dropFirst("adobe-dng:".count))
                backend = .adobeDNGConverter(path: path)
            } else {
                throw ValidationError("Unknown RAW backend '\(rawBackend)'. Use: libraw, apple, adobe-dng")
            }
        }

        // Parse output format
        guard let writerFormat = ImageWriter.Format(rawValue: format.lowercased()) else {
            throw ValidationError("Unsupported format '\(format)'. Use: jpeg, png, tiff")
        }

        let dngCache = makeProcessDNGCache()
        defer { dngCache.purge() }
        var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
        loader.dngConverterCache = dngCache
        loader.cameraRAWBackend = backend

        let start = CFAbsoluteTimeGetCurrent()

        // Pass 1: Align all images
        print("[Pass 1] Aligning...")
        var chainAligner = ChainAligner(aligner: aligner)
        var firstImage: DeptheosImage?
        var previousTexture: MTLTexture?

        let alignPrefetcher = ImagePrefetcher(
            urls: urls, loader: loader,
            metalContext: metalContext, prefetchCount: prefetch
        )
        for try await (index, image) in alignPrefetcher {
            let texture = try image.getTexture(cache: metalContext.textureCache)

            let pairwiseResult: AlignmentResult?
            if let prevTex = previousTexture {
                nonisolated(unsafe) let ref = prevTex
                nonisolated(unsafe) let src = texture
                pairwiseResult = await aligner.findAlignment(reference: ref, source: src)
            } else {
                pairwiseResult = nil
            }
            _ = chainAligner.addImage(texture, pairwiseResult: pairwiseResult)
            previousTexture = texture

            if index == 0 { firstImage = image }

            print("\r[Align] \(index + 1)/\(urls.count)", terminator: "")
            if index + 1 == urls.count { print() }
        }

        let transforms = chainAligner.getAllTransforms()
        let validArea = chainAligner.getValidArea()

        if let area = validArea {
            print("Valid crop area: \(area.width)×\(area.height) at (\(area.x), \(area.y))")
        }

        // Flush texture cache between passes
        metalContext.flushTextureCache()

        // Set up output directory
        let outputURL: URL
        if let outputPath = output {
            outputURL = URL(fileURLWithPath: (outputPath as NSString).expandingTildeInPath)
        } else {
            outputURL = folderURL.appendingPathComponent("aligned")
        }
        try FileManager.default.createDirectory(at: outputURL, withIntermediateDirectories: true)

        let sourceBitDepth = firstImage?.sourceBitDepth ?? 16
        let dngPipeline = try DngRenderPipeline(context: metalContext)
        let writer = ImageWriter()

        // Pass 2: Warp, crop, color-convert, and save
        print("[Pass 2] Warping and saving...")

        let savePrefetcher = ImagePrefetcher(
            urls: urls, loader: loader,
            metalContext: metalContext, prefetchCount: prefetch
        )
        for try await (index, image) in savePrefetcher {
            let texture = try image.getTexture(cache: metalContext.textureCache)

            // Apply warp and crop
            let transform: AffineTransform2D? = index == 0 ? nil : (index < transforms.count ? transforms[index] : nil)
            let warped = try await metalContext.applyWarpAndCrop(
                source: texture, transform: transform, cropArea: validArea
            )

            // Apply DNG color pipeline (linear → sRGB)
            let desc = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: .rgba16Unorm,
                width: warped.width, height: warped.height, mipmapped: false
            )
            desc.usage = [.shaderRead, .shaderWrite]
            desc.storageMode = .private
            guard let srgbTexture = metalContext.device.makeTexture(descriptor: desc),
                  let cb = metalContext.commandQueue.makeCommandBuffer() else {
                throw ExitCode.failure
            }

            let resources = dngPipeline.makeResources(for: image.dngRenderParams)
            dngPipeline.encode(
                commandBuffer: cb,
                input: warped,
                output: srgbTexture,
                resources: resources
            )
            cb.commit()
            await cb.completed()

            // Export to pixel buffer and save
            guard let pixelBuffer = await srgbTexture.pixelBuffer(
                cache: metalContext.textureCache,
                commandQueue: metalContext.commandQueue,
                pixelFormat: .rgba16Unorm
            ) else {
                print("Warning: failed to export frame \(index)")
                continue
            }

            let filename = String(format: "aligned_%03d.\(writerFormat.rawValue)", index)
            let fileURL = outputURL.appendingPathComponent(filename)
            try writer.write(pixelBuffer, to: fileURL, format: writerFormat, quality: quality, bitDepth: sourceBitDepth)

            print("\r[Save] \(index + 1)/\(urls.count)", terminator: "")
            if index + 1 == urls.count { print() }
        }

        let elapsed = CFAbsoluteTimeGetCurrent() - start
        print("Aligned \(urls.count) images in \(String(format: "%.3f", elapsed))s")
        print("Output: \(outputURL.path)")
    }
}

// MARK: - Pyramid

struct Pyramid: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Run pyramid focus stacking on a folder of images"
    )

    @Argument(help: "Path to folder of images")
    var folder: String

    @Option(name: .long, help: "Smoothness (1.0 sharp – 10.0 smooth)")
    var smoothness: Float = 4.0

    @Option(name: .long, help: "Number of images to prefetch ahead")
    var prefetch: Int = 4

    @OptionGroup var outputOptions: OutputOptions

    func run() async throws {
        DeptheosDng.initialize()

        let resolvedPath = (folder as NSString).expandingTildeInPath
        let folderURL = URL(fileURLWithPath: resolvedPath)
        let urls = ImageFormat.discoverImages(in: folderURL)

        guard !urls.isEmpty else {
            print("No supported images found in \(resolvedPath)")
            throw ExitCode.failure
        }
        print("Found \(urls.count) images in \(folderURL.lastPathComponent)")

        let metalContext = try MetalContext(debugShaders: outputOptions.trace != nil)
        let algorithm = try PyramidStacker(
            metalContext: metalContext,
            config: PyramidConfig(smoothness: smoothness)
        )
        let aligner = try outputOptions.makeAligner(metalContext: metalContext)
        let session = StackingSession(metalContext: metalContext, algorithm: algorithm, aligner: aligner)

        let start = CFAbsoluteTimeGetCurrent()
        let dngCache = makeProcessDNGCache()
        defer { dngCache.purge() }
        var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
        loader.dngConverterCache = dngCache
        loader.cameraRAWBackend = try outputOptions.cameraRAWBackend()
        let result = try await runStack(
            session: session, urls: urls, loader: loader, prefetch: prefetch, traceURL: outputOptions.traceURL
        )
        let elapsed = CFAbsoluteTimeGetCurrent() - start
        print("Stacking complete: \(String(format: "%.3f", elapsed))s")
        printBenchmark(
            width: result.resultTexture.width, height: result.resultTexture.height,
            imageCount: urls.count, bitDepth: result.sourceBitDepth,
            method: "Pyramid", elapsed: elapsed
        )

        try await saveResult(result, options: outputOptions, folderURL: folderURL)
    }
}

// MARK: - Depth

struct Depth: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Run depth-based focus stacking on a folder of images"
    )

    @Argument(help: "Path to folder of images")
    var folder: String

    @Option(name: .long, help: "Smoothing strength (0 off – 10 max)")
    var smoothing: Float = 5.0

    @Option(name: .long, help: "Blend range (0 hard snap – 10 smooth)")
    var blendRange: Float = 5.0

    @Option(name: .long, help: "Bandpass filter radius in pixels")
    var radius: Float = 8.0

    @Option(name: .long, help: "Number of images to prefetch ahead")
    var prefetch: Int = 4

    @OptionGroup var outputOptions: OutputOptions

    func run() async throws {
        DeptheosDng.initialize()

        let resolvedPath = (folder as NSString).expandingTildeInPath
        let folderURL = URL(fileURLWithPath: resolvedPath)
        let urls = ImageFormat.discoverImages(in: folderURL)

        guard !urls.isEmpty else {
            print("No supported images found in \(resolvedPath)")
            throw ExitCode.failure
        }
        print("Found \(urls.count) images in \(folderURL.lastPathComponent)")

        let metalContext = try MetalContext(debugShaders: outputOptions.trace != nil)
        let algorithm = try DepthStacker(
            metalContext: metalContext,
            config: DepthConfig(
                radius: radius, totalFrames: urls.count,
                smoothing: smoothing, blendRange: blendRange
            )
        )
        let aligner = try outputOptions.makeAligner(metalContext: metalContext)
        let session = StackingSession(metalContext: metalContext, algorithm: algorithm, aligner: aligner)

        let start = CFAbsoluteTimeGetCurrent()
        let dngCache = makeProcessDNGCache()
        defer { dngCache.purge() }
        var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
        loader.dngConverterCache = dngCache
        loader.cameraRAWBackend = try outputOptions.cameraRAWBackend()
        let result = try await runStack(
            session: session, urls: urls, loader: loader, prefetch: prefetch, traceURL: outputOptions.traceURL
        )
        let elapsed = CFAbsoluteTimeGetCurrent() - start
        print("Stacking complete: \(String(format: "%.3f", elapsed))s")
        printBenchmark(
            width: result.resultTexture.width, height: result.resultTexture.height,
            imageCount: urls.count, bitDepth: result.sourceBitDepth,
            method: "Depth", elapsed: elapsed
        )

        try await saveResult(result, options: outputOptions, folderURL: folderURL)
    }
}

// MARK: - Wavelet

struct Wavelet: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Run wavelet focus stacking on a folder of images"
    )

    @Argument(help: "Path to folder of images")
    var folder: String

    @Option(name: .long, help: "Wavelet denoise level (0 = off, default 1.0, range 0–10)")
    var denoise: Float = 0

    @Option(name: .long, help: "Consistency filter level (0 = off, 1 = subband, 2 = subband + neighbor)")
    var consistency: Int = 2

    @Option(name: .long, help: "Number of images to prefetch ahead")
    var prefetch: Int = 4

    @OptionGroup var outputOptions: OutputOptions

    func run() async throws {
        DeptheosDng.initialize()

        let resolvedPath = (folder as NSString).expandingTildeInPath
        let folderURL = URL(fileURLWithPath: resolvedPath)
        let urls = ImageFormat.discoverImages(in: folderURL)

        guard !urls.isEmpty else {
            print("No supported images found in \(resolvedPath)")
            throw ExitCode.failure
        }
        print("Found \(urls.count) images in \(folderURL.lastPathComponent)")

        let metalContext = try MetalContext(debugShaders: outputOptions.trace != nil)
        let algorithm = try WaveletStacker(
            metalContext: metalContext,
            config: WaveletConfig(totalFrames: urls.count, denoiseLevel: denoise, consistency: consistency)
        )
        let aligner = try outputOptions.makeAligner(metalContext: metalContext)
        let session = StackingSession(metalContext: metalContext, algorithm: algorithm, aligner: aligner)

        let start = CFAbsoluteTimeGetCurrent()
        let dngCache = makeProcessDNGCache()
        defer { dngCache.purge() }
        var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
        loader.dngConverterCache = dngCache
        loader.cameraRAWBackend = try outputOptions.cameraRAWBackend()
        let result = try await runStack(
            session: session, urls: urls, loader: loader, prefetch: prefetch, traceURL: outputOptions.traceURL
        )
        let elapsed = CFAbsoluteTimeGetCurrent() - start
        print("Stacking complete: \(String(format: "%.3f", elapsed))s")
        printBenchmark(
            width: result.resultTexture.width, height: result.resultTexture.height,
            imageCount: urls.count, bitDepth: result.sourceBitDepth,
            method: "Wavelet", elapsed: elapsed
        )

        try await saveResult(result, options: outputOptions, folderURL: folderURL)
    }
}

// MARK: - Benchmark

struct Benchmark: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Benchmark stacking throughput using images from a folder"
    )

    @Argument(help: "Path to folder of images")
    var folder: String

    @Option(name: .long, help: "Number of images fed to stacker")
    var iterations: Int = 500

    @Option(name: .long, help: "RAW backend: libraw, apple, adobe-dng")
    var rawBackend: String = "libraw"

    private static let allMethods = ["depth", "pyramid", "wavelet"]

    func run() async throws {
        DeptheosDng.initialize()

        let resolvedPath = (folder as NSString).expandingTildeInPath
        let folderURL = URL(fileURLWithPath: resolvedPath)
        let urls = ImageFormat.discoverImages(in: folderURL)

        guard urls.count >= 2 else {
            print("Need at least 2 images in \(resolvedPath), found \(urls.count)")
            throw ExitCode.failure
        }

        let metalContext = try MetalContext()

        let dngCache = makeProcessDNGCache()
        defer { dngCache.purge() }

        // Parse RAW backend
        var loader = UnifiedImageLoader(ciContext: metalContext.ciContext)
        loader.dngConverterCache = dngCache
        switch rawBackend.lowercased() {
        case "libraw": loader.cameraRAWBackend = .libRaw
        case "apple": loader.cameraRAWBackend = .apple
        case "adobe-dng": loader.cameraRAWBackend = .adobeDNGConverter()
        default:
            if rawBackend.lowercased().hasPrefix("adobe-dng:") {
                let path = String(rawBackend.dropFirst("adobe-dng:".count))
                loader.cameraRAWBackend = .adobeDNGConverter(path: path)
            } else {
                throw ValidationError("Unknown RAW backend '\(rawBackend)'. Use: libraw, apple, adobe-dng")
            }
        }

        // Load first 2 images once
        print("Loading images...")
        let images = [
            try loader.load(url: urls[0], metalContext: metalContext),
            try loader.load(url: urls[1], metalContext: metalContext),
        ]
        let width = images[0].width
        let height = images[0].height
        let bitDepth = images[0].sourceBitDepth
        print("Loaded: \(width)×\(height), \(bitDepth)-bit")

        func makeAlgorithm(_ method: String, count: Int) throws -> any StackingAlgorithm {
            switch method {
            case "depth":
                return try DepthStacker(
                    metalContext: metalContext,
                    config: DepthConfig(radius: 8.0, totalFrames: count, smoothing: 5.0, blendRange: 5.0)
                )
            case "pyramid":
                return try PyramidStacker(metalContext: metalContext, config: PyramidConfig(smoothness: 4.0))
            case "wavelet":
                return try WaveletStacker(metalContext: metalContext, config: WaveletConfig(totalFrames: count))
            default:
                fatalError("Unknown method '\(method)'")
            }
        }

        func makeImageArray(count: Int) -> [DeptheosImage] {
            (0..<count).map { images[$0 % 2] }
        }

        let benchImages = makeImageArray(count: iterations)

        var results: [(method: String, elapsed: Double)] = []

        for method in Self.allMethods {
            print("\n[\(method)] \(iterations) images...")
            let benchAlgorithm = try makeAlgorithm(method, count: iterations)
            let benchSession = StackingSession(metalContext: metalContext, algorithm: benchAlgorithm, aligner: nil)
            let start = CFAbsoluteTimeGetCurrent()
            _ = try await runStack(session: benchSession, images: benchImages)
            let elapsed = CFAbsoluteTimeGetCurrent() - start
            results.append((method: method, elapsed: elapsed))
        }

        // Print summary
        print("\n" + String(repeating: "=", count: 60))
        print("Benchmark Results: \(width)×\(height), \(bitDepth)-bit, \(iterations) images")
        print(String(repeating: "=", count: 60))
        for r in results {
            let megapixels = Double(width) * Double(height) * Double(iterations) / 1_000_000
            let mpPerSec = megapixels / r.elapsed
            print(String(format: "  %-12s  %8.2f Mpx/s  (%6.3fs)", (r.method as NSString).utf8String!, mpPerSec, r.elapsed))
        }
        print(String(repeating: "=", count: 60))
    }
}

