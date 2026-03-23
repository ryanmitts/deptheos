#include <metal_stdlib>
using namespace metal;

#include "ShaderTypes.h"

// Gaussian kernel weights (separable 5-tap)
constant float GAUSSIAN[5] = { 0.0625f, 0.25f, 0.375f, 0.25f, 0.0625f };

// =============================================================================
// DOWNSAMPLE - Fused 5x5 Gaussian blur + 2x subsample
// Applies the outer product of the 5-tap Gaussian kernel in a single pass.
// Each output pixel samples a 5x5 neighborhood centered at (gid*2) in the source.
// =============================================================================

kernel void downsample(
    texture2d<float, access::read> src [[texture(0)]],
    texture2d<float, access::write> dst [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint dst_w = dst.get_width();
    uint dst_h = dst.get_height();
    if (gid.x >= dst_w || gid.y >= dst_h) {
        return;
    }

    int src_w = int(src.get_width());
    int src_h = int(src.get_height());
    int x = int(gid.x) * 2;
    int y = int(gid.y) * 2;

    float4 sum = float4(0.0f);
    for (int j = -2; j <= 2; j++) {
        int sy = clamp(y + j, 0, src_h - 1);
        float wy = GAUSSIAN[j + 2];
        for (int i = -2; i <= 2; i++) {
            int sx = clamp(x + i, 0, src_w - 1);
            sum += src.read(uint2(sx, sy)) * (wy * GAUSSIAN[i + 2]);
        }
    }

    dst.write(sum, gid);
}

// =============================================================================
// Parity-based Gaussian upsample from coarse texture
// Uses the 5-tap Gaussian kernel; only evaluates taps that land on even
// positions (parity-aware), then normalizes by 4x to compensate.
// =============================================================================

inline float4 gaussian_upsample(uint2 gid, texture2d<float, access::read> coarse) {
    int coarse_w = int(coarse.get_width());
    int coarse_h = int(coarse.get_height());
    int px = int(gid.x) & 1;
    int py = int(gid.y) & 1;
    int x_start = (px == 1) ? -1 : -2;
    uint x_count = (px == 1) ? 2u : 3u;
    int y_start = (py == 1) ? -1 : -2;
    uint y_count = (py == 1) ? 2u : 3u;

    float4 sum = float4(0.0f);
    for (uint jj = 0u; jj < y_count; jj++) {
        int j = y_start + int(jj) * 2;
        for (uint ii = 0u; ii < x_count; ii++) {
            int i = x_start + int(ii) * 2;
            int2 expanded_pos = int2(gid) + int2(i, j);
            int2 coarse_pos = clamp(expanded_pos / 2, int2(0), int2(coarse_w, coarse_h) - 1);
            float weight = GAUSSIAN[i + 2] * GAUSSIAN[j + 2];
            sum += coarse.read(uint2(coarse_pos)) * weight;
        }
    }
    // Parity-aware upsample sums ~0.25 of the kernel weight; multiply by 4 to normalize
    return sum * 4.0f;
}

// =============================================================================
// LAPLACIAN ADD: G[i] = L[i] + Upsample(G[i+1])  (reconstruction)
// =============================================================================

kernel void laplacian_add(
    texture2d<float, access::read> laplacian_in [[texture(0)]],
    texture2d<float, access::read> gauss_coarse_in [[texture(1)]],
    texture2d<float, access::write> gauss_fine_out [[texture(2)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint fine_w = gauss_fine_out.get_width();
    uint fine_h = gauss_fine_out.get_height();
    if (gid.x >= fine_w || gid.y >= fine_h) {
        return;
    }

    float4 upsampled = gaussian_upsample(gid, gauss_coarse_in);
    float4 lap = laplacian_in.read(gid);
    // Normalize accumulated Laplacian by total weight (alpha channel)
    // alpha >= 1.0 after any frame is processed; max(1.0) is a safety floor
    float alpha = max(lap.a, 1.0f);
    float3 normalized_lap = lap.rgb / alpha;

    float3 result = normalized_lap + upsampled.rgb;
    gauss_fine_out.write(float4(result, 1.0f), gid);
}

// =============================================================================
// FUSED LAPLACIAN + WTA
// Computes Laplacian inline (upsample from coarse, subtract from fine) then
// immediately does WTA accumulation. Eliminates temp_laplacian round-trip.
// =============================================================================

kernel void fused_laplacian_wta(
    texture2d<float, access::read> gauss_fine [[texture(0)]],
    texture2d<float, access::read> gauss_coarse [[texture(1)]],
    texture2d<float, access::read_write> best_rgb [[texture(2)]],
    texture2d<float, access::read_write> best_focus [[texture(3)]],
    constant WtaParams &wta_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint fine_w = gauss_fine.get_width();
    uint fine_h = gauss_fine.get_height();
    if (gid.x >= fine_w || gid.y >= fine_h) {
        return;
    }

    float4 upsampled = gaussian_upsample(gid, gauss_coarse);
    float4 fine = gauss_fine.read(gid);
    float4 lap_pixel = fine - upsampled;

    // Focus metric: mean absolute per-channel Laplacian
    float focus_metric = (abs(lap_pixel.r) + abs(lap_pixel.g) + abs(lap_pixel.b)) / 3.0f;

    float current_best = best_focus.read(gid).r;
    float4 acc = best_rgb.read(gid);
    float THRESHOLD = 1e-10f;

    if (focus_metric > current_best) {
        float ratio = clamp(current_best / max(focus_metric, THRESHOLD), 0.0f, 1.0f);
        float weight = pow(ratio, wta_params.exponent);
        best_rgb.write(float4(
            acc.rgb * weight + lap_pixel.rgb,
            acc.a * weight + 1.0f
        ), gid);
        best_focus.write(float4(focus_metric, 0.0f, 0.0f, 0.0f), gid);
    } else {
        float ratio = clamp(focus_metric / max(current_best, THRESHOLD), 0.0f, 1.0f);
        float weight = pow(ratio, wta_params.exponent);
        best_rgb.write(float4(
            acc.rgb + lap_pixel.rgb * weight,
            acc.a + weight
        ), gid);
    }
}

// =============================================================================
// ACCUMULATE SUM (for coarsest level - simple averaging)
// =============================================================================

kernel void accumulate_sum(
    texture2d<float, access::read> curr_rgb [[texture(0)]],
    texture2d<float, access::read_write> acc_rgb [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = curr_rgb.get_width();
    uint h = curr_rgb.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    float4 curr = curr_rgb.read(gid);
    float4 acc = acc_rgb.read(gid);
    acc_rgb.write(acc + curr, gid);
}

// =============================================================================
// NORMALIZE LEVEL (divide by frame count for averaging)
// =============================================================================

kernel void normalize_level(
    texture2d<float, access::read> src [[texture(0)]],
    texture2d<float, access::write> dst [[texture(1)]],
    constant NormalizeParams &params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = dst.get_width();
    uint h = dst.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    float4 color = src.read(gid);
    dst.write(color * params.factor, gid);
}

// =============================================================================
// FOCUS TO GRAYSCALE 8-bit (for visualization / download)
// =============================================================================

kernel void focus_to_grayscale_8bit(
    texture2d<float, access::read> focus_src [[texture(0)]],
    texture2d<float, access::write> focus_dst [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = focus_dst.get_width();
    uint h = focus_dst.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    float focus = focus_src.read(gid).r;
    float g = clamp(sqrt(max(focus, 0.0f)) * 2.0f, 0.0f, 1.0f);

    focus_dst.write(float4(g, g, g, 1.0f), gid);
}
