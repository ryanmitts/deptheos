// Wavelet focus stacking kernels.
//
// After Petteri Aimonen's focus-stack: a complex Daubechies 6-tap wavelet
// (J.M. Lina 1997) of each frame's luma, winner-take-all on the detail
// coefficients, an inverse transform to a merged gray target, then each
// pixel takes the color of the frame whose luma is closest to it. Only the
// running winners are held, not every frame's coefficients.
//
// The running state is in row-major vec4f storage buffers (one 16-byte load
// and store a coefficient); complex scratch between passes is rg32float.

#import deptheos::warp::{WarpParams, warp_point, warp_lanczos}
#import deptheos::color::linear_to_srgb3

// deptheos_aligner::REC709_LUMA (a test checks they match).
const LUMA: vec3f = vec3f(0.2126, 0.7152, 0.0722);

// Complex Daubechies 6-tap coefficients.
const LOWPASS: array<vec2f, 6> = array<vec2f, 6>(
    vec2f(-0.0662912607, -0.0855816496),
    vec2f( 0.1104854346, -0.0855816496),
    vec2f( 0.6629126074,  0.1711632992),
    vec2f( 0.6629126074,  0.1711632992),
    vec2f( 0.1104854346, -0.0855816496),
    vec2f(-0.0662912607, -0.0855816496)
);

const HIGHPASS: array<vec2f, 6> = array<vec2f, 6>(
    vec2f(-0.0662912607,  0.0855816496),
    vec2f(-0.1104854346, -0.0855816496),
    vec2f( 0.6629126074, -0.1711632992),
    vec2f(-0.6629126074,  0.1711632992),
    vec2f( 0.1104854346,  0.0855816496),
    vec2f( 0.0662912607, -0.0855816496)
);

