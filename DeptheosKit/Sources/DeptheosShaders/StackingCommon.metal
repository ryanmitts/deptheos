#include <metal_stdlib>
using namespace metal;

#include "ShaderTypes.h"

// =============================================================================
// Texture copy
// =============================================================================

kernel void copy_texture(
    texture2d<float, access::read> src [[texture(0)]],
    texture2d<float, access::write> dst [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dst.get_width() || gid.y >= dst.get_height()) {
        return;
    }
    dst.write(src.read(gid), gid);
}

// =============================================================================
// Clear texture: fill every pixel with a constant value (compute-based clear)
// =============================================================================

kernel void clear_texture(
    texture2d<float, access::write> dst [[texture(0)]],
    constant float4 &clear_value [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dst.get_width() || gid.y >= dst.get_height()) {
        return;
    }
    dst.write(clear_value, gid);
}

// =============================================================================
// Make opaque: clamp RGB to [0,1] and force alpha = 1
// =============================================================================

kernel void make_opaque(
    texture2d<float, access::read> src [[texture(0)]],
    texture2d<float, access::write> dst [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dst.get_width() || gid.y >= dst.get_height()) {
        return;
    }
    float4 color = src.read(gid);
    dst.write(float4(clamp(color.rgb, 0.0f, 1.0f), 1.0f), gid);
}
