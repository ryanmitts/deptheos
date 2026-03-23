#include <metal_stdlib>
using namespace metal;

// MARK: - Shared Types

struct VertexOut {
    float4 position [[position]];
    float2 texCoord;
};

struct DisplayUniforms {
    float4x4 transform;
};

// MARK: - Blit Vertex Shader

/// Fullscreen quad with transform uniform for pan/zoom.
/// Vertices are generated from vertex_id (no vertex buffer needed).
vertex VertexOut blit_vertex(
    uint vid [[vertex_id]],
    constant DisplayUniforms &uniforms [[buffer(0)]]
) {
    // Triangle strip: 0=BL, 1=BR, 2=TL, 3=TR
    float2 positions[] = {
        float2(-1, -1),
        float2( 1, -1),
        float2(-1,  1),
        float2( 1,  1),
    };

    float2 texCoords[] = {
        float2(0, 1),
        float2(1, 1),
        float2(0, 0),
        float2(1, 0),
    };

    VertexOut out;
    float4 pos = float4(positions[vid], 0, 1);
    out.position = uniforms.transform * pos;
    out.texCoord = texCoords[vid];
    return out;
}

// MARK: - Standard Blit Fragment

/// Simple texture sample — used for result display.
fragment float4 blit_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> tex [[texture(0)]]
) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);
    return tex.sample(s, in.texCoord);
}

// MARK: - Compare Fragment

struct CompareUniforms {
    float splitPosition;  // in drawable pixels
};

/// Split-screen comparison: samples source left of divider, result right.
fragment float4 compare_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> resultTex [[texture(0)]],
    texture2d<float> sourceTex [[texture(1)]],
    constant CompareUniforms &uniforms [[buffer(0)]]
) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);

    float pixelX = in.position.x;
    if (pixelX < uniforms.splitPosition) {
        return sourceTex.sample(s, in.texCoord);
    } else {
        return resultTex.sample(s, in.texCoord);
    }
}

// MARK: - Loupe Fragment

struct LoupeUniforms {
    float2 center;        // loupe center in drawable pixels
    float radius;         // loupe radius in drawable pixels
    float magnification;  // zoom factor (e.g. 3.0)
    float2 uvCenter;      // UV coordinate at the loupe center
    float2 texelSize;     // 1.0 / (imageSize * viewScale)
};

/// Loupe overlay — outputs premultiplied alpha so it composites over the existing framebuffer.
fragment float4 loupe_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> tex [[texture(0)]],
    constant LoupeUniforms &uniforms [[buffer(0)]]
) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);

    float2 delta = in.position.xy - uniforms.center;
    float dist = max(abs(delta.x), abs(delta.y));

    if (dist > uniforms.radius) {
        return float4(0, 0, 0, 0);
    }

    // Inside loupe: sample at magnified UV
    float2 uvOffset = delta * uniforms.texelSize / uniforms.magnification;
    float2 magnifiedUV = uniforms.uvCenter + uvOffset;
    float4 loupeColor = tex.sample(s, magnifiedUV);

    // Border ring
    float borderInner = uniforms.radius - 3.0;
    if (dist > borderInner) {
        float t = smoothstep(borderInner, borderInner + 1.0, dist);
        loupeColor = mix(loupeColor, float4(1, 1, 1, 1), t * 0.8);
    }

    // Edge fade
    float edgeFade = smoothstep(uniforms.radius, uniforms.radius - 1.0, dist);

    // Premultiplied alpha output
    return float4(loupeColor.rgb * edgeFade, edgeFade);
}
