//! The build-script helpers deptheos_raw and deptheos_dng share: collecting
//! a vendored library's sources, the compiler settings of vendored code and
//! of ours, and the per-user cache of the native libraries they build.

pub mod build_sources;
pub mod flags;
pub mod native_cache;
