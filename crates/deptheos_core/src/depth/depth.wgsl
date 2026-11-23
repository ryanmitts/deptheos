// Depth focus stacking kernels.
//
// Pipeline: focus metric on the frame's own grid → WTA (warping the focus
// map as it reads it, `warped_focus`) → confidence → IIR smoothing →
// feathering → per-level pyramid blend.
//
// Scalar maps are f32: r32float textures, or row-major storage buffers
// where a pass updates in place. No read-write storage textures, which
// some Metal devices (the iOS Simulator's) lack.

#import deptheos::warp::{WarpParams, warp_src_pos, bilinear_sample, keys_sample, apply_exposure}
#import deptheos::resample::{GAUSS_HALF, DOWN_K, gauss2_up, box2_down_r}
#import deptheos::focus
#import deptheos::accum::{FinishParams, finish_rgb}
#import deptheos::plane::plane_weight

// =============================================================================
// FOCUS METRIC — the shared focus module's kernels, on the map grid.
// =============================================================================

@compute @workgroup_size(16, 16)
fn gray_downscale_h(@builtin(global_invocation_id) gid: vec3u) {
    focus::gray_downscale_h_kernel(gid);
}

@compute @workgroup_size(16, 16)
fn gray_downscale_v(@builtin(global_invocation_id) gid: vec3u) {
    focus::gray_downscale_v_kernel(gid);
}

@compute @workgroup_size(16, 16)
fn gaussian_blur_h(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(local_invocation_index) local_idx: u32,
) {
    focus::gaussian_blur_h_kernel(gid, local_idx);
}

@compute @workgroup_size(16, 16)
fn gaussian_blur_v(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(local_invocation_index) local_idx: u32,
) {
    focus::gaussian_blur_v_kernel(gid, local_idx);
}

@compute @workgroup_size(16, 16)
fn bandpass_diff(@builtin(global_invocation_id) gid: vec3u) {
    focus::bandpass_kernel(gid);
}

// =============================================================================
// BILINEAR UPSAMPLE — map-grid depth → full resolution, with the map
// grid's pixel-center mapping (x+0.5)·ratio − 0.5.
// =============================================================================

@group(0) @binding(0) var us_src: texture_2d<f32>;
@group(0) @binding(1) var us_dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn upsample_bilinear(@builtin(global_invocation_id) gid: vec3u) {
    let dst_dims = textureDimensions(us_dst);
    if (gid.x >= dst_dims.x || gid.y >= dst_dims.y) {
        return;
    }
    let src_dims = vec2f(textureDimensions(us_src));
    let ratio = src_dims / vec2f(dst_dims);
    let pos = (vec2f(gid.xy) + 0.5) * ratio - 0.5;
    let v = bilinear_sample(us_src, pos).r;
    textureStore(us_dst, gid.xy, vec4f(v, 0.0, 0.0, 0.0));
}

// =============================================================================
// DEPTH WTA ACCUMULATION — best focus, depth index, opacity mean
// =============================================================================

// The frame's plane (`FramePlane`), recorded where the frame wins.
struct DepthWtaParams {
    plane: f32,
}

// The frame's finished focus map, on its own map grid.
@group(0) @binding(0) var wta_focus: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> wta_best_focus: array<f32>;
@group(0) @binding(2) var wta_depth_index: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var<storage, read_write> wta_opacity_map: array<f32>;
@group(0) @binding(4) var<uniform> wta_params: DepthWtaParams;
// Reference map grid → the frame's map grid; the identity for a frame
// without an alignment.
@group(0) @binding(5) var<uniform> wta_warp: WarpParams;

