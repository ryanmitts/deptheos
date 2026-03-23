#include "ShaderTypes.h"

struct LLNormParams {
    float inv_frame_count;
};

// ---------------------------------------------------------------------------
// Complex Daubechies 6-tap wavelet coefficients
// ---------------------------------------------------------------------------

constant float2 LOWPASS[6] = {
    float2(-0.0662912607f, -0.0855816496f),
    float2( 0.1104854346f, -0.0855816496f),
    float2( 0.6629126074f,  0.1711632992f),
    float2( 0.6629126074f,  0.1711632992f),
    float2( 0.1104854346f, -0.0855816496f),
    float2(-0.0662912607f, -0.0855816496f)
};

constant float2 HIGHPASS[6] = {
    float2(-0.0662912607f,  0.0855816496f),
    float2(-0.1104854346f, -0.0855816496f),
    float2( 0.6629126074f, -0.1711632992f),
    float2(-0.6629126074f,  0.1711632992f),
    float2( 0.1104854346f,  0.0855816496f),
    float2( 0.0662912607f, -0.0855816496f)
};

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

inline float2 cmul(float2 a, float2 b) {
    return float2(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

inline float2 cmul_conj(float2 val, float2 coeff) {
    return float2(
        val.x * coeff.x + val.y * coeff.y,
        val.y * coeff.x - val.x * coeff.y
    );
}

inline int circular(int idx, int size) {
    return ((idx % size) + size) % size;
}

inline int reflect_idx(int idx, int size) {
    if (idx < 0) { return -idx; }
    if (idx >= size) { return 2 * size - idx - 2; }
    return idx;
}

// ---------------------------------------------------------------------------
// extract_luma: RGBA -> complex luma (RG32Float)
// ---------------------------------------------------------------------------

kernel void extract_luma(
    texture2d<float, access::read>  src [[texture(0)]],
    texture2d<float, access::write> dst [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dst.get_width() || gid.y >= dst.get_height()) { return; }
    float4 c = src.read(gid);
    float luma = dot(c.rgb, float3(LUMA_R, LUMA_G, LUMA_B));
    dst.write(float4(luma, 0.0f, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// pad_reflect: reflection padding
// ---------------------------------------------------------------------------

kernel void pad_reflect(
    texture2d<float, access::read>  pad_src [[texture(0)]],
    texture2d<float, access::write> pad_dst [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint2 src_dims = uint2(pad_src.get_width(), pad_src.get_height());
    int x = reflect_idx(int(gid.x), int(src_dims.x));
    int y = reflect_idx(int(gid.y), int(src_dims.y));
    float4 val = pad_src.read(uint2(uint(x), uint(y)));
    pad_dst.write(float4(val.r, val.g, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// forward_decompose: 1D 6-tap wavelet decomposition
// ---------------------------------------------------------------------------

kernel void forward_decompose(
    texture2d<float, access::read>  src      [[texture(0)]],
    texture2d<float, access::write> dst_low  [[texture(1)]],
    texture2d<float, access::write> dst_high [[texture(2)]],
    constant WaveletDims& dims               [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dims.dst_width || gid.y >= dims.dst_height) { return; }

    float2 sum_low  = float2(0.0f);
    float2 sum_high = float2(0.0f);

    if (dims.direction == 0u) {  // horizontal
        int center = int(gid.x) * 2;
        for (int i = 0; i < 6; i++) {
            int sample_x = circular(center - 3 + i, int(dims.src_width));
            float4 val = src.read(uint2(uint(sample_x), gid.y));
            sum_low  += cmul(val.rg, LOWPASS[i]);
            sum_high += cmul(val.rg, HIGHPASS[i]);
        }
    } else {  // vertical
        int center = int(gid.y) * 2;
        for (int i = 0; i < 6; i++) {
            int sample_y = circular(center - 3 + i, int(dims.src_height));
            float4 val = src.read(uint2(gid.x, uint(sample_y)));
            sum_low  += cmul(val.rg, LOWPASS[i]);
            sum_high += cmul(val.rg, HIGHPASS[i]);
        }
    }

    dst_low.write(float4(sum_low, 0.0f, 1.0f), gid);
    dst_high.write(float4(sum_high, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// inverse_reconstruct: polyphase wavelet reconstruction
// ---------------------------------------------------------------------------

kernel void inverse_reconstruct(
    texture2d<float, access::read>  src_low  [[texture(0)]],
    texture2d<float, access::read>  src_high [[texture(1)]],
    texture2d<float, access::write> dst      [[texture(2)]],
    constant WaveletDims& dims               [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dims.dst_width || gid.y >= dims.dst_height) { return; }

    int x = int(gid.x);
    int y = int(gid.y);
    float2 sum = float2(0.0f);

    if (dims.direction == 0u) {  // horizontal
        int start_j = (x + 3) % 2;
        for (int j = start_j; j < 6; j += 2) {
            int pos = circular((x - j + 3) / 2, int(dims.src_width));
            float2 lo = src_low.read(uint2(uint(pos), uint(y))).rg;
            float2 hi = src_high.read(uint2(uint(pos), uint(y))).rg;
            sum += cmul_conj(lo, LOWPASS[j]) + cmul_conj(hi, HIGHPASS[j]);
        }
    } else {  // vertical
        int start_j = (y + 3) % 2;
        for (int j = start_j; j < 6; j += 2) {
            int pos = circular((y - j + 3) / 2, int(dims.src_height));
            float2 lo = src_low.read(uint2(uint(x), uint(pos))).rg;
            float2 hi = src_high.read(uint2(uint(x), uint(pos))).rg;
            sum += cmul_conj(lo, LOWPASS[j]) + cmul_conj(hi, HIGHPASS[j]);
        }
    }

    dst.write(float4(sum, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// coupled_wta: 3x3 smoothed magnitude WTA for detail subbands
// Tracks frame index and 2nd-best for post-hoc consistency filtering.
// ---------------------------------------------------------------------------

kernel void coupled_wta(
    texture2d<float, access::read>       curr             [[texture(0)]],
    texture2d<float, access::read_write> best             [[texture(1)]],
    texture2d<float, access::read_write> best_mag         [[texture(2)]],
    texture2d<uint,  access::read_write> best_idx         [[texture(3)]],
    texture2d<float, access::read_write> second_best      [[texture(4)]],
    texture2d<float, access::read_write> second_best_mag  [[texture(5)]],
    texture2d<uint,  access::read_write> second_best_idx  [[texture(6)]],
    constant MagnitudeDims& dims                          [[buffer(0)]],
    constant uint& frame_index                            [[buffer(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    int w = int(dims.width);
    int h = int(dims.height);
    if (int(gid.x) >= w || int(gid.y) >= h) { return; }

    float curr_mag = 0.0f;
    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            int sx = clamp(int(gid.x) + dx, 0, w - 1);
            int sy = clamp(int(gid.y) + dy, 0, h - 1);
            float4 c = curr.read(uint2(uint(sx), uint(sy)));
            curr_mag += c.r * c.r + c.g * c.g;
        }
    }

    float prev_mag = best_mag.read(gid).r;

    if (curr_mag > prev_mag) {
        // Demote current best → second-best
        second_best_mag.write(best_mag.read(gid), gid);
        second_best.write(best.read(gid), gid);
        second_best_idx.write(best_idx.read(gid), gid);
        // Promote current → best
        best_mag.write(float4(curr_mag, 0.0f, 0.0f, 1.0f), gid);
        float4 cc = curr.read(gid);
        best.write(float4(cc.r, cc.g, 0.0f, 1.0f), gid);
        best_idx.write(uint4(frame_index, 0, 0, 0), gid);
    } else {
        float prev_second_mag = second_best_mag.read(gid).r;
        if (curr_mag > prev_second_mag) {
            // Update second-best only
            second_best_mag.write(float4(curr_mag, 0.0f, 0.0f, 1.0f), gid);
            float4 cc = curr.read(gid);
            second_best.write(float4(cc.r, cc.g, 0.0f, 1.0f), gid);
            second_best_idx.write(uint4(frame_index, 0, 0, 0), gid);
        }
    }
}

// ---------------------------------------------------------------------------
// ll_accumulate: sum LL band
// ---------------------------------------------------------------------------

kernel void ll_accumulate(
    texture2d<float, access::read>  ll_curr [[texture(0)]],
    texture2d<float, access::read>  ll_prev [[texture(1)]],
    texture2d<float, access::write> ll_out  [[texture(2)]],
    uint2 gid [[thread_position_in_grid]]
) {
    float4 c = ll_curr.read(gid);
    float4 a = ll_prev.read(gid);
    float2 new_val = a.rg + c.rg;
    ll_out.write(float4(new_val, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// ll_wta: LL band WTA by magnitude
// ---------------------------------------------------------------------------

kernel void ll_wta(
    texture2d<float, access::read>       ll_curr     [[texture(0)]],
    texture2d<float, access::read_write> ll_best     [[texture(1)]],
    texture2d<float, access::read_write> ll_best_mag [[texture(2)]],
    texture2d<uint,  access::read_write> ll_best_idx [[texture(3)]],
    constant uint& frame_index                       [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    float4 c = ll_curr.read(gid);
    float curr_mag = c.r * c.r + c.g * c.g;
    float prev_mag = ll_best_mag.read(gid).r;

    if (curr_mag > prev_mag) {
        ll_best_mag.write(float4(curr_mag, 0.0f, 0.0f, 1.0f), gid);
        ll_best.write(float4(c.r, c.g, 0.0f, 1.0f), gid);
        ll_best_idx.write(uint4(frame_index, 0, 0, 0), gid);
    }
}

// ---------------------------------------------------------------------------
// ll_normalize: divide by frame count
// ---------------------------------------------------------------------------

kernel void ll_normalize(
    texture2d<float, access::read>  src    [[texture(0)]],
    texture2d<float, access::write> dst    [[texture(1)]],
    constant LLNormParams& params          [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    float4 s = src.read(gid);
    float2 normalized = s.rg * params.inv_frame_count;
    dst.write(float4(normalized, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// color_reassign_rw: luma comparison + color update (read_write)
// ---------------------------------------------------------------------------

kernel void color_reassign_rw(
    texture2d<float, access::read>       source     [[texture(0)]],
    texture2d<float, access::read>       merged     [[texture(1)]],
    texture2d<float, access::read_write> best_dist  [[texture(2)]],
    texture2d<float, access::read_write> best_color [[texture(3)]],
    constant ReassignParams& params                  [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    float4 source_rgba = source.read(gid);
    float merged_luma = merged.read(gid).r;

    // All images are linear — convert to sRGB for perceptual luma comparison
    float3 luma_rgb = float3(
        linear_to_srgb(source_rgba.r),
        linear_to_srgb(source_rgba.g),
        linear_to_srgb(source_rgba.b)
    );
    float source_luma = dot(luma_rgb, float3(LUMA_R, LUMA_G, LUMA_B));
    float dist = abs(source_luma - merged_luma);

    float prev_dist = best_dist.read(gid).r;
    if (dist < prev_dist) {
        best_dist.write(float4(dist, 0.0f, 0.0f, 1.0f), gid);
        best_color.write(source_rgba, gid);
    }
}

// ---------------------------------------------------------------------------
// init_best_distance: fill with 1e10
// ---------------------------------------------------------------------------

kernel void init_best_distance(
    texture2d<float, access::write> dst [[texture(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dst.get_width() || gid.y >= dst.get_height()) { return; }
    dst.write(float4(1e10f, 0.0f, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// sum_energy_map: sum level-0 detail subband magnitudes for live preview
// ---------------------------------------------------------------------------

kernel void sum_energy_map(
    texture2d<float, access::read>  mag_lh  [[texture(0)]],
    texture2d<float, access::read>  mag_hl  [[texture(1)]],
    texture2d<float, access::read>  mag_hh  [[texture(2)]],
    texture2d<float, access::write> energy  [[texture(3)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= energy.get_width() || gid.y >= energy.get_height()) { return; }
    float e = mag_lh.read(gid).r + mag_hl.read(gid).r + mag_hh.read(gid).r;
    energy.write(float4(e, 0.0f, 0.0f, 1.0f), gid);
}

// ---------------------------------------------------------------------------
// subband_consistency: 2-of-3 voting across LH/HL/HH detail subbands
// If 2 subbands agree on frame index, override the dissenter using its
// second-best coefficient (if it matches the voted index).
// ---------------------------------------------------------------------------

kernel void subband_consistency(
    // LH (subband 0)
    texture2d<uint,  access::read_write> idx_lh          [[texture(0)]],
    texture2d<float, access::read_write> best_lh         [[texture(1)]],
    texture2d<float, access::read>       second_best_lh  [[texture(2)]],
    texture2d<uint,  access::read>       second_idx_lh   [[texture(3)]],
    // HL (subband 1)
    texture2d<uint,  access::read_write> idx_hl          [[texture(4)]],
    texture2d<float, access::read_write> best_hl         [[texture(5)]],
    texture2d<float, access::read>       second_best_hl  [[texture(6)]],
    texture2d<uint,  access::read>       second_idx_hl   [[texture(7)]],
    // HH (subband 2)
    texture2d<uint,  access::read_write> idx_hh          [[texture(8)]],
    texture2d<float, access::read_write> best_hh         [[texture(9)]],
    texture2d<float, access::read>       second_best_hh  [[texture(10)]],
    texture2d<uint,  access::read>       second_idx_hh   [[texture(11)]],
    constant MagnitudeDims& dims                         [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dims.width || gid.y >= dims.height) { return; }

    uint v0 = idx_lh.read(gid).r;
    uint v1 = idx_hl.read(gid).r;
    uint v2 = idx_hh.read(gid).r;

    if (v0 == v1 && v1 == v2) { return; }  // All agree

    // Determine majority and dissenter(s)
    if (v1 == v2 && v0 != v1) {
        // HL and HH agree → override LH
        uint voted = v1;
        if (second_idx_lh.read(gid).r == voted) {
            best_lh.write(second_best_lh.read(gid), gid);
            idx_lh.write(uint4(voted, 0, 0, 0), gid);
        }
    } else if (v0 == v2 && v1 != v0) {
        // LH and HH agree → override HL
        uint voted = v0;
        if (second_idx_hl.read(gid).r == voted) {
            best_hl.write(second_best_hl.read(gid), gid);
            idx_hl.write(uint4(voted, 0, 0, 0), gid);
        }
    } else if (v0 == v1 && v2 != v0) {
        // LH and HL agree → override HH
        uint voted = v0;
        if (second_idx_hh.read(gid).r == voted) {
            best_hh.write(second_best_hh.read(gid), gid);
            idx_hh.write(uint4(voted, 0, 0, 0), gid);
        }
    }
    // All three differ → no majority, keep as-is
}

// ---------------------------------------------------------------------------
// neighbor_consistency: 4-connected outlier detection on frame indices
// If center index is strictly > or < all 4 neighbors, replace with averaged
// neighbor index using second-best coefficient if available.
// ---------------------------------------------------------------------------

kernel void neighbor_consistency(
    texture2d<uint,  access::read_write> best_idx        [[texture(0)]],
    texture2d<float, access::read_write> best_coeffs     [[texture(1)]],
    texture2d<float, access::read>       second_coeffs   [[texture(2)]],
    texture2d<uint,  access::read>       second_idx      [[texture(3)]],
    constant MagnitudeDims& dims                         [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    int w = int(dims.width);
    int h = int(dims.height);
    int x = int(gid.x);
    int y = int(gid.y);
    if (x <= 0 || x >= w - 1 || y <= 0 || y >= h - 1) { return; }

    uint center = best_idx.read(gid).r;
    uint left   = best_idx.read(uint2(gid.x - 1, gid.y)).r;
    uint right  = best_idx.read(uint2(gid.x + 1, gid.y)).r;
    uint top    = best_idx.read(uint2(gid.x, gid.y - 1)).r;
    uint bottom = best_idx.read(uint2(gid.x, gid.y + 1)).r;

    bool is_outlier = (center > left && center > right && center > top && center > bottom) ||
                      (center < left && center < right && center < top && center < bottom);

    if (!is_outlier) { return; }

    uint avg = (left + right + top + bottom + 2) / 4;
    if (avg == center) { return; }

    if (second_idx.read(gid).r == avg) {
        best_coeffs.write(second_coeffs.read(gid), gid);
        best_idx.write(uint4(avg, 0, 0, 0), gid);
    }
}

// ---------------------------------------------------------------------------
// wavelet_denoise: soft thresholding with phase preservation
// Applied to detail subbands only (not LL). Zeroes coefficients below
// threshold, shrinks those above while preserving complex phase angle.
// ---------------------------------------------------------------------------

kernel void wavelet_denoise(
    texture2d<float, access::read_write> coeffs    [[texture(0)]],
    constant float& threshold                      [[buffer(0)]],
    constant MagnitudeDims& dims                   [[buffer(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= dims.width || gid.y >= dims.height) { return; }

    float2 v = coeffs.read(gid).rg;
    float mag_sq = v.x * v.x + v.y * v.y;

    if (mag_sq <= threshold) {
        coeffs.write(float4(0.0f, 0.0f, 0.0f, 1.0f), gid);
    } else {
        // Shrinkage on squared magnitude: preserves phase, sharper cutoff than
        // standard soft thresholding (which operates on |x|).
        float ratio = (mag_sq - threshold) / mag_sq;
        coeffs.write(float4(v * ratio, 0.0f, 1.0f), gid);
    }
}
