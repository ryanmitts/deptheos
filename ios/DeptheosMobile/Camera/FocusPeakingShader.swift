// Metal source for FocusPeaking, compiled at runtime with
// `MTLDevice.makeLibrary(source:options:)` (no offline Metal compiler in this
// build). Each kernel is a step of the pipeline in FocusPeaking.swift; Metal
// Performance Shaders do the box means, blur and frame statistics between.

nonisolated enum FocusPeakingShader {
    static let source = """
    #include <metal_stdlib>
    using namespace metal;

    // ---------------------------------------------------------------------
    // Parameter blocks (must match the Swift structs in FocusPeaking.swift).
    // ---------------------------------------------------------------------
    struct LumaParams {
        float2 tapOffset;   // one destination pixel, in UV units (halved per tap)
        uint   multiTap;    // 1 = average four taps (downscaling), 0 = one tap
    };

    struct CoefParams {
        float epsilon;      // guided-filter regularizer (variance units)
    };

    struct GradParams {
        int   coarseStep;   // pixel spacing of the coarse gradient
        float gain;         // gradients are scaled by this before squaring (half-float range)
    };

    static inline bool nearBorder(uint2 gid, uint w, uint h, uint margin) {
        return gid.x < margin || gid.y < margin || gid.x + margin >= w || gid.y + margin >= h;
    }

    struct ScoreParams {
        float floorScale;   // noise energy = max(floorMin, floorScale * frame mean fine energy)
        float floorMin;
        float coarseWeight; // multiple of coarse energy subtracted from fine energy
        float contrastMin;  // a pixel's own fine energy must exceed this x noise energy
        float gateFraction; // ... and this x the local mean fine energy (keeps marks on edges)
        uint  margin;       // no score within this many pixels of the frame border
    };

    struct OverlayParams {
        float3 absolute;    // per-tier absolute thresholds on the score
        float3 spread;      // per-tier "this many standard deviations above mean"
        float4 color0;      // somewhat sharp
        float4 color1;      // clearly sharp
        float4 color2;      // very sharp
        uint   margin;      // nothing is marked within this many pixels of the border
    };

    // ---------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------
    // Gamma-encoded Rec. 709 luma. Encoded rather than linear light: sRGB
    // encoding roughly evens out sensor noise between shadows and highlights,
    // so one noise floor fits the whole frame.
    static inline float lumaOf(float3 rgb) {
        // deptheos_aligner::REC709_LUMA.
        return dot(rgb, float3(0.2126, 0.7152, 0.0722));
    }

    static inline float readClamped(texture2d<float, access::read> t, int2 p) {
        int2 hi = int2(t.get_width() - 1, t.get_height() - 1);
        return t.read(uint2(clamp(p, int2(0), hi))).r;
    }

    // ---------------------------------------------------------------------
    // 1. Resample the camera frame to the output size and reduce it to one
    //    channel. Writes (L, L*L) so the box filter can produce local means of
    //    both in one pass for the guided filter.
    // ---------------------------------------------------------------------
    kernel void fpLuma(texture2d<float, access::sample> src [[texture(0)]],
                       texture2d<float, access::write>  dst [[texture(1)]],
                       constant LumaParams &p             [[buffer(0)]],
                       uint2 gid [[thread_position_in_grid]]) {
        if (gid.x >= dst.get_width() || gid.y >= dst.get_height()) return;
        constexpr sampler s(coord::normalized, address::clamp_to_edge, filter::linear);
        float2 uv = (float2(gid) + 0.5) / float2(dst.get_width(), dst.get_height());
        float l;
        if (p.multiTap != 0) {
            float2 o = p.tapOffset * 0.5;
            l = 0.25 * (lumaOf(src.sample(s, uv + float2(-o.x, -o.y)).rgb)
                      + lumaOf(src.sample(s, uv + float2( o.x, -o.y)).rgb)
                      + lumaOf(src.sample(s, uv + float2(-o.x,  o.y)).rgb)
                      + lumaOf(src.sample(s, uv + float2( o.x,  o.y)).rgb));
        } else {
            l = lumaOf(src.sample(s, uv).rgb);
        }
        dst.write(float4(l, l * l, 0, 0), gid);
    }

    // ---------------------------------------------------------------------
    // 2. Guided filter, image as its own guide: from local means of I and I^2,
    //    a = var / (var + eps), b = mean * (1 - a).
    // ---------------------------------------------------------------------
    kernel void fpGuideCoefficients(texture2d<float, access::read>  means [[texture(0)]],
                                    texture2d<float, access::write> coef  [[texture(1)]],
                                    constant CoefParams &p               [[buffer(0)]],
                                    uint2 gid [[thread_position_in_grid]]) {
        if (gid.x >= coef.get_width() || gid.y >= coef.get_height()) return;
        float2 m = means.read(gid).rg;
        float var = max(m.y - m.x * m.x, 0.0);
        float a = var / (var + p.epsilon);
        coef.write(float4(a, m.x * (1.0 - a), 0, 0), gid);
    }

    // 3. Apply the averaged coefficients: q = mean(a) * I + mean(b).
    kernel void fpGuideApply(texture2d<float, access::read>  luma  [[texture(0)]],
                             texture2d<float, access::read>  coefM [[texture(1)]],
                             texture2d<float, access::write> dst   [[texture(2)]],
                             uint2 gid [[thread_position_in_grid]]) {
        if (gid.x >= dst.get_width() || gid.y >= dst.get_height()) return;
        float2 ab = coefM.read(gid).rg;
        dst.write(float4(ab.x * luma.read(gid).r + ab.y), gid);
    }

    // ---------------------------------------------------------------------
    // 4. Fine and coarse gradient. For each of four directions, the fine
    //    gradient is the larger one-pixel step to either side; the coarse
    //    gradient is the larger k-pixel step to either side divided by k (the
    //    average slope over k pixels). The direction with the strongest fine
    //    step wins. Written squared ("energy"), so that summing over a window
    //    rewards contrast packed into few pixels: a sharp step has about k^2
    //    times more fine than coarse energy, a soft ramp about the same.
    //
    //    Near the border the k-pixel reads would be clamped, which shortens
    //    the coarse step and fakes sharpness, so those pixels write zero.
    // ---------------------------------------------------------------------
    kernel void fpGradients(texture2d<float, access::read>  img    [[texture(0)]],
                            texture2d<float, access::write> dst    [[texture(1)]],
                            constant GradParams &p                [[buffer(0)]],
                            uint2 gid [[thread_position_in_grid]]) {
        uint w = dst.get_width(), h = dst.get_height();
        if (gid.x >= w || gid.y >= h) return;
        if (nearBorder(gid, w, h, uint(p.coarseStep) + 1)) {
            dst.write(float4(0), gid);
            return;
        }
        int2 c = int2(gid);
        float center = img.read(gid).r;
        const int2 dirs[4] = { int2(1, 0), int2(0, 1), int2(1, 1), int2(1, -1) };
        const float lens[4] = { 1.0, 1.0, 1.41421356, 1.41421356 };
        float bestFine = 0.0, bestCoarse = 0.0;
        for (int i = 0; i < 4; i++) {
            int2 d = dirs[i];
            float f = max(abs(readClamped(img, c + d) - center),
                          abs(center - readClamped(img, c - d))) / lens[i];
            int2 dk = d * p.coarseStep;
            float g = max(abs(readClamped(img, c + dk) - center),
                          abs(center - readClamped(img, c - dk))) / (lens[i] * float(p.coarseStep));
            if (f > bestFine) { bestFine = f; bestCoarse = g; }
        }
        float2 e = float2(bestFine, bestCoarse) * p.gain;
        dst.write(float4(e * e, 0, 0), gid);
    }

    // ---------------------------------------------------------------------
    // 5. Sharpness score from window-averaged energies: how far fine energy
    //    exceeds coarse energy, relative to coarse energy plus a noise floor
    //    taken from this frame's mean fine energy. Dimensionless, so it does
    //    not grow with contrast. Only pixels that sit on an edge themselves
    //    (their own fine energy is a fair share of the window's) keep a score.
    // ---------------------------------------------------------------------
    kernel void fpScore(texture2d<float, access::read>  grad      [[texture(0)]],
                        texture2d<float, access::read>  gradAvg   [[texture(1)]],
                        texture2d<float, access::read>  stats     [[texture(2)]],
                        texture2d<float, access::write> dst       [[texture(3)]],
                        constant ScoreParams &p                  [[buffer(0)]],
                        uint2 gid [[thread_position_in_grid]]) {
        uint w = dst.get_width(), h = dst.get_height();
        if (gid.x >= w || gid.y >= h) return;
        if (nearBorder(gid, w, h, p.margin)) { dst.write(float4(0), gid); return; }
        float noise = max(p.floorMin, p.floorScale * stats.read(uint2(0, 0)).r);
        float2 avg = gradAvg.read(gid).rg;
        float own = grad.read(gid).r;
        float s = max(avg.x - p.coarseWeight * avg.y, 0.0) / (avg.y + noise);
        s *= smoothstep(p.contrastMin, 2.0 * p.contrastMin, own / noise);
        s *= smoothstep(0.5 * p.gateFraction, p.gateFraction, own / max(avg.x, 1e-6));
        dst.write(float4(s), gid);
    }

    // ---------------------------------------------------------------------
    // 6. Overlay: compare the grown score with per-tier thresholds that
    //    are the larger of an absolute level and (frame mean + n * std dev).
    // ---------------------------------------------------------------------
    kernel void fpOverlay(texture2d<float, access::read>  score   [[texture(0)]],
                          texture2d<float, access::read>  stats   [[texture(1)]],
                          texture2d<float, access::write> dst     [[texture(2)]],
                          constant OverlayParams &p            [[buffer(0)]],
                          uint2 gid [[thread_position_in_grid]]) {
        uint w = dst.get_width(), h = dst.get_height();
        if (gid.x >= w || gid.y >= h) return;
        if (nearBorder(gid, w, h, p.margin)) { dst.write(float4(0), gid); return; }
        float mean = stats.read(uint2(0, 0)).r;
        float sd = sqrt(max(stats.read(uint2(1, 0)).r, 0.0));
        float3 t = max(p.absolute, mean + p.spread * sd);
        float s = score.read(gid).r;
        float4 o = float4(0);
        if (s > t.z)      o = p.color2;
        else if (s > t.y) o = p.color1;
        else if (s > t.x) o = p.color0;
        dst.write(o, gid);
    }
    """
}