// The frame's focus at reference map pixel `p`.
//
// Only the finished map is warped: warping the frame first would make the
// metric depend on subpixel phase, since bilinear interpolation low-passes
// more at a half-pixel than at an integer one.
//
// Bilinear, not Lanczos: ringing at edges would inflate focus and cause
// depth halos. At the identity it reproduces the map exactly.
//
// Outside [0, w−1] × [0, h−1] is zero, with no fade: a frame has no focus
// where it has no data. Clamping would smear a full-strength edge value
// across the margin into the WTA, histogram and sweeps, all before any
// crop. A fade like the pyramid's `footprint_weight` would zero every
// frame's outer pixels alike, the reference's included.
fn warped_focus(p: vec2u) -> f32 {
    let src_pos = warp_src_pos(wta_warp, p);
    let src_dims = vec2f(textureDimensions(wta_focus));
    if (any(src_pos < vec2f(0.0)) || any(src_pos > src_dims - 1.0)) {
        return 0.0;
    }
    return bilinear_sample(wta_focus, src_pos).r;
}

@compute @workgroup_size(16, 16)
fn depth_wta_accumulate(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(wta_focus);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    let i = gid.y * dims.x + gid.x;
    let new_focus = warped_focus(gid.xy);
    let max_focus = wta_best_focus[i];

    // A sum, made a mean at finalize once the stacked count is known.
    wta_opacity_map[i] = wta_opacity_map[i] + new_focus;

    // Strict `>`: a tie (flat regions, all 0) keeps the earlier frame.
    if (new_focus > max_focus) {
        wta_best_focus[i] = new_focus;
        textureStore(wta_depth_index, gid.xy, vec4f(wta_params.plane, 0.0, 0.0, 0.0));
    }
}

// =============================================================================
// CONFIDENCE: opacity contrast, its histogram and percentiles, then the
// normalized map. The four passes share one uniform, at binding 2 of each.
// =============================================================================

struct ConfidenceParams {
    // The map grid's row length, for passes with no texture to measure.
    width: u32,
    stacked_frames: u32,
    total_pixels: u32,
    // `clamp(inv_range·(x − offset), 0, 1)^gamma` after the stretch.
    gamma: f32,
}

@group(0) @binding(2) var<uniform> confidence: ConfidenceParams;

// =============================================================================
// OPACITY DIFFERENCE (best-vs-average contrast)
// High = clear winner; low = ambiguous region. Runs BEFORE normalization.
// =============================================================================

// In place on the WTA's opacity sum, against its best focus.
@group(0) @binding(0) var<storage, read_write> od_opacity: array<f32>;
@group(0) @binding(1) var<storage, read> od_best_focus: array<f32>;

@compute @workgroup_size(16, 16)
fn opacity_diff(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.y * confidence.width + gid.x;
    if (gid.x >= confidence.width || i >= arrayLength(&od_opacity)) {
        return;
    }
    let avg = od_opacity[i] / f32(max(confidence.stacked_frames, 1u));
    let best = od_best_focus[i];
    od_opacity[i] = clamp(abs(avg - best), 0.0, 1.0);
}

// =============================================================================
// HISTOGRAM (1024 bins over [0,1])
// =============================================================================

@group(0) @binding(0) var<storage, read> hist_src: array<f32>;
@group(0) @binding(1) var<storage, read_write> hist_bins: array<atomic<u32>, 1024>;

@compute @workgroup_size(16, 16)
fn histogram_accumulate(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.y * confidence.width + gid.x;
    if (gid.x >= confidence.width || i >= arrayLength(&hist_src)) {
        return;
    }
    let val = clamp(hist_src[i], 0.0, 1.0);
    let bin = min(u32(val * 1024.0), 1023u);
    atomicAdd(&hist_bins[bin], 1u);
}

// =============================================================================
// HISTOGRAM PERCENTILE → normalization params (1st/99th, single thread).
// A bin's value is its lower edge as a u16 code (bin << 6).
// =============================================================================

struct NormOpacityParams {
    offset: f32,
    inv_range: f32,
}

@group(0) @binding(0) var<storage, read_write> hp_bins: array<atomic<u32>, 1024>;
@group(0) @binding(1) var<storage, read_write> hp_norm_out: NormOpacityParams;

