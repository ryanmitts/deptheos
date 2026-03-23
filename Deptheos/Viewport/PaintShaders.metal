#include <metal_stdlib>
using namespace metal;

// MARK: - Paint Brush Compute Kernel

/// GPU-side parameters for a single brush dab.
/// Matches the Swift `PaintParams` struct layout.
struct PaintParams {
    float center_x;
    float center_y;
    float radius;
    float hardness;
    float opacity;
    float color_tolerance;
    uint bbox_x;
    uint bbox_y;
};

/// Paint a brush dab: blend source pixels onto the paint canvas within a circular brush.
/// Dispatched over the brush bounding box only (not the full image).
/// Uses read_write access to read and write the canvas directly (no temp texture or blit needed).
kernel void paint_brush(
    texture2d<float, access::read>       source  [[texture(0)]],
    texture2d<float, access::read_write> canvas  [[texture(1)]],
    constant PaintParams&                params  [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint2 img_pos = uint2(gid.x + params.bbox_x, gid.y + params.bbox_y);
    uint img_w = canvas.get_width();
    uint img_h = canvas.get_height();
    if (img_pos.x >= img_w || img_pos.y >= img_h) return;

    float2 px = float2(float(img_pos.x) + 0.5f, float(img_pos.y) + 0.5f);
    float dist = length(px - float2(params.center_x, params.center_y));

    if (dist > params.radius) return;

    float4 dst_pixel = canvas.read(img_pos);

    // Brush falloff using squared distance (libmypaint-style).
    // hardness controls where the falloff transitions between two linear segments:
    //   rr <= hardness: gentle slope from peak
    //   rr >  hardness: steeper slope to zero at edge
    float rr = (dist * dist) / (params.radius * params.radius);
    float h = params.hardness;
    float falloff;
    if (rr <= h) {
        falloff = 1.0f - rr * (1.0f - h) / max(h, 0.001f);
    } else {
        falloff = (1.0f - rr) * h / max(1.0f - h, 0.001f);
    }
    falloff = clamp(falloff, 0.0f, 1.0f);
    float alpha = params.opacity * falloff;

    if (alpha < 0.001f) return;

    float4 src_pixel = source.read(img_pos);

    // Color tolerance check — 1.0 = paint everything, 0.0 = exact match only
    if (params.color_tolerance < 1.0f) {
        float color_dist = length(src_pixel.rgb - dst_pixel.rgb);
        if (color_dist > params.color_tolerance) return;
    }

    float3 blended = mix(dst_pixel.rgb, src_pixel.rgb, alpha);
    canvas.write(float4(clamp(blended, float3(0.0f), float3(1.0f)), 1.0f), img_pos);
}

// MARK: - Brush Cursor Overlay

/// Shared vertex type with DisplayShaders.
struct BrushCursorVertexOut {
    float4 position [[position]];
    float2 texCoord;
};

/// GPU-side uniforms for the brush cursor overlay.
struct BrushCursorUniforms {
    float2 center;        // cursor center in drawable pixels
    float radius;         // cursor radius in drawable pixels
    float hardness;       // brush hardness (0-1)
    float opacity;        // brush opacity (0-1)
    float2 uvCenter;      // UV coordinate at cursor center
    float2 texelSize;     // 1.0 / (imageSize * viewScale)
};

/// Brush cursor overlay — previews the actual paint result (source blended onto canvas)
/// inside the brush circle with hardness falloff. Outputs premultiplied alpha for compositing.
fragment float4 brush_cursor_fragment(
    BrushCursorVertexOut in [[stage_in]],
    texture2d<float> sourceTex [[texture(0)]],
    texture2d<float> canvasTex [[texture(1)]],
    constant BrushCursorUniforms& uniforms [[buffer(0)]]
) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);

    float2 delta = in.position.xy - uniforms.center;
    float dist = length(delta);

    if (dist > uniforms.radius) {
        return float4(0, 0, 0, 0);
    }

    // Sample source and canvas at cursor position (1:1 mapping)
    float2 uvOffset = delta * uniforms.texelSize;
    float2 sampleUV = uniforms.uvCenter + uvOffset;
    float4 srcColor = sourceTex.sample(s, sampleUV);
    float4 dstColor = canvasTex.sample(s, sampleUV);

    // Apply brush falloff (same algorithm as paint_brush)
    float rr = (dist * dist) / (uniforms.radius * uniforms.radius);
    float h = uniforms.hardness;
    float falloff;
    if (rr <= h) {
        falloff = 1.0f - rr * (1.0f - h) / max(h, 0.001f);
    } else {
        falloff = (1.0f - rr) * h / max(1.0f - h, 0.001f);
    }
    falloff = clamp(falloff, 0.0f, 1.0f);

    // Preview the actual paint result: source blended onto canvas
    float alpha = uniforms.opacity * falloff;
    float3 blended = mix(dstColor.rgb, srcColor.rgb, alpha);

    // Show blended result as overlay (replace canvas region)
    float previewAlpha = max(falloff * 0.6f, alpha);

    // Border ring
    float borderOuter = uniforms.radius;
    float borderInner = uniforms.radius - 2.0f;
    if (dist > borderInner) {
        float t = smoothstep(borderInner, borderOuter, dist);
        blended = mix(blended, float3(1, 1, 1), t * 0.9f);
        previewAlpha = max(previewAlpha, t * 0.9f);
    }

    // Edge anti-aliasing
    float edgeFade = smoothstep(uniforms.radius, uniforms.radius - 1.0f, dist);
    previewAlpha *= edgeFade;

    // Premultiplied alpha output
    return float4(blended * previewAlpha, previewAlpha);
}
