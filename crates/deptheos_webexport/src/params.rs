//! `manifest.params`: the stackers' and aligners' parameters, computed by
//! the app's own functions (`deptheos_core::web_params`, the aligners'
//! configs), so the web side copies no formula or constant.
//!
//! - `constants`: scalar constants by group (pyramid, depth, ecc).
//! - `defaults`, `ranges`: each user setting's default and `{min, max, step}`
//!   (`step` is the app's slider step).
//! - `at`: derived values at the defaults and ends of each setting for
//!   article text.
//!
//! Every f32 is written in its shortest f32 spelling (`1.1`, not the
//! widened `1.100000023841858`), so the values read well in the articles;
//! `Math.fround` on the web side recovers the exact bits.

use deptheos_aligner::{EccConfig, GridConfig};
use deptheos_core::web_params as p;
use serde_json::{Value, json};

pub fn params() -> Value {
    let mut params = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "constants": constants(),
        "defaults": defaults(),
        "ranges": ranges(),
        "at": at(),
    });
    shortest_f32(&mut params);
    params
}

/// Rewrite every number that is exactly an f32 in the shortest decimal
/// that names that f32. The f64 values here (ECC's) are not f32-exact
/// unless their own spelling is already short (0.25), so they keep theirs.
fn shortest_f32(value: &mut Value) {
    match value {
        Value::Number(n) => {
            if let Some(x) = n.as_f64().filter(|x| x.fract() != 0.0 && (*x as f32) as f64 == *x) {
                let short: f64 = (x as f32).to_string().parse().expect("f32 spelling parses");
                assert_eq!(short as f32, x as f32);
                *n = serde_json::Number::from_f64(short).expect("finite");
            }
        }
        Value::Array(items) => items.iter_mut().for_each(shortest_f32),
        Value::Object(fields) => fields.values_mut().for_each(shortest_f32),
        _ => {}
    }
}

fn constants() -> Value {
    let ecc = EccConfig::default();
    json!({
        "pyramid": {
            "level_floor_px": p::PYRAMID_LEVEL_FLOOR_PX,
            "exponent_base": p::PYRAMID_EXPONENT_BASE,
            "coarsest_exponent_ratio": p::PYRAMID_COARSEST_EXPONENT_RATIO,
            "prior_exponent": p::PYRAMID_PRIOR_EXPONENT,
        },
        "depth": {
            "map_scale": p::DEPTH_MAP_SCALE,
        },
        "ecc": {
            "min_correlation": ecc.min_correlation,
            "max_shift_pct": ecc.max_shift_pct,
            "max_scale_pct": ecc.max_scale_pct,
            "max_rotation_deg": ecc.max_rotation_deg,
        },
    })
}

fn defaults() -> Value {
    let depth = p::DepthConfig::default();
    let grid = GridConfig::default();
    json!({
        "pyramid": { "smoothness": p::PyramidConfig::default().smoothness },
        "depth": {
            "radius": depth.radius,
            "smoothing": depth.smoothing,
            "blend_range": depth.blend_range,
        },
        "grid": {
            "horizontal_pct": grid.horizontal_pct,
            "vertical_pct": grid.vertical_pct,
            "rotation_deg": grid.rotation_deg,
            "magnification_pct": grid.magnification_pct,
        },
    })
}

fn range((min, max): (f32, f32), step: f32) -> Value {
    json!({ "min": min, "max": max, "step": step })
}

fn ranges() -> Value {
    json!({
        "pyramid": { "smoothness": range(p::SMOOTHNESS_RANGE, p::SMOOTHNESS_STEP) },
        "depth": {
            "radius": range(p::RADIUS_RANGE, p::RADIUS_STEP),
            "smoothing": range(p::SMOOTHING_RANGE, p::SMOOTHING_STEP),
            "blend_range": range(p::BLEND_RANGE, p::BLEND_RANGE_STEP),
        },
        "grid": {
            "shift_pct": range(GridConfig::SHIFT_PCT_RANGE, GridConfig::SHIFT_PCT_STEP),
            "rotation_deg": range(GridConfig::ROTATION_DEG_RANGE, GridConfig::ROTATION_DEG_STEP),
            "magnification_pct": range(
                GridConfig::MAGNIFICATION_PCT_RANGE,
                GridConfig::MAGNIFICATION_PCT_STEP
            ),
        },
    })
}

/// The integers in an inclusive range.
fn integers((min, max): (f32, f32)) -> impl Iterator<Item = f32> {
    assert!(min.fract() == 0.0 && max.fract() == 0.0, "table range {min}..={max} not integral");
    (min as i32..=max as i32).map(|v| v as f32)
}

/// Derived values at the default, lowest and highest settings, by name, so
/// article text can quote "the exponent at the default" or "the lowest
/// smoothness that blurs the score".
fn at() -> Value {
    let pyramid_row = |s: f32| {
        let finest_exponent = p::pyramid_exponents(s, p::PYRAMID_MIN_LEVELS)[0];
        json!({
            "finest_exponent": finest_exponent,
        })
    };
    let blur_from = integers(p::SMOOTHNESS_RANGE).find(|&s| p::pyramid_focus_blur(s).0 > 0.0);
    let post_blur_sigma = p::post_blur_sigma(p::DepthConfig::DEFAULT_RADIUS);
    json!({
        "depth": {
            "default": {
                "post_blur_sigma": post_blur_sigma,
                "post_blur_half_width": p::blur_half_width(post_blur_sigma),
            },
        },
        "pyramid": {
            "default": pyramid_row(p::PyramidConfig::DEFAULT_SMOOTHNESS),
            "min": pyramid_row(p::SMOOTHNESS_RANGE.0),
            "max": pyramid_row(p::SMOOTHNESS_RANGE.1),
            "score_blur_from": blur_from,
        },
    })
}
