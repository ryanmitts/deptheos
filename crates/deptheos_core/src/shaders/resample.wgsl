// Fixed 2:1 resampling for the depth stacker's blend pyramid: destination X
// is centered at source (X + 0.5)/scale − 0.5, the kernel is stretched when
// downscaling, and out-of-image taps are dropped and the rest renormalized
// per axis (not clamped), so edges are not over-weighted.
//
// Kernel: Gaussian σ = 0.666 truncated at |x| ≤ 2, keeping a level's detail
// while suppressing 2:1 aliasing.

#define_import_path deptheos::resample

// g(d) = exp(−d² / (2·0.666²)) at |d| = 0.25, 0.75, 1.25, 1.75, unnormalized.
const GAUSS_HALF: array<f32, 4> = array<f32, 4>(0.931970, 0.530424, 0.171816, 0.031676);

// Down 2:1 (the tiled `gauss_down2` in the depth shader): center 2X + 0.5,
// taps 2X−3 … 2X+4 at arguments 1.75 … −1.75 (2X+5 is beyond the support).
const DOWN_K: array<i32, 8> = array<i32, 8>(3, 2, 1, 0, 0, 1, 2, 3);

// Up 2:1: center X/2 − 0.25, kernel at unit scale. Even X = 2m samples
// m−2 … m+1 at 1.75, 0.75, −0.25, −1.25; odd X samples m−1 … m+2 at
// 1.25, 0.25, −0.75, −1.75.
const UP_EVEN_K: array<i32, 4> = array<i32, 4>(3, 1, 0, 2);
const UP_ODD_K: array<i32, 4> = array<i32, 4>(2, 0, 1, 3);

/// 2:1 upsample of `src` at destination pixel `dst` (destination ≤ 2·src).
fn gauss2_up(src: texture_2d<f32>, dst: vec2i) -> vec4f {
    let n = vec2i(textureDimensions(src));
    let m = dst >> vec2u(1u);
    let odd = (dst & vec2i(1)) == vec2i(1);
    let first = m - select(vec2i(2), vec2i(1), odd);

    var wx: array<f32, 4>;
    var wy: array<f32, 4>;
    var sx = 0.0;
    var sy = 0.0;
    for (var j: i32 = 0; j < 4; j++) {
        let gx = GAUSS_HALF[select(UP_EVEN_K[j], UP_ODD_K[j], odd.x)];
        let gy = GAUSS_HALF[select(UP_EVEN_K[j], UP_ODD_K[j], odd.y)];
        let x = first.x + j;
        let y = first.y + j;
        wx[j] = select(0.0, gx, x >= 0 && x < n.x);
        wy[j] = select(0.0, gy, y >= 0 && y < n.y);
        sx += wx[j];
        sy += wy[j];
    }
    // A destination past 2·src has no tap inside: take the nearest pixel.
    if (sx <= 0.0 || sy <= 0.0) {
        let p = clamp(m, vec2i(0), n - 1);
        return textureLoad(src, vec2u(p), 0);
    }

    var acc = vec4f(0.0);
    for (var j: i32 = 0; j < 4; j++) {
        if (wy[j] == 0.0) {
            continue;
        }
        let y = u32(first.y + j);
        for (var i: i32 = 0; i < 4; i++) {
            if (wx[i] == 0.0) {
                continue;
            }
            acc += textureLoad(src, vec2u(u32(first.x + i), y), 0) * (wx[i] * wy[j]);
        }
    }
    return acc / (sx * sy);
}

/// 2×2 box average of the red channel. On an odd size the right edge drops
/// the missing column (divisor 2); the bottom edge re-reads the last row.
fn box2_down_r(src: texture_2d<f32>, dst: vec2i) -> f32 {
    let n = vec2i(textureDimensions(src));
    let x0 = 2 * dst.x;
    let x1 = x0 + 1;
    let y0 = 2 * dst.y;
    let y1 = min(y0 + 1, n.y - 1);
    let left = textureLoad(src, vec2u(u32(x0), u32(y0)), 0).r
        + textureLoad(src, vec2u(u32(x0), u32(y1)), 0).r;
    if (x1 >= n.x) {
        return left * 0.5;
    }
    let right = textureLoad(src, vec2u(u32(x1), u32(y0)), 0).r
        + textureLoad(src, vec2u(u32(x1), u32(y1)), 0).r;
    return (left + right) * 0.25;
}
