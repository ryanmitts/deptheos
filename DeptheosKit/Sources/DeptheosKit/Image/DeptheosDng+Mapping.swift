import CDNGBridge
import CxxStdlib
import Foundation

func toCxxVector(_ array: [Float]) -> CxxFloatVector {
    var v = CxxFloatVector()
    v.reserve(array.count)
    for x in array { v.push_back(x) }
    return v
}

func toCxxVector(_ array: [Double]) -> CxxDoubleVector {
    var v = CxxDoubleVector()
    v.reserve(array.count)
    for x in array { v.push_back(x) }
    return v
}

func toCxxVector(_ array: [UInt32]) -> CxxUInt32Vector {
    var v = CxxUInt32Vector()
    v.reserve(array.count)
    for x in array { v.push_back(x) }
    return v
}

func toCxxVector(_ array: [DeptheosDng.URational]) -> CxxURationalVector {
    var v = CxxURationalVector()
    v.reserve(array.count)
    for x in array { v.push_back(CDNGBridge.DngURational(num: x.num, den: x.den)) }
    return v
}

func toCxxVector(_ array: [UInt8]) -> CxxUInt8Vector {
    var v = CxxUInt8Vector()
    v.reserve(array.count)
    for x in array { v.push_back(x) }
    return v
}

extension DeptheosDng {

    internal static func mapURational(_ r: CDNGBridge.DngURational) -> URational {
        URational(num: r.num, den: r.den)
    }

    internal static func mapSRational(_ r: CDNGBridge.DngSRational) -> SRational {
        SRational(num: r.num, den: r.den)
    }

    internal static func mapMatrix3x3(_ m: CDNGBridge.DngMatrix3x3) -> Matrix3x3 {
        Matrix3x3(values: Array(m.values), isValid: m.isValid)
    }

    private static func mapOptionalDims(_ opt: CxxOptionalHueSatDims) -> HueSatDims? {
        guard let d = Optional(fromCxx: opt) else { return nil }
        return HueSatDims(hueDivisions: d.hueDivisions, satDivisions: d.satDivisions, valDivisions: d.valDivisions)
    }

    private static func optionalFloatArray(_ v: CxxFloatVector) -> [Float]? {
        v.isEmpty ? nil : Array(v)
    }

    private static func optionalData(_ v: CxxUInt8Vector) -> Data? {
        v.isEmpty ? nil : Data(Array(v))
    }

