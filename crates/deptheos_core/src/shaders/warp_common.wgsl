// Shared affine warp sampling for stacker shaders.
//
// Transforms map reference pixels → frame pixels, pixel j centered at j
// (the aligner's convention). Every sampler clamps the position to the edge
// pixel, so borders smear rather than go black; Lanczos and Keys then drop
// out-of-image taps and renormalize (`lanczos_sample` says why).

#define_import_path deptheos::warp

// A frame's warp uniform; `WarpParams` in transform.rs is the CPU side.
// Three matrix columns (xyz used). Not `col0`: naga_oil can't export a
// struct with a member name ending in a digit.
struct WarpParams {
    x_axis: vec4f,
    y_axis: vec4f,
    z_axis: vec4f,
    // 1 to sRGB-encode a linear source, for the passes that encode.
    apply_srgb: u32,
    // Exposure correction (1.0 = none), `apply_exposure`.
    brightness: f32,
}

fn warp_matrix(warp: WarpParams) -> mat3x3f {
    return mat3x3f(warp.x_axis.xyz, warp.y_axis.xyz, warp.z_axis.xyz);
}

// Where reference position `p` lands in the frame.
fn warp_point(warp: WarpParams, p: vec2f) -> vec2f {
    return (warp_matrix(warp) * vec3f(p, 1.0)).xy;
}

fn warp_src_pos(warp: WarpParams, dst: vec2u) -> vec2f {
    return warp_point(warp, vec2f(dst));
}

// Clamp-to-edge on all four borders, before the floor/fract split.
fn bilinear_sample(tex: texture_2d<f32>, pos: vec2f) -> vec4f {
    let dims = vec2f(textureDimensions(tex));
    let p = clamp(pos, vec2f(0.0), dims - 1.0);
    let f = floor(p);
    let fx = p.x - f.x;
    let fy = p.y - f.y;

    let max_x = i32(dims.x) - 1;
    let max_y = i32(dims.y) - 1;
    let x0 = i32(f.x);
    let y0 = i32(f.y);
    let x1 = min(x0 + 1, max_x);
    let y1 = min(y0 + 1, max_y);

    let p00 = textureLoad(tex, vec2u(u32(x0), u32(y0)), 0);
    let p10 = textureLoad(tex, vec2u(u32(x1), u32(y0)), 0);
    let p01 = textureLoad(tex, vec2u(u32(x0), u32(y1)), 0);
    let p11 = textureLoad(tex, vec2u(u32(x1), u32(y1)), 0);

    return mix(mix(p00, p10, fx), mix(p01, p11, fx), fy);
}

// The six a=3 weights at x0, x0 − 1, …, x0 − 5 (x0 in [2, 3)), from three
// sines and a cosine: taps a pixel apart let sin(π(x0 − i)) and
// sin(π(x0 − i)/3) follow by sign flips and angle subtraction.
fn lanczos3_weights(x0: f32) -> array<f32, 6> {
    let PI = 3.14159265359;
    let s1 = sin(PI * x0);
    let a = PI * x0 / 3.0;
    let sa = sin(a);
    let ca = cos(a);
    // sin(iπ/3) and cos(iπ/3), i = 0..5.
    let S = array<f32, 6>(0.0, 0.8660254, 0.8660254, 0.0, -0.8660254, -0.8660254);
    let C = array<f32, 6>(1.0, 0.5, -0.5, -1.0, -0.5, 0.5);
    var w: array<f32, 6>;
    for (var i: i32 = 0; i < 6; i++) {
        let x = x0 - f32(i);
        let sign = select(-1.0, 1.0, (i & 1) == 0);
        let s3 = sa * C[i] - ca * S[i];
        if (abs(x) < 1e-6) {
            w[i] = 1.0;
        } else if (abs(x) >= 3.0) {
            w[i] = 0.0;
        } else {
            w[i] = 3.0 * (sign * s1) * s3 / (PI * PI * x * x);
        }
    }
    return w;
}

