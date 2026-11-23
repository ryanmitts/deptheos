//! How a run reads its frames: how many ahead, checked against the stack's
//! memory budget before any decode, and what the app has left as they feed.

use std::ops::RangeInclusive;
use std::path::Path;

use deptheos_core::{CoreError, FrameDepth, StagingPool, io};
use deptheos_gpu::{GpuContext, GpuError, MemoryBudget, MemoryPool, StagedFrames};

use super::{SessionOptions, StackingSession};
use crate::RawBackend;
use crate::Result;
use crate::runs::stacker::Stacker;

impl SessionOptions {
    /// Decoded bytes of standard images the prefetch window keeps in flight
    /// (`deptheos_core::io::FramePrefetcher`), also the decode parallelism.
    /// Past this, frames compete for memory bandwidth: on 16 cores 45 MP
    /// JPEGs are fastest at 8–12 in flight, 16-bit TIFFs at about 4.
    pub const PREFETCH_BYTES: u64 = 1200 << 20;
    /// The window for DNG sources: the SDK decode is multi-threaded, and 1
    /// stacks about twice as fast as 2, with up to 2 GB less peak memory.
    pub(crate) const PREFETCH_DNG: usize = 1;
    /// LibRaw decodes in flight: one per 8 hardware threads. Each holds
    /// ≈ 0.6 GB at 44 MP and is partly parallel itself: a second gains
    /// nothing on 10 threads, three or four are a quarter faster on 32.
    pub(crate) const PREFETCH_RAW_MAX: usize = 4;

    /// The prefetch window the source type chooses, from `first`'s `header`
    /// ([`io::header_info`]) and `budget` bytes for standard images
    /// ([`Self::PREFETCH_BYTES`] by default); a conversion decodes as DNG. A
    /// run may read fewer ([`ReadAhead`]).
    fn prefetch_window(
        first: &Path,
        header: io::Header,
        budget: u64,
        backend: RawBackend,
    ) -> usize {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        if io::is_dng(first) || backend.converts(first) {
            Self::PREFETCH_DNG
        } else if io::is_raw_file(first) {
            // Apple's decoder too: a RAW decode's working memory is several
            // frames.
            (threads / 8).clamp(1, Self::PREFETCH_RAW_MAX)
        } else {
            let pixels = u64::from(header.width) * u64::from(header.height);
            let frame = header.depth.rgb_bytes(pixels);
            let by_bytes = (budget / frame.max(1)) as usize;
            // At most half the hardware threads: on 18 threads, 8 in flight
            // beat 6 by 7–9% for 0.8 GB more peak memory.
            by_bytes.clamp(2, (threads / 2).max(2))
        }
    }
}

impl StackingSession {
    /// How this run reads its frames, decided before the first decode: the
    /// frames' depth and color space ([`Self::plan_frames`]) and the most
    /// frames ahead, up to [`SessionOptions::prefetch_window`], that the
    /// stack fits beside, or a refusal. The budget is read once, here, and
    /// every later check uses it. With no header, the fewest ahead, checked
    /// once a frame says its size ([`Self::ensure_fits`]).
    pub(super) fn reading(&self, first: &Path) -> Result<Reading> {
        // Free what was let go before the stack once the GPU is done with it.
        self.ctx.wait_all().map_err(CoreError::from)?;
        let budget = crate::engine::memory::stack_budget(&self.ctx);
        self.ctx.log_memory("before the stack", budget);
        crate::engine::memory::log("before the stack");
        let (depth, space) = self.plan_frames()?;
        let header = match &self.files {
            Some(_) => None,
            None => io::header_info(first, self.options.raw_backend.camera_raw()),
        };
        let ahead = match header {
            Some(header) => {
                let chosen = SessionOptions::prefetch_window(
                    first,
                    header,
                    crate::engine::memory::prefetch_bytes(self.options.prefetch_bytes),
                    self.options.raw_backend,
                );
                let frame = FrameShape::new(header.width, header.height, depth);
                let window = self.fit(budget, ReadAhead::FEWEST.min(chosen)..=chosen, frame)?;
                if window < chosen {
                    tracing::info!(
                        "read-ahead: {window} frames, not {chosen}: the stack doesn't fit in memory beside {chosen}"
                    );
                }
                ReadAhead::Sized { window, frame }
            }
            None => ReadAhead::Unsized,
        };
        tracing::debug!("read-ahead: {ahead:?}");
        Ok(Reading {
            ahead,
            // The window's frames, the one being fed and one the GPU is
            // still reading.
            staging: StagingPool::new(ahead.window() + 2),
            budget,
            depth,
            space,
            headroom: Headroom::for_budget(budget),
        })
    }

