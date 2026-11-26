// A test stage that reads neighbors: each channel's 3x3 median of the
// per-pixel result, scaled by a whole-frame input read at frame
// coordinates (`tests/integration/regions.rs`).

@group(0) @binding(0) var around: texture_2d<f32>;
#ifdef SOURCE_RGBA16UNORM
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16unorm, write>;
#else
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba32float, write>;
#endif
@group(0) @binding(2) var<uniform> place: NeighborPlace;
@group(0) @binding(3) var frame_input: texture_2d<f32>;

struct NeighborPlace {
    tile: vec4u,
    input: vec4u,
    out_frame: vec4u,
}

fn median9(values: array<f32, 9>) -> f32 {
    var a = values;
    for (var i = 0u; i < 9u; i++) {
        for (var j = 0u; j + 1u < 9u - i; j++) {
            if (a[j] > a[j + 1u]) {
                let t = a[j];
                a[j] = a[j + 1u];
                a[j + 1u] = t;
            }
        }
    }
    return a[4];
}

@compute @workgroup_size(16, 16)
fn stage(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= place.tile.z || gid.y >= place.tile.w) {
        return;
    }
    let p = vec2i(place.tile.xy + gid.xy);
    let lo = vec2i(place.input.xy);
    let hi = lo + vec2i(place.input.zw) - vec2i(1);
    var r: array<f32, 9>;
    var g: array<f32, 9>;
    var b: array<f32, 9>;
    var k = 0u;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let c = textureLoad(around, clamp(p + vec2i(dx, dy), lo, hi) - lo, 0).rgb;
            r[k] = c.r;
            g[k] = c.g;
            b[k] = c.b;
            k++;
        }
    }
    let scale = vec2i(textureDimensions(frame_input));
    let gain = textureLoad(frame_input, p * scale / vec2i(place.out_frame.zw), 0).rgb;
    let m = vec3f(median9(r), median9(g), median9(b)) * (0.5 + gain);
    textureStore(out_tex, vec2i(place.out_frame.xy + gid.xy), vec4f(clamp(m, vec3f(0.0), vec3f(1.0)), 1.0));
}
