// Contribution over a frame (contribution.rs). The share is the depth
// blend's own ramp around the frame's plane (`PlaneRamp`); a pyramid
// stack's map holds planes only, so there a pixel is lit or not.

#import deptheos::plane::plane_weight

struct ContributionParams {
    // As `PlaneRamp`.
    plane: f32,
    below: f32,
    above: f32,
    // `UNLIT` (plane.rs).
    unlit: f32,
    // Half a 16-bit quantization level, 0.5/65535.
    half_level: f32,
}

// The frame's share at `z`, snapped to 1 or 0 within the map's precision
// of either plane. `PlaneRamp::share` (plane.rs) is the CPU copy.
fn contribution_weight(z: f32, params: ContributionParams) -> f32 {
    let d = abs(z - params.plane);
    let span = select(params.above, params.below, z < params.plane);
    if (d <= params.half_level) {
        return 1.0;
    }
    if (d >= span - params.half_level) {
        return 0.0;
    }
    return plane_weight(z, params.plane, params.below, params.above);
}

@group(0) @binding(0) var ct_src: texture_2d<f32>;
@group(0) @binding(1) var ct_map: texture_2d<u32>;
#ifdef SOURCE_RGBA16UNORM
@group(0) @binding(2) var ct_dst: texture_storage_2d<rgba16unorm, write>;
#else
@group(0) @binding(2) var ct_dst: texture_storage_2d<rgba32float, write>;
#endif
@group(0) @binding(3) var<uniform> ct_params: ContributionParams;

@compute @workgroup_size(16, 16)
fn contribution_light(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(ct_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let color = textureLoad(ct_src, gid.xy, 0);
    // Nearest map pixel, should the sizes differ.
    let map_dims = textureDimensions(ct_map);
    let p = min(gid.xy * map_dims / dims, map_dims - 1u);
    let z = f32(textureLoad(ct_map, p, 0).r) / 65535.0;
    let w = contribution_weight(z, ct_params);
    let gray = dot(color.rgb, vec3f(0.2126, 0.7152, 0.0722));
    let lit = mix(vec3f(ct_params.unlit * gray), color.rgb, w);
    textureStore(ct_dst, gid.xy, vec4f(lit, color.a));
}
