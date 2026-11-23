// The pyramid and depth stackers' accumulator pyramids and the collapse's
// finish step.
//
// A level's accumulator is a row-major storage buffer of vec4f (r, g, b sums,
// weight in a): one 16-byte load and store per pixel (WebGPU has no
// read-write rgba32float texture), and writable through a plain array, where
// naga cannot store to a storage texture passed as an argument.

#define_import_path deptheos::accum

#import deptheos::color::srgb_to_linear3

// The finish step, run by the pass that writes level 0 (the result): clamp
// to [0, 1] and, with `to_linear`, decode sRGB back to linear. Other levels
// pass through. The CPU side is `FinishParams` in accum.rs.
struct FinishParams {
    finish: u32,
    // 1 when the pyramid stacker ran sRGB-encoded linear input; always 0 for
    // the depth stacker, whose blend keeps the source's encoding.
    to_linear: u32,
}

fn finish_rgb(rgb: vec3f, params: FinishParams) -> vec3f {
    if (params.finish == 0u) {
        return rgb;
    }
    // Clamp first: a negative to a fractional power is NaN.
    let clamped = clamp(rgb, vec3f(0.0), vec3f(1.0));
    return select(clamped, srgb_to_linear3(clamped), params.to_linear != 0u);
}