fn cmul(a: vec2f, b: vec2f) -> vec2f {
    return vec2f(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

// val · conj(coeff)
fn cmul_conj(val: vec2f, coeff: vec2f) -> vec2f {
    return vec2f(val.x * coeff.x + val.y * coeff.y, val.y * coeff.x - val.x * coeff.y);
}

// The transform's boundary: half-sample symmetric (-1 → 0, size → size - 1).
// The filters are symmetric (lowpass) and antisymmetric (highpass), so the
// coefficients mirror the same way, highpass with a sign flip
// (`mirror_sign`): no more coefficients than samples, exact reconstruction.
// Valid for overshoots up to `size`; the taps overshoot by at most 2.
fn mirror(idx: i32, size: i32) -> i32 {
    if (idx < 0) {
        return -idx - 1;
    }
    if (idx >= size) {
        return 2 * size - 1 - idx;
    }
    return idx;
}

// A highpass coefficient's sign at `idx`: −1 beyond the band's ends, 1 inside.
fn mirror_sign(idx: i32, size: i32) -> f32 {
    return select(1.0, -1.0, idx < 0 || idx >= size);
}

// The ingest's padding: mirror without repeating the edge pixel (-1 → 1,
// size → size - 2). Valid for overshoots up to size - 1, which the padding
// (under one coarsest-level block) never exceeds (`MIN_SIDE`).
fn reflect_idx(idx: i32, size: i32) -> i32 {
    if (idx < 0) {
        return -idx;
    }
    if (idx >= size) {
        return 2 * size - idx - 2;
    }
    return idx;
}

// Whether reference position `dst` lands on the frame, not the smear beyond.
fn on_frame(warp: WarpParams, dst: vec2f, src_dims: vec2f) -> bool {
    let src = warp_point(warp, dst);
    return all(src >= vec2f(-0.5)) && all(src <= src_dims - 0.5);
}

// A coefficient's center in full-resolution pixels, at `scale` pixels per
// coefficient: taps 2k−2 … 2k+3 center on 2k + ½, so cascaded it is the
// middle of the coefficient's block.
fn coeff_center(k: vec2u, scale: f32) -> vec2f {
    return vec2f(k) * scale + 0.5 * (scale - 1.0);
}

// A frame's energy weight off its footprint: data always wins, but where no
// frame has data the smear still ranks.
const OFF_FRAME: f32 = 1e-3;

fn frame_weight(warp: WarpParams, dst: vec2f, src_dims: vec2f) -> f32 {
    return select(OFF_FRAME, 1.0, on_frame(warp, dst, src_dims));
}

// One frame's sample, warped (Lanczos a=3) and exposure-corrected, in the
// source's own encoding.
fn sample_frame(src: texture_2d<f32>, warp: WarpParams, dst: vec2u) -> vec3f {
    return warp_lanczos(src, warp, dst).rgb;
}

// Luma in sRGB encoding, for linear sources too: the merge and color match
// run on perceptual values.
fn frame_luma(rgb: vec3f, apply_srgb: u32) -> f32 {
    var v = rgb;
    if (apply_srgb != 0u) {
        v = linear_to_srgb3(v);
    }
    return dot(v, LUMA);
}

// The frame's size or a level's coefficient grid.
struct LevelDims {
    width: u32,
    height: u32,
}

// =============================================================================
// INGEST — warp, luma and reflection padding into the power-of-two working
// size. Imaginary part 0.
// =============================================================================

@group(0) @binding(0) var in_src: texture_2d<f32>;
@group(0) @binding(1) var in_dst: texture_storage_2d<rg32float, write>;
@group(0) @binding(2) var<uniform> in_warp: WarpParams;
@group(0) @binding(3) var<uniform> in_dims: LevelDims; // the frame's size

@compute @workgroup_size(16, 16)
fn ingest(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(in_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let x = reflect_idx(i32(gid.x), i32(in_dims.width));
    let y = reflect_idx(i32(gid.y), i32(in_dims.height));
    let rgb = sample_frame(in_src, in_warp, vec2u(u32(x), u32(y)));
    textureStore(in_dst, gid.xy, vec4f(frame_luma(rgb, in_warp.apply_srgb), 0.0, 0.0, 0.0));
}

// =============================================================================
// FORWARD — one 1D analysis step (6 taps, `mirror`) into low and high halves.
// =============================================================================

struct WaveletDims {
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
    direction: u32, // 0 = horizontal, 1 = vertical
}

@group(0) @binding(0) var fd_src: texture_2d<f32>;
@group(0) @binding(1) var fd_dst_low: texture_storage_2d<rg32float, write>;
@group(0) @binding(2) var fd_dst_high: texture_storage_2d<rg32float, write>;
@group(0) @binding(3) var<uniform> fd_dims: WaveletDims;

@compute @workgroup_size(16, 16)
fn forward(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= fd_dims.dst_width || gid.y >= fd_dims.dst_height) {
        return;
    }
    let horizontal = fd_dims.direction == 0u;
    let center = i32(select(gid.y, gid.x, horizontal)) * 2;
    let size = i32(select(fd_dims.src_height, fd_dims.src_width, horizontal));
    var lo = vec2f(0.0);
    var hi = vec2f(0.0);
    for (var i: i32 = 0; i < 6; i++) {
        let t = u32(mirror(center - 2 + i, size));
        let v = textureLoad(fd_src, select(vec2u(gid.x, t), vec2u(t, gid.y), horizontal), 0).rg;
        lo += cmul(v, LOWPASS[i]);
        hi += cmul(v, HIGHPASS[i]);
    }
    textureStore(fd_dst_low, gid.xy, vec4f(lo, 0.0, 0.0));
    textureStore(fd_dst_high, gid.xy, vec4f(hi, 0.0, 0.0));
}

// =============================================================================
// WTA — winner-take-all on one level's three detail subbands, by energy over
// a center-weighted 3×3. Each coefficient keeps its winner as (re, im,
// energy, frame) in `best` and, with the consistency filter on, its
// runner-up in `runner` (otherwise a one-element placeholder, never read).
// =============================================================================

// Subband `s`'s (0 LH, 1 HL, 2 HH) winner at `p`: row-major, the three
// subbands side by side.
fn winner_index(p: vec2u, width: u32, s: u32) -> u32 {
    return (p.y * width + p.x) * 3u + s;
}

struct WtaParams {
    width: u32,       // this level's size
    height: u32,
    frame: f32,       // the frame's accumulation index
    runner_up: u32,   // track the runner-up (the consistency filter is on)
    scale: f32,       // full-resolution pixels per pixel of this level
    src_w: f32,       // frame size, for the footprint
    src_h: f32,
}

@group(0) @binding(0) var wta_lh: texture_2d<f32>;
@group(0) @binding(1) var wta_hl: texture_2d<f32>;
@group(0) @binding(2) var wta_hh: texture_2d<f32>;
@group(0) @binding(3) var<storage, read_write> wta_best: array<vec4f>;
@group(0) @binding(4) var<storage, read_write> wta_runner: array<vec4f>;
@group(0) @binding(5) var<uniform> wta_params: WtaParams;
@group(0) @binding(6) var<uniform> wta_warp: WarpParams;

// Center-weighted 4, 2, 1 (center, edges, corners), chosen over a flat 3×3
// and a per-coefficient pick (ADR-120). Returns (center.re, center.im,
// energy), so the caller reuses the center tap.
fn energy3x3(subband: texture_2d<f32>, p: vec2i, w: i32, h: i32, mask: f32) -> vec3f {
    var sum = 0.0;
    var center = vec2f(0.0);
    for (var dy: i32 = -1; dy <= 1; dy++) {
        for (var dx: i32 = -1; dx <= 1; dx++) {
            let q = vec2u(u32(clamp(p.x + dx, 0, w - 1)), u32(clamp(p.y + dy, 0, h - 1)));
            let c = textureLoad(subband, q, 0).rg;
            if (dx == 0 && dy == 0) {
                center = c;
            }
            let weight = f32(1 << u32(2 - abs(dx) - abs(dy)));
            sum += dot(c, c) * weight;
        }
    }
    return vec3f(center, sum * mask);
}

fn wta_subband(i: u32, coeff: vec2f, e: f32) {
    let candidate = vec4f(coeff, e, wta_params.frame);
    let best = wta_best[i];
    if (e > best.z) {
        if (wta_params.runner_up != 0u) {
            wta_runner[i] = best;
        }
        wta_best[i] = candidate;
    } else if (wta_params.runner_up != 0u && e > wta_runner[i].z) {
        wta_runner[i] = candidate;
    }
}

@compute @workgroup_size(16, 16)
fn wta(@builtin(global_invocation_id) gid: vec3u) {
    let w = i32(wta_params.width);
    let h = i32(wta_params.height);
    if (i32(gid.x) >= w || i32(gid.y) >= h) {
        return;
    }
    let p = vec2i(gid.xy);
    let center = coeff_center(gid.xy, wta_params.scale);
    let mask = frame_weight(wta_warp, center, vec2f(wta_params.src_w, wta_params.src_h));
    let lh = energy3x3(wta_lh, p, w, h, mask);
    let hl = energy3x3(wta_hl, p, w, h, mask);
    let hh = energy3x3(wta_hh, p, w, h, mask);
    let i = winner_index(gid.xy, wta_params.width, 0u);
    wta_subband(i, lh.xy, lh.z);
    wta_subband(i + 1u, hl.xy, hl.z);
    wta_subband(i + 2u, hh.xy, hh.z);
}

// The picture while pass 1 runs: per finest-level coefficient, the plane of
// its strongest subband's winner, standing in for the frame map pass 2
// decides. With no energy yet it reads as the first frame.
struct LiveParams {
    width: u32, // the finest level's width, for `winner_index`
    step: f32,  // the distance between neighboring frames' planes
}

@group(0) @binding(0) var live_dst: texture_storage_2d<r32float, write>;
@group(0) @binding(1) var<storage, read> live_best: array<vec4f>;
@group(0) @binding(2) var<uniform> live_params: LiveParams;

@compute @workgroup_size(16, 16)
fn live_planes(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(live_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let i = winner_index(gid.xy, live_params.width, 0u);
    var best = live_best[i];
    for (var s = 1u; s < 3u; s++) {
        let b = live_best[i + s];
        if (b.z > best.z) {
            best = b;
        }
    }
    textureStore(live_dst, gid.xy, vec4f(best.w * live_params.step, 0.0, 0.0, 0.0));
}

// =============================================================================
// LL — the coarsest lowpass band (mean brightness, which no frame is sharper
// in), averaged over the frames. A running (re, im, count) in two classes,
// as the pyramid stacker's coarsest level: the frames with data (count > 0)
// or, until one comes, those without (count negative).
// =============================================================================

struct LlParams {
    scale: f32,
    src_w: f32,
    src_h: f32,
    width: u32, // the band's size
    height: u32,
}

@group(0) @binding(0) var ll_curr: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> ll_state: array<vec4f>;
@group(0) @binding(2) var<uniform> ll_params: LlParams;
@group(0) @binding(3) var<uniform> ll_warp: WarpParams;

@compute @workgroup_size(16, 16)
fn ll_accumulate(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= ll_params.width || gid.y >= ll_params.height) {
        return;
    }
    let center = coeff_center(gid.xy, ll_params.scale);
    let inside = on_frame(ll_warp, center, vec2f(ll_params.src_w, ll_params.src_h));
    let c = textureLoad(ll_curr, gid.xy, 0).rg;
    let i = gid.y * ll_params.width + gid.x;
    let sum = ll_state[i];
    if (inside) {
        // The first frame with data replaces the frames without.
        let kept = select(vec3f(0.0), sum.xyz, sum.z > 0.0);
        ll_state[i] = vec4f(kept.xy + c, kept.z + 1.0, 0.0);
    } else if (sum.z <= 0.0) {
        ll_state[i] = vec4f(sum.xy + c, sum.z - 1.0, 0.0);
    }
}

// The averaged coarsest band, as complex scratch for the inverse.
@group(0) @binding(0) var lln_dst: texture_storage_2d<rg32float, write>;
@group(0) @binding(1) var<storage, read> lln_state: array<vec4f>;
@group(0) @binding(2) var<uniform> lln_dims: LevelDims;

@compute @workgroup_size(16, 16)
fn ll_normalize(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= lln_dims.width || gid.y >= lln_dims.height) {
        return;
    }
    let sum = lln_state[gid.y * lln_dims.width + gid.x];
    textureStore(lln_dst, gid.xy, vec4f(sum.xy / max(abs(sum.z), 1.0), 0.0, 0.0));
}

// =============================================================================
// CONSISTENCY — two filters on the winners' frame indices,
// applied with the runner-up since the other frames' coefficients are gone.
// =============================================================================

// A winner's frame, or -1 where it has no energy.
fn winner_frame(b: vec4f) -> f32 {
    return select(-1.0, b.w, b.z > 0.0);
}

// Whether a runner-up is frame `f`; an empty slot never is.
fn runner_is(runner: vec4f, f: f32) -> bool {
    return runner.z > 0.0 && runner.w == f;
}

@group(0) @binding(0) var<storage, read_write> sc_best: array<vec4f>;
@group(0) @binding(1) var<storage, read> sc_runner: array<vec4f>;
@group(0) @binding(2) var<uniform> sc_dims: LevelDims;

// Replace winner `i` with its runner-up when that is `voted`.
fn sc_take_runner(i: u32, voted: f32) {
    let runner = sc_runner[i];
    if (runner_is(runner, voted)) {
        sc_best[i] = runner;
    }
}

// 2-of-3 vote across LH/HL/HH: when two subbands agree on a frame, the
// third takes its runner-up if that is the agreed frame.
@compute @workgroup_size(16, 16)
fn subband_consistency(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= sc_dims.width || gid.y >= sc_dims.height) {
        return;
    }
    let i = winner_index(gid.xy, sc_dims.width, 0u);
    // Two subbands without a winner agree on -1, which no runner-up is.
    let v0 = winner_frame(sc_best[i]);
    let v1 = winner_frame(sc_best[i + 1u]);
    let v2 = winner_frame(sc_best[i + 2u]);
    if (v1 == v2 && v0 != v1) {
        sc_take_runner(i, v1);
    } else if (v0 == v2 && v1 != v0) {
        sc_take_runner(i + 1u, v0);
    } else if (v0 == v1 && v2 != v0) {
        sc_take_runner(i + 2u, v0);
    }
}

