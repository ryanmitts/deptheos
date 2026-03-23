#include <metal_stdlib>
using namespace metal;
#include "ShaderTypes.h"

// ---------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------

static inline float exposureRamp(float x, float black, float slope, float radius, float qScale) {
    if (x <= black - radius) return 0.0;
    if (x >= black + radius) return clamp((x - black) * slope, 0.0f, 1.0f);
    float y = x - (black - radius);
    return qScale * y * y;
}

static inline float3 huePreservingToneCurve(
    float3 rgb,
    texture1d<float, access::sample> toneCurve,
    sampler toneSampler
) {
    float maxVal = max(rgb.r, max(rgb.g, rgb.b));
    float minVal = min(rgb.r, min(rgb.g, rgb.b));
    float range = maxVal - minVal;

    float mappedMax = toneCurve.sample(toneSampler, maxVal).r;
    float mappedMin = toneCurve.sample(toneSampler, minVal).r;

    if (range < 1e-10) return float3(mappedMax);

    float scale = (mappedMax - mappedMin) / range;
    return float3(
        mappedMin + (rgb.r - minVal) * scale,
        mappedMin + (rgb.g - minVal) * scale,
        mappedMin + (rgb.b - minVal) * scale
    );
}


// ---------------------------------------------------------------
// HSV conversion (matches DNG SDK DNG_RGBtoHSV / DNG_HSVtoRGB)
// ---------------------------------------------------------------

static inline float3 rgbToHsv(float3 rgb) {
    float v = max(max(rgb.r, rgb.g), rgb.b);
    float mn = min(min(rgb.r, rgb.g), rgb.b);
    float gap = v - mn;
    if (gap <= 0.0) return float3(0.0, 0.0, v);
    float s = gap / v;
    float h;
    if (rgb.r >= rgb.g && rgb.r >= rgb.b) { h = (rgb.g - rgb.b) / gap; if (h < 0.0) h += 6.0; }
    else if (rgb.g >= rgb.r && rgb.g >= rgb.b) { h = 2.0 + (rgb.b - rgb.r) / gap; }
    else { h = 4.0 + (rgb.r - rgb.g) / gap; }
    return float3(h, s, v);
}

static inline float3 hsvToRgb(float h_in, float s, float v) {
    if (s <= 0.0) return float3(v, v, v);
    float h = fmod(h_in, 6.0);
    if (h < 0.0) h += 6.0;
    uint i = uint(h);
    float f = h - float(i);
    float p = v * (1.0 - s);
    float q = v * (1.0 - s * f);
    float t = v * (1.0 - s * (1.0 - f));
    switch (i) {
        case 0u: case 6u: return float3(v, t, p);
        case 1u: return float3(q, v, p);
        case 2u: return float3(p, v, t);
        case 3u: return float3(p, q, v);
        case 4u: return float3(t, p, v);
        default: return float3(v, p, q);
    }
}

// ---------------------------------------------------------------
// HueSatMap / LookTable application
// ---------------------------------------------------------------

static inline float3 applyHsvLut(
    float3 rgb,
    device const float* lut,
    uint hue_divs, uint sat_divs, uint val_divs
) {
    if (hue_divs < 1u || sat_divs < 2u) return rgb;

    float3 hsv = rgbToHsv(rgb);
    float h = hsv.x, s = hsv.y, v = hsv.z;
    float h_scale = float(hue_divs) / 6.0;
    float s_scale = float(sat_divs - 1u);
    uint max_hue_idx0 = hue_divs - 1u;
    uint max_sat_idx0 = sat_divs - 2u;
    uint hue_step = sat_divs;
    float hue_shift, sat_scale, val_scale;

    if (val_divs < 2u) {
        float h_scaled = h * h_scale, s_scaled = s * s_scale;
        uint h_idx0 = min(uint(h_scaled), max_hue_idx0);
        uint s_idx0 = min(uint(s_scaled), max_sat_idx0);
        uint h_idx1 = (h_idx0 >= max_hue_idx0) ? 0u : h_idx0 + 1u;
        float h_fract = h_scaled - float(h_idx0), s_fract = s_scaled - float(s_idx0);
        uint i00 = (h_idx0 * hue_step + s_idx0) * 3u;
        uint i01 = (h_idx1 * hue_step + s_idx0) * 3u;
        uint i10 = (h_idx0 * hue_step + s_idx0 + 1u) * 3u;
        uint i11 = (h_idx1 * hue_step + s_idx0 + 1u) * 3u;
        for (uint off = 0u; off < 3u; off++) {
            float top = lut[i00+off] * (1.0-h_fract) + lut[i01+off] * h_fract;
            float bot = lut[i10+off] * (1.0-h_fract) + lut[i11+off] * h_fract;
            float val2 = top * (1.0-s_fract) + bot * s_fract;
            if (off == 0u) hue_shift = val2;
            else if (off == 1u) sat_scale = val2;
            else val_scale = val2;
        }
    } else {
        uint val_step = hue_divs * sat_divs;
        float v_scale = float(val_divs - 1u);
        uint max_val_idx0 = val_divs - 2u;
        float h_scaled = h*h_scale, s_scaled = s*s_scale, v_scaled = v*v_scale;
        uint h_idx0 = min(uint(h_scaled), max_hue_idx0);
        uint s_idx0 = min(uint(s_scaled), max_sat_idx0);
        uint v_idx0 = min(uint(v_scaled), max_val_idx0);
        uint h_idx1 = (h_idx0 >= max_hue_idx0) ? 0u : h_idx0 + 1u;
        float h_fract = h_scaled - float(h_idx0);
        float s_fract = s_scaled - float(s_idx0);
        float v_fract = v_scaled - float(v_idx0);
        uint base0 = v_idx0 * val_step, base1 = (v_idx0 + 1u) * val_step;
        for (uint off = 0u; off < 3u; off++) {
            uint i00_0 = (base0 + h_idx0*hue_step + s_idx0) * 3u + off;
            uint i01_0 = (base0 + h_idx1*hue_step + s_idx0) * 3u + off;
            uint i10_0 = (base0 + h_idx0*hue_step + s_idx0 + 1u) * 3u + off;
            uint i11_0 = (base0 + h_idx1*hue_step + s_idx0 + 1u) * 3u + off;
            float top0 = lut[i00_0]*(1.0-h_fract) + lut[i01_0]*h_fract;
            float bot0 = lut[i10_0]*(1.0-h_fract) + lut[i11_0]*h_fract;
            float plane0 = top0*(1.0-s_fract) + bot0*s_fract;
            uint i00_1 = (base1 + h_idx0*hue_step + s_idx0) * 3u + off;
            uint i01_1 = (base1 + h_idx1*hue_step + s_idx0) * 3u + off;
            uint i10_1 = (base1 + h_idx0*hue_step + s_idx0 + 1u) * 3u + off;
            uint i11_1 = (base1 + h_idx1*hue_step + s_idx0 + 1u) * 3u + off;
            float top1 = lut[i00_1]*(1.0-h_fract) + lut[i01_1]*h_fract;
            float bot1 = lut[i10_1]*(1.0-h_fract) + lut[i11_1]*h_fract;
            float plane1 = top1*(1.0-s_fract) + bot1*s_fract;
            float val2 = plane0*(1.0-v_fract) + plane1*v_fract;
            if (off == 0u) hue_shift = val2;
            else if (off == 1u) sat_scale = val2;
            else val_scale = val2;
        }
    }

    h += hue_shift * (6.0 / 360.0);
    s = min(s * sat_scale, 1.0f);
    float v_new = clamp(v * val_scale, 0.0f, 1.0f);
    return hsvToRgb(h, s, v_new);
}

