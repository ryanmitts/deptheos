//! The color render: what turns a render's stored pixels into the colors
//! shown and exported. A crate of its own so the stackers can't reach it
//! (`deptheos_core` doesn't depend on it).

mod chroma;
mod curve;
mod curve_fits;
pub mod dng_render;
pub mod local_tone;
mod resolve;
// The law per pixel is the shaders' reference, read by the tests.
#[cfg_attr(not(test), allow(dead_code))]
mod vignette;
mod white;

pub use deptheos_adjust::{
    Adjustments, ProcessVersion, Reframe, Sharpening, Vignette, WhiteBalance,
};
pub use deptheos_adjust::{reframe, slider, temperature};
pub use resolve::{Color, Draw, OutputSpace, resolve, resolve_adjusted};
