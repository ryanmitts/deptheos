// Pack a region of an rgba32float result into rgba16unorm with the CPU's
// exact u16 rounding (pack.rs). Write-only, as wgpu grants it.

#import deptheos::quantize::quantize_rgb_u16

struct PackParams {
    // xy = top-left of the region in the source.
    origin: vec4u,
}

@group(0) @binding(0) var pack_src: texture_2d<f32>;
@group(0) @binding(1) var pack_dst: texture_storage_2d<rgba16unorm, write>;
@group(0) @binding(2) var<uniform> pack_params: PackParams;

@compute @workgroup_size(16, 16)
fn pack_u16(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(pack_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let c = textureLoad(pack_src, gid.xy + pack_params.origin.xy, 0);
    textureStore(pack_dst, gid.xy, vec4f(quantize_rgb_u16(c.rgb), c.a));
}
