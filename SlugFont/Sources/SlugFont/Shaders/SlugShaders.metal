#include <metal_stdlib>
#include "SlugShaderTypes.h"
using namespace metal;

// ---------------------------------------------------------------
// MARK: - Vertex shader (instanced quad per glyph)
// ---------------------------------------------------------------

struct VertexOut {
    float4 position [[position]];
    float2 emCoord;                           // em-space sample coordinates
    float4 glyphBandScale [[flat]];           // xy = glyph scale, zw = band scale
    uint4  bandMaxTexCoords [[flat]];         // xy = bandMax, zw = bandsTexCoord
};

vertex VertexOut slug_vertex(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    constant SlugGlyphInstance* instances [[buffer(0)]],
    constant SlugUniforms& uniforms [[buffer(1)]])
{
    // Two triangles forming a unit quad: [0,1] x [0,1]
    constexpr float2 corners[6] = {
        {0, 0}, {1, 0}, {0, 1},
        {1, 0}, {1, 1}, {0, 1}
    };
    float2 corner = corners[vid % 6];

    SlugGlyphInstance inst = instances[iid];
    float2 ndc = corner * inst.scaleBias.xy + inst.scaleBias.zw;

    VertexOut out;
    out.position = float4(ndc, 0.0, 1.0);
    // Em-space coordinate: [0..1] mapped to glyph em-space dimensions
    out.emCoord = corner;
    out.glyphBandScale = inst.glyphBandScale;
    out.bandMaxTexCoords = inst.bandMaxTexCoords;
    return out;
}

// ---------------------------------------------------------------
// MARK: - Fragment shader helpers
// ---------------------------------------------------------------

static constexpr constant float kEpsilon = 1.0 / 65536.0;

// Trace a horizontal ray against a single curve.
// Control points are relative to sample position.
// Returns signed coverage contribution.
float TraceRayCurveH(float2 p1, float2 p2, float2 p3, float pixelsPerEm)
{
    // Early-out: rightmost point is left of this pixel
    if (max(max(p1.x, p2.x), p3.x) * pixelsPerEm < -0.5) {
        return 0.0;
    }

    // Root eligibility code from sign bits of y-coordinates
    uint code = (0x2E74u >> (((p1.y > 0.0) ? 2u : 0u) +
                             ((p2.y > 0.0) ? 4u : 0u) +
                             ((p3.y > 0.0) ? 8u : 0u))) & 3u;
    if (code == 0u) {
        return 0.0;
    }

    // Solve: a*t^2 - 2*b*t + c = 0
    float2 a = p1 - p2 * 2.0 + p3;
    float2 b = p1 - p2;
    float c = p1.y;
    float ayr = 1.0 / a.y;
    float d = sqrt(max(b.y * b.y - a.y * c, 0.0));
    float t1 = (b.y - d) * ayr;
    float t2 = (b.y + d) * ayr;

    // Nearly linear: solve c - 2*b*t = 0
    if (abs(a.y) < kEpsilon) {
        t1 = t2 = c / (2.0 * b.y);
    }

    float coverage = 0.0;

    if ((code & 1u) != 0u) {
        float x1 = (a.x * t1 - b.x * 2.0) * t1 + p1.x;
        coverage += clamp(x1 * pixelsPerEm + 0.5, 0.0, 1.0);
    }

    if (code > 1u) {
        float x2 = (a.x * t2 - b.x * 2.0) * t2 + p1.x;
        coverage -= clamp(x2 * pixelsPerEm + 0.5, 0.0, 1.0);
    }

    return coverage;
}

// ---------------------------------------------------------------
// MARK: - Fragment shader
// ---------------------------------------------------------------

fragment float4 slug_fragment(
    VertexOut in [[stage_in]],
    texture2d<float, access::read> curveTex [[texture(0)]],
    texture2d<ushort, access::read> bandTex [[texture(1)]],
    constant SlugUniforms& uniforms [[buffer(0)]])
{
    float2 glyphScale = in.glyphBandScale.xy;
    float2 bandScale = in.glyphBandScale.zw;
    uint2 bandMax = in.bandMaxTexCoords.xy;
    uint2 bandsTexCoord = in.bandMaxTexCoords.zw;

    // Em-space coordinates (normalized [0,1] mapped to glyph dimensions)
    float2 texCoords = in.emCoord;

    float2 pixelsPerEm = 1.0 / fwidth(texCoords);

    // Band indices
    uint2 bandIndex = clamp(uint2(texCoords * bandScale), uint2(0u, 0u), bandMax);

    // Absolute offset into bands texture for this glyph
    uint baseOffset = uint(bandsTexCoord.y) * 4096u + uint(bandsTexCoord.x);

    // --- Horizontal band ---
    uint hBandOffset = baseOffset + bandIndex.y;
    ushort2 hBandRaw = bandTex.read(uint2(hBandOffset & 0xFFFu, hBandOffset >> 12u)).xy;
    uint hCurveCount = uint(hBandRaw.x);
    uint hCurveListOffset = uint(hBandRaw.y);

    float coverageX = 0.0;
    for (uint i = 0u; i < hCurveCount; i++) {
        uint curveRefOffset = hCurveListOffset + i;
        ushort2 curveLoc = bandTex.read(uint2(curveRefOffset & 0xFFFu, curveRefOffset >> 12u)).xy;

        float4 p12raw = curveTex.read(uint2(curveLoc));
        float2 p3raw = curveTex.read(uint2(uint(curveLoc.x) + 1u, uint(curveLoc.y))).xy;

        // Normalize curve coords and make relative to sample position
        float4 p12 = p12raw / float4(glyphScale, glyphScale) - float4(texCoords, texCoords);
        float2 p3 = p3raw / glyphScale - texCoords;

        coverageX += TraceRayCurveH(p12.xy, p12.zw, p3, pixelsPerEm.x);
    }

    // --- Vertical band ---
    uint vBandOffset = baseOffset + bandMax.y + 1u + bandIndex.x;
    ushort2 vBandRaw = bandTex.read(uint2(vBandOffset & 0xFFFu, vBandOffset >> 12u)).xy;
    uint vCurveCount = uint(vBandRaw.x);
    uint vCurveListOffset = uint(vBandRaw.y);

    float coverageY = 0.0;
    for (uint i = 0u; i < vCurveCount; i++) {
        uint curveRefOffset = vCurveListOffset + i;
        ushort2 curveLoc = bandTex.read(uint2(curveRefOffset & 0xFFFu, curveRefOffset >> 12u)).xy;

        float4 p12raw = curveTex.read(uint2(curveLoc));
        float2 p3raw = curveTex.read(uint2(uint(curveLoc.x) + 1u, uint(curveLoc.y))).xy;

        float4 p12 = p12raw / float4(glyphScale, glyphScale) - float4(texCoords, texCoords);
        float2 p3 = p3raw / glyphScale - texCoords;

        // Vertical ray: swizzle x/y
        coverageY += TraceRayCurveH(p12.yx, p12.wz, p3.yx, pixelsPerEm.y);
    }

    // Combine coverages
    coverageX = min(abs(coverageX), 1.0);
    coverageY = min(abs(coverageY), 1.0);
    float coverage = (coverageX + coverageY) * 0.5;

    return uniforms.color * coverage;
}
