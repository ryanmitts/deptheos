// The pyramid stacker's Laplacian and focus metric: the 5-tap Gaussian, the
// parity upsample that predicts a level from the next coarser one, and the
// relative Laplacian the soft winner-take-all selects by.

#define_import_path deptheos::laplacian

#import deptheos::metric::DARK_FLOOR

// Separable 5-tap binomial Gaussian, [1 4 6 4 1] / 16.
const GAUSSIAN: array<f32, 5> = array<f32, 5>(0.0625, 0.25, 0.375, 0.25, 0.0625);

// Burt & Adelson's EXPAND: only taps on even fine positions exist in the
// coarse level, and per axis they sum to 0.5, so the result is scaled by 4.
// A fixed 3×3 with parity weights (even (1, 6, 1)/16, odd (0, 4, 4)/16)
// keeps neighboring threads of different parity from diverging.
fn gaussian_upsample(gid: vec2u, coarse: texture_2d<f32>) -> vec4f {
    let cmax = vec2i(textureDimensions(coarse)) - 1;
    let c = vec2i(gid / 2u);
    var wx = array<f32, 3>(GAUSSIAN[0], GAUSSIAN[2], GAUSSIAN[4]);
    if ((gid.x & 1u) == 1u) {
        wx = array<f32, 3>(0.0, GAUSSIAN[1], GAUSSIAN[3]);
    }
    var wy = array<f32, 3>(GAUSSIAN[0], GAUSSIAN[2], GAUSSIAN[4]);
    if ((gid.y & 1u) == 1u) {
        wy = array<f32, 3>(0.0, GAUSSIAN[1], GAUSSIAN[3]);
    }

    var sum = vec4f(0.0);
    for (var jj: i32 = 0; jj < 3; jj++) {
        let y = clamp(c.y - 1 + jj, 0, cmax.y);
        for (var ii: i32 = 0; ii < 3; ii++) {
            let x = clamp(c.x - 1 + ii, 0, cmax.x);
            sum += textureLoad(coarse, vec2i(x, y), 0) * (wx[ii] * wy[jj]);
        }
    }
    return sum * 4.0;
}

// The focus metric: the mean per-channel relative Laplacian
// |G - U| / (U + DARK_FLOOR), so a bright defocused region can't out-score a
// dark focused one. Selection only: the pyramid stores an ordinary Laplacian.
fn focus_metric(lap_pixel: vec4f, upsampled: vec4f) -> f32 {
    let denom = max(upsampled.rgb, vec3f(0.0)) + vec3f(DARK_FLOOR);
    let rel = abs(lap_pixel.rgb) / denom;
    return (rel.r + rel.g + rel.b) / 3.0;
}