// Snapshot of the winning frames (into the free detail scratch), so the
// neighbor filter reads its neighbors before any of them changed.
@group(0) @binding(0) var snap_lh: texture_storage_2d<rg32float, write>;
@group(0) @binding(1) var snap_hl: texture_storage_2d<rg32float, write>;
@group(0) @binding(2) var snap_hh: texture_storage_2d<rg32float, write>;
@group(0) @binding(3) var<storage, read> snap_best: array<vec4f>;
@group(0) @binding(4) var<uniform> snap_dims: LevelDims;

@compute @workgroup_size(16, 16)
fn snapshot_frames(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= snap_dims.width || gid.y >= snap_dims.height) {
        return;
    }
    let i = winner_index(gid.xy, snap_dims.width, 0u);
    textureStore(snap_lh, gid.xy, vec4f(winner_frame(snap_best[i]), 0.0, 0.0, 0.0));
    textureStore(snap_hl, gid.xy, vec4f(winner_frame(snap_best[i + 1u]), 0.0, 0.0, 0.0));
    textureStore(snap_hh, gid.xy, vec4f(winner_frame(snap_best[i + 2u]), 0.0, 0.0, 0.0));
}

// A winner whose frame is above all four neighbors' or below all of them
// is an outlier: it takes its runner-up if that is the neighbors' rounded
// mean.
@group(0) @binding(0) var nc_snap_lh: texture_2d<f32>;
@group(0) @binding(1) var nc_snap_hl: texture_2d<f32>;
@group(0) @binding(2) var nc_snap_hh: texture_2d<f32>;
@group(0) @binding(3) var<storage, read_write> nc_best: array<vec4f>;
@group(0) @binding(4) var<storage, read> nc_runner: array<vec4f>;
@group(0) @binding(5) var<uniform> nc_dims: LevelDims;

