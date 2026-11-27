// The reduced copy: each texel the mean of the source area it covers,
// edge texels weighted by how much of them it covers, taken of the linear
// values as they show and turned linear again, as the viewer's levels take
// the mean of display values (ADR-138): through sRGB's curve, or through a
// linear stack's response, a table of how a gray of each level up to the
// camera's white shows. A standard stack's codes are decoded through its
// curves first. A copy of planes takes the plain mean of all four channels.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var reduced: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> decode: array<f32>;
@group(0) @binding(3) var<uniform> sizes: Sizes;
@group(0) @binding(4) var<storage, read> response: array<f32>;

const CODES: u32 = 65536u;

struct Sizes {
    // xy the source's size, zw the reduced copy's.
    size: vec4u,
    // x = 1 to decode through `decode`; y the means: 0 through sRGB's
    // curve, 1 plain (planes), 2 through `response`, of z entries.
    flags: vec4u,
    // The camera's white per channel, which `response` ends at.
    white: vec4f,
}

fn linear(v: vec3f) -> vec3f {
    if (sizes.flags.x == 0u) {
        return v;
    }
    let code = vec3u(round(clamp(v, vec3f(0.0), vec3f(1.0)) * 65535.0));
    return vec3f(decode[code.r], decode[CODES + code.g], decode[2u * CODES + code.b]);
}

fn srgb_encode(v: vec3f) -> vec3f {
    let low = v * 12.92;
    let high = 1.055 * pow(max(v, vec3f(0.0031308)), vec3f(1.0 / 2.4)) - 0.055;
    return select(high, low, v <= vec3f(0.0031308));
}

fn srgb_decode(e: vec3f) -> vec3f {
    let low = e / 12.92;
    let high = pow((max(e, vec3f(0.04045)) + 0.055) / 1.055, vec3f(2.4));
    return select(high, low, e <= vec3f(0.04045));
}

// How level `x` (0 black, 1 the camera's white) shows, past white along
// the table's last slope.
fn shown(x: f32) -> f32 {
    let n = sizes.flags.z;
    let last = f32(n - 1u);
    let at = clamp(x, 0.0, 1.0) * last;
    let i = min(u32(at), n - 2u);
    let v = mix(response[i], response[i + 1u], at - f32(i));
    let slope = (response[n - 1u] - response[n - 2u]) * last;
    return v + max(x - 1.0, 0.0) * slope;
}

// The level that shows as `e`: `shown` inverted.
fn level(e: f32) -> f32 {
    let n = sizes.flags.z;
    let last = f32(n - 1u);
    if (e >= response[n - 1u]) {
        let slope = (response[n - 1u] - response[n - 2u]) * last;
        return 1.0 + (e - response[n - 1u]) / max(slope, 1e-12);
    }
    var lo = 0u;
    var hi = n - 1u;
    while (hi - lo > 1u) {
        let mid = (lo + hi) / 2u;
        if (response[mid] <= e) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = (e - response[lo]) / max(response[hi] - response[lo], 1e-12);
    return (f32(lo) + clamp(t, 0.0, 1.0)) / last;
}

fn encoded(v: vec3f) -> vec3f {
    if (sizes.flags.y == 2u) {
        let x = v / sizes.white.xyz;
        return vec3f(shown(x.r), shown(x.g), shown(x.b));
    }
    return srgb_encode(v);
}

fn decoded(e: vec3f) -> vec3f {
    if (sizes.flags.y == 2u) {
        return vec3f(level(e.r), level(e.g), level(e.b)) * sizes.white.xyz;
    }
    return srgb_decode(e);
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
    let plain = sizes.flags.y == 1u;
    var sum = vec4f(0.0);
    var area = 0.0;
    for (var y = u32(floor(first.y)); f32(y) < last.y; y++) {
        let h = min(f32(y + 1u), last.y) - max(f32(y), first.y);
        for (var x = u32(floor(first.x)); f32(x) < last.x; x++) {
            let w = min(f32(x + 1u), last.x) - max(f32(x), first.x);
            let texel = textureLoad(source, vec2i(i32(x), i32(y)), 0);
            var rgb = texel.rgb;
            if (!plain) {
                rgb = encoded(linear(rgb));
            }
            sum += vec4f(rgb, texel.a) * (w * h);
            area += w * h;
        }
    }
    let mean = sum / max(area, 1e-12);
    if (plain) {
        textureStore(reduced, vec2i(gid.xy), mean);
    } else {
        textureStore(reduced, vec2i(gid.xy), vec4f(decoded(mean.rgb), 1.0));
    }
}
