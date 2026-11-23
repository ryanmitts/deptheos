// Checks BitmapDecode (ios/DeptheosMobile/Engine/BitmapDecode.swift), the
// iOS app's decoder for standard images, against Core Image's render, on a
// Mac: the same pixels, as stored and in the image's own space (the engine
// turns them), for JPEG in all eight EXIF orientations, HEIC, and 16-bit
// PNG and TIFF; and each one's peak memory.
// Run through script/check-bitmap-decode.sh.
//
//   check-bitmap-decode compare <frame.jpg> <scratch dir>
//   check-bitmap-decode peak <cg|ci> <file>

import CoreImage
import Foundation
import ImageIO
import UniformTypeIdentifiers

typealias Header = BitmapDecode.Opened

extension BitmapDecode.Opened {
    var sixteenBit: Bool { depth == .sixteen }
}

/// As AppleFrameDecoder opens it.
func header(_ url: URL) -> Header { try! BitmapDecode.open(url) }

/// A buffer of the header's size, RGBX at its depth.
func buffer(_ h: Header) -> (UnsafeMutableRawPointer, Int, Int) {
    precondition(h.layout == .rgbx && h.depth != .float, "the cases are RGB at 8 or 16 bits")
    let rowBytes = h.width * 4 * h.bytesPerSample
    let length = h.length
    let pixels = UnsafeMutableRawPointer.allocate(byteCount: length, alignment: 16)
    pixels.initializeMemory(as: UInt8.self, repeating: 0, count: length)
    return (pixels, rowBytes, length)
}

let ciContext = CIContext(options: [.cacheIntermediates: false])

/// Core Image's render of the same image, as stored (not turned), in the
/// space BitmapDecode draws in.
func decodeCI(_ url: URL, _ h: Header, _ pixels: UnsafeMutableRawPointer, _ rowBytes: Int) {
    var image = CIImage(contentsOf: url, options: [.applyOrientationProperty: false])!
    let extent = image.extent.integral
    image = image.transformed(by: CGAffineTransform(translationX: -extent.origin.x, y: -extent.origin.y))
    guard case let .space(space) = h.color else { fatalError("a refused color") }
    ciContext.render(
        image, toBitmap: pixels, rowBytes: rowBytes,
        bounds: CGRect(x: 0, y: 0, width: extent.width, height: extent.height),
        format: h.sixteenBit ? .RGBA16 : .RGBA8, colorSpace: space)
}

func decodeCG(_ url: URL, _ h: Header, _ pixels: UnsafeMutableRawPointer, _ rowBytes: Int) {
    try! h.draw(into: pixels, length: h.length)
}

/// The largest and mean difference of the color channels, in 8-bit steps.
func difference(_ a: UnsafeMutableRawPointer, _ b: UnsafeMutableRawPointer, _ h: Header) -> (Double, Double) {
    let n = h.width * h.height
    var (worst, sum) = (0.0, 0.0)
    for i in 0..<n {
        for c in 0..<3 {
            let d: Double = h.sixteenBit
                ? abs(Double(a.load(fromByteOffset: (i * 4 + c) * 2, as: UInt16.self).littleEndian)
                    - Double(b.load(fromByteOffset: (i * 4 + c) * 2, as: UInt16.self).littleEndian)) / 257
                : abs(Double(a.load(fromByteOffset: i * 4 + c, as: UInt8.self))
                    - Double(b.load(fromByteOffset: i * 4 + c, as: UInt8.self)))
            worst = max(worst, d)
            sum += d
        }
    }
    return (worst, sum / Double(n * 3))
}

/// `image` written to `url` as `type`, with EXIF `orientation`.
func write(_ image: CGImage, _ url: URL, _ type: UTType, orientation: UInt32 = 1) {
    let dest = CGImageDestinationCreateWithURL(url as CFURL, type.identifier as CFString, 1, nil)!
    let props: [CFString: Any] = [kCGImagePropertyOrientation: orientation, kCGImageDestinationLossyCompressionQuality: 0.95]
    CGImageDestinationAddImage(dest, image, props as CFDictionary)
    precondition(CGImageDestinationFinalize(dest))
}