// The five frames (center, left, right, top, bottom) of one subband.
fn nc_cross(t: texture_2d<f32>, p: vec2u) -> array<f32, 5> {
    return array<f32, 5>(
        textureLoad(t, p, 0).r,
        textureLoad(t, p - vec2u(1u, 0u), 0).r,
        textureLoad(t, p + vec2u(1u, 0u), 0).r,
        textureLoad(t, p - vec2u(0u, 1u), 0).r,
        textureLoad(t, p + vec2u(0u, 1u), 0).r,
    );
}

fn nc_subband(i: u32, v: array<f32, 5>) {
    if (v[0] < 0.0 || v[1] < 0.0 || v[2] < 0.0 || v[3] < 0.0 || v[4] < 0.0) {
        return;
    }
    let c = v[0];
    let above = c > v[1] && c > v[2] && c > v[3] && c > v[4];
    let below = c < v[1] && c < v[2] && c < v[3] && c < v[4];
    if (!(above || below)) {
        return;
    }
    let avg = floor((v[1] + v[2] + v[3] + v[4] + 2.0) / 4.0);
    let runner = nc_runner[i];
    if (avg != c && runner_is(runner, avg)) {
        nc_best[i] = runner;
    }
}

@compute @workgroup_size(16, 16)
fn neighbor_consistency(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x == 0u || gid.y == 0u || gid.x + 1u >= nc_dims.width || gid.y + 1u >= nc_dims.height) {
        return;
    }
    let i = winner_index(gid.xy, nc_dims.width, 0u);
    nc_subband(i, nc_cross(nc_snap_lh, gid.xy));
    nc_subband(i + 1u, nc_cross(nc_snap_hl, gid.xy));
    nc_subband(i + 2u, nc_cross(nc_snap_hh, gid.xy));
}

