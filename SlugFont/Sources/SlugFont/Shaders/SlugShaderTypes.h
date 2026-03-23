#ifndef SlugShaderTypes_h
#define SlugShaderTypes_h

#include <simd/simd.h>

// Per-glyph instance data uploaded as a vertex buffer.
struct SlugGlyphInstance {
    // xy = NDC scale (width, height), zw = NDC bias (bottom-left corner position)
    simd_float4 scaleBias;
    // xy = glyph em-space dimensions (width, height), zw = band scale (width/bandDimX, height/bandDimY)
    simd_float4 glyphBandScale;
    // x = bandMax X, y = bandMax Y, z = bandsTexCoordX, w = bandsTexCoordY
    simd_uint4 bandMaxTexCoords;
};

// Per-draw-call uniforms.
struct SlugUniforms {
    simd_float4 color;
};

#endif