    /// The frames planned from their headers (ADR-132): the first readable
    /// frame's color space, and the deepest frame, which sizes the stacker's
    /// source. An unstackable frame (CMYK, Lab, HDR, a TIFF layout) stops
    /// the stack here, by name. An unreadable header, camera RAW and DNG
    /// count as 16-bit.
    fn plan_frames(&self) -> Result<(FrameDepth, io::StackSpace)> {
        if self.files.is_some() {
            return Ok((FrameDepth::Sixteen, io::StackSpace::default()));
        }
        let raw = self.options.raw_backend.camera_raw();
        let mut space = None;
        let mut deep = false;
        // A replay plans from the frames it reads ([`Self::to_read`]).
        for path in &self.to_read(&self.paths) {
            // Camera RAW is always 16-bit: later headers say nothing new.
            if io::is_raw_file(path) && (space.is_some() || io::is_raw_source(path, raw)) {
                deep = true;
                continue;
            }
            let header = match io::standard_header(path, raw) {
                Ok(Some(header)) => header,
                Ok(None) => {
                    deep = true;
                    continue;
                }
                Err(e @ CoreError::Refused { .. }) => return Err(e.into()),
                Err(e) => {
                    tracing::debug!("{}: no header to plan from ({e})", path.display());
                    deep = true;
                    continue;
                }
            };
            if space.is_none() {
                let first = io::StackSpace::of(path, &header)?;
                tracing::debug!("the stack's color space: {:?}", first.profile());
                space = Some(first);
            }
            let into = io::Target::Stack(space.as_ref().expect("set above"));
            match io::input_header(path, &header, into) {
                Ok(header) => deep |= header.depth == FrameDepth::Sixteen,
                Err(e @ CoreError::Refused { .. }) => return Err(e.into()),
                Err(e) => {
                    tracing::debug!("{}: can't be planned ({e})", path.display());
                    deep = true;
                }
            }
        }
        let depth = if deep { FrameDepth::Sixteen } else { FrameDepth::Eight };
        tracing::debug!("frames' depth: {depth:?}");
        Ok((depth, space.unwrap_or_default()))
    }

    /// Refuse a stack of frames like `frame` that doesn't fit `reading`'s
    /// budget, before allocating, rather than run out partway (on iOS, the
    /// system ends the app). For a shape not checked yet: no header, a
    /// reversed restart, or a reference unlike its header. The window stays.
    pub(super) fn ensure_fits(&self, reading: &mut Reading, frame: FrameShape) -> Result<()> {
        let window = match reading.ahead {
            ReadAhead::Sized { frame: checked, .. } if checked == frame => return Ok(()),
            ahead => ahead.window(),
        };
        self.fit(reading.budget, window..=window, frame)?;
        let next = ReadAhead::Sized { window, frame };
        tracing::debug!("read-ahead: {:?} → {next:?}", reading.ahead);
        reading.ahead = next;
        Ok(())
    }

    /// The most frames ahead in `windows` that fit `budget`
    /// ([`window_that_fits`], [`Self::memory_need`]), logged; refused when
    /// none fits.
    fn fit(
        &self,
        budget: Option<MemoryBudget>,
        windows: RangeInclusive<usize>,
        frame: FrameShape,
    ) -> Result<usize> {
        let FrameShape { width, height, .. } = frame;
        let mut tried = (0, MemoryNeed::default());
        let fitted = window_that_fits(windows, |window| {
            let need = self.memory_need(budget.map(|b| b.pool), window, frame);
            tried = (window, need);
            budget.map_or(Ok(()), |b| b.check(need.total(), width, height))
        });
        let (window, need) = tried;
        need.log(frame, window, budget);
        fitted.map_err(|e| CoreError::from(e).into())
    }

