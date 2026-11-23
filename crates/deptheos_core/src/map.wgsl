// A stacker's frame map (r32float planes) rendered to 8-bit gray. The r8
// entry point (MAP_R8 adapters) downloads a byte per pixel; rgba8 is the
// fallback.

struct MapParams {
    // 0 box average, 1 center texel (`MapReduce`).
    reduce: u32,
}

@group(0) @binding(0) var map_src: texture_2d<f32>;
@group(0) @binding(2) var<uniform> map_params: MapParams;

fn map_load(p: vec2u) -> f32 {
    return clamp(textureLoad(map_src, p, 0).r, 0.0, 1.0);
}

// Destination pixel `dst`'s source footprint, [lo, hi).
struct Footprint {
    lo: vec2u,
    hi: vec2u,
}

fn footprint(dst: vec2u, dims: vec2u, src_dims: vec2u) -> Footprint {
    let lo = dst * src_dims / dims;
    return Footprint(lo, max((dst + 1u) * src_dims / dims, lo + 1u));
}

fn map_gray(dst: vec2u, dims: vec2u) -> f32 {
    let f = footprint(dst, dims, textureDimensions(map_src));
    if (map_params.reduce == 1u) {
        return map_load(f.lo + (f.hi - f.lo) / 2u);
    }
    var sum = 0.0;
    for (var y = f.lo.y; y < f.hi.y; y++) {
        for (var x = f.lo.x; x < f.hi.x; x++) {
            sum += map_load(vec2u(x, y));
        }
    }
    return sum / f32((f.hi.x - f.lo.x) * (f.hi.y - f.lo.y));
}

@group(0) @binding(1) var map_dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16)
fn map_to_rgba8(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(map_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let g = map_gray(gid.xy, dims);
    textureStore(map_dst, gid.xy, vec4f(g, g, g, 1.0));
}

#ifdef MAP_R8
@group(0) @binding(1) var map_dst_r8: texture_storage_2d<r8unorm, write>;

@compute @workgroup_size(16, 16)
fn map_to_r8(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(map_dst_r8);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    textureStore(map_dst_r8, gid.xy, vec4f(map_gray(gid.xy, dims), 0.0, 0.0, 1.0));
}
#endif