// =============================================================================
// DENOISE — scale each detail coefficient by (e − threshold)/e (e its
// energy, zero at or below the threshold), keeping its phase.
// =============================================================================

struct DenoiseParams {
    threshold: f32,
    width: u32,
    height: u32,
}

@group(0) @binding(0) var<storage, read_write> dn_best: array<vec4f>;
@group(0) @binding(1) var<uniform> dn_params: DenoiseParams;

@compute @workgroup_size(16, 16)
fn denoise(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= dn_params.width || gid.y >= dn_params.height) {
        return;
    }
    for (var s: u32 = 0u; s < 3u; s++) {
        let i = winner_index(gid.xy, dn_params.width, s);
        let b = dn_best[i];
        let e = dot(b.xy, b.xy);
        let k = select(0.0, (e - dn_params.threshold) / e, e > dn_params.threshold);
        dn_best[i] = vec4f(b.xy * k, b.zw);
    }
}

// =============================================================================
// INVERSE — polyphase synthesis, the forward step's adjoint: sample n takes
// coefficient k through tap n − 2k + 2 (`mirror`, `mirror_sign`). Vertical
// (LL, LH) → A and (HL, HH) → B, then horizontal (A, B) → the finer LL.
// =============================================================================

@group(0) @binding(0) var iv_ll: texture_2d<f32>;
@group(0) @binding(1) var iv_a: texture_storage_2d<rg32float, write>;
@group(0) @binding(2) var iv_b: texture_storage_2d<rg32float, write>;
@group(0) @binding(3) var<storage, read> iv_best: array<vec4f>;
@group(0) @binding(4) var<uniform> iv_dims: LevelDims; // the level's size