kernel void DngRender(
    texture2d<float, access::read>  inTex    [[texture(0)]],
    texture2d<float, access::write> outTex   [[texture(1)]],
    texture1d<float, access::sample> toneCurve [[texture(2)]],
    constant DngColorParams& params          [[buffer(0)]],
    device const float* hsmData              [[buffer(1)]],
    device const float* ltData               [[buffer(2)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= outTex.get_width() || gid.y >= outTex.get_height()) {
        return;
    }

    float4 c = inTex.read(gid);

    // Step 1: Normalize — Stage2 data is already linearized to [s3bl_norm, 1.0].
    // range_inv.w = Stage3BlackLevel/65535 (offset to subtract).
    // range_inv.xyz = 1/(1 - offset) (scale factor, 1.0 when offset is 0).
    c.rgb = (c.rgb - params.range_inv.w) * params.range_inv.xyz;

    // Step 1b: Clip at camera white (prevents oversaturated channels from
    // producing wrong colors after the matrix multiply).
    c.rgb = min(c.rgb, params.camera_white.xyz);

    // Step 2: White balance
    c.rgb = c.rgb * params.wb_inv.xyz;

    // Steps 3+4: Camera RGB → ProPhoto RGB + clamp
    float3x3 cameraToProPhoto = float3x3(
        params.camToProPhoto_c0.xyz,
        params.camToProPhoto_c1.xyz,
        params.camToProPhoto_c2.xyz
    );
    float3 proPhoto = clamp(cameraToProPhoto * c.rgb, 0.0, 1.0);

    // Step 5: HueSatMap (if present)
    if (params.flags.x != 0u) {
        proPhoto = applyHsvLut(
            proPhoto, hsmData,
            params.hsm_dims.x, params.hsm_dims.y, params.hsm_dims.z
        );
    }

    // Step 6: Exposure ramp
    float black  = params.exposure.x;
    float slope  = params.exposure.y;
    float radius = params.exposure.z;
    float qScale = params.exposure.w;
    proPhoto.r = exposureRamp(proPhoto.r, black, slope, radius, qScale);
    proPhoto.g = exposureRamp(proPhoto.g, black, slope, radius, qScale);
    proPhoto.b = exposureRamp(proPhoto.b, black, slope, radius, qScale);

    // Step 7: Look Table (if present) — applied before tone curve per DNG spec
    if (params.flags.y != 0u) {
        proPhoto = applyHsvLut(
            proPhoto, ltData,
            params.lt_dims.x, params.lt_dims.y, params.lt_dims.z
        );
    }

    // Step 8: Hue-preserving tone curve (exposureTone + ACR3 precomposed into texture,
    // matching SDK's dng_1d_concatenate → RefBaselineRGBTone)
    constexpr sampler toneSampler(filter::linear, address::clamp_to_edge);
    proPhoto = huePreservingToneCurve(proPhoto, toneCurve, toneSampler);

    // Step 9: ProPhoto RGB → linear sRGB
    float3x3 proPhotoToSrgb = float3x3(
        params.proToSrgb_c0.xyz,
        params.proToSrgb_c1.xyz,
        params.proToSrgb_c2.xyz
    );
    float3 linearSrgb = clamp(proPhotoToSrgb * proPhoto, 0.0, 1.0);

    // Step 10: sRGB gamma
    float3 srgb = float3(
        linear_to_srgb(linearSrgb.r),
        linear_to_srgb(linearSrgb.g),
        linear_to_srgb(linearSrgb.b)
    );

    outTex.write(float4(srgb, 1.0), gid);
}
