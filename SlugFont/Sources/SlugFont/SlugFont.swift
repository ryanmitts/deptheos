import Metal
import Foundation

public enum SlugFontError: Error, CustomStringConvertible {
    case fileNotFound(String)
    case invalidHeader
    case noCodePoints
    case invalidTexture(String)
    case textureCreationFailed

    public var description: String {
        switch self {
        case .fileNotFound(let path): "Font file not found: \(path)"
        case .invalidHeader: "Invalid .sluggish file header"
        case .noCodePoints: "No code points in font"
        case .invalidTexture(let msg): "Invalid texture data: \(msg)"
        case .textureCreationFailed: "Failed to create Metal texture"
        }
    }
}

/// A loaded Slug font with GPU textures ready for rendering.
public final class SlugFont: @unchecked Sendable {

    /// Font-level vertical metrics in font units.
    public struct FontMetrics: Sendable {
        public let unitsPerEm: Int32
        public let ascender: Int32
        public let descender: Int32
        public let lineGap: Int32
    }

    public struct GlyphInfo: Sendable {
        public let codePoint: UInt32
        public let width: UInt32
        public let height: UInt32
        public let bandCount: UInt32
        public let bandDimX: UInt32
        public let bandDimY: UInt32
        public let bandsTexCoordX: UInt16
        public let bandsTexCoordY: UInt16
        public let advanceWidth: Int32
        public let leftSideBearing: Int32
        public let bearingY: Int32          // y offset from baseline to top of bounding box
    }

    public let metrics: FontMetrics
    public let glyphs: [UInt32: GlyphInfo]
    /// Kerning table: key is (left << 32 | right) code point pair, value is kern advance in font units.
    public let kernTable: [UInt64: Int32]
    public let curvesTexture: MTLTexture
    public let bandsTexture: MTLTexture

    /// Look up the kerning adjustment between two code points (in font units).
    public func kern(left: UInt32, right: UInt32) -> Int32 {
        let key = (UInt64(left) << 32) | UInt64(right)
        return kernTable[key] ?? 0
    }

    /// Load the bundled default font (Roboto-Regular).
    public convenience init(device: MTLDevice) throws {
        guard let url = Bundle.module.url(forResource: "Roboto-Regular", withExtension: "sluggish") else {
            throw SlugFontError.fileNotFound("Roboto-Regular.sluggish (bundled)")
        }
        try self.init(device: device, url: url)
    }

