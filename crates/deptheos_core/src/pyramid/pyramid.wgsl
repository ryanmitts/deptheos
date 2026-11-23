// Pyramid focus stacking: Laplacian pyramid fusion (Burt & Adelson 1983)
// with soft winner-take-all and a margin mask.
//
// The soft-WTA accumulators (`deptheos::accum`) and running best keys
// (`ScalarMap`) are row-major storage buffers: no read-write storage
// texture, which some Metal devices lack.

#import deptheos::warp::{WarpParams, warp_lanczos, footprint_weight}
#import deptheos::color::linear_to_srgb3
#import deptheos::metric::{BlurParams, gaussian_weight}
#import deptheos::laplacian::{GAUSSIAN, gaussian_upsample, focus_metric}
#import deptheos::accum::{FinishParams, finish_rgb}

// =============================================================================
// WARP INGEST — Lanczos affine warp of the source into the pyramid base.
// =============================================================================

@group(0) @binding(0) var warp_src: texture_2d<f32>;
@group(0) @binding(1) var warp_dst: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> warp_params: WarpParams;

@compute @workgroup_size(16, 16)
fn warp_rgba(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(warp_dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    // Lanczos a=3, not bilinear: bilinear's blur varies with subpixel phase,
    // softening aligned frames and biasing the WTA toward integer shifts.
    // An identity transform reproduces the source exactly.
    var color = warp_lanczos(warp_src, warp_params, gid.xy);
    // A linear-space Laplacian over-weights highlights, so encode to sRGB.
    if (warp_params.apply_srgb != 0u) {
        color = vec4f(linear_to_srgb3(color.rgb), color.a);
    }
    textureStore(warp_dst, gid.xy, color);
}

// =============================================================================
// DOWNSAMPLE — fused 5x5 Gaussian blur + 2x subsample, centered at gid*2.
// Clamps at the edges, unlike depth.wgsl's pyramid, which drops and renormalizes: on purpose (ADR-120).
//
// Tiled: an 8×8 workgroup loads its 19×19 source region (5.8 KB) once,
// clamped at load, so the result is bit-identical to an untiled kernel.
// Dispatched in `DS_GROUP` workgroups (`dispatch_compute_with(.., 8)`).
// =============================================================================

@group(0) @binding(0) var downsample_src: texture_2d<f32>;
@group(0) @binding(1) var downsample_dst: texture_storage_2d<rgba32float, write>;

const DS_GROUP: u32 = 8u;
const DS_TILE: u32 = 19u; // 2 · DS_GROUP + 3
var<workgroup> ds_tile: array<vec4f, 361>; // DS_TILE²

@compute @workgroup_size(8, 8)
fn downsample(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(workgroup_id) wg: vec3u,
    @builtin(local_invocation_index) li: u32,
) {
    let src_max = vec2i(textureDimensions(downsample_src)) - 1;
    // Source coordinate of tile[0]: the first tap of the group's first output.
    let origin = 2 * vec2i(wg.xy * DS_GROUP) - 2;
    for (var k = li; k < DS_TILE * DS_TILE; k += DS_GROUP * DS_GROUP) {
        let p = clamp(origin + vec2i(i32(k % DS_TILE), i32(k / DS_TILE)), vec2i(0), src_max);
        ds_tile[k] = textureLoad(downsample_src, vec2u(p), 0);
    }
    // Every thread, in range or not, must reach the barrier.
    workgroupBarrier();

    let dst_dims = textureDimensions(downsample_dst);
    if (gid.x >= dst_dims.x || gid.y >= dst_dims.y) {
        return;
    }
    // Tile index of this output's center tap (source 2·gid).
    let c = 2 * vec2i(gid.xy) - origin;

    var sum = vec4f(0.0);
    for (var j: i32 = -2; j <= 2; j++) {
        let row = u32(c.y + j) * DS_TILE;
        let wy = GAUSSIAN[j + 2];
        for (var i: i32 = -2; i <= 2; i++) {
            sum += ds_tile[row + u32(c.x + i)] * (wy * GAUSSIAN[i + 2]);
        }
    }

    textureStore(downsample_dst, gid.xy, sum);
}

// =============================================================================
// LAPLACIAN ADD: G[i] = L[i] + Upsample(G[i+1])  (reconstruction)
// =============================================================================

@group(0) @binding(0) var<storage, read> laplacian_in: array<vec4f>;
@group(0) @binding(1) var gauss_coarse_in: texture_2d<f32>;
@group(0) @binding(2) var gauss_fine_out: texture_storage_2d<rgba32float, write>;
// The collapse's finish step (`deptheos::accum::finish_rgb`).
@group(0) @binding(3) var<uniform> laplacian_finish: FinishParams;

@compute @workgroup_size(16, 16)
fn laplacian_add(@builtin(global_invocation_id) gid: vec3u) {
    let fine_dims = textureDimensions(gauss_fine_out);
    if (gid.x >= fine_dims.x || gid.y >= fine_dims.y) {
        return;
    }

    let upsampled = gaussian_upsample(gid.xy, gauss_coarse_in);
    let lap = laplacian_in[gid.y * fine_dims.x + gid.x];
    // Normalize by the total weight: the winner contributes 1, so max(1.0)
    // only guards an untouched pixel.
    let alpha = max(lap.a, 1.0);
    let normalized_lap = lap.rgb / alpha;

    let result = finish_rgb(normalized_lap + upsampled.rgb, laplacian_finish);
    textureStore(gauss_fine_out, gid.xy, vec4f(result, 1.0));
}

// =============================================================================
// LEVEL PARAMETERS AND MARGIN MASK — the margin mask is the frame's weight
// at a pyramid pixel: zero outside its footprint, one pixel of fade inside,
// so the warp's edge smear can't win pixels the frame has no data for.
// =============================================================================

struct LevelParams {
    // Soft-WTA exponent (unused by the coarsest level).
    exponent: f32,
    // 2^level: full-resolution pixels per pixel of this level.
    level_scale: f32,
    // Full-resolution frame size.
    src_w: f32,
    src_h: f32,
    // PRIOR_FROM_POOLED (the recording level, which computes the prior),
    // PRIOR_FROM_LEVEL (this level of the prior pyramid) or
    // PRIOR_FROM_UPSAMPLE (level 0).
    prior_source: u32,
}

const PRIOR_FROM_POOLED: u32 = 0u;
const PRIOR_FROM_LEVEL: u32 = 1u;
const PRIOR_FROM_UPSAMPLE: u32 = 2u;
// Keeps a frame with no fine detail at all from scoring exactly zero.
const PRIOR_EPSILON: f32 = 1e-6;
// The stacker's `PRIOR_EXPONENT`.
const PRIOR_EXPONENT: f32 = 0.75;

// Whether this level records the winners (and computes the prior).
fn level_records(level: LevelParams) -> bool {
    return level.prior_source == PRIOR_FROM_POOLED;
}

fn level_mask(gid: vec2u, warp: WarpParams, level: LevelParams) -> f32 {
    return footprint_weight(
        warp,
        vec2f(gid) * level.level_scale,
        vec2f(level.src_w, level.src_h),
        level.level_scale,
    );
}

// =============================================================================
// FOCUS + WTA — one pass per level: focus metric, its blur, and the soft
// winner-take-all accumulation of the Laplacian.
//
// Tiled: a 16×16 workgroup computes the Laplacian and metric for its 22×22
// region (outputs plus a FW_HALO halo) in workgroup memory, clamped into the
// level, so the blur matches a clamped H-then-V blur over a stored map and
// the expensive Laplacian is computed once per pixel.
//
// The blur (σ from the smoothness slider, `iir_blur_sigma` in focus.rs;
// half_width 0 is none) suppresses noise-driven winner flipping. The winner
// keeps full weight, the loser blends in at pow(ratio, exponent). The
// footprint mask applies after the blur, so its zero is hard at the boundary.
//
// The mask is a class, not just a scale: a frame with data always beats one
// without, and frames without data compete among themselves on raw focus,
// so where no frame has data the stack blends as an unmasked one would.
// Letting one frame own those pixels leaves a bright band along the crop
// once the coarse upsample spreads it.
//
// The focus prior: each level's focus is multiplied by prior^exponent, the
// frame's fine-scale sharpness around the pixel (the recording level's
// pooled metric, pyramided down and upsampled to level 0). It keeps coarse
// levels from picking a frame blurred at a bright edge, which leaves a glow.
// The recording level runs first in each frame, since it writes the prior.
//
// Stored key: focus × mask ≥ 0 for data frames; −1/(1 + focus) in [−1, 0)
// for masked ones (monotonic, below any data key). best_focus starts at −2
// (`FOCUS_SENTINEL`), so the first frame always wins.
// =============================================================================

// The focus a key encodes (the sentinel decodes negative; callers clamp).
fn key_focus(key: f32) -> f32 {
    return select(-1.0 / key - 1.0, key, key >= 0.0);
}

const FW_GROUP: u32 = 16u;
// Largest blur half-width the tile holds (asserted where the params are built).
const FW_HALO: u32 = 3u;
const FW_TILE: u32 = 22u; // FW_GROUP + 2 · FW_HALO
var<workgroup> fw_focus: array<f32, 484>; // FW_TILE², the unblurred metric
var<workgroup> fw_rows: array<f32, 352>; // FW_TILE rows × FW_GROUP columns, blurred along x
var<workgroup> fw_pool_rows: array<f32, 352>; // the same, pooled for the winner record
var<workgroup> fw_lap: array<vec4f, 256>; // FW_GROUP², the outputs' Laplacians

struct FrameParams {
    // The frame's plane (`FramePlane`), written to the winner record.
    plane: f32,
}

// The winner record (`fw_winner`) names, per full-resolution pixel, the
// frame whose detail the result shows. It is decided at one level
// (`RECORD_LEVEL`) on the metric pooled over the halo, not by the WTA's own
// decision, which a sharp frame's Laplacian zero crossings would speckle.
// FW_POOL_SIGMA is that pooling's σ, over ±FW_HALO taps.
const FW_POOL_SIGMA: f32 = 1.5;

@group(0) @binding(0) var fw_gauss_fine: texture_2d<f32>;
@group(0) @binding(1) var fw_gauss_coarse: texture_2d<f32>;
// The soft-WTA sums (r, g, b) and weight (a).
@group(0) @binding(2) var<storage, read_write> fw_best_rgb: array<vec4f>;
// The running best key per pixel (`encode_key`).
@group(0) @binding(3) var<storage, read_write> fw_best_focus: array<f32>;
@group(0) @binding(4) var<uniform> fw_level: LevelParams;
@group(0) @binding(5) var<uniform> fw_warp: WarpParams;
@group(0) @binding(6) var<uniform> fw_blur: BlurParams;
@group(0) @binding(7) var<uniform> fw_frame: FrameParams;
// The winner record (full resolution) and each block's best pooled key;
// touched only at the recording level.
@group(0) @binding(8) var fw_winner: texture_storage_2d<r32float, write>;
@group(0) @binding(9) var<storage, read_write> fw_winner_best: array<f32>;
// The frame's prior: written at the recording level, read by the others.
// A level binds a 1×1 placeholder for the one it does not use.
@group(0) @binding(10) var fw_prior_out: texture_storage_2d<r32float, write>;
@group(0) @binding(11) var fw_prior_in: texture_2d<f32>;

// The stored key for `focus` under mask `m`; `key_focus` decodes it.
fn encode_key(focus: f32, m: f32) -> f32 {
    return select(-1.0 / (1.0 + max(focus, 0.0)), focus * m, m > 0.0);
}

// The metric over the tile into `fw_focus` and the group's own Laplacians
// into `fw_lap`. The halo is skipped when neither the blur nor the winner
// record's pooling reads it.
fn load_tile(li: u32, origin: vec2i, level_max: vec2i, r: i32, record: bool) {
    for (var k = li; k < FW_TILE * FW_TILE; k += FW_GROUP * FW_GROUP) {
        // `t` is relative to the group's outputs (negative in the halo).
        let tile = vec2i(i32(k % FW_TILE), i32(k / FW_TILE));
        let t = tile - i32(FW_HALO);
        let inner = all(t >= vec2i(0)) && all(t < vec2i(i32(FW_GROUP)));
        if (r == 0 && !record && !inner) {
            continue;
        }
        let p = vec2u(clamp(origin + tile, vec2i(0), level_max));
        let upsampled = gaussian_upsample(p, fw_gauss_coarse);
        let lap_pixel = textureLoad(fw_gauss_fine, p, 0) - upsampled;
        fw_focus[k] = focus_metric(lap_pixel, upsampled);
        if (inner) {
            fw_lap[u32(t.y) * FW_GROUP + u32(t.x)] = lap_pixel;
        }
    }
}

// Where row-blur output `k` (tile row k / FW_GROUP, group column
// k % FW_GROUP) is centered in `fw_focus`.
fn row_center(k: u32) -> u32 {
    return (k / FW_GROUP) * FW_TILE + k % FW_GROUP + FW_HALO;
}

// Normalized Gaussian blur of `fw_focus` along x, taps −r…r.
fn row_blur(center: u32, r: i32, sigma: f32) -> f32 {
    var sum = 0.0;
    var weight_sum = 0.0;
    for (var i: i32 = -r; i <= r; i++) {
        let wt = gaussian_weight(f32(i), sigma);
        sum += fw_focus[u32(i32(center) + i)] * wt;
        weight_sum += wt;
    }
    return sum / weight_sum;
}

// Where tap `j` of a column blur at group pixel `lid` reads a row-blurred
// array (`fw_rows`, `fw_pool_rows`).
fn column_tap(lid: vec2u, j: i32) -> u32 {
    return u32(i32(lid.y + FW_HALO) + j) * FW_GROUP + lid.x;
}

// Normalized Gaussian blurs along y, one per row-blurred array, since naga
// takes no pointer to workgroup memory as an argument.
fn column_blur_rows(lid: vec2u, r: i32, sigma: f32) -> f32 {
    var sum = 0.0;
    var weight_sum = 0.0;
    for (var j: i32 = -r; j <= r; j++) {
        let wt = gaussian_weight(f32(j), sigma);
        sum += fw_rows[column_tap(lid, j)] * wt;
        weight_sum += wt;
    }
    return sum / weight_sum;
}

fn column_blur_pool(lid: vec2u, r: i32, sigma: f32) -> f32 {
    var sum = 0.0;
    var weight_sum = 0.0;
    for (var j: i32 = -r; j <= r; j++) {
        let wt = gaussian_weight(f32(j), sigma);
        sum += fw_pool_rows[column_tap(lid, j)] * wt;
        weight_sum += wt;
    }
    return sum / weight_sum;
}

// The winner record's WTA on the pooled metric, with its own running best.
// A win writes this frame's plane over the pixel's full-resolution block.
fn record_winner(pixel: vec2u, width: u32, pooled: f32, m: f32) {
    let pooled_key = encode_key(pooled, m);
    let best_index = pixel.y * width + pixel.x;
    if (pooled_key > fw_winner_best[best_index]) {
        fw_winner_best[best_index] = pooled_key;
        let block = u32(fw_level.level_scale);
        let record_dims = textureDimensions(fw_winner);
        for (var y = 0u; y < block; y++) {
            for (var x = 0u; x < block; x++) {
                let p = pixel * block + vec2u(x, y);
                if (all(p < record_dims)) {
                    textureStore(fw_winner, p, vec4f(fw_frame.plane, 0.0, 0.0, 0.0));
                }
            }
        }
    }
}

// Add this frame's Laplacian to the pixel's running sum under `key`.
fn accumulate_soft_wta(pixel: vec2u, width: u32, key: f32, lap_pixel: vec4f) {
    let acc_index = pixel.y * width + pixel.x;
    let best = fw_best_focus[acc_index];
    let acc = fw_best_rgb[acc_index];
    // Floor on the divisor of the soft-WTA ratio.
    let THRESHOLD = 1e-10;
    // Across classes the loser gets nothing.
    let same_class = (key >= 0.0) == (best >= 0.0);
    let f_new = max(key_focus(key), 0.0);
    let f_best = max(key_focus(best), 0.0);

    if (key > best) {
        let ratio = select(0.0, clamp(f_best / max(f_new, THRESHOLD), 0.0, 1.0), same_class);
        let weight = pow(ratio, fw_level.exponent);
        fw_best_rgb[acc_index] = vec4f(
            acc.rgb * weight + lap_pixel.rgb,
            acc.a * weight + 1.0
        );
        fw_best_focus[acc_index] = key;
    } else {
        let ratio = select(0.0, clamp(f_new / max(f_best, THRESHOLD), 0.0, 1.0), same_class);
        let weight = pow(ratio, fw_level.exponent);
        fw_best_rgb[acc_index] = vec4f(
            acc.rgb + lap_pixel.rgb * weight,
            acc.a + weight
        );
    }
}

@compute @workgroup_size(16, 16)
fn focus_wta(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(local_invocation_id) lid: vec3u,
    @builtin(workgroup_id) wg: vec3u,
    @builtin(local_invocation_index) li: u32,
) {
    let dims = textureDimensions(fw_gauss_fine);
    let r = i32(fw_blur.half_width);
    let pool_r = i32(FW_HALO);
    let record = level_records(fw_level);

    load_tile(li, vec2i(wg.xy * FW_GROUP) - i32(FW_HALO), vec2i(dims) - 1, r, record);
    workgroupBarrier();

    // Blur every tile row along x, for the focus blur and the record's pooling.
    if (r > 0 || record) {
        for (var k = li; k < FW_TILE * FW_GROUP; k += FW_GROUP * FW_GROUP) {
            if (r > 0) {
                fw_rows[k] = row_blur(row_center(k), r, fw_blur.sigma);
            }
            if (record) {
                fw_pool_rows[k] = row_blur(row_center(k), pool_r, FW_POOL_SIGMA);
            }
        }
        // Every thread, in range or not, must reach the barriers.
        workgroupBarrier();
    }

    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    // Blur along y at this pixel (or the raw metric when the blur is off).
    var raw_focus: f32;
    if (r > 0) {
        raw_focus = column_blur_rows(lid.xy, r, fw_blur.sigma);
    } else {
        raw_focus = fw_focus[(lid.y + FW_HALO) * FW_TILE + lid.x + FW_HALO];
    }

    let m = level_mask(gid.xy, fw_warp, fw_level);

    // The pooled metric: the winner record's score and the frame's prior.
    var pooled = 0.0;
    if (record) {
        pooled = column_blur_pool(lid.xy, pool_r, FW_POOL_SIGMA);
        textureStore(fw_prior_out, gid.xy, vec4f(max(pooled, 0.0), 0.0, 0.0, 0.0));
    }

    var prior: f32;
    switch fw_level.prior_source {
        case PRIOR_FROM_LEVEL: { prior = textureLoad(fw_prior_in, gid.xy, 0).r; }
        case PRIOR_FROM_UPSAMPLE: { prior = gaussian_upsample(gid.xy, fw_prior_in).r; }
        default: { prior = pooled; }
    }
    let focus = max(raw_focus, 0.0) * pow(max(prior, 0.0) + PRIOR_EPSILON, PRIOR_EXPONENT);

    if (record) {
        record_winner(gid.xy, dims.x, pooled, m);
    }
    accumulate_soft_wta(gid.xy, dims.x, encode_key(focus, m), fw_lap[li]);
}

// =============================================================================
// ACCUMULATE SUM (coarsest level — mask-weighted average across frames)
// While no frame has had data at a pixel, alpha counts down and rgb holds
// the masked frames' plain sum; the first frame with data replaces it.
// =============================================================================

@group(0) @binding(0) var curr_rgb: texture_2d<f32>;
// As `fw_best_rgb`, at the coarsest level's size.
@group(0) @binding(1) var<storage, read_write> acc_rgb: array<vec4f>;
@group(0) @binding(2) var<uniform> acc_level: LevelParams;
@group(0) @binding(3) var<uniform> acc_warp: WarpParams;

@compute @workgroup_size(16, 16)
fn accumulate_sum(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(curr_rgb);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    let curr = textureLoad(curr_rgb, gid.xy, 0);
    let i = gid.y * dims.x + gid.x;
    let acc = acc_rgb[i];
    let m = level_mask(gid.xy, acc_warp, acc_level);
    if (m > 0.0) {
        if (acc.a > 0.0) {
            acc_rgb[i] = vec4f(acc.rgb + curr.rgb * m, acc.a + m);
        } else {
            acc_rgb[i] = vec4f(curr.rgb * m, m);
        }
    } else if (acc.a <= 0.0) {
        acc_rgb[i] = vec4f(acc.rgb + curr.rgb, acc.a - 1.0);
    }
}

// =============================================================================
// NORMALIZE COARSEST (divide by accumulated mask weight; a negative alpha is
// the masked-frame count of the fallback average)
// =============================================================================

@group(0) @binding(0) var<storage, read> normalize_in: array<vec4f>;
@group(0) @binding(1) var normalize_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> normalize_finish: FinishParams;

@compute @workgroup_size(16, 16)
fn normalize_coarsest(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(normalize_out);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let acc = normalize_in[gid.y * dims.x + gid.x];
    let rgb = select(acc.rgb, acc.rgb / abs(acc.a), acc.a != 0.0);
    textureStore(normalize_out, gid.xy, vec4f(finish_rgb(rgb, normalize_finish), 1.0));
}

// =============================================================================
// PRIOR DOWNSAMPLE — `downsample` for the one-channel prior, untiled (the
// prior maps are a quarter of full size or smaller).
// =============================================================================

@group(0) @binding(0) var prior_src: texture_2d<f32>;
@group(0) @binding(1) var prior_dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn prior_downsample(@builtin(global_invocation_id) gid: vec3u) {
    let dst_dims = textureDimensions(prior_dst);
    if (gid.x >= dst_dims.x || gid.y >= dst_dims.y) {
        return;
    }
    let src_max = vec2i(textureDimensions(prior_src)) - 1;
    var sum = 0.0;
    for (var j: i32 = -2; j <= 2; j++) {
        for (var i: i32 = -2; i <= 2; i++) {
            let p = clamp(2 * vec2i(gid.xy) + vec2i(i, j), vec2i(0), src_max);
            sum += textureLoad(prior_src, vec2u(p), 0).r * GAUSSIAN[i + 2] * GAUSSIAN[j + 2];
        }
    }
    textureStore(prior_dst, gid.xy, vec4f(sum, 0.0, 0.0, 0.0));
}
