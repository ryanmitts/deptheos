// Make the sample stack the apps carry (ADR-151) from a folder of full-size
// JPEG frames: each frame at half size, quality 0.7 (or the divisor and
// quality given), named <name>-NN.jpg in name order, keeping the color profile and what Render Info shows (camera,
// lens, exposure, capture time) and dropping everything else: serial
// numbers, the owner's name, XMP and IPTC.
//
//   swift script/sample-stack.swift <frames folder> <output folder> <name> [divisor] [quality]
//   swift script/sample-stack.swift ~/sample_images/jpeg/hornet samples/hornet hornet
//   swift script/sample-stack.swift ~/Pictures/pcb samples/pcb pcb 2 0.5

import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard (4...6).contains(args.count),
    let divisor = args.count > 4 ? Int(args[4]) : 2, divisor >= 1,
    let quality = args.count > 5 ? Double(args[5]) : 0.7, (0.1...1).contains(quality)
else {
    FileHandle.standardError.write(
        Data("usage: sample-stack.swift <frames folder> <output folder> <name> [divisor] [quality]\n".utf8))
    exit(2)
}
let source = URL(fileURLWithPath: args[1], isDirectory: true)
let output = URL(fileURLWithPath: args[2], isDirectory: true)
let name = args[3]

let files = try FileManager.default.contentsOfDirectory(at: source, includingPropertiesForKeys: nil)
    .filter { ["jpg", "jpeg"].contains($0.pathExtension.lowercased()) }
    .sorted { $0.lastPathComponent.compare($1.lastPathComponent, options: .numeric) == .orderedAscending }
guard !files.isEmpty else {
    FileHandle.standardError.write(Data("no JPEG frames in \(source.path)\n".utf8))
    exit(1)
}
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

/// The keys kept from a frame's Exif and TIFF dictionaries.
let exifKept: [CFString] = [
    kCGImagePropertyExifExposureTime, kCGImagePropertyExifFNumber,
    kCGImagePropertyExifISOSpeedRatings, kCGImagePropertyExifFocalLength,
    kCGImagePropertyExifDateTimeOriginal, kCGImagePropertyExifSubsecTimeOriginal,
    kCGImagePropertyExifOffsetTimeOriginal, kCGImagePropertyExifLensModel,
    kCGImagePropertyExifExposureBiasValue,
]
let tiffKept: [CFString] = [kCGImagePropertyTIFFMake, kCGImagePropertyTIFFModel]

func kept(_ all: [CFString: Any], _ dictionary: CFString, _ keys: [CFString]) -> [CFString: Any] {
    let from = all[dictionary] as? [CFString: Any] ?? [:]
    return from.filter { keys.contains($0.key) }
}

let digits = max(2, String(files.count).count)
for (index, file) in files.enumerated() {
    guard let image = CGImageSourceCreateWithURL(file as CFURL, nil),
        let all = CGImageSourceCopyPropertiesAtIndex(image, 0, nil) as? [CFString: Any],
        let width = all[kCGImagePropertyPixelWidth] as? Int,
        let height = all[kCGImagePropertyPixelHeight] as? Int
    else {
        FileHandle.standardError.write(Data("can't read \(file.path)\n".utf8))
        exit(1)
    }
    // Smaller, with the file's orientation applied so none is needed.
    let half = CGImageSourceCreateThumbnailAtIndex(
        image, 0,
        [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: max(width, height) / divisor,
        ] as CFDictionary)
    let number = String(format: "%0\(digits)d", index + 1)
    let target = output.appendingPathComponent("\(name)-\(number).jpg")
    guard let half,
        let destination = CGImageDestinationCreateWithURL(
            target as CFURL, UTType.jpeg.identifier as CFString, 1, nil)
    else {
        FileHandle.standardError.write(Data("can't write \(target.path)\n".utf8))
        exit(1)
    }
    let properties: [CFString: Any] = [
        kCGImageDestinationLossyCompressionQuality: quality,
        kCGImagePropertyExifDictionary: kept(all, kCGImagePropertyExifDictionary, exifKept),
        kCGImagePropertyTIFFDictionary: kept(all, kCGImagePropertyTIFFDictionary, tiffKept),
    ]
    CGImageDestinationAddImage(destination, half, properties as CFDictionary)
    guard CGImageDestinationFinalize(destination) else {
        FileHandle.standardError.write(Data("can't write \(target.path)\n".utf8))
        exit(1)
    }
    print(target.path)
}
