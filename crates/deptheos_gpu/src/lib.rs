//! GPU device management for Deptheos.
//!
//! Owns the engine's one wgpu device; nothing else in the engine creates
//! one. The apps draw the images read back from it on their own devices
//! (ADR-100).

mod context;
mod error;
pub mod gpu_utils;
pub mod profile;
pub mod wgsl_mirror;

pub use context::{
    DeviceChoice, DeviceInfo, DeviceKind, FormatSupport, GpuContext, MemoryBudget, MemoryPool,
    StagedFrames, clear_thread_faults,
};
pub use error::{FrameLimit, GpuError, Result};