/// `image` redrawn at 16 bits a channel, with a gradient in the low bits
/// so a 16-bit decode that kept only 8 would show.
func sixteen(_ image: CGImage) -> CGImage {
    let ctx = CGContext(
        data: nil, width: image.width, height: image.height, bitsPerComponent: 16,
        bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
    let p = ctx.data!.assumingMemoryBound(to: UInt16.self)
    let stride = ctx.bytesPerRow / 2
    for y in 0..<image.height {
        for x in 0..<image.width { p[y * stride + x * 4] ^= UInt16(x & 0xff) }
    }
    return ctx.makeImage()!
}

/// The process's peak physical footprint, as iOS counts it against the app.
func peakFootprint() -> UInt64 {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    _ = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    return UInt64(info.ledger_phys_footprint_peak)
}

/// The process's physical footprint now.
func footprint() -> UInt64 {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    _ = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    return info.phys_footprint
}

/// What BitmapDecode opens each kind of image as: a Display P3 HEIC in
/// P3 with its profile, gray as gray, alpha premultiplied (gray with alpha
/// interleaved), CMYK refused, and the orientation as stored.
func checkKinds(_ image: CGImage, _ dir: URL) -> Bool {
    func redrawn(_ space: CGColorSpace, _ alpha: CGImageAlphaInfo, _ bits: Int = 8) -> CGImage {
        let ctx = CGContext(
            data: nil, width: 64, height: 48, bitsPerComponent: bits, bytesPerRow: 0,
            space: space, bitmapInfo: alpha.rawValue)!
        if alpha != .none && alpha != .noneSkipLast {
            ctx.clear(CGRect(x: 0, y: 0, width: 64, height: 48))
            ctx.draw(image, in: CGRect(x: 16, y: 0, width: 32, height: 48))
        } else {
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: 64, height: 48))
        }
        return ctx.makeImage()!
    }
    let p3 = CGColorSpace(name: CGColorSpace.displayP3)!
    let gray = CGColorSpace(name: CGColorSpace.genericGrayGamma2_2)!
    let cmyk = CGColorSpace(name: CGColorSpace.genericCMYK)!
    let srgb = CGColorSpace(name: CGColorSpace.sRGB)!
    let cases: [(String, CGImage, UTType, UInt32, (BitmapDecode.Opened) -> Bool)] = [
        ("Display P3 HEIC", redrawn(p3, .noneSkipLast), .heic, 1, {
            if case let .space(s) = $0.color { s.name == CGColorSpace.displayP3 && $0.icc != nil } else { false }
        }),
        ("gray PNG", redrawn(gray, .none), .png, 1, { $0.layout == .gray }),
        ("RGBA PNG", redrawn(srgb, .premultipliedLast), .png, 1, { $0.layout == .rgba }),
        ("CMYK JPEG", redrawn(cmyk, .none), .jpeg, 1, { if case .cmyk = $0.color { true } else { false } }),
        ("JPEG, orientation 6", redrawn(srgb, .noneSkipLast), .jpeg, 6, {
            $0.orientation == 6 && $0.width == 64 && $0.height == 48
        }),
    ]
    var ok = true
    for (name, image, type, orientation, expected) in cases {
        let url = dir.appendingPathComponent("kind-\(name.filter(\.isLetter)).\(type.preferredFilenameExtension!)")
        write(image, url, type, orientation: orientation)
        let opened = try! BitmapDecode.open(url)
        var drew = true
        if case .space = opened.color {
            let pixels = UnsafeMutableRawPointer.allocate(byteCount: opened.length, alignment: 16)
            defer { pixels.deallocate() }
            drew = (try? opened.draw(into: pixels, length: opened.length)) != nil
        }
        let pass = expected(opened) && drew
        ok = ok && pass
        print("\(pass ? "ok  " : "FAIL") \(name): \(opened.layout), \(opened.depth), \(opened.color), orientation \(opened.orientation)")
    }
    return ok
}

let args = CommandLine.arguments
switch args[1] {
case "compare":
    let frame = URL(fileURLWithPath: args[2])
    let dir = URL(fileURLWithPath: args[3])
    let source = CGImageSourceCreateWithURL(frame as CFURL, nil)!
    let image = CGImageSourceCreateImageAtIndex(source, 0, nil)!
    // (name, file, largest difference allowed, mean allowed). Lossless
    // files decode exactly alike. JPEG and HEIC differ a little where
    // color changes sharply, as Core Image and ImageIO upsample their
    // chroma differently: a mean of a fraction of a step, a few pixels
    // more.
    var cases: [(String, URL, Double, Double)] = []
    for o in UInt32(1)...8 {
        let url = dir.appendingPathComponent("orientation-\(o).jpg")
        write(image, url, .jpeg, orientation: o)
        cases.append(("JPEG, orientation \(o)", url, 16, 0.5))
    }
    let heic = dir.appendingPathComponent("frame.heic")
    write(image, heic, .heic)
    cases.append(("HEIC", heic, 16, 0.5))
    let deep = sixteen(image)
    for (name, type, ext) in [("16-bit PNG", UTType.png, "png"), ("16-bit TIFF", .tiff, "tif")] {
        let url = dir.appendingPathComponent("sixteen.\(ext)")
        write(deep, url, type)
        cases.append((name, url, 0, 0))
    }
    let tiff6 = dir.appendingPathComponent("sixteen-6.tif")
    write(deep, tiff6, .tiff, orientation: 6)
    cases.append(("16-bit TIFF, orientation 6", tiff6, 0, 0))
    var failed = !checkKinds(image, dir)
    for (name, url, allowed, allowedMean) in cases {
        let h = header(url)
        let (a, rowBytes, _) = buffer(h)
        let (b, _, _) = buffer(h)
        decodeCI(url, h, a, rowBytes)
        decodeCG(url, h, b, rowBytes)
        let (worst, mean) = difference(a, b, h)
        let ok = worst <= allowed && mean <= allowedMean
        failed = failed || !ok
        print(String(format: "%@ %@: %d×%d%@, largest difference %.2f, mean %.4f (8-bit steps)",
                     ok ? "ok  " : "FAIL", name, h.width, h.height, h.sixteenBit ? " 16-bit" : "", worst, mean))
    }
    exit(failed ? 1 : 0)
case "peak":
    let url = URL(fileURLWithPath: args[3])
    let h = header(url)
    let before = peakFootprint()
    let (pixels, rowBytes, length) = buffer(h)
    // The destination is the engine's, counted by its estimate; the rest
    // of the peak is the decoder's own.
    let baseline = max(before, peakFootprint())
    let start = footprint()
    if args[2] == "cg" { decodeCG(url, h, pixels, rowBytes) } else { decodeCI(url, h, pixels, rowBytes) }
    let decoder = Int64(peakFootprint()) - Int64(baseline)
    let kept = Int64(footprint()) - Int64(start)
    print("\(args[2]): \(h.width)×\(h.height), destination \(length >> 20) MB, the decoder's own peak \(decoder >> 20) MB, kept after \(kept >> 20) MB")
default:
    fatalError("compare or peak")
}
