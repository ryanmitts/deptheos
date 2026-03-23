#include <metal_stdlib>
using namespace metal;

#include "ShaderTypes.h"

// ============================================================================
// ECC Alignment — Metal Compute Kernels
// Evangelidis & Psarakis, "Parametric Image Alignment Using Enhanced
// Correlation Coefficient Maximization", IEEE TPAMI 2008.
// Implementation matches OpenCV's findTransformECC (forward-additive, affine).
// ============================================================================

// EccWarpParams, EccReductionParams defined in ShaderTypes.h

// Reduction output layout (36 floats):
//   [0..20]  = 21 upper-triangle Hessian values
//   [21]     = correlation (sum of img_zm * tmpl_zm)
//   [22..27] = imgProj (J^T * img_zm)
//   [28..33] = tmplProj (J^T * tmpl_zm)
//   [34]     = imgNormSq (sum of img_zm^2)
//   [35]     = tmplNormSq (sum of tmpl_zm^2)
constant uint REDUCTION_FLOATS = 36;

// Workgroup size for 1D reduction kernels.
// SoA layout: 36 arrays of 128 floats = 18,432 bytes — fits within 32 KB threadgroup memory.
constant uint ECC_WG_SIZE = 128;

// ============================================================================
// Affine Warp with Mask
//
// Warps image + gradients using hardware bilinear sampling.
// Outputs packed rgba32Float (.r=image, .g=gradX, .b=gradY) + separate mask.
// Out-of-bounds pixels get 0 in all outputs (matching OpenCV BORDER_CONSTANT).
// ============================================================================

kernel void ecc_warp_triple(
    texture2d<float, access::sample> src_image [[texture(0)]],
    texture2d<float, access::sample> src_grad_x [[texture(1)]],
    texture2d<float, access::sample> src_grad_y [[texture(2)]],
    texture2d<float, access::write> dst_packed [[texture(3)]],   // rgba32Float: (img, gx, gy, 0)
    texture2d<float, access::write> dst_mask [[texture(4)]],     // r32Float
    constant EccWarpParams& params [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= params.width || gid.y >= params.height) return;

    float dx = float(gid.x);
    float dy = float(gid.y);
    float src_x = params.inv_a * dx + params.inv_c * dy + params.inv_tx;
    float src_y = params.inv_b * dx + params.inv_d * dy + params.inv_ty;

    int src_w = int(src_image.get_width());
    int src_h = int(src_image.get_height());

    // Bounds check for mask: bilinear neighborhood must be fully in-bounds.
    bool in_bounds = (src_x >= 0.0f && src_y >= 0.0f &&
                      src_x <= float(src_w - 1) && src_y <= float(src_h - 1));

    // Hardware bilinear sampling. Metal pixel centers are at +0.5, and
    // clamp_to_edge ensures edge pixels clamp correctly (matching the
    // original min(x0+1, src_w-1) behavior for in-bounds coordinates).
    constexpr sampler s(coord::pixel, address::clamp_to_edge, filter::linear);
    float2 sample_coord = float2(src_x + 0.5f, src_y + 0.5f);

    float img = in_bounds ? src_image.sample(s, sample_coord).r : 0.0f;
    float gx  = in_bounds ? src_grad_x.sample(s, sample_coord).r : 0.0f;
    float gy  = in_bounds ? src_grad_y.sample(s, sample_coord).r : 0.0f;

    dst_packed.write(float4(img, gx, gy, 0.0f), gid);
    dst_mask.write(float4(in_bounds ? 1.0f : 0.0f, 0.0f, 0.0f, 1.0f), gid);
}

// ============================================================================
// Masked Mean + StdDev — Phase 1 (per-workgroup)
// Only accumulates pixels where mask == 1.
// Output per workgroup: [sum, sum_sq, count]
// ============================================================================

