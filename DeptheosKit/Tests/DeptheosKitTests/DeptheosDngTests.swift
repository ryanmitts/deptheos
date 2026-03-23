import Testing
import Foundation
import DeptheosKit

@Suite("DeptheosDng")
struct DeptheosDngTests {
    
    init() {
        DeptheosDng.initialize()
    }
    
    private func testDngURL() throws -> URL {
        try #require(
            Bundle.module.url(forResource: "test", withExtension: "dng", subdirectory: "Resources"),
            "Put a test.dng in Tests/DeptheosDngTests/Resources/"
        )
    }
    
    @Test func canReadDngFile() async throws {
        let url = try testDngURL()

        let result = try #require(
            DeptheosDng.readDng(at: url.path),
            "readDng returned nil"
        )

        #expect(result.width > 0)
        #expect(result.height > 0)
        #expect(result.channels > 0)
        #expect(result.pixels.count == result.width * result.height * result.channels)
        #expect(!result.metadata.camera.make.isEmpty || !result.metadata.camera.model.isEmpty)
    }
    
    @Test func roundTrip() throws {
        let url = try testDngURL()

        let readResult = try #require(DeptheosDng.readDng(at: url.path))

        let tempPath = NSTemporaryDirectory() + "deptheos_roundtrip_test.dng"
        defer { try? FileManager.default.removeItem(atPath: tempPath) }

        #expect(DeptheosDng.writeDng(to: tempPath, metadata: readResult.metadata, pixels: readResult.pixels, width: readResult.width, height: readResult.height, channels: readResult.channels))

        #expect(DeptheosDng.validate(at: tempPath))

        let readBackResult = try #require(DeptheosDng.readDng(at: tempPath))

        #expect(readResult.width == readBackResult.width)
        #expect(readResult.height == readBackResult.height)
        #expect(readResult.channels == readBackResult.channels)
        expectMetadataEqual(readResult.metadata, readBackResult.metadata)
    }

    @Test func roundTripJxl() throws {
        let url = try testDngURL()

        let readResult = try #require(DeptheosDng.readDng(at: url.path))
        var newMetadata = readResult.metadata
        newMetadata.geometry.useJXL = true;

        let tempPath = NSTemporaryDirectory() + "deptheos_roundtrip_jxl_test.dng"
        defer { try? FileManager.default.removeItem(atPath: tempPath) }

        #expect(DeptheosDng.writeDng(to: tempPath, metadata: newMetadata, pixels: readResult.pixels, width: readResult.width, height: readResult.height, channels: readResult.channels))

        #expect(DeptheosDng.validate(at: tempPath))

        let readBackResult = try #require(DeptheosDng.readDng(at: tempPath))

        #expect(readBackResult.metadata.geometry.useJXL)

        #expect(readResult.width == readBackResult.width)
        #expect(readResult.height == readBackResult.height)
        #expect(readResult.channels == readBackResult.channels)

        expectMetadataEqual(readBackResult.metadata, newMetadata)
    }

    @Test func roundTripWithRenderCompare() throws {
        let url = try testDngURL()

        let readResult = try #require(DeptheosDng.readDng(at: url.path))
        let (renderedPixels, _, _, _) = try #require(DeptheosDng.renderToSrgb(pixels: readResult.pixels, width: readResult.width, height: readResult.height, channels: readResult.channels, metadata: readResult.metadata))

        let tempPath = NSTemporaryDirectory() + "deptheos_roundtrip_with_render_test.dng"
        defer { try? FileManager.default.removeItem(atPath: tempPath) }

        #expect(DeptheosDng.writeDng(to: tempPath, metadata: readResult.metadata, pixels: readResult.pixels, width: readResult.width, height: readResult.height, channels: readResult.channels))

        #expect(DeptheosDng.validate(at: tempPath))

        let readBackResult = try #require(DeptheosDng.readDng(at: tempPath))
        let (renderedReadBackPixels, _, _, _) = try #require(DeptheosDng.renderToSrgb(pixels: readBackResult.pixels, width: readBackResult.width, height: readBackResult.height, channels: readBackResult.channels, metadata: readBackResult.metadata))

        #expect(readResult.width == readBackResult.width)
        #expect(readResult.height == readBackResult.height)
        #expect(readResult.channels == readBackResult.channels)
        expectMetadataEqual(readResult.metadata, readBackResult.metadata)

        let db = psnr(renderedPixels, renderedReadBackPixels)
        #expect(db > 60, "PSNR \(db) dB is too low — rendered images differ significantly")
    }

    @Test func corruptedMetadataProducesDifferentRender() throws {
        let url = try testDngURL()

        let readResult = try #require(DeptheosDng.readDng(at: url.path))
        let (goodPixels, _, _, _) = try #require(DeptheosDng.renderToSrgb(pixels: readResult.pixels, width: readResult.width, height: readResult.height, channels: readResult.channels, metadata: readResult.metadata))

        // Scramble camera neutral — forces a completely wrong white balance
        var corrupted = readResult.metadata
        corrupted.colorScience.asShotNeutral = [0.1, 0.9, 0.1]

        let tempPath = NSTemporaryDirectory() + "deptheos_corrupted_test.dng"
        defer { try? FileManager.default.removeItem(atPath: tempPath) }

        #expect(DeptheosDng.writeDng(to: tempPath, metadata: corrupted, pixels: readResult.pixels, width: readResult.width, height: readResult.height, channels: readResult.channels))

        let readBack = try #require(DeptheosDng.readDng(at: tempPath))
        let (badPixels, _, _, _) = try #require(DeptheosDng.renderToSrgb(pixels: readBack.pixels, width: readBack.width, height: readBack.height, channels: readBack.channels, metadata: readBack.metadata))

        let db = psnr(goodPixels, badPixels)
        #expect(db < 60, "Expected corrupted metadata to produce different render, but PSNR was \(db) dB")
    }
}