@compute @workgroup_size(1)
fn histogram_percentile(@builtin(global_invocation_id) gid: vec3u) {
    // At least one pixel, or grids under 100 px would skip the stretch.
    let trim_target = max(u32(f32(confidence.total_pixels) * 0.01), 1u);

    var cumulative_low = 0u;
    var p01_bin = 0u;
    for (var i = 0u; i < 1024u; i++) {
        cumulative_low += atomicLoad(&hp_bins[i]);
        if (cumulative_low >= trim_target) {
            p01_bin = i;
            break;
        }
    }

    var cumulative_high = 0u;
    var p99_bin = 1023u;
    for (var i: i32 = 1023; i >= 0; i--) {
        cumulative_high += atomicLoad(&hp_bins[u32(i)]);
        if (cumulative_high >= trim_target) {
            p99_bin = u32(i);
            break;
        }
    }

    let p01_u16 = p01_bin << 6u;
    let p99_u16 = p99_bin << 6u;

    if (p99_bin <= p01_bin) {
        // No range to stretch: a unit range would be a hard threshold.
        hp_norm_out.offset = 0.0;
        hp_norm_out.inv_range = 1.0;
        return;
    }
    hp_norm_out.offset = f32(p01_u16) / 65535.0;
    hp_norm_out.inv_range = 65535.0 / f32(p99_u16 - p01_u16);
}

// =============================================================================
// NORMALIZE OPACITY MAP (percentile stretch), from the opacity buffer into
// the texture the smoothing reads as its guide.
// =============================================================================

@group(0) @binding(0) var<storage, read> norm_opacity: array<f32>;
@group(0) @binding(1) var<storage, read_write> norm_params: NormOpacityParams;
@group(0) @binding(3) var norm_out: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn normalize_opacity(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(norm_out);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let val = norm_opacity[gid.y * dims.x + gid.x];
    var result = clamp((val - norm_params.offset) * norm_params.inv_range, 0.0, 1.0);
    if (confidence.gamma != 1.0) {
        result = pow(result, confidence.gamma);
    }
    textureStore(norm_out, gid.xy, vec4f(result, 0.0, 0.0, 0.0));
}

// =============================================================================
// SMOOTHING SWEEPS
// One thread per line, serial along it. Each sweep is its own dispatch,
// reading the last one's output as a sampled texture: reading back a
// storage write is unordered on Metal without a fence, which naga does not
// emit. `MAX_MAP_SIDE` (`map_grid`) bounds a sweep's length to stay inside
// a driver's watchdog timeout.
// =============================================================================

// Texel `k` of line `line`'s sweep, from the far end when `reverse`.
fn sweep_texel(k: u32, n: u32, line: u32, horizontal: bool, reverse: bool) -> vec2u {
    let i = select(k, n - 1u - k, reverse);
    return select(vec2u(line, i), vec2u(i, line), horizontal);
}

// =============================================================================
// CONFIDENCE-MODULATED EXPONENTIAL IIR
//
// Per axis: r[n] = k1·r[n-1] + k2·(in[n-1] + in[n]),  k1 + 2·k2 = 1
// forward and backward on the same input, the two averaged (not cascaded).
// Confidence 1 → k1_weak (σ floor ≈ 0.71), confidence 0 → k1_strong.
// =============================================================================

struct IirParams {
    k1_weak: f32,
    k1_strong: f32,
    horizontal: u32,
    backward: u32,
}

fn iir_k1(confidence: f32, k1_weak: f32, k1_strong: f32) -> f32 {
    // k1 = 1 − 2·k2 is linear, so this equals interpolating k2.
    return mix(k1_strong, k1_weak, clamp(confidence, 0.0, 1.0));
}

@group(0) @binding(0) var iir_src: texture_2d<f32>;
@group(0) @binding(1) var iir_guide: texture_2d<f32>;
@group(0) @binding(2) var iir_out: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var<uniform> iir_params: IirParams;
// The forward result, for the backward sweep; the forward one binds its input.
@group(0) @binding(4) var iir_fwd: texture_2d<f32>;

