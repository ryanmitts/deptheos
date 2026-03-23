#include "ShaderTypes.h"

// =============================================================================
// FUSED WARP-TO-GRAY (bilinear sampling + optional linear->sRGB + luma)
// Uses bilinear hardware sampling (not Lanczos) to avoid ringing artifacts
// that inflate focus metrics at sharp edges, causing depth map halos.
// =============================================================================

kernel void warp_to_gray(
    sampler smp [[sampler(0)]],
    texture2d<float, access::sample> wtg_src [[texture(0)]],
    texture2d<float, access::write> wtg_dst [[texture(1)]],
    constant WarpToGrayParams& wtg_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint dst_w = wtg_dst.get_width();
    uint dst_h = wtg_dst.get_height();
    if (gid.x >= dst_w || gid.y >= dst_h) {
        return;
    }

    float3x3 inv_matrix = float3x3(
        wtg_params.col0.xyz,
        wtg_params.col1.xyz,
        wtg_params.col2.xyz
    );

    float2 dst_coord = float2(float(gid.x) + 0.5f, float(gid.y) + 0.5f);
    float3 src_h = inv_matrix * float3(dst_coord, 1.0f);

    float2 src_dims = float2(wtg_src.get_width(), wtg_src.get_height());
    float2 tex_coords = src_h.xy / src_dims;

    float3 rgb = wtg_src.sample(smp, tex_coords).rgb;

    if (wtg_params.apply_srgb != 0u) {
        rgb = float3(
            linear_to_srgb(rgb.r),
            linear_to_srgb(rgb.g),
            linear_to_srgb(rgb.b)
        );
    }

    float gray = dot(rgb, float3(LUMA_R, LUMA_G, LUMA_B));
    wtg_dst.write(float4(gray, 0.0f, 0.0f, 0.0f), gid);
}

// =============================================================================
// RGB TO GRAYSCALE (no-warp path: direct texel load + optional linear->sRGB)
// =============================================================================

kernel void rgb_to_gray(
    texture2d<float, access::read> rgb_texture [[texture(0)]],
    texture2d<float, access::write> gray_dst [[texture(1)]],
    constant RgbToGrayParams& rgb_to_gray_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = gray_dst.get_width();
    uint h = gray_dst.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    float3 rgb = rgb_texture.read(gid).rgb;
    if (rgb_to_gray_params.is_linear != 0u) {
        rgb = float3(
            linear_to_srgb(rgb.r),
            linear_to_srgb(rgb.g),
            linear_to_srgb(rgb.b)
        );
    }
    float gray = dot(rgb, float3(LUMA_R, LUMA_G, LUMA_B));
    gray_dst.write(float4(gray, 0.0f, 0.0f, 0.0f), gid);
}

// =============================================================================
// BANDPASS DIFFERENCE with Weber contrast correction
// Computes |original - blurred| as a focus metric (high-pass filter),
// then applies Modified Weber contrast (Peli's dark-floor variant) to
// normalize across brightness, and a radius-dependent amplifier to
// normalize across blur radius settings.
// =============================================================================

kernel void bandpass_diff(
    texture2d<float, access::read> bp_original [[texture(0)]],
    texture2d<float, access::read> bp_blurred [[texture(1)]],
    texture2d<float, access::write> bp_output [[texture(2)]],
    constant BandpassDiffParams& bp_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = bp_output.get_width();
    uint h = bp_output.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    float orig = bp_original.read(gid).r;
    float blurred = bp_blurred.read(gid).r;
    float diff = abs(orig - blurred);

    // Normalizes focus signal by brightness so dark and bright regions
    // compete fairly in WTA. epsilon prevents division by zero in black regions.
    const float DARK_FLOOR = 0.1f;
    float brightness = max(orig, blurred);
    float weber = diff / (DARK_FLOOR + brightness);

    // Radius-normalized amplifier: larger blur radii produce larger
    // raw differences, so we scale down proportionally.
    float result = clamp(weber * bp_params.amplifier, 0.0f, 1.0f);
    bp_output.write(float4(result, 0.0f, 0.0f, 0.0f), gid);
}

// =============================================================================
// LAPLACIAN FOCUS METRIC with Weber contrast correction
// Takes biased Laplacian output (0.5 = zero, shifted by MPSImageLaplacian.bias)
// and the original grayscale, computes |laplacian - 0.5| * 2 as focus strength,
// then applies Weber contrast normalization.
// =============================================================================

