//! The color render: what turns a render's stored pixels into the colors
//! shown and exported. A crate of its own so the stackers can't reach it
//! (`deptheos_core` doesn't depend on it).

pub mod dng_render;
