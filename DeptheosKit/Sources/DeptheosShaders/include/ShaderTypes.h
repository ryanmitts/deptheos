#ifndef SHADER_TYPES_H
#define SHADER_TYPES_H

#ifdef __METAL_VERSION__
#include <metal_stdlib>
using namespace metal;
typedef metal::float4 shader_float4;
typedef metal::uint4  shader_uint4;
#else
#include <simd/simd.h>
typedef simd_float4 shader_float4;
typedef simd_uint4  shader_uint4;
#endif

// --- Stacking ---

struct WtaParams {
    float exponent;
};

struct NormalizeParams {
    float factor;
};

struct WarpParams {
    shader_float4 col0;       // inverse affine matrix column 0
    shader_float4 col1;       // inverse affine matrix column 1
    shader_float4 col2;       // inverse affine matrix column 2
    uint32_t lanczos_a;       // Lanczos lobe count: 3 or 5
    uint32_t apply_srgb;      // 1 = apply linear→sRGB after interpolation
};

// --- Depth stacker ---

struct WarpToGrayParams {
    shader_float4 col0;
    shader_float4 col1;
    shader_float4 col2;
    uint32_t apply_srgb;
};

struct RgbToGrayParams {
    uint32_t is_linear;
};

struct BandpassDiffParams {
    float amplifier;
};

struct WtaColorUpdateParams {
    uint32_t frame_index;
    uint32_t num_frames;
    uint32_t has_warp;
    shader_float4 col0;
    shader_float4 col1;
    shader_float4 col2;
};

struct DepthWtaParams {
    uint32_t frame_index;
    uint32_t num_frames;
};

struct DTRFParams {
    float log_a;
    float sigma_s;
    float sigma_r;
    float bg_max_weight;      // max blend weight at confidence=0 (background)
    float subject_max_weight; // max blend weight at confidence=1 (subject)
    uint32_t width;
    uint32_t height;
};


struct DepthFeatherParams {
    float step_size;  // 1.0 / (num_frames - 1)
    float power;      // >1 snaps toward steps, 1 = no change
};

struct NormOpacityParams {
    float offset;
    float inv_range;
};

struct BlendIterativeParams {
    float scaled_frame_idx;
    float inter_frame_dist;
    uint32_t plane_range;     // max frame-distance for contribution (integer plane steps)
    uint32_t num_frames;
    uint32_t width;
    uint32_t height;
    uint32_t first_layer;     // 1 for the first frame (background), 0 otherwise
};

struct BlendWarpParams {
    shader_float4 col0;
    shader_float4 col1;
    shader_float4 col2;
    uint32_t lanczos_a;
    uint32_t apply_srgb;
};

// --- Wavelet stacker ---

struct WaveletDims {
    uint32_t src_width;
    uint32_t src_height;
    uint32_t dst_width;
    uint32_t dst_height;
    uint32_t direction;
};

struct MagnitudeDims {
    uint32_t width;
    uint32_t height;
};

struct ReassignParams {
    uint32_t width;
    uint32_t height;
};

// --- Focus map display ---

struct FocusColorbarParams {
    uint32_t image_width;
    uint32_t total_width;
    uint32_t total_height;
    uint32_t sidebar_x;
    uint32_t gradient_top;
    uint32_t gradient_bottom;
    uint32_t gradient_x;
    uint32_t gradient_w;
};

// --- ECC alignment ---

struct EccWarpParams {
    float inv_a, inv_b, inv_c, inv_d, inv_tx, inv_ty;
    uint32_t width;
    uint32_t height;
};

struct EccReductionParams {
    uint32_t width;
    uint32_t height;
    uint32_t pixel_count;
    uint32_t num_workgroups;
};

// --- DNG color pipeline ---

struct DngColorParams {
    shader_float4 range_inv;
    shader_float4 wb_inv;
    shader_float4 camToProPhoto_c0;
    shader_float4 camToProPhoto_c1;
    shader_float4 camToProPhoto_c2;
    shader_float4 exposure;
    shader_float4 tone;
    shader_float4 proToSrgb_c0;
    shader_float4 proToSrgb_c1;
    shader_float4 proToSrgb_c2;
    shader_float4 camera_white;
    shader_uint4  flags;
    shader_uint4  hsm_dims;
    shader_uint4  lt_dims;
};

// =============================================================================
// Shared inline functions (Metal only)
// =============================================================================

#ifdef __METAL_VERSION__

// sRGB ↔ Linear color space conversion (exact piecewise sRGB standard)

inline float srgb_to_linear(float c) {
    if (c <= 0.04045f) {
        return c / 12.92f;
    } else {
        return pow((c + 0.055f) / 1.055f, 2.4f);
    }
}

inline float linear_to_srgb(float c) {
    if (c <= 0.0031308f) {
        return c * 12.92f;
    } else {
        return 1.055f * pow(c, 1.0f / 2.4f) - 0.055f;
    }
}

// Luma conversion (Rec. 709)

constant float LUMA_R = 0.2126f;
constant float LUMA_G = 0.7152f;
constant float LUMA_B = 0.0722f;

inline float rgb_to_luma(float3 rgb) {
    return LUMA_R * rgb.r + LUMA_G * rgb.g + LUMA_B * rgb.b;
}

// Lanczos interpolation helpers

constant float LANCZOS_PI = 3.14159265358979f;

inline float lanczos_sinc(float x) {
    if (abs(x) < 1e-6f) {
        return 1.0f;
    }
    float px = LANCZOS_PI * x;
    return sin(px) / px;
}

inline float lanczos_weight(float x, float a) {
    if (abs(x) >= a) {
        return 0.0f;
    }
    return lanczos_sinc(x) * lanczos_sinc(x / a);
}

#endif /* __METAL_VERSION__ */

#endif // SHADER_TYPES_H
