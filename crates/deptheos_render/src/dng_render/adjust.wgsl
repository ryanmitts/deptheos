// The adjustment stages both kinds of stack share, each a function of
// linear ProPhoto values and numbers the resolve step worked out.

#define_import_path deptheos::adjust

#import deptheos::render_constants::{PRO_TO_LMS, LMS_TO_PRO, LMS_TO_LAB, LAB_TO_LMS}
#import deptheos::render_constants::{SKIN_HUE, SKIN_PROTECTION, SKIN_WIDTH, VIBRANCE_REACH, VIBRANCE_GAIN}
#import deptheos::render_constants::{STRENGTH, SHADOW_LOW, SHADOW_HIGH, HIGHLIGHT_LOW, HIGHLIGHT_HIGH}
#import deptheos::render_constants::{TONE_TERM, CLARITY_TERM}
#import deptheos::render_constants::{CLARITY_STRENGTH, CLARITY_LIMIT, CLARITY_MIDTONES, CLARITY_FLOOR}

// Identity up to `knee`, then the gap to white closes in stops, halving
// each stop in the end and joined at slope 1: lightcraft's roll-off (MIT
// OR Apache-2.0, craft-libs b2d2a1e94 `crates/develop/src/tone.rs`,
// `rolloff_past`). A knee of 1 clamps at white.
fn shoulder(v: f32, knee: f32) -> f32 {
    if (v <= knee) {
        return v;
    }
    let gap = 1.0 - knee;
    return 1.0 - gap * gap / (gap + v - knee);
}

// The gain (`gain_knee.x`), then the local tone's gain at the pixel
// (`local`, 1 without it), then the shoulder (knee `gain_knee.y`)
// that brings what passes white back under it.
fn exposure(prophoto: vec3f, gain_knee: vec4f, local: f32) -> vec3f {
    let v = prophoto * gain_knee.x * local;
    let knee = gain_knee.y;
    return vec3f(shoulder(v.r, knee), shoulder(v.g, knee), shoulder(v.b, knee));
}

fn rise(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// The local tone's gain, as a factor, at a pixel whose planes are
// `planes`: its base (log2 of brightness over middle gray, before
// exposure), or its log brightness without Highlights and Shadows, then
// its log brightness less Clarity's filter. `amounts` holds Highlights',
// Shadows' and Clarity's strengths and exposure's stops in z; `terms` the
// flag bits of the terms that run. lightcraft's law (MIT OR Apache-2.0,
// lightcraft 5bdb4e7a6 `crates/pipeline/src/finish.rs`,
// `local_tone_delta`).
fn local_tone(planes: vec4f, amounts: vec4f, terms: u32) -> f32 {
    let at = planes.x + amounts.z;
    var stops = 0.0;
    if ((terms & TONE_TERM) != 0u) {
        let shadow = 1.0 - rise(SHADOW_LOW, SHADOW_HIGH, at);
        let highlight = rise(HIGHLIGHT_LOW, HIGHLIGHT_HIGH, at);
        stops += amounts.y * STRENGTH * shadow * sqrt(shadow) + amounts.x * STRENGTH * highlight;
    }
    if ((terms & CLARITY_TERM) != 0u) {
        let detail = clamp(planes.y, -CLARITY_LIMIT, CLARITY_LIMIT);
        let m = at / CLARITY_MIDTONES;
        let midtone = exp(-(m * m));
        stops += amounts.w * CLARITY_STRENGTH * detail * (CLARITY_FLOOR + (1.0 - CLARITY_FLOOR) * midtone);
    }
    return exp2(stops);
}

// A cube root that keeps the sign.
fn signed_cbrt(v: vec3f) -> vec3f {
    return sign(v) * pow(abs(v), vec3f(1.0 / 3.0));
}

// An angle within -π to π.
fn wrap_angle(a: f32) -> f32 {
    let tau = 6.283185307179586;
    return a - tau * floor((a + 0.5 * tau) / tau);
}

// Vibrance, then Saturation, each -1 to 1, scaling OkLab's chroma:
// lightcraft's (MIT OR Apache-2.0, craft-libs b2d2a1e94
// `crates/develop/src/colorops.rs`). Vibrance raises low chroma most, and
// less near skin's hue when raised.
fn chroma(prophoto: vec3f, vibrance: f32, saturation: f32) -> vec3f {
    let lab = LMS_TO_LAB * signed_cbrt(PRO_TO_LMS * prophoto);
    let c0 = length(lab.yz);
    if (c0 <= 0.0) {
        return prophoto;
    }
    var c = c0;
    if (vibrance != 0.0) {
        let low = 1.0 - clamp(c / VIBRANCE_REACH, 0.0, 1.0);
        var skin = 1.0;
        if (vibrance > 0.0) {
            let off = wrap_angle(atan2(lab.z, lab.y) - SKIN_HUE) / SKIN_WIDTH;
            skin = 1.0 - SKIN_PROTECTION * exp(-off * off);
        }
        c *= max(1.0 + vibrance * low * low * skin * VIBRANCE_GAIN, 0.0);
    }
    c *= max(1.0 + saturation, 0.0);
    let scaled = vec3f(lab.x, lab.yz * (c / c0));
    let lms = LAB_TO_LMS * scaled;
    return LMS_TO_PRO * (lms * lms * lms);
}
