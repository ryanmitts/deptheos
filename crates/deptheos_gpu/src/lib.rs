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
    DeviceChoice, DeviceInfo, DeviceKind, Drawing, FormatSupport, GpuContext, MemoryBudget,
    MemoryPool, POOL_LIMIT, PooledTexture, StagedFrames, clear_thread_faults, trim_pools,
};
pub use error::{FrameLimit, GpuError, Result};

/// The longest any wait for the GPU blocks, a readback's included, before
/// it fails with [`GpuError::TimedOut`]. No one submission comes near it.
pub const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);
