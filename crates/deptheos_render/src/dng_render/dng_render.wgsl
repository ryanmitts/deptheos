// DNG color render: linear camera RGB -> sRGB, in the DNG SDK renderer's
// order: normalize -> white clip -> camera->ProPhoto (+clamp; white balance
// is inside this matrix) -> HueSatMap -> exposure ramp -> LookTable ->
// hue-preserving tone curve -> ProPhoto->sRGB (+clamp) -> sRGB transfer.
//
// Not implemented: RGBTables, ProfileGainTableMap and overrange (HDR)
// profiles; no installed Adobe camera profile carries the first two.
//
// Each stage is a function, run in one fused pass with f32 values between
// them. The tone curve is a storage buffer lerped by hand, so no
// float32-filterable texture is needed. The output is the source format:
// rgba16unorm quantized exactly as the CPU would, or rgba32float.
//
// One dispatch draws one tile (`place`): source texel src_origin + p into
// output texel out_origin + p, for p within size.

#import deptheos::quantize::quantize_rgb_u16
#import deptheos::color::{linear_to_srgb_channel, srgb_to_linear_channel}

@group(0) @binding(0) var in_tex: texture_2d<f32>;
#ifdef SOURCE_RGBA16UNORM
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16unorm, write>;
#else
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba32float, write>;
#endif
@group(0) @binding(2) var<uniform> params: DngColorParams;
@group(0) @binding(3) var<storage, read> tone_curve: array<f32>;
@group(0) @binding(4) var<storage, read> hsm_lut: array<f32>;
@group(0) @binding(5) var<storage, read> look_lut: array<f32>;
@group(0) @binding(6) var<uniform> place: Placement;

struct Placement {
    // xy = the tile's first source texel, zw = its first output texel.
    origins: vec4u,
    // xy = the tile's width and height.
    size: vec4u,
}

struct DngColorParams {
    // xyz = 1/(1 - black), broadcast; w = Stage3 black level / 65535.
    range_inv: vec4f,
    // Column-major camera RGB -> ProPhoto RGB; white balance is inside it
    // (see `pack_params` in mod.rs).
    cam_to_prophoto_c0: vec4f,
    cam_to_prophoto_c1: vec4f,
    cam_to_prophoto_c2: vec4f,
    // (black, slope, radius, quadratic scale) for the exposure ramp.
    exposure: vec4f,
    // Column-major ProPhoto RGB -> linear sRGB.
    pro_to_srgb_c0: vec4f,
    pro_to_srgb_c1: vec4f,
    pro_to_srgb_c2: vec4f,
    // Max-normalized camera white (clip ceiling).
    camera_white: vec4f,
    // x = HueSatMap present, y = LookTable present.
    flags: vec4u,
    // Hue, sat and val divisions; w = 1 for an sRGB-encoded table.
    hsm_dims: vec4u,
    lt_dims: vec4u,
}

fn exposure_ramp(x: f32, black: f32, slope: f32, radius: f32, q_scale: f32) -> f32 {
    if (x <= black - radius) {
        return 0.0;
    }
    if (x >= black + radius) {
        return clamp((x - black) * slope, 0.0, 1.0);
    }
    let y = x - (black - radius);
    return q_scale * y * y;
}

// Linear interpolation of the curve, whose sample i is at i/(n-1)
// (precompose_tone_curve): sample space is v*(n-1).
fn tone_sample(v: f32) -> f32 {
    let n = i32(arrayLength(&tone_curve));
    let x = clamp(v, 0.0, 1.0) * f32(n - 1);
    let i0 = clamp(i32(floor(x)), 0, n - 1);
    let i1 = min(i0 + 1, n - 1);
    let f = clamp(x - f32(i0), 0.0, 1.0);
    return mix(tone_curve[i0], tone_curve[i1], f);
}

fn hue_preserving_tone_curve(rgb: vec3f) -> vec3f {
    let max_val = max(rgb.r, max(rgb.g, rgb.b));
    let min_val = min(rgb.r, min(rgb.g, rgb.b));
    let range = max_val - min_val;

    let mapped_max = tone_sample(max_val);
    let mapped_min = tone_sample(min_val);

    if (range < 1e-10) {
        return vec3f(mapped_max);
    }

    let scale = (mapped_max - mapped_min) / range;
    return vec3f(
        mapped_min + (rgb.r - min_val) * scale,
        mapped_min + (rgb.g - min_val) * scale,
        mapped_min + (rgb.b - min_val) * scale,
    );
}

// HSV conversion matching the DNG SDK's DNG_RGBtoHSV / DNG_HSVtoRGB
// (hue in [0, 6)).