    internal static func mapFromCxx(_ cpp: CDNGBridge.DngImageMetadata) -> ImageMetadata {
        let cam = cpp.camera
        let lens = cpp.lens
        let exp = cpp.exposure
        let dates = cpp.dates
        let gps = cpp.gps
        let geo = cpp.geometry
        let cs = cpp.colorScience
        let luts = cpp.luts
        let np = cpp.noiseProfile
        let opc = cpp.opcodes

        return ImageMetadata(
            camera: .init(
                make: String(cam.make),
                model: String(cam.model),
                uniqueCameraModel: String(cam.uniqueCameraModel),
                serialNumber: String(cam.serialNumber),
                software: String(cam.software),
                artist: String(cam.artist),
                copyright: String(cam.copyright)
            ),
            lens: .init(
                make: String(lens.make),
                model: String(lens.model),
                serialNumber: String(lens.serialNumber),
                info: Array(lens.info).map { mapURational($0) },
                focalLengthIn35mm: lens.focalLengthIn35mm
            ),
            exposure: .init(
                exposureTime: mapURational(exp.exposureTime),
                fnumber: mapURational(exp.fnumber),
                focalLength: mapURational(exp.focalLength),
                maxApertureValue: mapURational(exp.maxApertureValue),
                exposureBias: mapSRational(exp.exposureBias),
                isoSpeed: Array(exp.isoSpeed),
                program: exp.program,
                mode: exp.mode,
                meteringMode: exp.meteringMode,
                flash: exp.flash,
                whiteBalance: exp.whiteBalance,
                sceneCaptureType: exp.sceneCaptureType,
                focalPlaneXRes: mapURational(exp.focalPlaneXRes),
                focalPlaneYRes: mapURational(exp.focalPlaneYRes),
                focalPlaneResUnit: exp.focalPlaneResUnit
            ),
            dates: .init(
                original: String(dates.original),
                digitized: String(dates.digitized)
            ),
            gps: .init(
                versionId: gps.versionId,
                latitudeRef: String(gps.latitudeRef),
                latitude: Array(gps.latitude).map { mapURational($0) },
                longitudeRef: String(gps.longitudeRef),
                longitude: Array(gps.longitude).map { mapURational($0) },
                altitudeRef: gps.altitudeRef,
                altitude: mapURational(gps.altitude)
            ),
            geometry: .init(
                orientation: geo.orientation,
                channels: geo.channels,
                useJXL: geo.useJXL
            ),
            colorScience: .init(
                colorMatrix1: mapMatrix3x3(cs.colorMatrix1),
                colorMatrix2: mapMatrix3x3(cs.colorMatrix2),
                forwardMatrix1: mapMatrix3x3(cs.forwardMatrix1),
                forwardMatrix2: mapMatrix3x3(cs.forwardMatrix2),
                cameraCalibration1: mapMatrix3x3(cs.cameraCalibration1),
                cameraCalibration2: mapMatrix3x3(cs.cameraCalibration2),
                calibrationIlluminant1: cs.calibrationIlluminant1,
                calibrationIlluminant2: cs.calibrationIlluminant2,
                cameraCalibrationSig: String(cs.cameraCalibrationSig),
                profileCalibrationSig: String(cs.profileCalibrationSig),
                profileName: String(cs.profileName),
                asShotNeutral: Array(cs.asShotNeutral),
                analogBalance: Array(cs.analogBalance),
                baselineExposure: Optional(fromCxx: cs.baselineExposure),
                baselineNoise: Optional(fromCxx: cs.baselineNoise),
                baselineSharpness: Optional(fromCxx: cs.baselineSharpness),
                shadowScale: Optional(fromCxx: cs.shadowScale),
                linearResponseLimit: Optional(fromCxx: cs.linearResponseLimit)
            ),
            luts: .init(
                hueSatMapDims1: mapOptionalDims(luts.hueSatMapDims1),
                hueSatMapData1: optionalFloatArray(luts.hueSatMapData1),
                hueSatMapDims2: mapOptionalDims(luts.hueSatMapDims2),
                hueSatMapData2: optionalFloatArray(luts.hueSatMapData2),
                hueSatMapEncoding: luts.hueSatMapEncoding,
                lookTableDims: mapOptionalDims(luts.lookTableDims),
                lookTableData: optionalFloatArray(luts.lookTableData),
                lookTableEncoding: luts.lookTableEncoding,
                profileToneCurve: optionalFloatArray(luts.profileToneCurve)
            ),
            noiseProfile: .init(
                scale: Array(np.scale),
                offset: Array(np.offset)
            ),
            opcodes: .init(
                opcodeList2: optionalData(opc.opcodeList2),
                opcodeList3: optionalData(opc.opcodeList3)
            )
        )
    }

    private static func toCxxMatrix(_ m: Matrix3x3) -> CDNGBridge.DngMatrix3x3 {
        var result = CDNGBridge.DngMatrix3x3()
        result.values = toCxxVector(m.values)
        result.isValid = m.isValid
        return result
    }

    private static func toCxxOptionalDims(_ dims: HueSatDims?) -> CxxOptionalHueSatDims {
        guard let dims else { return CxxOptionalHueSatDims() }
        return makeOptionalHueSatDims(
            CDNGBridge.DngHueSatDims(
                hueDivisions: dims.hueDivisions,
                satDivisions: dims.satDivisions,
                valDivisions: dims.valDivisions
            )
        )
    }

