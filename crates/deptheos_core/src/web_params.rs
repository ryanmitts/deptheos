//! The stackers' parameter math and constants, for the website's text.
//!
//! `deptheos_webexport` writes these into the website's manifest (ADR-160),
//! so the articles cite the numbers the stackers use.

use crate::{depth, focus, pyramid};

pub use crate::depth::{BLEND_RANGE, DepthConfig, RADIUS_RANGE, SMOOTHING_RANGE};
pub use crate::depth::{BLEND_RANGE_STEP, RADIUS_STEP, SMOOTHING_STEP};
pub use crate::pyramid::{PyramidConfig, SMOOTHNESS_RANGE, SMOOTHNESS_STEP};

// Pyramid stacker.
/// Levels go down until the shorter side is about this many pixels.
pub const PYRAMID_LEVEL_FLOOR_PX: f32 = pyramid::LEVEL_FLOOR_PX;
pub const PYRAMID_MIN_LEVELS: usize = pyramid::MIN_LEVELS;
/// The finest level's WTA exponent is this minus the smoothness.
pub const PYRAMID_EXPONENT_BASE: f32 = pyramid::EXPONENT_BASE;
/// The coarsest level's exponent is the finest's times this.
pub const PYRAMID_COARSEST_EXPONENT_RATIO: f32 = pyramid::COARSEST_EXPONENT_RATIO;
/// The focus prior's exponent at every level but the coarsest.
pub const PYRAMID_PRIOR_EXPONENT: f32 = pyramid::PRIOR_EXPONENT;

/// The focus blur (σ, half-width; 0 = none) for a smoothness.
pub fn pyramid_focus_blur(smoothness: f32) -> (f32, u32) {
    let blur = pyramid::focus_blur(smoothness);
    (blur.sigma, blur.half_width)
}

/// The WTA exponent at every level of a `levels`-level pyramid (≥ 2).
pub fn pyramid_exponents(smoothness: f32, levels: usize) -> Vec<f32> {
    pyramid::exponent_schedule(smoothness, levels)
}

// Depth-map stacker.
pub const DEPTH_MAP_SCALE: f32 = depth::MAP_SCALE;

/// Focus post-blur σ for a radius.
pub fn post_blur_sigma(radius: f32) -> f32 {
    depth::iir_post_blur_sigma(radius)
}

/// Half-width of the Gaussian kernel the focus blurs use for `sigma`.
pub fn blur_half_width(sigma: f32) -> u32 {
    focus::blur_half_width(sigma)
}
