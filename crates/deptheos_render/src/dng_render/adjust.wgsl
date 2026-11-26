// The adjustment stages both kinds of stack share, each a function of
// linear ProPhoto values and numbers the resolve step worked out.

#define_import_path deptheos::adjust

#import deptheos::render_constants::{PRO_TO_LMS, LMS_TO_PRO, LMS_TO_LAB, LAB_TO_LMS}
#import deptheos::render_constants::{SKIN_HUE, SKIN_PROTECTION, SKIN_WIDTH, VIBRANCE_REACH, VIBRANCE_GAIN}

// Identity up to `knee`, then the gap to white closes in stops, halving
// each stop in the end and joined at slope 1: lightcraft's roll-off (MIT
// OR Apache-2.0, craft-libs `crates/develop/src/tone.rs`, `rolloff_past`).
// A knee of 1 clamps at white.
fn shoulder(v: f32, knee: f32) -> f32 {
    if (v <= knee) {
        return v;
    }
    let gap = 1.0 - knee;
    return 1.0 - gap * gap / (gap + v - knee);
}

// The gain (`gain_knee.x`), then the shoulder (knee `gain_knee.y`) that
// brings what passes white back under it.
fn exposure(prophoto: vec3f, gain_knee: vec4f) -> vec3f {
    let v = prophoto * gain_knee.x;
    let knee = gain_knee.y;
    return vec3f(shoulder(v.r, knee), shoulder(v.g, knee), shoulder(v.b, knee));
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
// lightcraft's (MIT OR Apache-2.0, craft-libs
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
