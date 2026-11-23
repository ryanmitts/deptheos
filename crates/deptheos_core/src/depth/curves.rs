//! The depth stacker's curves: settings to blur widths (`crate::focus`),
//! the map grid, and the blend pyramid's level count.

use crate::focus::{iir_blur_sigma, iir_k1};

// --- The focus blurs ---------------------------------------------------------
//
// The post-blur pools the metric before the WTA. The depth-map smoothing
// can't replace it: in low texture every frame scores ~0 plus noise, the
// WTA picks at random, and smoothing doesn't repair that. A ground-truth
// depth score can't see it either.

/// The focus blurs' arguments are `radius · RADIUS_BLUR_SCALE` times a
/// factor: [`PRE_BLUR_FACTOR`] before the bandpass, [`POST_BLUR_FACTOR`]
/// after it.
///
/// Much smaller and the metric pools under a map pixel, so faint texture
/// (insect wings, silk) loses to a nearby edge's defocused halo.
const RADIUS_BLUR_SCALE: f32 = 2.4;
const PRE_BLUR_FACTOR: f32 = 0.5;
const POST_BLUR_FACTOR: f32 = 2.0;

/// Focus pre-blur σ: blur argument `radius · 2.4 · 0.5`.
pub(super) fn iir_pre_blur_sigma(radius: f32) -> f32 {
    iir_blur_sigma(radius * RADIUS_BLUR_SCALE * PRE_BLUR_FACTOR)
}

/// Focus post-blur σ: blur argument `radius · 2.4 · 2`.
pub(crate) fn iir_post_blur_sigma(radius: f32) -> f32 {
    iir_blur_sigma(radius * RADIUS_BLUR_SCALE * POST_BLUR_FACTOR)
}

/// The map grid's longest side: it bounds the serial smoothing sweeps, which
/// could otherwise hit a driver's watchdog timeout.
const MAX_MAP_SIDE: f32 = 6000.0;
/// The map grid's scale below the cap: half resolution.
pub(crate) const MAP_SCALE: f32 = 0.5;
/// Added to `dim · scale` before truncating (see [`map_grid`]).
const MAP_SIDE_BIAS: f32 = 0.01;

/// The map grid: the frame at [`MAP_SCALE`], capped at [`MAX_MAP_SIDE`].
/// The 0.01 bias keeps `w · (6000/w)` from flooring to 5999. Returns
/// `(width, height, scale)`.
pub(super) fn map_grid(width: u32, height: u32) -> (u32, u32, f32) {
    let scale = MAP_SCALE.min(MAX_MAP_SIDE / width.max(height) as f32);
    let map_width = ((width as f32 * scale + MAP_SIDE_BIAS) as u32).max(1);
    let map_height = ((height as f32 * scale + MAP_SIDE_BIAS) as u32).max(1);
    (map_width, map_height, scale)
}

/// The weak smoothing blur's argument per unit of smoothing, `0.75 / 32`.
const SMOOTHING_WEAK_SCALE: f32 = 0.75 * 0.03125;
/// The strong blur's argument is the weak one's times this.
const SMOOTHING_STRONG_FACTOR: f32 = 64.0;
/// Cascaded smoothing passes, each horizontal then vertical.
pub(super) const SMOOTHING_PASSES: usize = 2;

/// `(k1_weak, k1_strong)` for a smoothing value; confidence 1 selects weak.
pub(super) fn iir_smoothing_k1(smoothing: f32) -> (f32, f32) {
    let weak = smoothing * SMOOTHING_WEAK_SCALE;
    (iir_k1(weak), iir_k1(weak * SMOOTHING_STRONG_FACTOR))
}

/// The feathering power: 1 (blend range 10) leaves depths as they are.
pub(super) fn feather_power(blend_range: f32) -> f32 {
    1.0 + (10.0 - blend_range) / 3.0
}

/// The blend pyramids stop halving once a side is at most this many pixels.
const BLEND_LEVEL_FLOOR_PX: u32 = 128;
/// And have at most this many levels.
const BLEND_MAX_LEVELS: usize = 32;

/// Level count of the pass-2 pyramids: halve (rounding up) until a side is
/// ≤ 128 px, at most 32 levels.
pub(super) fn blend_pyramid_levels(width: u32, height: u32) -> usize {
    let (mut w, mut h, mut levels) = (width, height, 1);
    while levels < BLEND_MAX_LEVELS && w.min(h) > BLEND_LEVEL_FLOOR_PX {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        levels += 1;
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::focus::iir_sigma;

    /// Pass-2 pyramid depth: halve until a side is ≤ 128 px.
    #[test]
    fn blend_pyramid_level_rule() {
        assert_eq!(blend_pyramid_levels(6000, 4000), 6); // coarsest 188×125
        assert_eq!(blend_pyramid_levels(512, 512), 3); // 512, 256, 128
        assert_eq!(blend_pyramid_levels(100, 100), 1);
        assert_eq!(blend_pyramid_levels(129, 4000), 2);
    }

    /// The blur arguments at a few radius/smoothing values.
    #[test]
    fn iir_parameter_mapping() {
        assert!((iir_pre_blur_sigma(8.0) - 2.992).abs() < 2e-3);
        assert!((iir_post_blur_sigma(8.0) - 11.723).abs() < 5e-3);
        assert!((iir_post_blur_sigma(20.0) - 29.152).abs() < 5e-3);
        let (weak, strong) = iir_smoothing_k1(10.0);
        assert!((iir_sigma(weak) - 0.708).abs() < 2e-3);
        assert!((iir_sigma(strong) - 4.627).abs() < 5e-3);
    }

    /// The largest radius's blurs fit the metric shader's weight table
    /// (the post-blur at radius 50 has half-width 216 of 255).
    #[test]
    fn largest_radius_fits_the_blur_table() {
        let radius = super::super::RADIUS_RANGE.1;
        for sigma in [iir_pre_blur_sigma(radius), iir_post_blur_sigma(radius)] {
            let half = crate::focus::blur_params(sigma).half_width;
            assert!(half <= crate::focus::MAX_BLUR_HALF_WIDTH, "σ {sigma}: {half}");
        }
    }
}