    /// A stack's peak need, read `window` ahead: the stacker's peak and the
    /// frame-size staging and upload buffers. With [`MemoryPool::Shared`],
    /// also the CPU's frames: each decode an RGBA frame plus about twice it
    /// of decoder memory (`script/check-bitmap-decode.sh`), and, when
    /// aligning, their preparations and the reference. Pass 1 bounds pass 2.
    ///
    /// Completing the stack holds less (`deptheos_core/tests/gpu_memory.rs`).
    fn memory_need(
        &self,
        pool: Option<MemoryPool>,
        window: usize,
        frame: FrameShape,
    ) -> MemoryNeed {
        let FrameShape { width, height, depth, frame_bytes } = frame;
        // As the staging pool is sized, plus the upload buffer.
        let upload = match self.ctx.staged_frames() {
            StagedFrames::Copied => 1,
            StagedFrames::InPlace => 0,
        };
        let frames = (window as u64 + 2 + upload) * frame_bytes;
        let host = match pool {
            Some(MemoryPool::Shared) => {
                // The RGBA destination, and twice it of the decoder's own.
                let decode = frame_bytes / 3 * 4 * 3;
                let prepared = self.new_aligner().map_or(0, |a| a.preparer().bytes(width, height));
                let decoding = window as u64 + 1;
                decoding * decode + (decoding + 1) * prepared
            }
            Some(MemoryPool::Gpu) | None => 0,
        };
        let gpu = Stacker::gpu_bytes(&self.ctx, self.config, width, height, depth);
        MemoryNeed { gpu, frames, host }
    }
}

/// The most frames ahead in `windows` that `check` passes, or its error at
/// the fewest: the window only sets speed.
fn window_that_fits<E>(
    windows: RangeInclusive<usize>,
    mut check: impl FnMut(usize) -> std::result::Result<(), E>,
) -> std::result::Result<usize, E> {
    let (fewest, mut window) = (*windows.start(), *windows.end().max(windows.start()));
    loop {
        match check(window) {
            Ok(()) => return Ok(window),
            Err(e) if window <= fewest => return Err(e),
            Err(_) => window -= 1,
        }
    }
}

/// How many frames a run reads ahead, owned by its [`Reading`]: set by
/// [`StackingSession::reading`], changed only by
/// [`StackingSession::ensure_fits`], and kept by both passes and the
/// restart. It changes only the speed.
///
/// `Unsized` → `Sized` once a frame says its size, at the fewest ahead.
/// `Sized` → `Sized` for another shape, same window. A stack that doesn't
/// fit is refused. Transitions are logged at debug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadAhead {
    /// No header to size by yet: the fewest frames ahead.
    Unsized,
    /// Checked for `frame`'s shape: the most frames ahead that fit.
    Sized { window: usize, frame: FrameShape },
}

impl ReadAhead {
    /// The fewest frames read ahead (DNGs always read 1).
    const FEWEST: usize = 1;

    pub(super) fn window(self) -> usize {
        match self {
            Self::Unsized => Self::FEWEST,
            Self::Sized { window, .. } => window,
        }
    }
}

/// How a run reads its frames ([`StackingSession::reading`]).
pub(super) struct Reading {
    pub(super) ahead: ReadAhead,
    /// Staging buffers for the window, shared by both passes' prefetchers.
    pub(super) staging: StagingPool,
    /// What the stack may use, read once before the first decode.
    budget: Option<MemoryBudget>,
    /// The deepest frame's depth, which the stacker is sized for
    /// ([`StackingSession::plan_frames`]).
    pub(super) depth: FrameDepth,
    /// The color space standard images are converted into.
    pub(super) space: io::StackSpace,
    /// What the app has left, watched as frames are fed.
    pub(super) headroom: Headroom,
}

/// What the app has left before the system ends it, read before each frame
/// with [`MemoryPool::Shared`] (iOS): the estimate's safety net. The stack
/// stops with [`GpuError::LowMemory`] on a second low reading in a row (one
/// may be memory not yet returned).
///
/// `Off` → nothing. `Clear` → `LowOnce` on a reading under
/// [`crate::engine::memory::FLOOR`]; `LowOnce` → `Clear` on one over it, or
/// the stack stops on a second under it. Transitions and readings are
/// logged at debug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Headroom {
    /// No limit to watch: a budget of the GPU's alone, or none.
    Off,
    Clear,
    LowOnce,
}