    /// Load a .sluggish font from a file URL.
    public init(device: MTLDevice, url: URL) throws {
        let data = try Data(contentsOf: url)
        var offset = 0

        func read<T>(_ type: T.Type) throws -> T {
            let size = MemoryLayout<T>.size
            guard offset + size <= data.count else {
                throw SlugFontError.invalidTexture("Unexpected end of file at offset \(offset)")
            }
            let value = data[offset..<offset+size].withUnsafeBytes { $0.loadUnaligned(as: T.self) }
            offset += size
            return value
        }

        func readBytes(_ count: Int) throws -> Data {
            guard offset + count <= data.count else {
                throw SlugFontError.invalidTexture("Unexpected end of file at offset \(offset)")
            }
            let slice = data[offset..<offset+count]
            offset += count
            return Data(slice)
        }

        // 1. Verify header
        let headerBytes = try readBytes(8)
        guard String(data: headerBytes, encoding: .ascii) == "SLUGGISH" else {
            throw SlugFontError.invalidHeader
        }

        // 2. Font metrics header
        let unitsPerEm = try read(Int32.self)
        let ascender = try read(Int32.self)
        let descender = try read(Int32.self)
        let lineGap = try read(Int32.self)
        self.metrics = FontMetrics(
            unitsPerEm: unitsPerEm, ascender: ascender,
            descender: descender, lineGap: lineGap
        )

        // 3. Code point count
        let codePointCount = try read(UInt16.self)
        guard codePointCount > 0 else {
            throw SlugFontError.noCodePoints
        }

        // 4. Read glyph info structs (packed)
        var glyphMap: [UInt32: GlyphInfo] = [:]
        for _ in 0..<Int(codePointCount) {
            let cp = try read(UInt32.self)
            let w = try read(UInt32.self)
            let h = try read(UInt32.self)
            let bc = try read(UInt32.self)
            let bdx = try read(UInt32.self)
            let bdy = try read(UInt32.self)
            let btx = try read(UInt16.self)
            let bty = try read(UInt16.self)
            let aw = try read(Int32.self)
            let lsb = try read(Int32.self)
            let by = try read(Int32.self)
            glyphMap[cp] = GlyphInfo(
                codePoint: cp, width: w, height: h,
                bandCount: bc, bandDimX: bdx, bandDimY: bdy,
                bandsTexCoordX: btx, bandsTexCoordY: bty,
                advanceWidth: aw, leftSideBearing: lsb, bearingY: by
            )
        }
        self.glyphs = glyphMap

        // 5. Kern pairs
        let kernPairCount = try read(UInt16.self)
        var kern: [UInt64: Int32] = [:]
        for _ in 0..<Int(kernPairCount) {
            let left = try read(UInt32.self)
            let right = try read(UInt32.self)
            let advance = try read(Int32.self)
            let key = (UInt64(left) << 32) | UInt64(right)
            kern[key] = advance
        }
        self.kernTable = kern

        // 6. Curves texture (RGBA32F)
        let curvesW = try read(UInt16.self)
        let curvesH = try read(UInt16.self)
        let curvesBytes = try read(UInt32.self)
        guard curvesW == 4096 else {
            throw SlugFontError.invalidTexture("curves width \(curvesW) != 4096")
        }

        let curvesData = try readBytes(Int(curvesBytes))

        let curvesDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba32Float,
            width: Int(curvesW), height: Int(curvesH),
            mipmapped: false
        )
        curvesDesc.usage = .shaderRead
        curvesDesc.storageMode = .shared
        guard let curvesTex = device.makeTexture(descriptor: curvesDesc) else {
            throw SlugFontError.textureCreationFailed
        }
        curvesTex.label = "SlugFont curves"

        let curvesTexelCount = Int(curvesW) * Int(curvesH)
        let curvesBytesPerRow = Int(curvesW) * 16
        let curvesBytesTotal = curvesTexelCount * 16

        var curvesBuffer = Data(count: curvesBytesTotal)
        curvesBuffer.replaceSubrange(0..<min(curvesData.count, curvesBytesTotal), with: curvesData)
        curvesBuffer.withUnsafeBytes { ptr in
            curvesTex.replace(
                region: MTLRegion(origin: MTLOrigin(), size: MTLSize(width: Int(curvesW), height: Int(curvesH), depth: 1)),
                mipmapLevel: 0,
                withBytes: ptr.baseAddress!,
                bytesPerRow: curvesBytesPerRow
            )
        }
        self.curvesTexture = curvesTex

        // 6. Bands texture (RG16UI)
        let bandsW = try read(UInt16.self)
        let bandsH = try read(UInt16.self)
        let bandsBytesCount = try read(UInt32.self)
        guard bandsW == 4096 else {
            throw SlugFontError.invalidTexture("bands width \(bandsW) != 4096")
        }

        let bandsData = try readBytes(Int(bandsBytesCount))

        let bandsDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rg16Uint,
            width: Int(bandsW), height: Int(bandsH),
            mipmapped: false
        )
        bandsDesc.usage = .shaderRead
        bandsDesc.storageMode = .shared
        guard let bandsTex = device.makeTexture(descriptor: bandsDesc) else {
            throw SlugFontError.textureCreationFailed
        }
        bandsTex.label = "SlugFont bands"

        let bandsTexelCount = Int(bandsW) * Int(bandsH)
        let bandsBytesPerRow = Int(bandsW) * 4
        let bandsBytesTotal = bandsTexelCount * 4

        var bandsBuffer = Data(count: bandsBytesTotal)
        bandsBuffer.replaceSubrange(0..<min(bandsData.count, bandsBytesTotal), with: bandsData)
        bandsBuffer.withUnsafeBytes { ptr in
            bandsTex.replace(
                region: MTLRegion(origin: MTLOrigin(), size: MTLSize(width: Int(bandsW), height: Int(bandsH), depth: 1)),
                mipmapLevel: 0,
                withBytes: ptr.baseAddress!,
                bytesPerRow: bandsBytesPerRow
            )
        }
        self.bandsTexture = bandsTex
    }
}
