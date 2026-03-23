import Foundation

extension DeptheosDng.ImageMetadata {

    /// Convert LibRaw metadata to DNG metadata so the GPU color pipeline can process it.
    /// Populates color science from LibRaw's dual-illuminant matrices, derives asShotNeutral
    /// from WB multipliers if not provided, and fills EXIF fields with sensible defaults.
    static func fromLibRaw(_ raw: DeptheosRaw.RawMetadata) -> DeptheosDng.ImageMetadata {
        let colorScience = Self.buildColorScience(from: raw)

        let zeroUR = DeptheosDng.URational(num: 0, den: 0)
        let zeroSR = DeptheosDng.SRational(num: 0, den: 0)

        return DeptheosDng.ImageMetadata(
            camera: .init(
                make: raw.make,
                model: raw.model,
                uniqueCameraModel: raw.uniqueCameraModel,
                serialNumber: "",
                software: raw.software,
                artist: raw.artist,
                copyright: ""
            ),
            lens: .init(
                make: raw.lens.make,
                model: raw.lens.model,
                serialNumber: raw.lens.serial,
                info: [
                    .init(num: UInt32(raw.lens.minFocal), den: 1),
                    .init(num: UInt32(raw.lens.maxFocal), den: 1),
                    zeroUR, zeroUR
                ],
                focalLengthIn35mm: UInt32(raw.lens.focalLengthIn35mm)
            ),
            exposure: .init(
                // EXIF ExposureTime as rational: ≥1s → seconds/1, <1s → 1/round(1/shutter)
                exposureTime: raw.shutter >= 1.0
                    ? .init(num: UInt32(raw.shutter + 0.5), den: 1)
                    : raw.shutter > 0
                        ? .init(num: 1, den: UInt32(1.0 / raw.shutter + 0.5))
                        : zeroUR,
                // EXIF FNumber stored as rational with 1 decimal: e.g. f/2.8 → 28/10
                fnumber: raw.aperture > 0
                    ? .init(num: UInt32(raw.aperture * 10), den: 10)
                    : zeroUR,
                // EXIF FocalLength stored as rational with 1 decimal: e.g. 50mm → 500/10
                focalLength: raw.focalLength > 0
                    ? .init(num: UInt32(raw.focalLength * 10), den: 10)
                    : zeroUR,
                maxApertureValue: zeroUR,
                exposureBias: zeroSR,
                isoSpeed: [UInt32(raw.isoSpeed), 0, 0],
                program: 0, mode: 0, meteringMode: 0,
                flash: 0, whiteBalance: 0, sceneCaptureType: 0,
                focalPlaneXRes: zeroUR, focalPlaneYRes: zeroUR,
                focalPlaneResUnit: 0
            ),
            dates: .init(original: "", digitized: ""),
            gps: .init(
                versionId: 0,
                latitudeRef: "", latitude: [zeroUR, zeroUR, zeroUR],
                longitudeRef: "", longitude: [zeroUR, zeroUR, zeroUR],
                altitudeRef: 0, altitude: zeroUR
            ),
            geometry: .init(
                orientation: UInt32(raw.orientation),
                channels: Int32(raw.rawColors > 0 ? min(raw.rawColors, 3) : 3),
                useJXL: false
            ),
            colorScience: colorScience,
            luts: .init(
                hueSatMapDims1: nil, hueSatMapData1: nil,
                hueSatMapDims2: nil, hueSatMapData2: nil,
                hueSatMapEncoding: 0,
                lookTableDims: nil, lookTableData: nil,
                lookTableEncoding: 0,
                profileToneCurve: nil
            ),
            noiseProfile: .init(scale: [], offset: []),
            opcodes: .init(opcodeList2: nil, opcodeList3: nil)
        )
    }

    /// Build DNG ColorScienceInfo from LibRaw's dual-illuminant color matrices.
    private static func buildColorScience(from raw: DeptheosRaw.RawMetadata) -> ColorScienceInfo {
        var cs = ColorScienceInfo(
            colorMatrix1: .init(values: [], isValid: false),
            colorMatrix2: .init(values: [], isValid: false),
            forwardMatrix1: .init(values: [], isValid: false),
            forwardMatrix2: .init(values: [], isValid: false),
            cameraCalibration1: .init(values: [], isValid: false),
            cameraCalibration2: .init(values: [], isValid: false),
            calibrationIlluminant1: 0,
            calibrationIlluminant2: 0,
            cameraCalibrationSig: "",
            profileCalibrationSig: "",
            profileName: "",
            asShotNeutral: [],
            analogBalance: [],
            baselineExposure: nil,
            baselineNoise: nil,
            baselineSharpness: nil,
            shadowScale: nil,
            linearResponseLimit: nil
        )

        // Map the two DNG illuminants from LibRaw
        for (i, color) in raw.dngColor.enumerated() {
            guard i < 2 else { break }
            let illuminant = UInt32(color.illuminant)

            // ColorMatrix: LibRaw stores as 4x3 (XYZ→camera), DNG uses first 3x3
            if let cm = color.colorMatrix, cm.count >= 9 {
                let mat = DeptheosDng.Matrix3x3(
                    values: (0..<9).map { Double(cm[$0]) },
                    isValid: true
                )
                if i == 0 {
                    cs.colorMatrix1 = mat
                    cs.calibrationIlluminant1 = illuminant
                } else {
                    cs.colorMatrix2 = mat
                    cs.calibrationIlluminant2 = illuminant
                }
            }

            // ForwardMatrix: LibRaw stores as 3x4 (camera→XYZ), DNG uses first 3x3
            if let fm = color.forwardMatrix, fm.count >= 9 {
                let mat = DeptheosDng.Matrix3x3(
                    values: (0..<9).map { Double(fm[$0]) },
                    isValid: true
                )
                if i == 0 {
                    cs.forwardMatrix1 = mat
                } else {
                    cs.forwardMatrix2 = mat
                }
            }

            // CameraCalibration — LibRaw may provide all-zero arrays; treat as absent
            if let cal = color.calibration, cal.count >= 9, cal.contains(where: { $0 != 0 }) {
                let mat = DeptheosDng.Matrix3x3(
                    values: (0..<9).map { Double(cal[$0]) },
                    isValid: true
                )
                if i == 0 {
                    cs.cameraCalibration1 = mat
                } else {
                    cs.cameraCalibration2 = mat
                }
            }
        }

        // AsShotNeutral from LibRaw
        if let asn = raw.asShotNeutral, !asn.isEmpty {
            cs.asShotNeutral = asn.map { Double($0) }
        } else {
            // Derive from camera white balance multipliers.
            // camMul is what you multiply by to get neutral white,
            // asShotNeutral is the inverse: what neutral looks like in camera space.
            let cm = raw.camMul
            if cm.count >= 3, cm[0] > 0, cm[1] > 0, cm[2] > 0 {
                let inv = [1.0 / cm[0], 1.0 / cm[1], 1.0 / cm[2]]
                let maxInv = max(inv[0], max(inv[1], inv[2]))
                cs.asShotNeutral = [
                    Double(inv[0] / maxInv),
                    Double(inv[1] / maxInv),
                    Double(inv[2] / maxInv)
                ]
            }
        }

        if let ab = raw.analogBalance {
            cs.analogBalance = ab.map { Double($0) }
        }
        if let be = raw.baselineExposure {
            cs.baselineExposure = Double(be)
        }
        if let lrl = raw.linearResponseLimit {
            cs.linearResponseLimit = Double(lrl)
        }

        return cs
    }
}