@compute @workgroup_size(64)
fn iir_sweep(@builtin(global_invocation_id) gid: vec3u) {
    let horizontal = iir_params.horizontal != 0u;
    let backward = iir_params.backward != 0u;
    let dims = textureDimensions(iir_src);
    let n = select(dims.y, dims.x, horizontal);
    let lines = select(dims.x, dims.y, horizontal);
    let line = gid.x;
    if (line >= lines) {
        return;
    }

    // Seeded with the input at its own end, so both ends behave alike.
    var acc = textureLoad(iir_src, sweep_texel(0u, n, line, horizontal, backward), 0).r;
    var prev = acc;
    for (var k: u32 = 0u; k < n; k++) {
        let p = sweep_texel(k, n, line, horizontal, backward);
        let cur = textureLoad(iir_src, p, 0).r;
        let k1 = iir_k1(textureLoad(iir_guide, p, 0).r, iir_params.k1_weak, iir_params.k1_strong);
        let k2 = (1.0 - k1) * 0.5;
        acc = k1 * acc + k2 * (prev + cur);
        prev = cur;
        var out = acc;
        if (backward) {
            out = 0.5 * (textureLoad(iir_fwd, p, 0).r + acc);
        }
        textureStore(iir_out, p, vec4f(out, 0.0, 0.0, 0.0));
    }
}

// =============================================================================
// DEPTH FEATHERING — power-curve snap toward the stacked frames' planes.
// power > 1 pulls values toward the nearer stacked frame, narrowing
// transitions.
// =============================================================================

struct DepthFeatherParams {
    step_size: f32,
    power: f32,
}

@group(0) @binding(0) var df_depth: texture_2d<f32>;
@group(0) @binding(1) var<uniform> df_params: DepthFeatherParams;
// Per step between frame i and i + 1, the stacked frames either side
// (`feather_spans`); equal ends leave the depth alone.
@group(0) @binding(2) var<storage, read> df_spans: array<vec2u>;
@group(0) @binding(3) var df_out: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn depth_feather(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(df_out);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let z = textureLoad(df_depth, gid.xy, 0).r;
    textureStore(df_out, gid.xy, vec4f(feather_depth(z), 0.0, 0.0, 0.0));
}

// The feathered value of depth `z`; `z` itself where there is nothing to
// pull toward.
fn feather_depth(z: f32) -> f32 {
    let step = df_params.step_size;
    if (step <= 0.0) {
        return z;
    }

    let x = z / step; // in frames
    let span = df_spans[u32(clamp(floor(x), 0.0, f32(arrayLength(&df_spans) - 1u)))];
    if (span.y <= span.x) {
        return z;
    }
    let half_width = f32(span.y - span.x) * 0.5;
    let nearest = f32(select(span.x, span.y, x - f32(span.x) >= half_width)) * step;
    let half_step = half_width * step;
    let dist = (z - nearest) / half_step; // normalized [-1, 1]
    let s = select(-1.0, 1.0, dist >= 0.0);
    let abs_dist = clamp(abs(dist), 0.0, 1.0);
    let feathered = pow(abs_dist, df_params.power);
    return clamp(nearest + s * half_step * feathered, 0.0, 1.0);
}

// =============================================================================
// PASS 2 — PYRAMID BLEND
//
// Per pixel the depth picks the two bracketing frames and their mix
// (`1 − |z − frame|/step`), applied at every Laplacian level against a
// box-averaged depth pyramid, so coarse content blends across depth edges.
//
// The Gaussian σ = 0.666 2:1 resampler drops taps outside the image and
// renormalizes, unlike pyramid.wgsl, which clamps: on purpose (ADR-120).
// Accumulators are weighted sums, normalized as the collapse reads them.
// =============================================================================

// Warp the uploaded frame into reference geometry — the pyramid base.
// Keys bicubic (C = 0.6121): one under/overshoot per edge, not Lanczos-3's
// side lobes. Exact at integer positions, so the unaligned path uses the
// identity. No sRGB step: the composite keeps the source's encoding.
@group(0) @binding(0) var bw_src: texture_2d<f32>;
@group(0) @binding(1) var bw_dst: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> bw_params: WarpParams;