impl Headroom {
    fn for_budget(budget: Option<MemoryBudget>) -> Self {
        match budget.map(|b| b.pool) {
            Some(MemoryPool::Shared) => Self::Clear,
            Some(MemoryPool::Gpu) | None => Self::Off,
        }
    }

    /// The state after reading `left` bytes; `None` stops the stack.
    fn after(self, left: u64) -> Option<Self> {
        match (self, left < crate::engine::memory::FLOOR) {
            (Self::Off, _) => Some(Self::Off),
            (_, false) => Some(Self::Clear),
            (Self::Clear, true) => Some(Self::LowOnce),
            (Self::LowOnce, true) => None,
        }
    }

    /// Read what is left, once wgpu has freed what finished work let go.
    pub(super) fn check(&mut self, ctx: &GpuContext) -> Result<()> {
        if *self == Self::Off {
            return Ok(());
        }
        let _ = ctx.device.poll(wgpu::PollType::Poll);
        let Some(left) = crate::engine::memory::left() else { return Ok(()) };
        tracing::debug!("memory left: {} MB", left >> 20);
        let Some(next) = self.after(left) else {
            tracing::warn!("stopping the stack: {} MB of memory left", left >> 20);
            return Err(CoreError::from(GpuError::LowMemory { left }).into());
        };
        if next != *self {
            tracing::debug!("headroom: {self:?} → {next:?}");
            *self = next;
        }
        Ok(())
    }
}

/// The frames' size, depth and bytes as staged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FrameShape {
    width: u32,
    height: u32,
    depth: FrameDepth,
    frame_bytes: u64,
}

impl FrameShape {
    /// RGB frames of `width`×`height` at `depth`, as they are staged.
    pub(super) fn new(width: u32, height: u32, depth: FrameDepth) -> Self {
        let frame_bytes = depth.rgb_bytes(u64::from(width) * u64::from(height));
        Self { width, height, depth, frame_bytes }
    }
}

/// A stack's peak need ([`StackingSession::memory_need`]), in parts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MemoryNeed {
    /// The stacker's peak, with Metal's padding.
    gpu: u64,
    /// The frame-size buffers frames are staged and uploaded through.
    frames: u64,
    /// CPU frames, decoded and prepared, when the CPU shares the budget.
    host: u64,
}

impl MemoryNeed {
    fn total(self) -> u64 {
        self.gpu + self.frames + self.host
    }

    /// Log at info, in parts, with what `budget` leaves usable.
    fn log(self, frame: FrameShape, window: usize, budget: Option<MemoryBudget>) {
        let FrameShape { width, height, .. } = frame;
        tracing::info!(
            "a {width}x{height} stack reading {window} ahead needs about {} MB: {} MB on the GPU, {} MB of staged frames, {} MB of frames on the CPU (usable {:?} MB)",
            self.total() >> 20,
            self.gpu >> 20,
            self.frames >> 20,
            self.host >> 20,
            budget.map(|b| b.usable >> 20)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second low reading in a row stops the stack; one over the floor
    /// resets the count.
    #[test]
    fn headroom_stops_on_a_second_low_reading() {
        use super::Headroom::*;
        let (low, fine) = (crate::engine::memory::FLOOR - 1, crate::engine::memory::FLOOR);
        assert_eq!(Clear.after(fine), Some(Clear));
        assert_eq!(Clear.after(low), Some(LowOnce));
        assert_eq!(LowOnce.after(fine), Some(Clear));
        assert_eq!(LowOnce.after(low), None);
        assert_eq!(Off.after(0), Some(Off));
    }

    /// The most frames ahead that fit; refused only when one doesn't.
    #[test]
    fn the_window_narrows_before_a_stack_is_refused() {
        let fits_up_to = |most: usize| move |w: usize| if w <= most { Ok(()) } else { Err(w) };
        assert_eq!(window_that_fits(1..=4, fits_up_to(9)), Ok(4));
        assert_eq!(window_that_fits(1..=4, fits_up_to(2)), Ok(2));
        assert_eq!(window_that_fits(1..=4, fits_up_to(1)), Ok(1));
        assert_eq!(window_that_fits(1..=4, fits_up_to(0)), Err(1));
        assert_eq!(window_that_fits(1..=1, fits_up_to(0)), Err(1));
        // A window already under way is checked alone.
        assert_eq!(window_that_fits(3..=3, fits_up_to(2)), Err(3));
    }
}