fn rgb_to_hsv(rgb: vec3f) -> vec3f {
    let v = max(max(rgb.r, rgb.g), rgb.b);
    let mn = min(min(rgb.r, rgb.g), rgb.b);
    let gap = v - mn;
    if (gap <= 0.0) {
        return vec3f(0.0, 0.0, v);
    }
    let s = gap / v;
    var h: f32;
    if (rgb.r >= rgb.g && rgb.r >= rgb.b) {
        h = (rgb.g - rgb.b) / gap;
        if (h < 0.0) {
            h += 6.0;
        }
    } else if (rgb.g >= rgb.r && rgb.g >= rgb.b) {
        h = 2.0 + (rgb.b - rgb.r) / gap;
    } else {
        h = 4.0 + (rgb.r - rgb.g) / gap;
    }
    return vec3f(h, s, v);
}

fn hsv_to_rgb(h_in: f32, s: f32, v: f32) -> vec3f {
    if (s <= 0.0) {
        return vec3f(v);
    }
    var h = h_in % 6.0;
    if (h < 0.0) {
        h += 6.0;
    }
    let i = u32(h);
    let f = h - f32(i);
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    switch i {
        case 0u, 6u: { return vec3f(v, t, p); }
        case 1u: { return vec3f(q, v, p); }
        case 2u: { return vec3f(p, v, t); }
        case 3u: { return vec3f(p, q, v); }
        case 4u: { return vec3f(t, p, v); }
        default: { return vec3f(v, p, q); }
    }
}

// Storage buffers can't be passed to functions portably, so the two LUT
// bindings are selected by index.
fn lut_at(use_look: bool, idx: u32) -> f32 {
    if (use_look) {
        return look_lut[idx];
    }
    return hsm_lut[idx];
}

// Trilinear (bilinear for a 2.5D table, val_divs < 2) interpolation of the
// HSV delta table: entries are (hue shift degrees, sat scale, val scale),
// indexed ((v * hue_divs + h) * sat_divs + s) * 3. Hue wraps; sat/val clamp.
//
// An sRGB-encoded table (dims.w), as the SDK applies it: value is encoded
// before a 3D table's lookup (not a 2.5D one's) and decoded after in both.
fn apply_hsv_lut(rgb: vec3f, dims: vec4u, use_look: bool) -> vec3f {
    let hue_divs = dims.x;
    let sat_divs = dims.y;
    let val_divs = dims.z;
    let srgb = dims.w != 0u;
    if (hue_divs < 1u || sat_divs < 2u) {
        return rgb;
    }

    let hsv = rgb_to_hsv(rgb);
    let h = hsv.x;
    let s = hsv.y;
    var v = hsv.z;
    if (srgb && val_divs >= 2u) {
        v = linear_to_srgb_channel(clamp(v, 0.0, 1.0));
    }
    let h_scale = f32(hue_divs) / 6.0;
    let s_scale = f32(sat_divs - 1u);
    let max_hue_idx0 = hue_divs - 1u;
    let max_sat_idx0 = sat_divs - 2u;
    let hue_step = sat_divs;
    var hue_shift = 0.0;
    var sat_scale = 0.0;
    var val_scale = 0.0;

    // A 2.5D table is the 3D case with both planes plane 0 and v_fract 0.
    let has_val = val_divs >= 2u;
    let val_step = hue_divs * sat_divs;
    let h_scaled = h * h_scale;
    let s_scaled = s * s_scale;
    let v_scaled = v * f32(max(val_divs, 1u) - 1u);
    let h_idx0 = min(u32(h_scaled), max_hue_idx0);
    let s_idx0 = min(u32(s_scaled), max_sat_idx0);
    let v_idx0 = select(0u, min(u32(v_scaled), max(val_divs, 2u) - 2u), has_val);
    var h_idx1 = h_idx0 + 1u;
    if (h_idx0 >= max_hue_idx0) {
        h_idx1 = 0u;
    }
    let h_fract = h_scaled - f32(h_idx0);
    let s_fract = s_scaled - f32(s_idx0);
    let v_fract = select(0.0, v_scaled - f32(v_idx0), has_val);
    let base0 = v_idx0 * val_step;
    let base1 = select(base0, (v_idx0 + 1u) * val_step, has_val);
    for (var off = 0u; off < 3u; off++) {
        let i00_0 = (base0 + h_idx0 * hue_step + s_idx0) * 3u + off;
        let i01_0 = (base0 + h_idx1 * hue_step + s_idx0) * 3u + off;
        let i10_0 = (base0 + h_idx0 * hue_step + s_idx0 + 1u) * 3u + off;
        let i11_0 = (base0 + h_idx1 * hue_step + s_idx0 + 1u) * 3u + off;
        let top0 = lut_at(use_look, i00_0) * (1.0 - h_fract) + lut_at(use_look, i01_0) * h_fract;
        let bot0 = lut_at(use_look, i10_0) * (1.0 - h_fract) + lut_at(use_look, i11_0) * h_fract;
        let plane0 = top0 * (1.0 - s_fract) + bot0 * s_fract;
        let i00_1 = (base1 + h_idx0 * hue_step + s_idx0) * 3u + off;
        let i01_1 = (base1 + h_idx1 * hue_step + s_idx0) * 3u + off;
        let i10_1 = (base1 + h_idx0 * hue_step + s_idx0 + 1u) * 3u + off;
        let i11_1 = (base1 + h_idx1 * hue_step + s_idx0 + 1u) * 3u + off;
        let top1 = lut_at(use_look, i00_1) * (1.0 - h_fract) + lut_at(use_look, i01_1) * h_fract;
        let bot1 = lut_at(use_look, i10_1) * (1.0 - h_fract) + lut_at(use_look, i11_1) * h_fract;
        let plane1 = top1 * (1.0 - s_fract) + bot1 * s_fract;
        let val2 = plane0 * (1.0 - v_fract) + plane1 * v_fract;
        if (off == 0u) {
            hue_shift = val2;
        } else if (off == 1u) {
            sat_scale = val2;
        } else {
            val_scale = val2;
        }
    }

    let h_new = h + hue_shift * (6.0 / 360.0);
    let s_new = min(s * sat_scale, 1.0);
    var v_new = clamp(v * val_scale, 0.0, 1.0);
    if (srgb) {
        v_new = srgb_to_linear_channel(v_new);
    }
    return hsv_to_rgb(h_new, s_new, v_new);
}

