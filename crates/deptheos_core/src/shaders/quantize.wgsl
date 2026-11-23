#define_import_path deptheos::quantize

// Exact float -> u16 quantization, bit-identical to the CPU's
// `io::quantize_u16` (clamp to [0, 1], `* 65535 + 0.5`, truncate).
//
// Metal's fast math may fuse the multiply-add and round differently within
// an ulp of a tie, so the formula only guesses (at most one step off) and
// `u16_thresholds` (pack.rs: entry k is the smallest f32 that quantizes to k
// or above) corrects it; comparisons are exact under any math mode.
@group(1) @binding(0) var<storage, read> u16_thresholds: array<f32>;

// The u16 code as k/65535, which an rgba16unorm store keeps exactly as k.
fn quantize_u16(v: f32) -> f32 {
    let x = clamp(v, 0.0, 1.0);
    var k = min(u32(x * 65535.0 + 0.5), 65535u);
    if (x < u16_thresholds[k]) {
        k -= 1u;
    } else if (k < 65535u && x >= u16_thresholds[k + 1u]) {
        k += 1u;
    }
    return f32(k) / 65535.0;
}

fn quantize_rgb_u16(c: vec3f) -> vec3f {
    return vec3f(quantize_u16(c.r), quantize_u16(c.g), quantize_u16(c.b));
}
