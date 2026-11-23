//! Focus stacking engine: algorithms, GPU pipelines, image I/O.
//!
//! Three stackers share the upload, warp and map plumbing: a Laplacian
//! pyramid with soft winner-take-all ([`pyramid`]), a depth-map stacker
//! that picks a frame per pixel and blends neighbors across a pyramid
//! ([`depth`]), and a complex-wavelet merge with color reassignment
//! ([`wavelet`]).

mod accum;
pub mod cancel;
pub mod contribution;
pub mod converter;
pub mod dcp;
pub mod depth;
pub mod dng_render;
mod error;
mod focus;
pub mod io;
mod ledger;
mod map;
pub mod pack;
pub mod pipelines;
mod plane;
pub mod pyramid;
mod scalar;
mod stacker;
pub mod threads;
pub mod transform;
pub mod upload;
pub mod warp;
pub mod wavelet;
pub mod web_params;

pub use cancel::Cancel;
pub use error::{ConverterError, CoreError, Result, Setting};
pub use pipelines::CommonPipelines;
pub use plane::{FramePlane, PlaneRamp, UNLIT};
pub use transform::{Affine2, FrameAlignment};
pub use upload::{FrameDepth, FrameInput, StagedUpload, StagingPool};

/// What the stackers learn from the inputs rather than from the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackInfo {
    /// Frames in the stack; the depth stacker spreads its planes over them.
    pub total_frames: u32,
    /// The frames are linear camera RGB (RAW/DNG). Focus metrics and the
    /// pyramid run on sRGB-encoded values either way; the result stays
    /// linear so it can be written to a DNG.
    pub is_linear: bool,
}
