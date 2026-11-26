// The adjustment stages both kinds of stack share, each a function of
// linear ProPhoto values and numbers the resolve step worked out.

#define_import_path deptheos::adjust

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
