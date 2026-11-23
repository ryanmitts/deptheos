import CoreVideo
import Foundation
import Metal
import QuartzCore
import os

private nonisolated let log = AppLog(category: "peaking")

/// Keeps a value alive into a completion handler.
private nonisolated final class Held<T>: @unchecked Sendable {
    let value: T
    init(_ value: T) { self.value = value }
}

/// Focus peaking over the preview: `FocusPeaking` on each video frame, drawn
/// into `layer`. `draw` runs on the video queue; the layer is resized on the
/// main thread, and frames before that are skipped.
nonisolated final class Peaking: @unchecked Sendable {
    /// The video feed is about 1900 px; a larger one is read at this size.
    private static let maxLongSide: CGFloat = 1920

    let layer = CAMetalLayer()
    private let commands: MTLCommandQueue
    private let cache: CVMetalTextureCache
    private let peaking: FocusPeaking

    private let lock = NSLock()
    /// Set on the main thread.
    private var layerSize = CGSize.zero
    private var resizing = false

    init?() {
        guard let device = MTLCreateSystemDefaultDevice(),
              let commands = device.makeCommandQueue()
        else { return nil }
        var cache: CVMetalTextureCache?
        CVMetalTextureCacheCreate(nil, nil, device, nil, &cache)
        guard let cache else { return nil }
        guard let peaking = FocusPeaking(device: device) else {
            log.error("focus peaking kernels did not compile")
            return nil
        }
        self.commands = commands
        self.cache = cache
        self.peaking = peaking
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = false
        layer.isOpaque = false
        layer.contentsGravity = .resize
    }

    /// Its own size, or scaled down to `maxLongSide`.
    private static func grid(frame: CGSize) -> CGSize {
        let long = max(frame.width, frame.height)
        guard long > maxLongSide else { return frame }
        let scale = maxLongSide / long
        return CGSize(
            width: (frame.width * scale).rounded(.down), height: (frame.height * scale).rounded(.down))
    }

    func draw(_ pixels: CVPixelBuffer) {
        let frame = CGSize(
            width: CVPixelBufferGetWidth(pixels), height: CVPixelBufferGetHeight(pixels))
        let size = Self.grid(frame: frame)
        // Frames are skipped until the layer is resized, once, on the main
        // thread.
        let sized = lock.withLock { () -> Bool in
            guard layerSize != size else { return true }
            if !resizing {
                resizing = true
                DispatchQueue.main.async { self.resize(to: size) }
            }
            return false
        }
        guard sized else { return }

        var cvTexture: CVMetalTexture?
        CVMetalTextureCacheCreateTextureFromImage(
            nil, cache, pixels, nil, .bgra8Unorm, Int(frame.width), Int(frame.height), 0,
            &cvTexture)
        guard let cvTexture, let source = CVMetalTextureGetTexture(cvTexture),
              let drawable = layer.nextDrawable(),
              let buffer = commands.makeCommandBuffer()
        else { return }
        peaking.encode(into: buffer, source: source, output: drawable.texture)
        buffer.present(drawable)
        // The camera's buffer stays mapped until the GPU is done with it.
        let held = Held(cvTexture)
        buffer.addCompletedHandler { _ in _ = held }
        buffer.commit()
    }

    @MainActor private func resize(to size: CGSize) {
        log.info("peaking grid \(Int(size.width))×\(Int(size.height))")
        layer.drawableSize = size
        lock.withLock {
            layerSize = size
            resizing = false
        }
    }
}
