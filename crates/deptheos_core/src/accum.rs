//! What the pyramid and depth stackers share about their accumulator
//! pyramids (`deptheos::accum` in the shaders): the levels' sizes, how a
//! level's accumulator is stored, and the collapse's finish step.

use deptheos_gpu::{FrameLimit, GpuContext, GpuError};

use crate::error::Result;

/// The sizes of a `levels`-level pyramid over a `width`×`height` frame,
/// finest first: each level is half the one before, rounded up.
pub(crate) fn level_sizes(
    width: u32,
    height: u32,
    levels: usize,
) -> impl Iterator<Item = (u32, u32)> {
    std::iter::successors(Some((width, height)), |&(w, h)| Some((w.div_ceil(2), h.div_ceil(2))))
        .take(levels)
}

/// Bytes per pixel of a level's accumulator: a storage buffer of `vec4f`
/// (r, g, b sums and the weight), row-major at the level's size.
pub(crate) const ACCUM_BYTES: u64 = 16;

/// The bytes of the accumulator for a `width`×`height` level.
fn accum_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * ACCUM_BYTES
}

/// A `width`×`height` level's accumulator, uncleared.
pub(crate) fn create_accum(ctx: &GpuContext, width: u32, height: u32, label: &str) -> wgpu::Buffer {
    ctx.create_storage_buffer(label, accum_bytes(width, height), wgpu::BufferUsages::COPY_DST)
}

/// Refuse frames whose level-0 accumulator (one storage binding) exceeds
/// what the device binds or allocates (Metal 4 GiB, about 268 MP; D3D12
/// 2 GiB), before anything is allocated rather than failing validation.
pub(crate) fn check_accum_fits(ctx: &GpuContext, width: u32, height: u32) -> Result<()> {
    check_binding_fits(ctx, [width, height], accum_bytes(width, height), "accumulator")
}

/// Refuse a storage buffer of `bytes`, `what` for `size` frames, that is
/// larger than the device binds or allocates ([`check_accum_fits`]).
pub(crate) fn check_binding_fits(
    ctx: &GpuContext,
    [width, height]: [u32; 2],
    bytes: u64,
    what: &'static str,
) -> Result<()> {
    let limits = ctx.device.limits();
    let limit = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
    if bytes > limit {
        return Err(GpuError::FrameTooLarge {
            width,
            height,
            limit: FrameLimit::Buffer { what, bytes, max: limit },
        }
        .into());
    }
    Ok(())
}

// Wrapped in a module so the inner allow covers the layout-check fns the
// encase derive generates.
mod params {
    #![allow(dead_code)]

    /// `FinishParams` in the shaders (`deptheos::accum::finish_rgb`).
    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct FinishParams {
        pub(super) finish: u32,
        pub(super) to_linear: u32,
    }
}
use params::FinishParams;

/// The two finish uniforms of a collapse: the pass writing level 0 clamps
/// (and with `to_linear` decodes to linear), so no extra full-resolution
/// texture is needed; other levels pass through.
pub(crate) struct FinishBuffers {
    intermediate: wgpu::Buffer,
    finishing: wgpu::Buffer,
}

impl FinishBuffers {
    pub(crate) fn new(ctx: &GpuContext, to_linear: bool) -> Self {
        let buffer = |finish: bool| {
            ctx.create_uniform_buffer(
                &FinishParams { finish: finish as u32, to_linear: to_linear as u32 },
                "finish_params",
            )
        };
        Self { intermediate: buffer(false), finishing: buffer(true) }
    }

    /// The uniform for the pass that writes `level`.
    pub(crate) fn for_level(&self, level: usize) -> &wgpu::Buffer {
        if level == 0 { &self.finishing } else { &self.intermediate }
    }
}