kernel void laplacian_focus(
    texture2d<float, access::read> lf_laplacian [[texture(0)]],
    texture2d<float, access::read> lf_gray [[texture(1)]],
    texture2d<float, access::write> lf_output [[texture(2)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = lf_output.get_width();
    uint h = lf_output.get_height();
    if (gid.x >= w || gid.y >= h) { return; }

    float lap = lf_laplacian.read(gid).r;
    float focus = abs(lap - 0.5f) * 2.0f; // undo bias, take absolute value

    // Weber contrast: normalize by brightness
    float brightness = lf_gray.read(gid).r;
    const float DARK_FLOOR = 0.1f;
    float result = clamp(focus / (DARK_FLOOR + brightness), 0.0f, 1.0f);

    lf_output.write(float4(result, 0.0f, 0.0f, 0.0f), gid);
}

// =============================================================================
// DEPTH WTA ACCUMULATION
// Updates best focus, depth index, and opacity map
// =============================================================================

kernel void depth_wta_accumulate(
    texture2d<float, access::read> curr_focus [[texture(0)]],
    texture2d<float, access::read_write> best_focus [[texture(1)]],
    texture2d<float, access::read_write> depth_index [[texture(2)]],
    texture2d<float, access::read_write> opacity_map [[texture(3)]],
    constant DepthWtaParams& wta_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = curr_focus.get_width();
    uint h = curr_focus.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    float new_focus = curr_focus.read(gid).r;
    float max_focus = best_focus.read(gid).r;

    // Accumulate focus into opacity map (additive average)
    float old_op = opacity_map.read(gid).r;
    float contribution = new_focus / float(max(wta_params.num_frames, 1u));
    float next_op = old_op + contribution;
    opacity_map.write(float4(next_op, 0.0f, 0.0f, 0.0f), gid);

    // Later frames win ties (standard depth-from-focus convention: >= favors last-seen)
    if (new_focus >= max_focus) {
        best_focus.write(float4(new_focus, 0.0f, 0.0f, 0.0f), gid);
        // Store depth as [0,1] float
        uint n_eff = max(wta_params.num_frames, 2u);
        float depth = float(wta_params.frame_index) / float(n_eff - 1u);
        depth_index.write(float4(depth, 0.0f, 0.0f, 0.0f), gid);
    }
}

// =============================================================================
// WTA COLOR UPDATE
// After WTA, copies the winning frame's RGB pixel to the WTA color accumulator.
// Applies bilinear warp if transform is provided (has_warp != 0).
// =============================================================================

// WtaColorUpdateParams defined in ShaderTypes.h

kernel void wta_color_update(
    sampler smp                                      [[sampler(0)]],
    texture2d<float, access::sample>     source_rgb  [[texture(0)]],
    texture2d<float, access::read>       depth_index [[texture(1)]],
    texture2d<float, access::read_write> wta_color   [[texture(2)]],
    constant WtaColorUpdateParams&       params      [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = wta_color.get_width();
    uint h = wta_color.get_height();
    if (gid.x >= w || gid.y >= h) return;

    float stored = depth_index.read(gid).r;
    uint n_eff = max(params.num_frames, 2u);
    float mine = float(params.frame_index) / float(n_eff - 1u);
    if (abs(stored - mine) > 0.0001f) return;

    float4 color;
    if (params.has_warp != 0u) {
        float3x3 inv_matrix = float3x3(
            params.col0.xyz, params.col1.xyz, params.col2.xyz
        );
        float2 dst_coord = float2(float(gid.x) + 0.5f, float(gid.y) + 0.5f);
        float3 src_h = inv_matrix * float3(dst_coord, 1.0f);
        float2 src_dims = float2(source_rgb.get_width(), source_rgb.get_height());
        float2 tex_coords = src_h.xy / src_dims;
        color = source_rgb.sample(smp, tex_coords);
    } else {
        color = source_rgb.read(uint2(gid.x, gid.y));
    }
    wta_color.write(float4(color.rgb, 1.0f), gid);
}

// =============================================================================
// OPACITY DIFFERENCE (best-vs-average contrast)
// Converts the accumulated mean focus into a contrast signal:
//   opacity_map = abs(mean_focus - best_focus)
// High values = clear winner (one frame much sharper than average).
// Low values = ambiguous region (all frames similarly focused).
// This must run BEFORE histogram normalization.
//
// Note: per-frame focus values are already Weber-corrected (in bandpass_diff),
// so this computes the raw difference without additional brightness correction.
// =============================================================================

kernel void opacity_diff(
    texture2d<float, access::read_write> od_opacity [[texture(0)]],
    texture2d<float, access::read> od_best_focus [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = uint(od_opacity.get_width());
    uint h = uint(od_opacity.get_height());
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    float avg = od_opacity.read(gid).r;
    float best = od_best_focus.read(gid).r;
    float result = clamp(abs(avg - best), 0.0f, 1.0f);
    od_opacity.write(float4(result, 0.0f, 0.0f, 0.0f), gid);
}

// =============================================================================
// NORMALIZE OPACITY MAP (histogram-based percentile normalization)
// =============================================================================

kernel void normalize_opacity(
    texture2d<float, access::read_write> norm_opacity_inout [[texture(0)]],
    constant NormOpacityParams& norm_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = norm_opacity_inout.get_width();
    uint h = norm_opacity_inout.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }

    float val = norm_opacity_inout.read(gid).r;
    float result = clamp((val - norm_params.offset) * norm_params.inv_range, 0.0f, 1.0f);

    norm_opacity_inout.write(float4(result, 0.0f, 0.0f, 0.0f), gid);
}

// =============================================================================
// GPU HISTOGRAM for opacity map normalization
// Replaces CPU readback + histogram loop with two GPU passes.
//
// Pass 1 (histogram_accumulate): each thread reads one pixel from the opacity
//   map, bins it into [0..1023], and atomically increments the corresponding
//   counter.  Dispatched as ceil(W/16) x ceil(H/16) workgroups.
//
// Pass 2 (histogram_percentile): a single workgroup scans the 1024 bins,
//   finds the 5th and 95th percentiles, computes offset / inv_range, and
//   writes them into the NormOpacityParams buffer so the existing
//   normalize_opacity shader can consume them without any CPU round-trip.
// =============================================================================

// --- Pass 1: histogram accumulation now handled by MPSImageHistogram (Swift side) ---

// --- Pass 2: scan bins, compute percentiles, write NormOpacityParams ---

kernel void histogram_percentile(
    device const uint* hist_bins_ro [[buffer(0)]],
    device NormOpacityParams* norm_out [[buffer(1)]],
    constant uint& hist_total_pixels [[buffer(2)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint total = hist_total_pixels;
    uint trim_target = uint(float(total) * 0.05f);

    // Find p05 bin (from low end)
    uint cumulative_low = 0u;
    uint p05_bin = 0u;
    for (uint i = 0u; i < 256u; i++) {
        cumulative_low += hist_bins_ro[i];
        if (cumulative_low >= trim_target) {
            p05_bin = i;
            break;
        }
    }

    // Find p95 bin (from high end)
    uint cumulative_high = 0u;
    uint p95_bin = 255u;
    for (int i = 255; i >= 0; i--) {
        cumulative_high += hist_bins_ro[uint(i)];
        if (cumulative_high >= trim_target) {
            p95_bin = uint(i);
            break;
        }
    }

    uint p05_u16 = p05_bin << 8u;
    uint p95_u16 = p95_bin << 8u;
    uint range_u16 = max(p95_u16 - p05_u16, 1u);

    norm_out->offset = float(p05_u16) / 65535.0f;
    norm_out->inv_range = 65535.0f / float(range_u16);
}

// =============================================================================
// DEPTH FEATHERING
// Applies a power curve to push depth values toward the nearest frame step.
// At power=1, no change. At power>1, values snap toward steps, reducing
// the transition zone where multiple frames blend.
// =============================================================================

kernel void depth_feather(
    texture2d<float, access::read_write> df_depth [[texture(0)]],
    constant DepthFeatherParams& df_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint w = df_depth.get_width();
    uint h = df_depth.get_height();
    if (gid.x >= w || gid.y >= h) return;

    float z = df_depth.read(gid).r;
    float step = df_params.step_size;
    if (step <= 0.0f) return;

    float nearest = round(z / step) * step;
    float half_step = step * 0.5f;
    float dist = (z - nearest) / half_step; // normalized [-1, 1]
    float s = dist >= 0.0f ? 1.0f : -1.0f;
    float abs_dist = clamp(abs(dist), 0.0f, 1.0f);
    float feathered = pow(abs_dist, df_params.power);
    float result = clamp(nearest + s * half_step * feathered, 0.0f, 1.0f);

    df_depth.write(float4(result, 0.0f, 0.0f, 0.0f), gid);
}

// =============================================================================
// DOMAIN TRANSFORM RECURSIVE FILTER (Gastal & Oliveira, SIGGRAPH 2011)
// Edge-preserving smoothing for depth maps using the "RF" variant.
// Each thread processes one row (horizontal) or column (vertical).
// The domain transform stretches distances at edges: d = 1 + (sigma_s/sigma_r) * |grad(guide)|
// Feedback coefficient: weight = exp(log_a * d) -- exponential cutoff at edges.
// =============================================================================

// --- Horizontal pass: depth_in -> depth_out, guided by guide texture ---

kernel void dt_rf_horizontal(
    texture2d<float, access::read> dt_h_depth_in [[texture(0)]],
    texture2d<float, access::read_write> dt_h_depth_out [[texture(1)]],
    texture2d<float, access::read> dt_h_guide [[texture(2)]],
    constant DTRFParams& dt_h_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint y = gid.x; // one thread per row
    if (y >= dt_h_params.height) { return; }

    int w = int(dt_h_params.width);
    float log_a = dt_h_params.log_a;
    float sr_ratio = dt_h_params.sigma_s / max(dt_h_params.sigma_r, 0.0001f);
    float bg_max_w = dt_h_params.bg_max_weight;
    float subj_max_w = dt_h_params.subject_max_weight;

    // Forward sweep L->R
    float prev_guide = dt_h_guide.read(uint2(0u, y)).r;
    float acc = dt_h_depth_in.read(uint2(0u, y)).r;
    dt_h_depth_out.write(float4(acc, 0.0f, 0.0f, 0.0f), uint2(0u, y));

    for (int x = 1; x < w; x++) {
        float cur_guide = dt_h_guide.read(uint2(uint(x), y)).r;
        float d = 1.0f + sr_ratio * abs(cur_guide - prev_guide);
        float weight = exp(log_a * d);
        // Confidence damping: high-confidence pixels resist smoothing
        float confidence = max(cur_guide, prev_guide);
        weight = min(weight, mix(bg_max_w, subj_max_w, confidence));
        float val = dt_h_depth_in.read(uint2(uint(x), y)).r;
        acc = (1.0f - weight) * val + weight * acc;
        prev_guide = cur_guide;
        dt_h_depth_out.write(float4(acc, 0.0f, 0.0f, 0.0f), uint2(uint(x), y));
    }

    // Reverse sweep R->L -- applied on top of forward result (cascaded)
    prev_guide = dt_h_guide.read(uint2(uint(w - 1), y)).r;
    acc = dt_h_depth_out.read(uint2(uint(w - 1), y)).r; // seed from forward output

    for (int x = w - 2; x >= 0; x--) {
        float cur_guide = dt_h_guide.read(uint2(uint(x), y)).r;
        float d = 1.0f + sr_ratio * abs(cur_guide - prev_guide);
        float weight = exp(log_a * d);
        float confidence = max(cur_guide, prev_guide);
        weight = min(weight, mix(bg_max_w, subj_max_w, confidence));
        float fwd = dt_h_depth_out.read(uint2(uint(x), y)).r; // forward result
        acc = (1.0f - weight) * fwd + weight * acc;
        prev_guide = cur_guide;
        dt_h_depth_out.write(float4(acc, 0.0f, 0.0f, 0.0f), uint2(uint(x), y));
    }
}

// --- Vertical pass: depth_in -> depth_out, guided by guide texture ---

kernel void dt_rf_vertical(
    texture2d<float, access::read> dt_v_depth_in [[texture(0)]],
    texture2d<float, access::read_write> dt_v_depth_out [[texture(1)]],
    texture2d<float, access::read> dt_v_guide [[texture(2)]],
    constant DTRFParams& dt_v_params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint x = gid.x; // one thread per column
    if (x >= dt_v_params.width) { return; }

    int h = int(dt_v_params.height);
    float log_a = dt_v_params.log_a;
    float sr_ratio = dt_v_params.sigma_s / max(dt_v_params.sigma_r, 0.0001f);
    float bg_max_w = dt_v_params.bg_max_weight;
    float subj_max_w = dt_v_params.subject_max_weight;

    // Forward sweep T->B
    float prev_guide = dt_v_guide.read(uint2(x, 0u)).r;
    float acc = dt_v_depth_in.read(uint2(x, 0u)).r;
    dt_v_depth_out.write(float4(acc, 0.0f, 0.0f, 0.0f), uint2(x, 0u));

    for (int y = 1; y < h; y++) {
        float cur_guide = dt_v_guide.read(uint2(x, uint(y))).r;
        float d = 1.0f + sr_ratio * abs(cur_guide - prev_guide);
        float weight = exp(log_a * d);
        float confidence = max(cur_guide, prev_guide);
        weight = min(weight, mix(bg_max_w, subj_max_w, confidence));
        float val = dt_v_depth_in.read(uint2(x, uint(y))).r;
        acc = (1.0f - weight) * val + weight * acc;
        prev_guide = cur_guide;
        dt_v_depth_out.write(float4(acc, 0.0f, 0.0f, 0.0f), uint2(x, uint(y)));
    }

    // Reverse sweep B->T -- applied on top of forward result (cascaded)
    prev_guide = dt_v_guide.read(uint2(x, uint(h - 1))).r;
    acc = dt_v_depth_out.read(uint2(x, uint(h - 1))).r; // seed from forward output

    for (int y = h - 2; y >= 0; y--) {
        float cur_guide = dt_v_guide.read(uint2(x, uint(y))).r;
        float d = 1.0f + sr_ratio * abs(cur_guide - prev_guide);
        float weight = exp(log_a * d);
        float confidence = max(cur_guide, prev_guide);
        weight = min(weight, mix(bg_max_w, subj_max_w, confidence));
        float fwd = dt_v_depth_out.read(uint2(x, uint(y))).r; // forward result
        acc = (1.0f - weight) * fwd + weight * acc;
        prev_guide = cur_guide;
        dt_v_depth_out.write(float4(acc, 0.0f, 0.0f, 0.0f), uint2(x, uint(y)));
    }
}

// =============================================================================
// FEATHERING (power-curve snap toward discrete plane centers)
// Smooths depth transitions by pulling values toward nearest plane boundary.
// =============================================================================
// BLEND RENDER PIPELINE (hardware alpha blending with fused Lanczos warp)
// =============================================================================

// Lanczos sampling helper for the blend compute shader
inline float4 blend_lanczos_sample(
    texture2d<float, access::read> src,
    float2 src_pos,
    int a
) {
    int2 dims = int2(src.get_width(), src.get_height());
    a = min(a, 5); // max Lanczos5 (10 taps)
    int half_kernel = a - 1;
    int2 base = int2(floor(src_pos - 0.5f)) - half_kernel;
    int tap_count = a * 2;
    float a_f = float(a);

    // Precompute separable weights (max 10 taps for Lanczos5)
    float wx[10];
    float wy[10];
    for (int i = 0; i < tap_count; i++) {
        wx[i] = lanczos_weight(src_pos.x - (float(base.x + i) + 0.5f), a_f);
        wy[i] = lanczos_weight(src_pos.y - (float(base.y + i) + 0.5f), a_f);
    }

    float4 color = float4(0.0f);
    float total_w = 0.0f;

    int start_y = max(0, -base.y);
    int end_y = min(tap_count, dims.y - base.y);
    int start_x = max(0, -base.x);
    int end_x = min(tap_count, dims.x - base.x);
    for (int dy = start_y; dy < end_y; dy++) {
        int py = base.y + dy;
        for (int dx = start_x; dx < end_x; dx++) {
            int px = base.x + dx;
            float w = wx[dx] * wy[dy];
            color += src.read(uint2(px, py)) * w;
            total_w += w;
        }
    }

    if (total_w > 0.0f) {
        color /= total_w;
    }
    return clamp(color, float4(0.0f), float4(1.0f));
}

// Vertex/fragment inter-stage struct
struct BlendVertexOut {
    float4 position [[position]];
};

// Full-screen triangle vertex shader
vertex BlendVertexOut blend_vs(uint vi [[vertex_id]]) {
    // Full-screen triangle: vertices at (-1,-1), (3,-1), (-1,3)
    float x = float(int(vi & 1u) * 4 - 1);
    float y = float(int(vi & 2u) * 2 - 1);
    BlendVertexOut out;
    out.position = float4(x, y, 0.0f, 1.0f);
    return out;
}

// Depth-weighted fragment shader with fused Lanczos warp.
// Order-independent: outputs pre-multiplied (color * weight, weight) for additive accumulation.
// A normalize pass divides by total weight after all frames are blended.
fragment float4 blend_fs(
    BlendVertexOut in [[stage_in]],
    texture2d<float, access::read> blend_r_frame_rgb [[texture(0)]],
    texture2d<float, access::read> blend_r_depth_map [[texture(1)]],
    constant BlendIterativeParams& blend_r_params [[buffer(0)]],
    constant BlendWarpParams& blend_r_warp [[buffer(1)]]
) {
    uint2 pixel = uint2(uint(in.position.x), uint(in.position.y));

    float z = blend_r_depth_map.read(pixel).r;
    float frame_pos = blend_r_params.scaled_frame_idx;
    float t = max(blend_r_params.inter_frame_dist, 0.00001f);
    float weight = saturate(1.0f - abs(z - frame_pos) / t);

    if (weight <= 0.0f) {
        discard_fragment();
    }

    // Fused warp: sample source image using inverse affine matrix (Lanczos)
    float4 source;
    if (blend_r_warp.lanczos_a == 0u) {
        // No warp -- direct pixel load (identity)
        source = blend_r_frame_rgb.read(pixel);
    } else {
        float3x3 inv_matrix = float3x3(
            blend_r_warp.col0.xyz,
            blend_r_warp.col1.xyz,
            blend_r_warp.col2.xyz
        );
        float3 src_h = inv_matrix * float3(in.position.x, in.position.y, 1.0f);
        source = blend_lanczos_sample(blend_r_frame_rgb, src_h.xy, int(blend_r_warp.lanczos_a));
    }

    if (blend_r_warp.apply_srgb != 0u) {
        source = float4(
            linear_to_srgb(source.r),
            linear_to_srgb(source.g),
            linear_to_srgb(source.b),
            source.a
        );
    }

    return float4(source.rgb * weight, weight);
}

// Normalize accumulated blend: divide RGB by total weight (alpha channel).
kernel void blend_normalize(
    texture2d<float, access::read_write> tex [[texture(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= tex.get_width() || gid.y >= tex.get_height()) return;
    float4 v = tex.read(gid);
    if (v.a > 0.0f) {
        tex.write(float4(v.rgb / v.a, 1.0f), gid);
    }
}

// =============================================================================
// COMPUTE BLEND — accumulating weighted average
//
// Frames are processed in order 0..N-1. For each pixel, the depth map encodes
// which frame is sharpest [0,1]. Each frame contributes weighted by proximity
// to the depth value, with quadratic falloff over ±plane_range frame steps.
//
// RGB accumulates weighted color, A accumulates total weight.
// blend_normalize divides RGB/A after all frames are processed.
// =============================================================================

kernel void blend_frame_compute(
    texture2d<float, access::read>       bfc_source    [[texture(0)]],
    texture2d<float, access::read>       bfc_depth_map [[texture(1)]],
    texture2d<float, access::read_write> bfc_dest      [[texture(2)]],
    constant BlendIterativeParams&       bfc_params    [[buffer(0)]],
    constant BlendWarpParams&            bfc_warp      [[buffer(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= bfc_params.width || gid.y >= bfc_params.height) return;

    float z = bfc_depth_map.read(gid).r;
    float frame_pos = bfc_params.scaled_frame_idx;
    float t = max(bfc_params.inter_frame_dist, 0.00001f);
    float plane_range = float(bfc_params.plane_range);

    // Continuous signed distance in frame-step units.
    float diff = (z - frame_pos) / t;
    float fade_frames = max(plane_range, 1.0f);

    // Weight peaks at diff=0 (exact depth match), fades to zero at ±fade_frames.
    float abs_diff = abs(diff);
    if (abs_diff >= fade_frames) return;
    float weight = 1.0f - abs_diff / fade_frames;
    weight = weight * weight; // quadratic falloff for smoother transitions

    if (weight <= 0.001f) return;

    // Sample source (with optional Lanczos warp)
    float4 source;
    if (bfc_warp.lanczos_a == 0u) {
        source = bfc_source.read(gid);
    } else {
        float3x3 inv_matrix = float3x3(
            bfc_warp.col0.xyz,
            bfc_warp.col1.xyz,
            bfc_warp.col2.xyz
        );
        float2 pixel_center = float2(float(gid.x) + 0.5f, float(gid.y) + 0.5f);
        float3 src_h = inv_matrix * float3(pixel_center, 1.0f);
        source = blend_lanczos_sample(bfc_source, src_h.xy, int(bfc_warp.lanczos_a));
    }

    if (bfc_warp.apply_srgb != 0u) {
        source = float4(
            linear_to_srgb(source.r),
            linear_to_srgb(source.g),
            linear_to_srgb(source.b),
            source.a
        );
    }

    // Accumulating weighted average: RGB accumulates weighted color, A accumulates weight.
    // Normalized in blend_normalize after all frames are processed.
    float4 current = bfc_dest.read(gid);
    bfc_dest.write(float4(current.rgb + source.rgb * weight, current.a + weight), gid);
}

