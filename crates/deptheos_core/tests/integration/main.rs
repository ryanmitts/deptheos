//! The integration tests, as one binary: separate ones would each link
//! wgpu and the native libraries, and run one after another.

mod common;

mod alignment;
mod contribution;
mod corrupt;
mod dcp;
mod decode;
mod depth;
mod dng_render;
mod golden;
mod hard;
mod known;
mod pyramid;
mod staging;
mod upload;
mod warp;
mod wavelet;
