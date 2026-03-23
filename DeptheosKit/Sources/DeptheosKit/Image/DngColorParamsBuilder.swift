import simd
import DeptheosShaders

/// Pure CPU color science math: converts `RenderParams` into GPU-ready `DngColorParams`.
enum DngColorParamsBuilder {

    /// Build GPU params from precomputed RenderParams. No color science needed here —
    /// the DNG SDK has already done interpolation, matrix selection, and WB resolution.
    static func buildParams(from rp: DeptheosDng.RenderParams) -> DngColorParams {
        var params = DngColorParams()

        // Normalize black level to [0, 1] range
        let s3bl = Float(rp.blackLevel) / 65535.0
        let rangeInv: Float = s3bl < 1.0 ? 1.0 / (1.0 - s3bl) : 1.0
        params.range_inv = SIMD4<Float>(rangeInv, rangeInv, rangeInv, s3bl)

        // Camera white clipping (normalized, max=1.0)
        let cw = rp.cameraWhite
        let maxCW = max(cw[0], max(cw[1], cw[2]))
        if maxCW > 1e-10 {
            params.camera_white = SIMD4<Float>(cw[0] / maxCW, cw[1] / maxCW, cw[2] / maxCW, 0)
        }

        // White balance
        if rp.wbBakedIn {
            params.wb_inv = SIMD4<Float>(1, 1, 1, 0)
        } else {
            // ColorMatrix fallback: WB applied separately as max/neutral
            let cw = rp.cameraWhite
            let maxN = max(cw[0], max(cw[1], cw[2]))
            if maxN > 0 {
                params.wb_inv = SIMD4<Float>(maxN / cw[0], maxN / cw[1], maxN / cw[2], 0)
            }
        }

        // Camera → ProPhoto RGB (already computed by SDK)
        let m = rp.cameraToProPhoto
        params.camToProPhoto_c0 = SIMD4<Float>(m[0], m[3], m[6], 0)  // column 0
        params.camToProPhoto_c1 = SIMD4<Float>(m[1], m[4], m[7], 0)  // column 1
        params.camToProPhoto_c2 = SIMD4<Float>(m[2], m[5], m[8], 0)  // column 2

        // Exposure ramp (matches DNG SDK dng_render::SetDefaultExposure)
        let exposure = Float(rp.baselineExposure)
        let white = 1.0 / powf(2.0, max(0.0, exposure))
        // Shadow constant from DNG SDK (dng_render default shadow amount)
        let shadows: Float = 5.0
        var black = shadows * Float(rp.shadowScale) * 0.001
        black = min(black, 0.99 * white)
        let slope = 1.0 / (white - black)
        let radius = min(0.5 * black, (1.0 / 16.0) / slope)
        let qScale = slope / (4.0 * radius)
        params.exposure = SIMD4<Float>(black, slope, radius, qScale)

        // Exposure tone correction (for negative baselineExposure only).
        // Quadratic curve that smoothly compresses highlights when exposure < 0,
        // matching DNG SDK's dng_tone_curve_exposure::Evaluate.
        if exposure < 0 {
            let toneSlope = powf(2.0, exposure)
            let a = (16.0 / 9.0) * (1.0 - toneSlope)
            let b = toneSlope - 0.5 * a
            let c = 1.0 - a - b
            params.tone = SIMD4<Float>(toneSlope, a, b, c)
        }

        // ProPhoto → sRGB (fixed matrix)
        let proPhotoToSrgb = buildProPhotoToSrgbMatrix()
        params.proToSrgb_c0 = SIMD4<Float>(proPhotoToSrgb.columns.0, 0)
        params.proToSrgb_c1 = SIMD4<Float>(proPhotoToSrgb.columns.1, 0)
        params.proToSrgb_c2 = SIMD4<Float>(proPhotoToSrgb.columns.2, 0)

        // HueSatMap / LookTable flags and dimensions
        let hasHsm = rp.hueSatMapData != nil && !(rp.hueSatMapData!.isEmpty)
        let hasLt = rp.lookTableData != nil && !(rp.lookTableData!.isEmpty)
        params.flags = SIMD4<UInt32>(hasHsm ? 1 : 0, hasLt ? 1 : 0, 0, 0)
        if hasHsm, let dims = rp.hueSatMapDims {
            params.hsm_dims = SIMD4<UInt32>(dims.hueDivisions, dims.satDivisions, dims.valDivisions, 0)
        }
        if hasLt, let dims = rp.lookTableDims {
            params.lt_dims = SIMD4<UInt32>(dims.hueDivisions, dims.satDivisions, dims.valDivisions, 0)
        }

        return params
    }

    /// ProPhoto RGB → linear sRGB (fixed, precomputed from known primaries).
    /// Both matrices use D50 PCS (Profile Connection Space), matching the DNG SDK.
    static func buildProPhotoToSrgbMatrix() -> simd_float3x3 {
        // ProPhoto RGB → XYZ D50 (from dng_space_ProPhoto)
        let proPhotoToXYZ = simd_float3x3(rows: [
            SIMD3<Float>(0.7977, 0.1352, 0.0313),
            SIMD3<Float>(0.2880, 0.7119, 0.0001),
            SIMD3<Float>(0.0000, 0.0000, 0.8249),
        ])
        // sRGB → XYZ D50 (from dng_space_sRGB) — invert to get XYZ D50 → sRGB
        let srgbToXYZ = simd_float3x3(rows: [
            SIMD3<Float>(0.4361, 0.3851, 0.1431),
            SIMD3<Float>(0.2225, 0.7169, 0.0606),
            SIMD3<Float>(0.0139, 0.0971, 0.7141),
        ])
        let xyzToSrgb = srgbToXYZ.inverse
        return xyzToSrgb * proPhotoToXYZ
    }

    /// Precompose exposureTone + raw tone curve into `count` entries, matching SDK's
    /// dng_1d_concatenate(exposureTone, toneCurve) → dng_1d_table(4096).
    /// The shader then applies this precomposed curve with hue preservation only.
    static func precomposeToneCurve(
        rawCurve: [Float], tone: SIMD4<Float>, count: Int
    ) -> [Float] {
        var result = [Float](repeating: 0, count: count)
        let n = count - 1
        let curveN = rawCurve.count - 1
        for i in 0...n {
            var x = Float(i) / Float(n)
            // Apply exposureTone (matches shader's exposureTone function)
            if tone.x != 0 {
                let slope = tone.x, a = tone.y, b = tone.z, c = tone.w
                x = x <= 0.25 ? x * slope : (a * x + b) * x + c
            }
            // Interpolate in the raw curve (matches SDK's dng_tone_curve_acr3_default::Evaluate)
            let y = x * Float(curveN)
            let idx = min(max(Int(y), 0), curveN - 1)
            let fract = y - Float(idx)
            result[i] = rawCurve[idx] * (1.0 - fract) + rawCurve[idx + 1] * fract
        }
        return result
    }

}