    private static func toCxxFloats(_ arr: [Float]?) -> CxxFloatVector {
        guard let arr else { return CxxFloatVector() }
        return toCxxVector(arr)
    }

    private static func toCxxBytes(_ data: Data?) -> CxxUInt8Vector {
        guard let data else { return CxxUInt8Vector() }
        return toCxxVector(Array(data))
    }

    internal static func mapToCxx(_ s: ImageMetadata) -> CDNGBridge.DngImageMetadata {
        var m = CDNGBridge.DngImageMetadata()

        // Camera
        m.camera.make = std.string(s.camera.make)
        m.camera.model = std.string(s.camera.model)
        m.camera.uniqueCameraModel = std.string(s.camera.uniqueCameraModel)
        m.camera.serialNumber = std.string(s.camera.serialNumber)
        m.camera.software = std.string(s.camera.software)
        m.camera.artist = std.string(s.camera.artist)
        m.camera.copyright = std.string(s.camera.copyright)

        // Lens
        m.lens.make = std.string(s.lens.make)
        m.lens.model = std.string(s.lens.model)
        m.lens.serialNumber = std.string(s.lens.serialNumber)
        m.lens.info = toCxxVector(s.lens.info)
        m.lens.focalLengthIn35mm = s.lens.focalLengthIn35mm

        // Exposure
        m.exposure.exposureTime = CDNGBridge.DngURational(num: s.exposure.exposureTime.num, den: s.exposure.exposureTime.den)
        m.exposure.fnumber = CDNGBridge.DngURational(num: s.exposure.fnumber.num, den: s.exposure.fnumber.den)
        m.exposure.focalLength = CDNGBridge.DngURational(num: s.exposure.focalLength.num, den: s.exposure.focalLength.den)
        m.exposure.maxApertureValue = CDNGBridge.DngURational(num: s.exposure.maxApertureValue.num, den: s.exposure.maxApertureValue.den)
        m.exposure.exposureBias = CDNGBridge.DngSRational(num: s.exposure.exposureBias.num, den: s.exposure.exposureBias.den)
        m.exposure.isoSpeed = toCxxVector(s.exposure.isoSpeed)
        m.exposure.program = s.exposure.program
        m.exposure.mode = s.exposure.mode
        m.exposure.meteringMode = s.exposure.meteringMode
        m.exposure.flash = s.exposure.flash
        m.exposure.whiteBalance = s.exposure.whiteBalance
        m.exposure.sceneCaptureType = s.exposure.sceneCaptureType
        m.exposure.focalPlaneXRes = CDNGBridge.DngURational(num: s.exposure.focalPlaneXRes.num, den: s.exposure.focalPlaneXRes.den)
        m.exposure.focalPlaneYRes = CDNGBridge.DngURational(num: s.exposure.focalPlaneYRes.num, den: s.exposure.focalPlaneYRes.den)
        m.exposure.focalPlaneResUnit = s.exposure.focalPlaneResUnit

        // Dates
        m.dates.original = std.string(s.dates.original)
        m.dates.digitized = std.string(s.dates.digitized)

        // GPS
        m.gps.versionId = s.gps.versionId
        m.gps.latitudeRef = std.string(s.gps.latitudeRef)
        m.gps.latitude = toCxxVector(s.gps.latitude)
        m.gps.longitudeRef = std.string(s.gps.longitudeRef)
        m.gps.longitude = toCxxVector(s.gps.longitude)
        m.gps.altitudeRef = s.gps.altitudeRef
        m.gps.altitude = CDNGBridge.DngURational(num: s.gps.altitude.num, den: s.gps.altitude.den)

        // Geometry
        m.geometry.orientation = s.geometry.orientation
        m.geometry.channels = s.geometry.channels
        m.geometry.useJXL = s.geometry.useJXL

        // Color science
        let cs = s.colorScience
        m.colorScience.colorMatrix1 = toCxxMatrix(cs.colorMatrix1)
        m.colorScience.colorMatrix2 = toCxxMatrix(cs.colorMatrix2)
        m.colorScience.forwardMatrix1 = toCxxMatrix(cs.forwardMatrix1)
        m.colorScience.forwardMatrix2 = toCxxMatrix(cs.forwardMatrix2)
        m.colorScience.cameraCalibration1 = toCxxMatrix(cs.cameraCalibration1)
        m.colorScience.cameraCalibration2 = toCxxMatrix(cs.cameraCalibration2)
        m.colorScience.calibrationIlluminant1 = cs.calibrationIlluminant1
        m.colorScience.calibrationIlluminant2 = cs.calibrationIlluminant2
        m.colorScience.cameraCalibrationSig = std.string(cs.cameraCalibrationSig)
        m.colorScience.profileCalibrationSig = std.string(cs.profileCalibrationSig)
        m.colorScience.profileName = std.string(cs.profileName)
        m.colorScience.asShotNeutral = toCxxVector(cs.asShotNeutral)
        m.colorScience.analogBalance = toCxxVector(cs.analogBalance)
        if let v = cs.baselineExposure { m.colorScience.baselineExposure = makeOptionalDouble(v) }
        if let v = cs.baselineNoise { m.colorScience.baselineNoise = makeOptionalDouble(v) }
        if let v = cs.baselineSharpness { m.colorScience.baselineSharpness = makeOptionalDouble(v) }
        if let v = cs.shadowScale { m.colorScience.shadowScale = makeOptionalDouble(v) }
        if let v = cs.linearResponseLimit { m.colorScience.linearResponseLimit = makeOptionalDouble(v) }

        // LUTs
        m.luts.hueSatMapDims1 = toCxxOptionalDims(s.luts.hueSatMapDims1)
        m.luts.hueSatMapData1 = toCxxFloats(s.luts.hueSatMapData1)
        m.luts.hueSatMapDims2 = toCxxOptionalDims(s.luts.hueSatMapDims2)
        m.luts.hueSatMapData2 = toCxxFloats(s.luts.hueSatMapData2)
        m.luts.hueSatMapEncoding = s.luts.hueSatMapEncoding
        m.luts.lookTableDims = toCxxOptionalDims(s.luts.lookTableDims)
        m.luts.lookTableData = toCxxFloats(s.luts.lookTableData)
        m.luts.lookTableEncoding = s.luts.lookTableEncoding
        m.luts.profileToneCurve = toCxxFloats(s.luts.profileToneCurve)

        // Noise profile
        m.noiseProfile.scale = toCxxVector(s.noiseProfile.scale)
        m.noiseProfile.offset = toCxxVector(s.noiseProfile.offset)

        // Opcodes
        m.opcodes.opcodeList2 = toCxxBytes(s.opcodes.opcodeList2)
        m.opcodes.opcodeList3 = toCxxBytes(s.opcodes.opcodeList3)

        return m
    }

    internal static func mapRenderParamsFromCxx(_ cpp: CDNGBridge.DngRenderParams) -> RenderParams {
        RenderParams(
            cameraToProPhoto: Array(cpp.cameraToProPhoto),
            cameraWhite: Array(cpp.cameraWhite),
            wbBakedIn: cpp.wbBakedIn,
            blackLevel: cpp.blackLevel,
            baselineExposure: cpp.baselineExposure,
            shadowScale: cpp.shadowScale,
            hueSatMapDims: mapOptionalDims(cpp.hueSatMapDims),
            hueSatMapData: optionalFloatArray(cpp.hueSatMapData),
            lookTableDims: mapOptionalDims(cpp.lookTableDims),
            lookTableData: optionalFloatArray(cpp.lookTableData),
            toneCurve: Array(cpp.toneCurve)
        )
    }
}
