// A standard stack's draw: stored values decoded through the profile's
// curves (a table of every 16-bit code, channel by channel), to linear
// ProPhoto, through the adjustment stages, then to the output space's
// linear RGB and encoded: through
// sRGB's curve, or back through the profile's own curves to the nearest
// code. An input linearized on the CPU (a table-based profile) is
// already linear ProPhoto.
//
// One dispatch draws one tile (`place`), as dng_render.wgsl's does.

#import deptheos::quantize::quantize_rgb_u16
#import deptheos::color::linear_to_srgb_channel
#import deptheos::render_constants::{CODES, TONE_N, OP_N, GREY, TONE_MIN_EV, TONE_MAX_EV, PRO_Y, TONE_Y, PRO_TO_2020}
#import deptheos::adjust

@group(0) @binding(0) var in_tex: texture_2d<f32>;
#ifdef SOURCE_RGBA16UNORM
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16unorm, write>;
#else
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba32float, write>;
#endif
@group(0) @binding(2) var<uniform> params: StandardParams;
@group(0) @binding(3) var<storage, read> decode: array<f32>;
@group(0) @binding(4) var<uniform> place: Placement;
// The local tone's planes over the region drawn, at output texels
// (`local_tone::Planes::at`).
@group(0) @binding(6) var local_planes: texture_2d<f32>;

struct StandardParams {
    // Column-major: the input's linear RGB to linear ProPhoto.
    to_prophoto_c0: vec4f,
    to_prophoto_c1: vec4f,
    to_prophoto_c2: vec4f,
    // Column-major: linear ProPhoto to the output space's linear RGB.
    from_prophoto_c0: vec4f,
    from_prophoto_c1: vec4f,
    from_prophoto_c2: vec4f,
    // x = 1 when the input is already linear; y = 1 to encode through the
    // profile's curves, 0 through sRGB's; z the exposure stage (1) and the
    // local tone's terms in it (adjust.wgsl); w the stages after the tone curve:
    // 1, 2 and 4 the curve's (curve.wgsl), 8 chroma (adjust.wgsl).
    flags: vec4u,
    // Exposure's gain and the shoulder's knee (adjust.wgsl).
    exposure_gain_knee: vec4f,
    // The curve's operators' weights (curve.wgsl), then Vibrance and
    // Saturation (adjust.wgsl).
    late: vec4f,
    // Highlights', Shadows' and Clarity's strengths, exposure's stops in z
    // (adjust.wgsl).
    local_tone: vec4f,    // The fine terms' numbers: Texture's strength (adjust.wgsl).
    local_detail: vec4f,
}

struct Placement {
    origins: vec4u,
    size: vec4u,
}

fn decoded(v: vec3f) -> vec3f {
    if (params.flags.x != 0u) {
        return v;
    }
    let code = vec3u(round(clamp(v, vec3f(0.0), vec3f(1.0)) * 65535.0));
    return vec3f(decode[code.r], decode[CODES + code.g], decode[2u * CODES + code.b]);
}

fn to_prophoto(linear: vec3f) -> vec3f {
    let m = mat3x3f(params.to_prophoto_c0.xyz, params.to_prophoto_c1.xyz, params.to_prophoto_c2.xyz);
    return m * linear;
}

// Clamped per channel, as every export clips.
fn from_prophoto(prophoto: vec3f) -> vec3f {
    let m = mat3x3f(
        params.from_prophoto_c0.xyz,
        params.from_prophoto_c1.xyz,
        params.from_prophoto_c2.xyz,
    );
    return clamp(m * prophoto, vec3f(0.0), vec3f(1.0));
}

// The code of `channel` whose decoded value is nearest `linear`, as a
// unorm value: a binary search of the table, which never decreases.
fn encode_code(channel: u32, linear: f32) -> f32 {
    let base = channel * CODES;
    var lo = 0u;
    var hi = CODES - 1u;
    for (var i = 0u; i < 17u; i++) {
        if (lo < hi) {
            let mid = (lo + hi) / 2u;
            if (decode[base + mid] < linear) {
                lo = mid + 1u;
            } else {
                hi = mid;
            }
        }
    }
    var code = lo;
    if (lo > 0u && linear - decode[base + lo - 1u] <= decode[base + lo] - linear) {
        code = lo - 1u;
    }
    return f32(code) / 65535.0;
}

@compute @workgroup_size(16, 16)
fn standard_render(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= place.size.x || gid.y >= place.size.y) {
        return;
    }
    let stored = textureLoad(in_tex, vec2i(gid.xy + place.origins.xy), 0).rgb;
    let at = vec2i(gid.xy + place.origins.zw);
    var prophoto = to_prophoto(decoded(stored));
    if (params.flags.z != 0u) {
        var local = 1.0;
        if (params.flags.z > 1u) {
            local = adjust::local_tone(textureLoad(local_planes, at, 0), params.local_tone, params.local_detail, params.flags.z);
        }
        prophoto = adjust::exposure(prophoto, params.exposure_gain_knee, local);
    }
    if ((params.flags.w & 7u) != 0u) {
        prophoto = curve(prophoto, params.flags.w, params.late);
    }
    if ((params.flags.w & 8u) != 0u) {
        prophoto = adjust::chroma(prophoto, params.late.z, params.late.w);
    }
    let out = from_prophoto(prophoto);
    if (params.flags.y != 0u) {
        let codes = vec3f(encode_code(0u, out.r), encode_code(1u, out.g), encode_code(2u, out.b));
        textureStore(out_tex, at, vec4f(codes, 1.0));
        return;
    }
    let srgb = vec3f(
        linear_to_srgb_channel(out.r),
        linear_to_srgb_channel(out.g),
        linear_to_srgb_channel(out.b),
    );
#ifdef SOURCE_RGBA16UNORM
    textureStore(out_tex, at, vec4f(quantize_rgb_u16(srgb), 1.0));
#else
    textureStore(out_tex, at, vec4f(srgb, 1.0));
#endif
}

@group(0) @binding(7) var brightness_out: texture_storage_2d<r32float, write>;

// What the local tone's planes are taken of: log2 of each pixel's
// brightness over middle gray before exposure (Rec. 2020's luminance), as
// lightcraft's `log_lum`.
@compute @workgroup_size(16, 16)
fn log_brightness(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= place.size.x || gid.y >= place.size.y) {
        return;
    }
    let stored = textureLoad(in_tex, vec2i(gid.xy + place.origins.xy), 0).rgb;
    let prophoto = to_prophoto(decoded(stored));
    let y = log2(max(dot(TONE_Y, prophoto), 1e-7) / GREY);
    textureStore(brightness_out, vec2i(gid.xy + place.origins.zw), vec4f(y, 0.0, 0.0, 0.0));
}
