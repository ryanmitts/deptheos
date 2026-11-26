use thiserror::Error;

/// GPU failures. The text is technical, for the log; what a person is told
/// is worded in `deptheos_app::words::explain` from the variant and its fields.
#[derive(Error, Debug)]
pub enum GpuError {
    #[error("no suitable GPU adapter found")]
    NoAdapter,

    #[error("GPU device creation: {0}")]
    DeviceCreationFailed(#[from] wgpu::RequestDeviceError),

    /// The device stopped (a driver reset, the GPU removed or hung). Every
    /// later use of the same device fails the same way.
    #[error("GPU device lost: {0}")]
    DeviceLost(String),

    /// wgpu reported that an allocation did not fit in GPU memory.
    #[error("the GPU ran out of memory")]
    OutOfMemory,

    /// A stack of `width`×`height` frames was refused before allocating:
    /// it would need more than the memory the device may use
    /// ([`crate::MemoryBudget::check`]).
    #[error(
        "{width}×{height} frames need about {needed} bytes of GPU memory; the budget is {budget}"
    )]
    OverBudget { width: u32, height: u32, needed: u64, budget: u64 },

    /// A stack stopped partway because the app had only `left` bytes left
    /// before the system would end it (iOS): more than its estimate was
    /// spent, by the stack or by the rest of the app.
    #[error("stopped with {left} bytes of memory left")]
    LowMemory { left: u64 },

    /// A frame larger than one of the device's limits.
    #[error("{width}×{height} frame exceeds {limit}")]
    FrameTooLarge { width: u32, height: u32, limit: FrameLimit },

    /// A validation or internal wgpu error: a bug, not an input problem.
    /// The message holds wgpu's description.
    #[error("GPU error: {0}")]
    Internal(String),

    #[error("buffer mapping: {0}")]
    BufferMappingFailed(String),

    /// A wait for the GPU ran past [`crate::WAIT_LIMIT`]: the GPU hung, or
    /// far more was queued than one wait should cover.
    #[error("the GPU didn't finish within {0:?}")]
    TimedOut(std::time::Duration),

    /// A readback handed over `arrived` of its `rows` rows.
    #[error("a readback got {arrived} of {rows} rows")]
    ShortReadback { arrived: u32, rows: u32 },

    /// An operation this crate does not support for the arguments given:
    /// a caller's bug.
    #[error("unsupported GPU operation: {0}")]
    Unsupported(String),
}

/// A failed wait for the GPU as an error.
pub(crate) fn poll_error(e: wgpu::PollError) -> GpuError {
    match e {
        wgpu::PollError::Timeout => GpuError::TimedOut(crate::WAIT_LIMIT),
        e => GpuError::Internal(format!("waiting for the GPU: {e}")),
    }
}

/// Which device limit a frame is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameLimit {
    /// The largest texture side, in pixels.
    Side { max: u32 },
    /// The largest buffer `what` may be, in bytes, and the bytes it needs.
    Buffer { what: &'static str, bytes: u64, max: u64 },
}

impl std::fmt::Display for FrameLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Side { max } => write!(f, "the GPU's {max} px texture limit"),
            Self::Buffer { what, bytes, max } => {
                write!(f, "the GPU's {max}-byte buffer limit: its {what} needs {bytes} bytes")
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, GpuError>;
