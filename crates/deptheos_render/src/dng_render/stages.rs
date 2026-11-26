//! The adjustment stages' numbers as the shaders take them, and the WGSL
//! module of the Rust constants the shaders read, written from the Rust
//! values when a pipeline is built so the two can't drift.

use std::fmt::Write;

use glam::Vec4;

use super::standard;

/// What the resolve step works out from the sliders for the shaders
/// (`crate::resolve`). With every stage `None` a linear stack draws
/// exactly as the SDK renders it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Stages {
    /// A standard stack's white balance, in linear ProPhoto.
    pub(crate) white_balance: Option<crate::white::Mat3>,
    pub(crate) exposure: Option<Exposure>,
}

/// Exposure's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Exposure {
    /// Stops the SDK's ramp and darkening curve take on a linear stack, in
    /// place of its baseline.
    pub(crate) ramp: f64,
    /// The linear gain after the ramp.
    pub(crate) gain: f64,
    /// Where the shoulder starts, below 1; 1 clamps at white.
    pub(crate) knee: f64,
}

impl Exposure {
    pub(super) fn gain_knee(self) -> Vec4 {
        Vec4::new(self.gain as f32, self.knee as f32, 0.0, 0.0)
    }
}

/// The import path of [`constants_module`].
pub(super) const CONSTANTS_MODULE: &str = "deptheos::render_constants";

/// The WGSL module of the constants the shaders share with Rust.
pub(super) fn constants_module() -> String {
    let mut wgsl = format!("#define_import_path {CONSTANTS_MODULE}\n\n");
    let mut u32_const = |name: &str, value: usize| {
        let value = u32::try_from(value).expect("a u32 constant");
        writeln!(wgsl, "const {name}: u32 = {value}u;").unwrap();
    };
    u32_const("CODES", standard::CODES);
    wgsl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_are_the_rust_values() {
        let wgsl = constants_module();
        assert!(wgsl.contains("const CODES: u32 = 65536u;"), "{wgsl}");
    }
}
