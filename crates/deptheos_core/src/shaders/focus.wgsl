// The depth stacker's per-frame focus metric:
//
//   gray + Keys downscale to the map grid (H, V) → Gaussian pre-blur (H, V)
//   → Weber bandpass → Gaussian post-blur (H, V)
//
// Each `*_kernel` is one pass's body, called from a 16×16 entry point in the
// including shader.

#define_import_path deptheos::focus

#import deptheos::warp::keys_weight
#import deptheos::color::linear_to_srgb3
#import deptheos::metric::{BlurParams, DARK_FLOOR, gaussian_weight}

// =============================================================================
// GRAY + KEYS DOWNSCALE — source frame → map-grid gray, in two passes.
// Equal-weight gray (not Rec.709 luma, which favors one channel's detail).
// Keys cubic (C = 0.6121) stretched by the scale ratio to anti-alias;
// out-of-image taps dropped and renormalized. Separable through `gray rows`,
// equal to the 2D downscale since each axis renormalizes on its own.
// =============================================================================

struct GrayParams {
    apply_srgb: u32,
}

// One destination pixel's Keys taps along an axis, in source pixels.
struct KeysSpan {
    center: f32,
    ratio: f32,
    first: i32,
    last: i32,
}

fn keys_span(dst: u32, src_size: u32, dst_size: u32) -> KeysSpan {
    let ratio = f32(src_size) / f32(dst_size);
    let center = (f32(dst) + 0.5) * ratio - 0.5;
    // Kernel support ±2 in destination units → ±2·ratio source pixels.
    let support = 2.0 * ratio;
    return KeysSpan(
        center,
        ratio,
        max(i32(ceil(center - support)), 0),
        min(i32(floor(center + support)), i32(src_size) - 1),
    );
}

@group(0) @binding(0) var gh_src: texture_2d<f32>;
@group(0) @binding(1) var gh_dst: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var<uniform> gh_params: GrayParams;

fn gray_of(p: vec2u) -> f32 {
    var rgb = textureLoad(gh_src, p, 0).rgb;
    if (gh_params.apply_srgb != 0u) {
        rgb = linear_to_srgb3(rgb);
    }
    return (rgb.r + rgb.g + rgb.b) / 3.0;
}

// Source → gray rows: along x only.
fn gray_downscale_h_kernel(gid: vec3u) {
    let dst_dims = textureDimensions(gh_dst);
    if (gid.x >= dst_dims.x || gid.y >= dst_dims.y) {
        return;
    }
    let span = keys_span(gid.x, textureDimensions(gh_src).x, dst_dims.x);
    var sum = 0.0;
    var wsum = 0.0;
    for (var x: i32 = span.first; x <= span.last; x++) {
        let w = keys_weight((f32(x) - span.center) / span.ratio);
        if (w == 0.0) {
            continue;
        }
        sum += gray_of(vec2u(u32(x), gid.y)) * w;
        wsum += w;
    }
    let v = select(sum / wsum, 0.0, abs(wsum) < 1e-6);
    textureStore(gh_dst, gid.xy, vec4f(v, 0.0, 0.0, 0.0));
}

@group(0) @binding(0) var gv_src: texture_2d<f32>;
@group(0) @binding(1) var gv_dst: texture_storage_2d<r32float, write>;

// Gray rows → the map grid: along y only.
fn gray_downscale_v_kernel(gid: vec3u) {
    let dst_dims = textureDimensions(gv_dst);
    if (gid.x >= dst_dims.x || gid.y >= dst_dims.y) {
        return;
    }
    let span = keys_span(gid.y, textureDimensions(gv_src).y, dst_dims.y);
    var sum = 0.0;
    var wsum = 0.0;
    for (var y: i32 = span.first; y <= span.last; y++) {
        let w = keys_weight((f32(y) - span.center) / span.ratio);
        if (w == 0.0) {
            continue;
        }
        sum += textureLoad(gv_src, vec2u(gid.x, u32(y)), 0).r * w;
        wsum += w;
    }
    let v = select(sum / wsum, 0.0, abs(wsum) < 1e-6);
    textureStore(gv_dst, gid.xy, vec4f(v, 0.0, 0.0, 0.0));
}

// =============================================================================
// SEPARABLE GAUSSIAN BLUR (edges clamped), weights from σ, normalized.
// =============================================================================

@group(0) @binding(0) var blur_src: texture_2d<f32>;
@group(0) @binding(1) var blur_dst: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var<uniform> blur_params: BlurParams;

// Tap weights computed once per workgroup. The host refuses a half-width
// past 255 (`focus::blur_params`), so the `min` never cuts a kernel short.
const MAX_BLUR_TAPS: u32 = 512u;
var<workgroup> blur_weights: array<f32, MAX_BLUR_TAPS>;

fn preload_blur_weights(r: i32, local_idx: u32) {
    let n_taps = min(u32(2 * r + 1), MAX_BLUR_TAPS);
    for (var k = local_idx; k < n_taps; k += 256u) {
        blur_weights[k] = gaussian_weight(f32(i32(k) - r), blur_params.sigma);
    }
    workgroupBarrier();
}

fn gaussian_blur_h_kernel(gid: vec3u, local_idx: u32) {
    let r = i32(blur_params.half_width);
    preload_blur_weights(r, local_idx);
    let dims = textureDimensions(blur_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let w = i32(dims.x);
    var sum = 0.0;
    var weight_sum = 0.0;
    for (var i: i32 = -r; i <= r; i++) {
        let x = clamp(i32(gid.x) + i, 0, w - 1);
        let wt = blur_weights[u32(i + r)];
        sum += textureLoad(blur_src, vec2u(u32(x), gid.y), 0).r * wt;
        weight_sum += wt;
    }
    textureStore(blur_dst, gid.xy, vec4f(sum / weight_sum, 0.0, 0.0, 0.0));
}

fn gaussian_blur_v_kernel(gid: vec3u, local_idx: u32) {
    let r = i32(blur_params.half_width);
    preload_blur_weights(r, local_idx);
    let dims = textureDimensions(blur_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let h = i32(dims.y);
    var sum = 0.0;
    var weight_sum = 0.0;
    for (var i: i32 = -r; i <= r; i++) {
        let y = clamp(i32(gid.y) + i, 0, h - 1);
        let wt = blur_weights[u32(i + r)];
        sum += textureLoad(blur_src, vec2u(gid.x, u32(y)), 0).r * wt;
        weight_sum += wt;
    }
    textureStore(blur_dst, gid.xy, vec4f(sum / weight_sum, 0.0, 0.0, 0.0));
}

// =============================================================================
// BANDPASS: |gray − blurred| over local brightness (Weber), times a gain,
// clamped to 1.
// =============================================================================

struct BandpassParams {
    amplifier: f32,
}

@group(0) @binding(0) var bp_original: texture_2d<f32>;
@group(0) @binding(1) var bp_blurred: texture_2d<f32>;
@group(0) @binding(2) var bp_output: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var<uniform> bp_params: BandpassParams;

fn bandpass_kernel(gid: vec3u) {
    let dims = textureDimensions(bp_output);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let orig = textureLoad(bp_original, gid.xy, 0).r;
    let blurred = textureLoad(bp_blurred, gid.xy, 0).r;
    let diff = abs(orig - blurred);
    let brightness = max(orig, blurred);
    let weber = diff / (DARK_FLOOR + brightness);
    let result = clamp(weber * bp_params.amplifier, 0.0, 1.0);
    textureStore(bp_output, gid.xy, vec4f(result, 0.0, 0.0, 0.0));
}