// Out the Stage3 black level, then clipped at camera white so oversaturated
// channels can't skew colors through the matrix.
fn normalize_camera(camera: vec3f) -> vec3f {
    let c = (camera - params.range_inv.w) * params.range_inv.xyz;
    return min(c, params.camera_white.xyz);
}

// Camera RGB to ProPhoto (white balance is inside the matrix), clamped.
fn camera_to_prophoto(c: vec3f) -> vec3f {
    let m = mat3x3f(
        params.cam_to_prophoto_c0.xyz,
        params.cam_to_prophoto_c1.xyz,
        params.cam_to_prophoto_c2.xyz,
    );
    return clamp(m * c, vec3f(0.0), vec3f(1.0));
}

fn hue_sat_map(prophoto: vec3f) -> vec3f {
    if (params.flags.x != 0u) {
        return apply_hsv_lut(prophoto, params.hsm_dims, false);
    }
    return prophoto;
}

fn exposure(prophoto: vec3f) -> vec3f {
    let e = params.exposure;
    return vec3f(
        exposure_ramp(prophoto.r, e.x, e.y, e.z, e.w),
        exposure_ramp(prophoto.g, e.x, e.y, e.z, e.w),
        exposure_ramp(prophoto.b, e.x, e.y, e.z, e.w),
    );
}

// The LookTable comes before the tone curve, per the DNG spec.
fn look_table(prophoto: vec3f) -> vec3f {
    if (params.flags.y != 0u) {
        return apply_hsv_lut(prophoto, params.lt_dims, true);
    }
    return prophoto;
}

// ProPhoto to linear sRGB, clamped.
fn prophoto_to_srgb(prophoto: vec3f) -> vec3f {
    let m = mat3x3f(
        params.pro_to_srgb_c0.xyz,
        params.pro_to_srgb_c1.xyz,
        params.pro_to_srgb_c2.xyz,
    );
    return clamp(m * prophoto, vec3f(0.0), vec3f(1.0));
}

fn encode_srgb(linear: vec3f) -> vec3f {
    return vec3f(
        linear_to_srgb_channel(linear.r),
        linear_to_srgb_channel(linear.g),
        linear_to_srgb_channel(linear.b),
    );
}

@compute @workgroup_size(16, 16)
fn dng_render(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= place.size.x || gid.y >= place.size.y) {
        return;
    }
    let camera = textureLoad(in_tex, vec2i(gid.xy + place.origins.xy), 0).rgb;

    var prophoto = camera_to_prophoto(normalize_camera(camera));
    prophoto = hue_sat_map(prophoto);
    prophoto = exposure(prophoto);
    prophoto = look_table(prophoto);
    prophoto = hue_preserving_tone_curve(prophoto);
    let srgb = encode_srgb(prophoto_to_srgb(prophoto));

    let at = vec2i(gid.xy + place.origins.zw);
#ifdef SOURCE_RGBA16UNORM
    textureStore(out_tex, at, vec4f(quantize_rgb_u16(srgb), 1.0));
#else
    textureStore(out_tex, at, vec4f(srgb, 1.0));
#endif
}