kernel void ecc_stats_phase1(
    texture2d<float, access::read> src [[texture(0)]],
    texture2d<float, access::read> mask [[texture(1)]],
    device float* partials [[buffer(0)]],
    constant uint& pixel_count [[buffer(1)]],
    constant uint& width [[buffer(2)]],
    uint tid [[thread_index_in_threadgroup]],
    uint3 wid [[threadgroup_position_in_grid]],
    uint3 num_wgs [[threadgroups_per_grid]]
) {
    threadgroup float shared_sum[ECC_WG_SIZE];
    threadgroup float shared_sq[ECC_WG_SIZE];
    threadgroup float shared_count[ECC_WG_SIZE];

    float local_sum = 0.0f;
    float local_sq = 0.0f;
    float local_count = 0.0f;

    uint global_tid = wid.x * ECC_WG_SIZE + tid;
    uint stride = num_wgs.x * ECC_WG_SIZE;

    for (uint idx = global_tid; idx < pixel_count; idx += stride) {
        uint px = idx % width;
        uint py = idx / width;
        uint2 coord = uint2(px, py);
        float m = mask.read(coord).r;
        if (m > 0.5f) {
            float val = src.read(coord).r;
            local_sum += val;
            local_sq += val * val;
            local_count += 1.0f;
        }
    }

    shared_sum[tid] = local_sum;
    shared_sq[tid] = local_sq;
    shared_count[tid] = local_count;
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Tree reduction: 128 → 32 (2 steps with barriers)
    for (uint s = ECC_WG_SIZE / 2; s >= 32; s >>= 1) {
        if (tid < s) {
            shared_sum[tid] += shared_sum[tid + s];
            shared_sq[tid] += shared_sq[tid + s];
            shared_count[tid] += shared_count[tid + s];
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }

    // SIMD reduction: 32 → 1 (no barriers needed within a SIMD group)
    if (tid < 32) {
        float sum_val = simd_sum(shared_sum[tid]);
        float sq_val = simd_sum(shared_sq[tid]);
        float count_val = simd_sum(shared_count[tid]);
        if (tid == 0) {
            partials[wid.x * 3] = sum_val;
            partials[wid.x * 3 + 1] = sq_val;
            partials[wid.x * 3 + 2] = count_val;
        }
    }
}

// ============================================================================
// Masked Mean + StdDev — Phase 2
// Output: [mean, stddev, validCount]
// Uses population stddev (no Bessel's correction), matching OpenCV.
// ============================================================================

kernel void ecc_stats_phase2(
    device const float* partials [[buffer(0)]],
    device float* result [[buffer(1)]],
    constant uint& num_groups [[buffer(2)]],
    uint tid [[thread_index_in_threadgroup]]
) {
    threadgroup float shared_sum[ECC_WG_SIZE];
    threadgroup float shared_sq[ECC_WG_SIZE];
    threadgroup float shared_count[ECC_WG_SIZE];

    float local_sum = 0.0f;
    float local_sq = 0.0f;
    float local_count = 0.0f;

    for (uint g = tid; g < num_groups; g += ECC_WG_SIZE) {
        local_sum += partials[g * 3];
        local_sq += partials[g * 3 + 1];
        local_count += partials[g * 3 + 2];
    }

    shared_sum[tid] = local_sum;
    shared_sq[tid] = local_sq;
    shared_count[tid] = local_count;
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Tree reduction: 128 → 32
    for (uint s = ECC_WG_SIZE / 2; s >= 32; s >>= 1) {
        if (tid < s) {
            shared_sum[tid] += shared_sum[tid + s];
            shared_sq[tid] += shared_sq[tid + s];
            shared_count[tid] += shared_count[tid + s];
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }

    // SIMD reduction: 32 → 1
    if (tid < 32) {
        float sum_val = simd_sum(shared_sum[tid]);
        float sq_val = simd_sum(shared_sq[tid]);
        float count_val = simd_sum(shared_count[tid]);
        if (tid == 0) {
            if (count_val > 0.0f) {
                float mean = sum_val / count_val;
                // Population stddev (no Bessel's correction), matching OpenCV
                float variance = (sq_val / count_val) - (mean * mean);
                result[0] = mean;
                result[1] = sqrt(max(variance, 0.0f));
                result[2] = count_val;
            } else {
                result[0] = 0.0f;
                result[1] = 0.0f;
                result[2] = 0.0f;
            }
        }
    }
}

// ============================================================================
// Masked Fused Hessian + Correlation + Projections — Phase 1
//
// Reads packed warp output (.r=image, .g=gradX, .b=gradY) + template + mask.
// Skips masked-out pixels. Uses raw pixel coordinates (no centering).
// SoA threadgroup layout: shared[channel][thread] to avoid bank conflicts.
// ============================================================================

kernel void ecc_hessian_phase1(
    texture2d<float, access::read> packed_warp [[texture(0)]],   // rgba32Float: (img, gx, gy, 0)
    texture2d<float, access::read> template_tex [[texture(1)]],
    texture2d<float, access::read> mask_tex [[texture(2)]],
    device float* partials [[buffer(0)]],
    constant EccReductionParams& params [[buffer(1)]],
    device const float* img_stats [[buffer(2)]],    // [mean, stddev, count] from stats phase2
    device const float* tmpl_stats [[buffer(3)]],   // [mean, stddev, count] from stats phase2
    uint tid [[thread_index_in_threadgroup]],
    uint3 wid [[threadgroup_position_in_grid]]
) {
    // SoA layout: shared[i * ECC_WG_SIZE + tid] — conflict-free bank access
    threadgroup float shared[REDUCTION_FLOATS * ECC_WG_SIZE];

    float acc[REDUCTION_FLOATS];
    for (uint i = 0; i < REDUCTION_FLOATS; i++) acc[i] = 0.0f;

    uint pixel_count = params.pixel_count;
    float img_mean = img_stats[0];
    float tmpl_mean = tmpl_stats[0];

    uint global_tid = wid.x * ECC_WG_SIZE + tid;
    uint stride = params.num_workgroups * ECC_WG_SIZE;

    for (uint idx = global_tid; idx < pixel_count; idx += stride) {
        uint px = idx % params.width;
        uint py = idx / params.width;
        uint2 coord = uint2(px, py);

        // Skip masked-out pixels
        if (mask_tex.read(coord).r < 0.5f) continue;

        float4 pw = packed_warp.read(coord);
        float gx = pw.g;
        float gy = pw.b;
        // Raw pixel coordinates (matching OpenCV — no centering)
        float x = float(px);
        float y = float(py);
        float izm = pw.r - img_mean;
        float tzm = template_tex.read(coord).r - tmpl_mean;

        // Jacobian: [gx*x, gy*x, gx*y, gy*y, gx, gy]
        float j0 = gx * x, j1 = gy * x, j2 = gx * y, j3 = gy * y;

        // Upper triangle of 6x6 Hessian (21 values)
        acc[0]  += j0 * j0;  acc[1]  += j0 * j1;  acc[2]  += j0 * j2;
        acc[3]  += j0 * j3;  acc[4]  += j0 * gx;  acc[5]  += j0 * gy;
        acc[6]  += j1 * j1;  acc[7]  += j1 * j2;  acc[8]  += j1 * j3;
        acc[9]  += j1 * gx;  acc[10] += j1 * gy;  acc[11] += j2 * j2;
        acc[12] += j2 * j3;  acc[13] += j2 * gx;  acc[14] += j2 * gy;
        acc[15] += j3 * j3;  acc[16] += j3 * gx;  acc[17] += j3 * gy;
        acc[18] += gx * gx;  acc[19] += gx * gy;  acc[20] += gy * gy;

        // Correlation
        acc[21] += izm * tzm;

        // Image projection (J^T * img_zm)
        acc[22] += j0 * izm;  acc[23] += j1 * izm;  acc[24] += j2 * izm;
        acc[25] += j3 * izm;  acc[26] += gx * izm;  acc[27] += gy * izm;

        // Template projection (J^T * tmpl_zm)
        acc[28] += j0 * tzm;  acc[29] += j1 * tzm;  acc[30] += j2 * tzm;
        acc[31] += j3 * tzm;  acc[32] += gx * tzm;  acc[33] += gy * tzm;

        // Norms
        acc[34] += izm * izm;
        acc[35] += tzm * tzm;
    }

    // Store in SoA layout
    for (uint i = 0; i < REDUCTION_FLOATS; i++) {
        shared[i * ECC_WG_SIZE + tid] = acc[i];
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Tree reduction: 128 → 32 (2 steps with barriers)
    for (uint s = ECC_WG_SIZE / 2; s >= 32; s >>= 1) {
        if (tid < s) {
            for (uint i = 0; i < REDUCTION_FLOATS; i++) {
                shared[i * ECC_WG_SIZE + tid] += shared[i * ECC_WG_SIZE + tid + s];
            }
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }

    // SIMD reduction: 32 → 1 (no barriers needed within a SIMD group)
    if (tid < 32) {
        for (uint i = 0; i < REDUCTION_FLOATS; i++) {
            float val = simd_sum(shared[i * ECC_WG_SIZE + tid]);
            if (tid == 0) {
                partials[wid.x * REDUCTION_FLOATS + i] = val;
            }
        }
    }
}

// ============================================================================
// Masked Error Projection — Phase 1
// Reads packed warp output (.r=image, .g=gradX, .b=gradY) + template + mask.
// Computes J^T * (lambda * tmpl_zm - img_zm), skipping masked-out pixels.
// SoA threadgroup layout for conflict-free reduction.
// ============================================================================

kernel void ecc_projection_phase1(
    texture2d<float, access::read> packed_warp [[texture(0)]],   // rgba32Float: (img, gx, gy, 0)
    texture2d<float, access::read> template_tex [[texture(1)]],
    texture2d<float, access::read> mask_tex [[texture(2)]],
    device float* partials [[buffer(0)]],
    constant EccReductionParams& params [[buffer(1)]],
    device const float* img_stats [[buffer(2)]],
    device const float* tmpl_stats [[buffer(3)]],
    constant float& lambda [[buffer(4)]],
    uint tid [[thread_index_in_threadgroup]],
    uint3 wid [[threadgroup_position_in_grid]]
) {
    // SoA layout: shared[channel][thread] — conflict-free
    threadgroup float shared[6 * ECC_WG_SIZE];

    float acc[6] = {0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f};

    uint pixel_count = params.pixel_count;
    float img_mean = img_stats[0];
    float tmpl_mean = tmpl_stats[0];
    float lam = lambda;

    uint global_tid = wid.x * ECC_WG_SIZE + tid;
    uint stride = params.num_workgroups * ECC_WG_SIZE;

    for (uint idx = global_tid; idx < pixel_count; idx += stride) {
        uint px = idx % params.width;
        uint py = idx / params.width;
        uint2 coord = uint2(px, py);

        if (mask_tex.read(coord).r < 0.5f) continue;

        float4 pw = packed_warp.read(coord);
        float gx = pw.g;
        float gy = pw.b;
        float x = float(px);
        float y = float(py);
        float izm = pw.r - img_mean;
        float tzm = template_tex.read(coord).r - tmpl_mean;

        float err = lam * tzm - izm;
        float j0 = gx * x, j1 = gy * x, j2 = gx * y, j3 = gy * y;

        acc[0] += j0 * err;  acc[1] += j1 * err;  acc[2] += j2 * err;
        acc[3] += j3 * err;  acc[4] += gx * err;  acc[5] += gy * err;
    }

    // Store in SoA layout
    for (uint i = 0; i < 6; i++) {
        shared[i * ECC_WG_SIZE + tid] = acc[i];
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Tree reduction: 128 → 32
    for (uint s = ECC_WG_SIZE / 2; s >= 32; s >>= 1) {
        if (tid < s) {
            for (uint i = 0; i < 6; i++) {
                shared[i * ECC_WG_SIZE + tid] += shared[i * ECC_WG_SIZE + tid + s];
            }
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }

    // SIMD reduction: 32 → 1
    if (tid < 32) {
        for (uint i = 0; i < 6; i++) {
            float val = simd_sum(shared[i * ECC_WG_SIZE + tid]);
            if (tid == 0) {
                partials[wid.x * 6 + i] = val;
            }
        }
    }
}
