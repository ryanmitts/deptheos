//! What the three stackers share besides their kernels and their own
//! `State`: how a frame comes in ([`Intake`]) and what a stack costs on the
//! GPU ([`gpu_bytes`]).

use deptheos_gpu::GpuContext;

use crate::error::{CoreError, Result};
use crate::ledger::{FrameLedger, SecondPass};
use crate::upload::{FrameDepth, FrameInput, ImageUploader, PreparedUpload, Source};

/// How frames come into a stacker: the ledger, the uploader into its
/// [`Source`], and the checks before taking one.
pub(crate) struct Intake {
    /// Pass 1's frames: which were stacked and which left out.
    pub(crate) frames: FrameLedger,
    uploader: ImageUploader,
}

impl Intake {
    pub(crate) fn new(ctx: &GpuContext, total_frames: u32) -> Self {
        Self { frames: FrameLedger::new(total_frames), uploader: ImageUploader::new(ctx) }
    }

    /// Refuse a stack of no frames, or a frame past the device's textures.
    pub(crate) fn check_size(&self, ctx: &GpuContext, width: u32, height: u32) -> Result<()> {
        self.frames.check_total()?;
        ctx.check_frame_fits(width, height, 0)?;
        Ok(())
    }

    /// Leave the next frame out, keeping its index; first pass only, as the
    /// planes are fixed after.
    pub(crate) fn skip(&mut self, in_first_pass: bool) -> Result<()> {
        if !in_first_pass {
            return Err(CoreError::InvalidState(
                "a frame can only be left out during the first pass".into(),
            ));
        }
        self.frames.skip()
    }

    /// Take a first-pass frame of the source's size and depth: its index and
    /// the prepared upload. Call `frames.push_stacked` once it's submitted.
    pub(crate) fn first_pass<'a>(
        &'a mut self,
        ctx: &GpuContext,
        frame: FrameInput<'a>,
        source: &Source,
    ) -> Result<(u32, PreparedUpload<'a>)> {
        source.check(&frame)?;
        let index = self.frames.check_room()?;
        Ok((index, self.uploader.prepare_frame(ctx, frame)?))
    }

    /// Take second-pass frame `index`: one pass 1 stacked, not yet through
    /// `pass`, of the same size and depth. Mark it on `pass` once submitted.
    pub(crate) fn second_pass<'a>(
        &'a mut self,
        ctx: &GpuContext,
        frame: FrameInput<'a>,
        index: u32,
        pass: &SecondPass,
        source: &Source,
    ) -> Result<PreparedUpload<'a>> {
        pass.check(&self.frames, index)?;
        source.check(&frame)?;
        self.uploader.prepare_frame(ctx, frame)
    }

    /// No frame comes after: the upload buffer can go.
    pub(crate) fn finish(&mut self) {
        self.uploader.release_resources();
    }
}

/// `resources` if all of it allocated: an invalid one would fail every pass
/// silently, so the stacker stays unsized instead.
pub(crate) fn allocated<R>(ctx: &GpuContext, resources: R) -> Result<R> {
    ctx.check()?;
    Ok(resources)
}

/// Peak GPU memory: `held`, the stacker's own, plus the source texture,
/// with Metal's 1/16 texture padding. The upload buffer is the caller's
/// (`deptheos_gpu::StagedFrames`). `tests/gpu_memory.rs` checks every
/// stacker's figure against Metal's.
pub(crate) fn gpu_bytes(
    ctx: &GpuContext,
    width: u32,
    height: u32,
    depth: FrameDepth,
    held: u64,
) -> u64 {
    held + depth.source_bytes(ctx, pixels(width, height))
}

/// Pixels in a `width`×`height` frame.
pub(crate) fn pixels(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height)
}
