//! The integration tests as one binary: separate ones would each link wgpu
//! and the native libraries, and run one after another.

mod formats;
mod history;
mod paint;
mod sequence;
mod session;
