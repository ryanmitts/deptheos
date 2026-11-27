// The reduced copy: each texel the mean of the source area it covers,
// edge texels weighted by how much of them it covers, in linear values. A
// standard stack's codes are decoded through its curves first.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var reduced: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> decode: array<f32>;
@group(0) @binding(3) var<uniform> sizes: Sizes;

const CODES: u32 = 65536u;

struct Sizes {
    // xy the source's size, zw the reduced copy's.
    size: vec4u,
    // x = 1 to decode through `decode`.
    flags: vec4u,
}

fn linear(v: vec3f) -> vec3f {
    if (sizes.flags.x == 0u) {
        return v;
    }
    let code = vec3u(round(clamp(v, vec3f(0.0), vec3f(1.0)) * 65535.0));
    return vec3f(decode[code.r], decode[CODES + code.g], decode[2u * CODES + code.b]);
}

@compute @workgroup_size(16, 16)
fn reduce(@builtin(global_invocation_id) gid: vec3u) {
    let out = sizes.size.zw;
    if (gid.x >= out.x || gid.y >= out.y) {
        return;
    }
    let step = vec2f(sizes.size.xy) / vec2f(out);
    let first = vec2f(gid.xy) * step;
    let last = min(first + step, vec2f(sizes.size.xy));
    var sum = vec3f(0.0);
    var area = 0.0;
    for (var y = u32(floor(first.y)); f32(y) < last.y; y++) {
        let h = min(f32(y + 1u), last.y) - max(f32(y), first.y);
        for (var x = u32(floor(first.x)); f32(x) < last.x; x++) {
            let w = min(f32(x + 1u), last.x) - max(f32(x), first.x);
            sum += linear(textureLoad(source, vec2i(i32(x), i32(y)), 0).rgb) * (w * h);
            area += w * h;
        }
    }
    textureStore(reduced, vec2i(gid.xy), vec4f(sum / max(area, 1e-12), 1.0));
}
