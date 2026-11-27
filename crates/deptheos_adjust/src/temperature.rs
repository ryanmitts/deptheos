//! A standard stack's relative temperature against kelvins. Its scene's
//! temperature is unknown, so the slider runs from -100 to 100 around
//! 6500 K, each step the same number of mireds, so equal steps look equal.
//! The scale is lightcraft's (MIT OR Apache-2.0: lightcraft 5bdb4e7a6
//! `crates/ui-egui/src/panels/edit/sliders.rs`).

/// The temperature the relative scale's 0 stands for.
pub const REFERENCE_KELVIN: f64 = 6500.0;
/// Mireds per step of the relative scale: +100 is 80 mireds warmer.
pub const MIREDS_PER_STEP: f64 = 0.8;

/// Kelvins in mireds, and back: the one is a million over the other.
pub fn mireds(kelvin: f64) -> f64 {
    1e6 / kelvin
}

/// A relative temperature's kelvins: positive is warmer.
pub fn relative_to_kelvin(relative: f64) -> f64 {
    mireds(mireds(REFERENCE_KELVIN) - relative * MIREDS_PER_STEP)
}

/// Kelvins on the relative scale, within -100 to 100.
pub fn kelvin_to_relative(kelvin: f64) -> f64 {
    let relative = (mireds(REFERENCE_KELVIN) - mireds(kelvin)) / MIREDS_PER_STEP;
    relative.clamp(-100.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relative_scale_is_even_in_mireds() {
        assert_eq!(relative_to_kelvin(0.0), REFERENCE_KELVIN);
        // +50: 40 mireds from 6500 K.
        let warmer = relative_to_kelvin(50.0);
        assert!((mireds(REFERENCE_KELVIN) - mireds(warmer) - 40.0).abs() < 1e-9);
        for r in (-100..=100).map(f64::from) {
            assert!((kelvin_to_relative(relative_to_kelvin(r)) - r).abs() < 1e-9, "{r}");
        }
        assert!(relative_to_kelvin(100.0) > relative_to_kelvin(99.0));
        assert_eq!(kelvin_to_relative(1e6), 100.0);
        assert_eq!(kelvin_to_relative(1000.0), -100.0);
    }
}
