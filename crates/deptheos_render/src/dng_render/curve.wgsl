// Contrast, Whites and Blacks (crates/deptheos_render/src/curve.rs), on
// linear ProPhoto. Appended to each shader after the binding of
// `curve_luts`: the tone map's table, then each present operator's.

// The tone map's table at scene luminance `y`: linear below its range.
fn curve_tone_at(y: f32) -> f32 {
    if (y <= 0.0) {
        return 0.0;
    }
    let ev = log2(y / GREY);
    let f = clamp((ev - TONE_MIN_EV) / (TONE_MAX_EV - TONE_MIN_EV), 0.0, 1.0) * f32(TONE_N - 1u);
    let i = min(u32(f), TONE_N - 2u);
    let t = f - f32(i);
    let v = curve_luts[i] + (curve_luts[i + 1u] - curve_luts[i]) * t;
    if (ev < TONE_MIN_EV) {
        return v * (y / (GREY * exp2(TONE_MIN_EV)));
    }
    return v;
}

// The tone map on a pixel: scaled by the map's ratio at its luminance,
// then pulled toward that luminance as far as a channel passes white.
fn curve_tone(p: vec3f) -> vec3f {
    let y = dot(p, PRO_Y);
    let o = curve_tone_at(y);
    var d = vec3f(0.0);
    if (y > 1e-9) {
        d = p * (o / y);
    }
    let top = max(d.r, max(d.g, d.b));
    if (top <= 1.0) {
        return d;
    }
    let t = clamp((top - 1.0) / max(top - o, 1e-6), 0.0, 1.0);
    return d + (vec3f(o) - d) * t;
}

// The operator's table from `base` at encoded `v`.
fn curve_op_at(base: u32, v: f32) -> f32 {
    let f = clamp(v, 0.0, 1.0) * f32(OP_N - 1u);
    let i = min(u32(f), OP_N - 2u);
    let t = f - f32(i);
    return curve_luts[base + i] + (curve_luts[base + i + 1u] - curve_luts[base + i]) * t;
}

// One linear value: above 1 it passes, below 0 it keeps its distance
// below the image of 0.
fn curve_op_channel(base: u32, x: f32) -> f32 {
    if (x > 1.0) {
        return x;
    }
    if (x < 0.0) {
        return curve_luts[base] + x;
    }
    return curve_op_at(base, linear_to_srgb_channel(x));
}

// An operator per channel, blended in linear light with the pixel scaled
// by the operator at its luminance, `weight` the per-channel share.
fn curve_op(base: u32, weight: f32, p: vec3f) -> vec3f {
    let per_channel = vec3f(
        curve_op_channel(base, p.r),
        curve_op_channel(base, p.g),
        curve_op_channel(base, p.b),
    );
    if (weight == 1.0) {
        return per_channel;
    }
    let y = dot(p, PRO_Y);
    var scaled = p;
    if (y > 1e-6 && y <= 1.0) {
        scaled = p * (curve_op_channel(base, y) / y);
    }
    return weight * per_channel + (1.0 - weight) * scaled;
}

// `stages`: 1 the tone map, 2 Contrast, 4 Blacks; `weights`: Contrast's
// and Blacks' per-channel shares.
fn curve(p: vec3f, stages: u32, weights: vec4f) -> vec3f {
    var q = p;
    var base = 0u;
    if ((stages & 1u) != 0u) {
        q = curve_tone(q);
        base = TONE_N;
    }
    if ((stages & 2u) != 0u) {
        q = curve_op(base, weights.x, q);
        base += OP_N;
    }
    if ((stages & 4u) != 0u) {
        q = curve_op(base, weights.y, q);
    }
    return q;
}