// Lanczos-3 resampling (6 taps per axis): out-of-image taps dropped and the
// rest renormalized, output clamped to [0,1].
//
// The position is clamped to the edge first, so margins smear rather than
// go black: a black margin would darken coarse pyramid levels inside the
// boundary and show a bright band along the crop edge. The margin mask
// keeps the smear from winning pixels.
//
// Pixel j is centered at j, as in bilinear_sample, so an identity transform
// is a no-op; centering at j+0.5 would shift half a pixel against the
// aligner's transforms.
//
// Constant loop bounds and the unchecked interior path make this about 40%
// faster on Apple GPUs than a general-`a` loop.
fn lanczos_sample(tex: texture_2d<f32>, pos: vec2f) -> vec4f {
    let dims = vec2i(textureDimensions(tex));
    let src_pos = clamp(pos, vec2f(0.0), vec2f(dims) - 1.0);
    let base = vec2i(floor(src_pos)) - 2;

    let wx = lanczos3_weights(src_pos.x - f32(base.x));
    let wy = lanczos3_weights(src_pos.y - f32(base.y));

    // Away from the edges all 36 taps exist: no bounds checks.
    if (all(base >= vec2i(0)) && all(base + 5 < dims)) {
        var sum = vec4f(0.0);
        for (var dy: i32 = 0; dy < 6; dy++) {
            var row = vec4f(0.0);
            for (var dx: i32 = 0; dx < 6; dx++) {
                row += textureLoad(tex, vec2u(u32(base.x + dx), u32(base.y + dy)), 0) * wx[dx];
            }
            sum += row * wy[dy];
        }
        let total = (wx[0] + wx[1] + wx[2] + wx[3] + wx[4] + wx[5])
            * (wy[0] + wy[1] + wy[2] + wy[3] + wy[4] + wy[5]);
        return clamp(sum / total, vec4f(0.0), vec4f(1.0));
    }

    var color = vec4f(0.0);
    var total_w = 0.0;
    for (var dy: i32 = 0; dy < 6; dy++) {
        let py = base.y + dy;
        if (py < 0 || py >= dims.y) {
            continue;
        }
        for (var dx: i32 = 0; dx < 6; dx++) {
            let px = base.x + dx;
            if (px < 0 || px >= dims.x) {
                continue;
            }
            let w = wx[dx] * wy[dy];
            color += textureLoad(tex, vec2u(u32(px), u32(py)), 0) * w;
            total_w += w;
        }
    }

    if (total_w > 0.0) {
        color /= total_w;
    }
    return clamp(color, vec4f(0.0), vec4f(1.0));
}

// Keys cubic (Mitchell–Netravali at B = 0), C = 0.6121: slightly sharper
// than Catmull-Rom (C = 0.5). Support |x| < 2.
const KEYS_C: f32 = 0.6121;

fn keys_weight(x: f32) -> f32 {
    let a = abs(x);
    if (a < 1.0) {
        return (2.0 - KEYS_C) * a * a * a - (3.0 - KEYS_C) * a * a + 1.0;
    } else if (a < 2.0) {
        return -KEYS_C * a * a * a + 5.0 * KEYS_C * a * a - 8.0 * KEYS_C * a + 4.0 * KEYS_C;
    }
    return 0.0;
}

// Keys bicubic resampling (4×4 taps), with `lanczos_sample`'s conventions.
// Exact at integer positions.
fn keys_sample(tex: texture_2d<f32>, pos: vec2f) -> vec4f {
    let dims = vec2i(textureDimensions(tex));
    let src_pos = clamp(pos, vec2f(0.0), vec2f(dims) - 1.0);
    let base = vec2i(floor(src_pos)) - 1;

    var wx: array<f32, 4>;
    var wy: array<f32, 4>;
    for (var i: i32 = 0; i < 4; i++) {
        wx[i] = keys_weight(src_pos.x - f32(base.x + i));
        wy[i] = keys_weight(src_pos.y - f32(base.y + i));
    }

    var color = vec4f(0.0);
    var total_w = 0.0;

    let start_y = max(0, -base.y);
    let end_y = min(4, dims.y - base.y);
    let start_x = max(0, -base.x);
    let end_x = min(4, dims.x - base.x);
    for (var dy: i32 = start_y; dy < end_y; dy++) {
        let py = base.y + dy;
        for (var dx: i32 = start_x; dx < end_x; dx++) {
            let px = base.x + dx;
            let w = wx[dx] * wy[dy];
            color += textureLoad(tex, vec2u(u32(px), u32(py)), 0) * w;
            total_w += w;
        }
    }

    if (abs(total_w) > 1e-6) {
        color /= total_w;
    }
    return clamp(color, vec4f(0.0), vec4f(1.0));
}

// The exposure correction (`WarpParams::brightness`), in the source's own
// encoding before any sRGB step, clamped to [0, 1].
fn apply_exposure(rgb: vec3f, brightness: f32) -> vec3f {
    return clamp(rgb * brightness, vec3f(0.0), vec3f(1.0));
}

// Reference pixel `dst` of frame `src`, Lanczos-3, exposure-corrected.
fn warp_lanczos(src: texture_2d<f32>, warp: WarpParams, dst: vec2u) -> vec4f {
    let c = lanczos_sample(src, warp_src_pos(warp, dst));
    return vec4f(apply_exposure(c.rgb, warp.brightness), c.a);
}

// Margin mask: a frame's weight at reference position `dst`: zero outside
// the frame (edge smear, not data), fading to one over the last `fade`
// source pixels from its outermost pixel centers.
//
// `dst` is in full-resolution pixels; pyramid level L passes
// `level_pixel · 2^L` and `fade = 2^L`, a one-pixel fade at that level.
fn footprint_weight(warp: WarpParams, dst: vec2f, src_dims: vec2f, fade: f32) -> f32 {
    let src = warp_point(warp, dst);
    let to_far = src_dims - 1.0 - src;
    let edge_distance = min(min(src.x, src.y), min(to_far.x, to_far.y));
    return clamp(edge_distance / fade, 0.0, 1.0);
}
