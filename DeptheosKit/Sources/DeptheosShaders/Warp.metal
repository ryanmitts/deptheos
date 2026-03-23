#include <metal_stdlib>
using namespace metal;

#include "ShaderTypes.h"

kernel void warp_lanczos(
    texture2d<float, access::read> src_texture [[texture(0)]],
    texture2d<float, access::write> dst_texture [[texture(1)]],
    constant WarpParams &params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint dst_w = dst_texture.get_width();
    uint dst_h = dst_texture.get_height();
    if (gid.x >= dst_w || gid.y >= dst_h) {
        return;
    }

    // Build the 3x3 inverse matrix from padded columns
    float3x3 inv_matrix = float3x3(
        params.col0.xyz,
        params.col1.xyz,
        params.col2.xyz
    );

    // Destination pixel center (add 0.5 for pixel center)
    float2 dst_coord = float2(float(gid.x) + 0.5f, float(gid.y) + 0.5f);

    // Transform to source coordinates using inverse matrix
    float3 src_homogeneous = inv_matrix * float3(dst_coord, 1.0f);
    float2 src_pos = src_homogeneous.xy;

    int2 src_dims = int2(src_texture.get_width(), src_texture.get_height());
    int a = min(int(params.lanczos_a), 5); // max Lanczos5 (10 taps)
    int half_kernel = a - 1;
    int2 base = int2(floor(src_pos - 0.5f)) - half_kernel;
    int tap_count = a * 2;
    float a_f = float(a);

    // Precompute separable weights (max 10 taps for Lanczos5)
    float wx[10];
    float wy[10];
    for (int i = 0; i < tap_count; i++) {
        wx[i] = lanczos_weight(src_pos.x - (float(base.x + i) + 0.5f), a_f);
        wy[i] = lanczos_weight(src_pos.y - (float(base.y + i) + 0.5f), a_f);
    }

    // Accumulate weighted samples
    float4 color = float4(0.0f);
    float weight_sum = 0.0f;

    int start_y = max(0, -base.y);
    int end_y = min(tap_count, src_dims.y - base.y);
    int start_x = max(0, -base.x);
    int end_x = min(tap_count, src_dims.x - base.x);

    for (int dy = start_y; dy < end_y; dy++) {
        int py = base.y + dy;
        for (int dx = start_x; dx < end_x; dx++) {
            int px = base.x + dx;
            float w = wx[dx] * wy[dy];
            color += src_texture.read(uint2(px, py)) * w;
            weight_sum += w;
        }
    }

    if (weight_sum > 0.01f) {
        color /= weight_sum;
    } else {
        // Fallback to nearest neighbor for edge cases with very low weight
        int2 nearest = clamp(int2(floor(src_pos)), int2(0), src_dims - 1);
        color = src_texture.read(uint2(nearest));
    }

    // Clamp to valid range and ensure opaque
    color = clamp(color, float4(0.0f), float4(1.0f));
    color.a = 1.0f;

    // Optionally apply linear→sRGB gamma for fused warp+color-convert
    if (params.apply_srgb != 0) {
        color = float4(
            linear_to_srgb(color.r),
            linear_to_srgb(color.g),
            linear_to_srgb(color.b),
            color.a
        );
    }

    dst_texture.write(color, gid);
}