@compute @workgroup_size(16, 16)
fn blend_warp(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(bw_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let color = keys_sample(
        bw_src,
        warp_src_pos(bw_params, gid.xy),
    );
    let corrected = vec4f(apply_exposure(color.rgb, bw_params.brightness), color.a);
    textureStore(bw_dst, gid.xy, corrected);
}

// Frame pyramid: G[L+1] = the Gaussian σ = 0.666 2:1 downsample of G[L]
// (the shared resample module's taps), tiled.
//
// An 8×8 workgroup loads its 22×22 source region (7.7 KB, inside WebGPU's
// 16 KB) once: 484 reads for 64 outputs instead of 64 each. Out-of-image
// taps are dropped and renormalized per axis. Dispatched in groups of
// `G2_GROUP` (`dispatch_compute_with(.., 8)`).
@group(0) @binding(0) var g2_src: texture_2d<f32>;
@group(0) @binding(1) var g2_dst: texture_storage_2d<rgba32float, write>;

const G2_GROUP: u32 = 8u;
const G2_TILE: u32 = 22u; // 2 · G2_GROUP + 6
var<workgroup> g2_tile: array<vec4f, 484>; // G2_TILE²

@compute @workgroup_size(8, 8)
fn gauss_down2(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(workgroup_id) wg: vec3u,
    @builtin(local_invocation_index) li: u32,
) {
    let n = vec2i(textureDimensions(g2_src));
    // Source coordinate of tile[0]: the first tap of the group's first output.
    let origin = 2 * vec2i(wg.xy * G2_GROUP) - 3;
    for (var k = li; k < G2_TILE * G2_TILE; k += G2_GROUP * G2_GROUP) {
        let p = origin + vec2i(i32(k % G2_TILE), i32(k / G2_TILE));
        var v = vec4f(0.0); // outside the image: never read (zero weight)
        if (all(p >= vec2i(0)) && all(p < n)) {
            v = textureLoad(g2_src, vec2u(p), 0);
        }
        g2_tile[k] = v;
    }
    // Every thread, in range or not, must reach the barrier.
    workgroupBarrier();

    let dims = textureDimensions(g2_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let first = 2 * vec2i(gid.xy) - 3;
    var wx: array<f32, 8>;
    var wy: array<f32, 8>;
    var sx = 0.0;
    var sy = 0.0;
    for (var j: i32 = 0; j < 8; j++) {
        let g = GAUSS_HALF[DOWN_K[j]];
        let x = first.x + j;
        let y = first.y + j;
        wx[j] = select(0.0, g, x >= 0 && x < n.x);
        wy[j] = select(0.0, g, y >= 0 && y < n.y);
        sx += wx[j];
        sy += wy[j];
    }
    if (sx <= 0.0 || sy <= 0.0) {
        // No surviving tap (sub-2px image): nearest at floor(center).
        let p = clamp(2 * vec2i(gid.xy), vec2i(0), n - 1);
        textureStore(g2_dst, gid.xy, textureLoad(g2_src, vec2u(p), 0));
        return;
    }

    let t = first - origin; // this output's first tap inside the tile
    var acc = vec4f(0.0);
    for (var j: i32 = 0; j < 8; j++) {
        if (wy[j] == 0.0) {
            continue;
        }
        let row = u32(t.y + j) * G2_TILE;
        for (var i: i32 = 0; i < 8; i++) {
            if (wx[i] == 0.0) {
                continue;
            }
            acc += g2_tile[row + u32(t.x + i)] * (wx[i] * wy[j]);
        }
    }
    textureStore(g2_dst, gid.xy, acc / (sx * sy));
}

// Depth pyramid: D[L+1] = box average of D[L] (built once in begin_blend).
@group(0) @binding(0) var bd_src: texture_2d<f32>;
@group(0) @binding(1) var bd_dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn box_down2_r(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(bd_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    textureStore(bd_dst, gid.xy, vec4f(box2_down_r(bd_src, vec2i(gid.xy)), 0.0, 0.0, 0.0));
}

struct BlendParams {
    scaled_frame_idx: f32, // this frame's plane, frame_index / (N − 1)
    dist_below: f32,       // to the next stacked frame's plane below
    dist_above: f32,       // and above: 1 / (N − 1) unless a frame was left out
    _pad: f32,
}

// The frame's weight at depth `z` (`deptheos::plane`).
fn blend_weight(z: f32, params: BlendParams) -> f32 {
    return plane_weight(z, params.scaled_frame_idx, params.dist_below, params.dist_above);
}

// Laplacian level: acc[L] += (G[L] − up(G[L+1])) · w, alpha += w.
@group(0) @binding(0) var bl_fine: texture_2d<f32>;
@group(0) @binding(1) var bl_coarse: texture_2d<f32>;
@group(0) @binding(2) var bl_depth: texture_2d<f32>;
// The level's weighted sums (r, g, b) and weight (a), row-major at its
// size (`deptheos::accum`).
@group(0) @binding(3) var<storage, read_write> bl_acc: array<vec4f>;
@group(0) @binding(4) var<uniform> bl_params: BlendParams;

@compute @workgroup_size(16, 16)
fn blend_level(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(bl_fine);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let w = blend_weight(textureLoad(bl_depth, gid.xy, 0).r, bl_params);
    if (w <= 0.0) {
        return;
    }
    let lap = textureLoad(bl_fine, gid.xy, 0).rgb - gauss2_up(bl_coarse, vec2i(gid.xy)).rgb;
    let i = gid.y * dims.x + gid.x;
    let cur = bl_acc[i];
    bl_acc[i] = vec4f(cur.rgb + lap * w, cur.a + w);
}

// Coarsest level: the Gaussian residual itself (no upsample).
@group(0) @binding(0) var bt_top: texture_2d<f32>;
@group(0) @binding(1) var bt_depth: texture_2d<f32>;
// As `bl_acc`, at the coarsest level's size.
@group(0) @binding(2) var<storage, read_write> bt_acc: array<vec4f>;
@group(0) @binding(3) var<uniform> bt_params: BlendParams;

@compute @workgroup_size(16, 16)
fn blend_top(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(bt_top);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let w = blend_weight(textureLoad(bt_depth, gid.xy, 0).r, bt_params);
    if (w <= 0.0) {
        return;
    }
    let src = textureLoad(bt_top, gid.xy, 0).rgb;
    let i = gid.y * dims.x + gid.x;
    let cur = bt_acc[i];
    bt_acc[i] = vec4f(cur.rgb + src * w, cur.a + w);
}

// An accumulator divided by its weight; a pixel no frame reached keeps its
// (zero) sum.
fn normalized(v: vec4f) -> vec3f {
    return select(v.rgb, v.rgb / v.a, v.a > 0.0);
}

// Collapse, coarse to fine: G[L] = acc[L] / weight (the normalized
// Laplacian) + up(G[L+1]).
@group(0) @binding(0) var cl_coarse: texture_2d<f32>;
@group(0) @binding(1) var cl_dst: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<storage, read> cl_lap: array<vec4f>;
// `finish_rgb`'s step, set on whichever pass writes level 0.
@group(0) @binding(3) var<uniform> cl_finish: FinishParams;

@compute @workgroup_size(16, 16)
fn collapse_level(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(cl_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let v = normalized(cl_lap[gid.y * dims.x + gid.x]) + gauss2_up(cl_coarse, vec2i(gid.xy)).rgb;
    textureStore(cl_dst, gid.xy, vec4f(finish_rgb(v, cl_finish), 1.0));
}

// =============================================================================
// BLEND RESOLVE TOP — the coarsest accumulator divided by its weight, into
// the frame pyramid's top level, where the collapse starts from
// =============================================================================

@group(0) @binding(0) var br_dst: texture_storage_2d<rgba32float, write>;
@group(0) @binding(1) var<storage, read> br_acc: array<vec4f>;
@group(0) @binding(2) var<uniform> br_finish: FinishParams;

@compute @workgroup_size(16, 16)
fn blend_resolve_top(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(br_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    // A pixel no frame reached is black; opaque, as the collapse reads rgb alone.
    let v = normalized(br_acc[gid.y * dims.x + gid.x]);
    textureStore(br_dst, gid.xy, vec4f(finish_rgb(v, br_finish), 1.0));
}
