import Testing
import Foundation
import CoreVideo
import Metal
import DeptheosKit

@Suite("DngRenderPipeline")
struct DngRenderPipelineTests {

    init() {
        DeptheosDng.initialize()
    }

    private func testDngURL() throws -> URL {
        try #require(
            Bundle.module.url(forResource: "test", withExtension: "dng", subdirectory: "Resources"),
            "Put a test.dng in Tests/DeptheosKitTests/Resources/"
        )
    }

    @Test func gpuRenderMatchesCpuRender() throws {
        let context = try MetalContext()
        let pipeline = try DngRenderPipeline(context: context)

        // Read DNG
        let url = try testDngURL()
        let readResult = try #require(DeptheosDng.readDng(at: url.path))
        let w = readResult.width
        let h = readResult.height
        
        // Baseline rendered sRGB image
        let (renderedPixels, _, _, _) = try #require(DeptheosDng.renderToSrgb(pixels: readResult.pixels, width: readResult.width, height: readResult.height, channels: readResult.channels, metadata: readResult.metadata))

        // Compute color science
        let renderParams = try #require(
            DeptheosDng.computeRenderParams(metadata: readResult.metadata)
        )

        // Pixels → CVPixelBuffer → MTLTexture
        let pixelBuffer = try #require(CVPixelBuffer.fromRGB16(
            pixels: readResult.pixels, width: w, height: h, channels: readResult.channels
        ))
        let textureResult = try #require(pixelBuffer.metalTextureWithRef(using: context.textureCache))
        let inputTexture = textureResult.texture

        // Create output texture (CPU-readable for readback)
        let outDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba16Unorm, width: w, height: h, mipmapped: false
        )
        outDesc.usage = [.shaderRead, .shaderWrite]
        outDesc.storageMode = .shared
        let outputTexture = try #require(context.device.makeTexture(descriptor: outDesc))

        // Encode and run
        let commandBuffer = try #require(context.commandQueue.makeCommandBuffer())
        let resources = pipeline.makeResources(for: renderParams)
        pipeline.encode(
            commandBuffer: commandBuffer,
            input: inputTexture,
            output: outputTexture,
            resources: resources
        )
        commandBuffer.commit()
        commandBuffer.waitUntilCompleted()

        // Read back pixels
        let bytesPerRow = w * 4 * MemoryLayout<UInt16>.stride
        let pixels = Array<UInt16>(unsafeUninitializedCapacity: w * h * 4) { buffer, count in
            outputTexture.getBytes(
                buffer.baseAddress!,
                bytesPerRow: bytesPerRow,
                from: MTLRegionMake2D(0, 0, w, h),
                mipmapLevel: 0
            )
            count = w * h * 4
        }

        // Verify output is non-empty
        #expect(pixels.count == w * h * 4)
        #expect(pixels.contains(where: { $0 > 0 }), "Output pixels are all zero")

        // Compare GPU render to CPU baseline (strip alpha from GPU RGBA → RGB)
        let gpuRgb = stripAlpha(rgba: pixels, width: w, height: h)
        let db = psnr(renderedPixels, gpuRgb)
        print("GPU vs CPU PSNR: \(db) dB")
        #expect(db > 60, "GPU and CPU renders differ too much: PSNR \(db) dB")
    }
}