// Subband `s`'s merged coefficient at `p`.
fn iv_coeff(p: vec2u, s: u32) -> vec2f {
    return iv_best[winner_index(p, iv_dims.width, s)].xy;
}

@compute @workgroup_size(16, 16)
fn inverse_vertical(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= iv_dims.width || gid.y >= iv_dims.height * 2u) {
        return;
    }
    let y = i32(gid.y);
    let size = i32(iv_dims.height);
    var a = vec2f(0.0);
    var b = vec2f(0.0);
    for (var j: i32 = y % 2; j < 6; j += 2) {
        let k = (y - j + 2) / 2;
        let q = vec2u(gid.x, u32(mirror(k, size)));
        let sign = mirror_sign(k, size);
        a += cmul_conj(textureLoad(iv_ll, q, 0).rg, LOWPASS[j])
            + sign * cmul_conj(iv_coeff(q, 0u), HIGHPASS[j]);
        b += cmul_conj(iv_coeff(q, 1u), LOWPASS[j]) + sign * cmul_conj(iv_coeff(q, 2u), HIGHPASS[j]);
    }
    textureStore(iv_a, gid.xy, vec4f(a, 0.0, 0.0));
    textureStore(iv_b, gid.xy, vec4f(b, 0.0, 0.0));
}

@group(0) @binding(0) var ih_a: texture_2d<f32>;
@group(0) @binding(1) var ih_b: texture_2d<f32>;
@group(0) @binding(2) var ih_dst: texture_storage_2d<rg32float, write>;
@group(0) @binding(3) var<uniform> ih_dims: LevelDims;

@compute @workgroup_size(16, 16)
fn inverse_horizontal(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= ih_dims.width * 2u || gid.y >= ih_dims.height * 2u) {
        return;
    }
    let x = i32(gid.x);
    let size = i32(ih_dims.width);
    var sum = vec2f(0.0);
    for (var j: i32 = x % 2; j < 6; j += 2) {
        let k = (x - j + 2) / 2;
        let q = vec2u(u32(mirror(k, size)), gid.y);
        sum += cmul_conj(textureLoad(ih_a, q, 0).rg, LOWPASS[j])
            + mirror_sign(k, size) * cmul_conj(textureLoad(ih_b, q, 0).rg, HIGHPASS[j]);
    }
    textureStore(ih_dst, gid.xy, vec4f(sum, 0.0, 0.0));
}

// The merged target: the reconstruction's real part, cropped to the frame.
@group(0) @binding(0) var er_src: texture_2d<f32>;
@group(0) @binding(1) var er_dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn extract_real(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(er_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    textureStore(er_dst, gid.xy, vec4f(textureLoad(er_src, gid.xy, 0).r, 0.0, 0.0, 0.0));
}

// =============================================================================
// REASSIGN — each pixel keeps the color of the frame whose luma is closest
// to the merged target: (r, g, b, key) in `pick`, the plane in `plane`. A
// key off the footprint is offset by `OUTSIDE`, so smear never beats data.
// =============================================================================

const OUTSIDE: f32 = 1e6;
const EMPTY: f32 = 1e30;

@group(0) @binding(0) var<storage, read_write> ri_pick: array<vec4f>;
@group(0) @binding(1) var<storage, read_write> ri_plane: array<f32>;
@group(0) @binding(2) var<uniform> ri_dims: LevelDims;

@compute @workgroup_size(16, 16)
fn reassign_init(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= ri_dims.width || gid.y >= ri_dims.height) {
        return;
    }
    let i = gid.y * ri_dims.width + gid.x;
    ri_pick[i] = vec4f(0.0, 0.0, 0.0, EMPTY);
    ri_plane[i] = 0.0;
}

struct ReassignParams {
    plane: f32,
    src_w: f32,
    src_h: f32,
}

@group(0) @binding(0) var cr_src: texture_2d<f32>;
@group(0) @binding(1) var cr_merged: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> cr_pick: array<vec4f>;
@group(0) @binding(3) var<storage, read_write> cr_plane: array<f32>;
@group(0) @binding(4) var<uniform> cr_warp: WarpParams;
@group(0) @binding(5) var<uniform> cr_params: ReassignParams;

@compute @workgroup_size(16, 16)
fn reassign(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(cr_merged);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let rgb = sample_frame(cr_src, cr_warp, gid.xy);
    let target_luma = textureLoad(cr_merged, gid.xy, 0).r;
    let inside = on_frame(cr_warp, vec2f(gid.xy), vec2f(cr_params.src_w, cr_params.src_h));
    let key = abs(frame_luma(rgb, cr_warp.apply_srgb) - target_luma) + select(OUTSIDE, 0.0, inside);
    // Ties go to the earlier frame, so feed order doesn't matter.
    let i = gid.y * dims.x + gid.x;
    let best = cr_pick[i].w;
    if (key < best || (key == best && cr_params.plane < cr_plane[i])) {
        cr_pick[i] = vec4f(rgb, key);
        cr_plane[i] = cr_params.plane;
    }
}

// The result (opaque, in the source's encoding) and the frame map.
@group(0) @binding(0) var fin_result: texture_storage_2d<rgba32float, write>;
@group(0) @binding(1) var fin_planes: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var<storage, read> fin_pick: array<vec4f>;
@group(0) @binding(3) var<storage, read> fin_plane: array<f32>;

@compute @workgroup_size(16, 16)
fn finish(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(fin_result);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let i = gid.y * dims.x + gid.x;
    textureStore(fin_result, gid.xy, vec4f(clamp(fin_pick[i].rgb, vec3f(0.0), vec3f(1.0)), 1.0));
    textureStore(fin_planes, gid.xy, vec4f(fin_plane[i], 0.0, 0.0, 0.0));
}
