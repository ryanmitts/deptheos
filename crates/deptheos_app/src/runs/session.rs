//! The stacking session: one run of the pipeline over a set of frames, with
//! per-frame events, cancellation checked between frames, and the session
//! (not the stacker) re-reading frames for the second pass.
//!
//! [`StackingSession::run`] consumes the session, so the aligner and stacker
//! (and their GPU memory) end with the run, and returns a [`CompletedStack`]
//! of plain CPU data. It runs on the calling thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_aligner::{AlignedFrame, Aligner, AlignmentError, FramePreparer, PreparedFrame};
use deptheos_core::{
    Cancel, CommonPipelines, CoreError, FrameAlignment, FrameDepth, StagedUpload, StagingPool, io,
};
use deptheos_gpu::GpuContext;
use deptheos_gpu::profile::cpu_scope;
use deptheos_render::dng_render::{Constants, DngRenderPipeline};
use image::math::Rect;

use crate::frames::names::file_label;
use crate::runs::stacker::{Stacker, StackerConfig};
use crate::words::{Words, say};
use crate::{Error, FrameKind, RawBackend, Result, StackConversions};
pub use deptheos_core::dcp::DcpPreference;

mod alignment;
mod contributions;
mod progress;
pub(crate) mod readback;
mod reading;
mod stack;

pub use alignment::{AlignSettings, Alignment};
pub(crate) use contributions::Contributions;
pub use progress::{Progress, Step};

use readback::{crop_texture, download_planes};
pub(crate) use readback::{download_gray, download_rgb};
use reading::{FrameShape, Reading};
use stack::flip;
pub(crate) use stack::render_params_or_identity;
pub use stack::{
    CompletedStack, Gray16Image, LeftOut, SaveOptions, StackColor, StackGeometry, left_out_line,
    left_out_message,
};
pub(crate) use stack::{export, export_pixels, left_out_limit};

/// How far past 1 the second frame's magnification must be to restart the
/// stack reversed; the aligner's estimate wavers around 1 by less.
const REVERSE_DEADBAND: f32 = 1e-3;

/// Whether the second frame, aligned as `transform`, is magnified relative
/// to the first: the stack was shot with magnification growing.
fn magnified(transform: &FrameAlignment) -> bool {
    transform.magnification() > 1.0 + REVERSE_DEADBAND
}

/// A load note about `count` frames, as a stack's notice says it.
fn load_note(note: &io::LoadNote, count: usize, words: &Words) -> String {
    match note {
        io::LoadNote::FloatClipped => say!(words, "note-float-clipped", count = count),
        io::LoadNote::OverWhite => say!(words, "note-over-white", count = count),
        io::LoadNote::Clipped { from, into } => {
            let from = profile_name(from.as_deref(), words);
            let into = profile_name(into.as_deref(), words);
            say!(words, "note-clipped", from = from, into = into, count = count)
        }
    }
}

/// A color profile's name for a sentence, or "an unnamed color profile".
pub(crate) fn profile_name(name: Option<&str>, words: &Words) -> String {
    match name {
        Some(name) => name.to_owned(),
        None => say!(words, "unnamed-profile"),
    }
}

/// Progress events, one per frame per phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Started {
        total: u32,
    },
    Accumulated {
        current: u32,
        total: u32,
    },
    /// The between-passes finalization ran; a two-pass stacker blends next.
    Finalized,
    /// The reverse check tripped: pass 1 restarts reversed (`Accumulated`
    /// counts start over).
    Reversed,
    Blended {
        current: u32,
        total: u32,
    },
    Completed,
}

#[derive(Clone, Copy, Debug)]
pub struct SessionOptions {
    /// Whether and how the frames are aligned (on by default).
    pub alignment: Alignment,
    /// How camera RAW frames are decoded, recorded with the stack, whose
    /// geometry holds only for that backend. [`crate::Engine::session`]
    /// converts them as the stack runs ([`StackingSession::with_frame_files`]).
    pub raw_backend: RawBackend,
    /// The installed Adobe camera profile for LibRaw-decoded RAW frames
    /// (DNG and Adobe-converted frames carry their own).
    pub camera_profile: DcpPreference,
    /// Decoded bytes of standard images the prefetch window may hold.
    pub prefetch_bytes: u64,
    /// The caller's order of the paths, recorded with the render; the
    /// session never reorders them.
    pub order: crate::frames::order::FrameOrder,
    /// Whether a front end shows the stack as it forms: only then does
    /// [`LiveMap`] render it. Off by default.
    pub live_map: bool,
}

impl SessionOptions {
    /// Threads for the aligner's own pool: a third of the hardware threads,
    /// 2 to 8 (at a quarter, prep became the limit with 8 frames decoding).
    /// Prep on the prefetch threads and the step share it; rayon's global
    /// pool would starve the single-threaded JPEG decoders.
    fn align_threads() -> usize {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        (threads / 3).clamp(2, 8)
    }
}

/// How a pass of `frames` frames, each the size of the DNG `file`, uses the
/// OS file cache. On Windows it bypasses it ([`FileCache::Bypass`]) when the
/// files would take over three quarters of the memory free for caching,
/// where every read waits for eviction (200 × 270 MB on 64 GB: 29 s
/// through, 16 s around); otherwise, and elsewhere, it reads through it.
fn file_cache_for(frames: u64, file: &Path) -> io::FileCache {
    if !io::is_dng(file) {
        return io::FileCache::Use;
    }
    let Some(available) = crate::engine::memory::available_physical() else {
        return io::FileCache::Use;
    };
    let total = std::fs::metadata(file).map_or(0, |m| m.len()).saturating_mul(frames);
    let cache = if total > available / 4 * 3 { io::FileCache::Bypass } else { io::FileCache::Use };
    tracing::debug!(
        "{frames} frames, {:.1} GB of DNG, {:.1} GB available: {cache:?} the file cache",
        total as f64 / 1e9,
        available as f64 / 1e9
    );
    cache
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            alignment: Alignment::default(),
            raw_backend: RawBackend::default(),
            camera_profile: DcpPreference::default(),
            prefetch_bytes: Self::PREFETCH_BYTES,
            order: crate::frames::order::FrameOrder::Name,
            live_map: false,
        }
    }
}

/// What a run returns: the stack, and for a linear stack what renders its
/// sRGB display.
pub struct SessionOutput {
    pub stack: CompletedStack,
    /// `None` for sRGB stacks.
    pub display: Option<LinearDisplay>,
}

/// A linear stack's display, rendered only on [`Self::render`]; holds the
/// cropped result on the GPU until then.
pub struct LinearDisplay {
    ctx: Arc<GpuContext>,
    cropped: wgpu::Texture,
}

impl LinearDisplay {
    /// The cropped result rendered through `constants` (the stack's look,
    /// resolved): Rgba16Unorm where the adapter stores it
    /// (`GpuContext::source_format`), else Rgba32Float.
    pub fn render(self, color: &DngRenderPipeline, constants: &Constants) -> wgpu::Texture {
        let _t = cpu_scope("complete: display render");
        color.render(&self.ctx, &self.cropped, constants)
    }
}

/// Which pictures of the stack partway through
/// [`StackingSession::run_snapshots`] hands back.
#[derive(Clone, Debug, Default)]
pub(crate) struct SnapshotPlan {
    /// After these frames, in processing order from 1, left-out ones
    /// included; one before any frame stacked is skipped.
    pub after: std::collections::BTreeSet<u32>,
    /// The region to cut, in full-frame coordinates, instead of the
    /// recorded crop.
    pub crop: Option<Rect>,
}

/// The stack partway through: what it would be if it stopped here.
pub struct Snapshot {
    /// The frame it was taken after, counted in processing order from 1.
    pub frame: u32,
    /// How many frames are in it (not left out).
    pub stacked: u32,
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB u16 in the plan's region, like
    /// [`CompletedStack::pixels`].
    pub pixels: Vec<u16>,
}

/// What a snapshot run left out, by index into its paths.
pub(crate) struct SnapshotRun {
    pub left_out: Vec<LeftOut>,
}

/// A snapshot run's plan, region and callback, while pass 1 runs.
struct Snapper<'a> {
    plan: &'a SnapshotPlan,
    region: Option<Rect>,
    sink: &'a mut dyn FnMut(Snapshot) -> Result<()>,
}

/// Handed to the event callback: renders the stack so far on request, only
/// when [`SessionOptions::live_map`] is on.
pub struct LiveMap<'a>(Option<(&'a Stacker, wgpu::TextureFormat)>);

impl<'a> LiveMap<'a> {
    /// `stacker`'s picture, rendered in `GpuContext::map_format`, if
    /// `options` show it.
    fn of(ctx: &GpuContext, options: &SessionOptions, stacker: &'a Stacker) -> Self {
        Self(options.live_map.then(|| (stacker, ctx.map_format())))
    }

    /// The frame map so far ([`Stacker::render_live`]) as a fresh gray
    /// texture in `GpuContext::map_format`, at most `max_side` on its
    /// longest side; `None` before the first accumulated frame.
    pub fn render(&self, max_side: u32) -> Option<wgpu::Texture> {
        let (stacker, format) = self.0?;
        stacker.render_live(max_side, format).ok()
    }
}

/// One run over a set of frames, built and then consumed by
/// [`Self::run`]; see the module.
pub struct StackingSession {
    ctx: Arc<GpuContext>,
    common: Arc<CommonPipelines>,
    /// In the caller's order.
    paths: Vec<PathBuf>,
    config: StackerConfig,
    options: SessionOptions,
    /// Alignment to replay instead of running the aligner.
    recorded: Option<StackGeometry>,
    /// The aligner's pool ([`SessionOptions::align_threads`]).
    align_pool: Arc<rayon::ThreadPool>,
    /// Where frames are read, when not from `paths` themselves.
    files: Option<Arc<StackConversions>>,
    /// The language a left-out frame's reason is worded in.
    words: Words,
}

impl StackingSession {
    /// Prepare a session over `paths`. RAW/DNG sources stack in linear
    /// camera RGB, standard images as stored in sRGB; mixing them is
    /// rejected here, before any frame is read.
    pub fn new(
        ctx: Arc<GpuContext>,
        common: Arc<CommonPipelines>,
        paths: Vec<PathBuf>,
        config: StackerConfig,
        options: SessionOptions,
        words: Words,
    ) -> Result<Self> {
        if paths.is_empty() {
            return Err(Error::NoFrames);
        }
        check_same_kind(&paths, options.raw_backend)?;
        let align_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(SessionOptions::align_threads())
            .thread_name(|i| format!("align-{i}"))
            .stack_size(crate::engine::work::STACK_SIZE)
            .build()
            .map_err(|e| Error::InvalidState(format!("aligner thread pool: {e}")))?;
        Ok(Self {
            ctx,
            common,
            paths,
            config,
            options,
            recorded: None,
            align_pool: Arc::new(align_pool),
            files: None,
            words,
        })
    }

    /// Read each frame from its Adobe conversion in `files`, made while
    /// the stack runs; the stack still records the path.
    pub fn with_frame_files(mut self, files: Arc<StackConversions>) -> Self {
        self.files = Some(files);
        self
    }

    /// The file to decode for frame `path`.
    fn file(&self, path: &Path) -> Result<PathBuf> {
        match &self.files {
            Some(files) => Ok(files.file(path)?),
            None => Ok(path.to_path_buf()),
        }
    }

    /// Refuse a stack whose conversions won't fit on their disk, judged
    /// from the first: a blend pass keeps every conversion until read twice
    /// (tens of GB). Passes when the free space is unknown.
    fn check_conversion_space(&self) -> Result<()> {
        let Some(files) = &self.files else { return Ok(()) };
        let Some(needed) = files.space_needed(self.config.needs_blend()) else { return Ok(()) };
        tracing::debug!("the conversions to come need about {} bytes", needed.bytes);
        match crate::fsutil::no_room_for(&needed.folder, needed.bytes) {
            Some(free) => {
                tracing::warn!(
                    "the conversions need about {} bytes more, and their disk has {free} free",
                    needed.bytes
                );
                Err(Error::NoConversionSpace { folder: needed.folder, needed: needed.bytes, free })
            }
            None => Ok(()),
        }
    }

    /// Replay a finished stack's alignment. The paths must be its frames in
    /// its order; they are processed as it did (reversed if it was) and its
    /// crop applies when the options crop.
    pub fn with_alignment(mut self, geometry: StackGeometry) -> Result<Self> {
        if geometry.transforms.len() != self.paths.len() {
            return Err(Error::InvalidState(format!(
                "recorded alignment has {} frames, the session {}",
                geometry.transforms.len(),
                self.paths.len()
            )));
        }
        if geometry.left_out.iter().any(|l| l.index >= self.paths.len()) {
            return Err(Error::InvalidState("recorded left-out frame out of range".into()));
        }
        self.recorded = Some(geometry);
        Ok(self)
    }

    /// Whether this session's sources stack linear (and so allow DNG output).
    pub(crate) fn is_linear(&self) -> bool {
        self.options.raw_backend.decodes_linear(&self.paths[0])
    }

    /// Replay the recorded alignment ([`Self::with_alignment`]), handing
    /// `on_snapshot` the stack so far after each frame `plan` names, cut to
    /// its region. Pyramid only; the stack isn't finished. A snapshot after
    /// `k` frames equals the `k`-frame stack up to float rounding
    /// (`PyramidStacker::collapse`). `on_snapshot`'s error stops the run.
    pub(crate) fn run_snapshots(
        self,
        plan: &SnapshotPlan,
        on_snapshot: &mut dyn FnMut(Snapshot) -> Result<()>,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
        cancel: &Cancel,
    ) -> Result<SnapshotRun> {
        let files = self.files.clone();
        stop_conversions_on_cancel(files, cancel, move || {
            self.snapshots(plan, on_snapshot, on_event, cancel)
        })
    }

    fn snapshots(
        self,
        plan: &SnapshotPlan,
        on_snapshot: &mut dyn FnMut(Snapshot) -> Result<()>,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
        cancel: &Cancel,
    ) -> Result<SnapshotRun> {
        if self.config.needs_blend() {
            return Err(Error::InvalidState("snapshots need a pyramid stack".into()));
        }
        let Some(recorded) = &self.recorded else {
            return Err(Error::InvalidState("snapshots replay a recorded alignment".into()));
        };
        if recorded.reversed {
            return Err(Error::InvalidState(
                "snapshots replay frames in the order given: pass the geometry unreversed".into(),
            ));
        }
        let region = plan.crop.or(recorded.crop.filter(|_| self.options.alignment.crops()));
        let total = self.paths.len() as u32;
        self.ctx.clear_fault();
        self.gpu_check()?;
        on_event(SessionEvent::Started { total }, &LiveMap(None));
        let read = self.to_read(&self.paths);
        let mut reading = self.reading(read.first().unwrap_or(&self.paths[0]))?;
        let mut frames = self.prefetcher(&reading, &read, None, false, cancel);
        let mut snapper = Snapper { plan, region, sink: on_snapshot };
        let pass: Pass = self
            .accumulate(&mut reading, false, &mut frames, on_event, cancel, Some(&mut snapper))?
            .ok_or_else(|| Error::InvalidState("a replay does not restart".into()))?;
        on_event(SessionEvent::Completed, &LiveMap(None));
        Ok(SnapshotRun { left_out: pass.left_out })
    }

    /// The stack so far, cut to `region`, read back.
    fn snapshot(
        &self,
        stacker: &Stacker,
        region: Option<Rect>,
        frame: u32,
        stacked: u32,
    ) -> Result<Snapshot> {
        let _t = cpu_scope("snapshot");
        let texture = stacker.collapse()?;
        let (w, h) = (texture.width(), texture.height());
        let rect = match region {
            Some(r) if r.x + r.width > w || r.y + r.height > h => {
                return Err(Error::InvalidState(format!(
                    "snapshot region {}×{} at ({}, {}) is outside the {w}×{h} frame",
                    r.width, r.height, r.x, r.y
                )));
            }
            Some(r) if trims(&r, w, h) => r,
            _ => Rect { x: 0, y: 0, width: w, height: h },
        };
        // Packed to rgba16 on the GPU where possible: half the read-back.
        let pixels = match &self.common.packer {
            Some(packer) => {
                let packed = packer.pack(&self.ctx, texture, rect);
                download_rgb(&self.ctx, &packed, Rect { x: 0, y: 0, ..rect })?
            }
            None => download_rgb(&self.ctx, texture, rect)?,
        };
        self.gpu_check()?;
        Ok(Snapshot { frame, stacked, width: rect.width, height: rect.height, pixels })
    }

    /// Run the pipeline on the calling thread. `on_event` gets every
    /// [`SessionEvent`] with a [`LiveMap`]; `cancel` is checked between
    /// frames and phases ([`Error::Canceled`]).
    pub fn run(
        self,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
        cancel: &Cancel,
    ) -> Result<SessionOutput> {
        let _stacking = crate::engine::work::stacking();
        let files = self.files.clone();
        stop_conversions_on_cancel(files, cancel, move || self.stack(on_event, cancel))
    }

    fn stack(
        self,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
        cancel: &Cancel,
    ) -> Result<SessionOutput> {
        let total = self.paths.len() as u32;
        // Clear earlier work's error; a lost device fails here.
        self.ctx.clear_fault();
        self.gpu_check()?;
        on_event(SessionEvent::Started { total }, &LiveMap(None));

        let (mut pass, mut reading, reversed) = self.pass_one(on_event, cancel)?;

        // The aligner ran inside pass 1, so every transform is recorded.
        check_cancel(cancel)?;
        {
            let _t = cpu_scope("end_pass_one");
            pass.stacker.end_pass_one()?;
            self.gpu_check()?;
        }
        on_event(SessionEvent::Finalized, &LiveMap::of(&self.ctx, &self.options, &pass.stacker));

        // Pass 2 re-reads the frames in accumulation order rather than
        // holding the whole stack in memory.
        if self.config.needs_blend() {
            // A left-out frame is not read again.
            if let Some(files) = &self.files {
                for l in &pass.left_out {
                    files.release(&pass.order[l.index]);
                }
            }
            let blended: Vec<PathBuf> = (0..pass.order.len())
                .filter(|&i| !pass.is_left_out(i))
                .map(|i| pass.order[i].clone())
                .collect();
            let mut frames = self.prefetcher(&reading, &blended, None, true, cancel);
            for i in 0..pass.order.len() {
                check_cancel(cancel)?;
                if pass.is_left_out(i) {
                    continue;
                }
                let staged = next_frame(&mut frames)?.staged;
                reading.headroom.check(&self.ctx)?;
                let _t = cpu_scope("blend_frame");
                pass.stacker.blend_frame(&staged, i as u32, pass.transforms[i].as_ref())?;
                self.gpu_check()?;
                drop(_t);
                on_event(SessionEvent::Blended { current: i as u32 + 1, total }, &LiveMap(None));
            }
            check_cancel(cancel)?;
            let _t = cpu_scope("finalize");
            pass.stacker.finalize()?;
            self.gpu_check()?;
        }

        check_cancel(cancel)?;
        let output = {
            let _t = cpu_scope("complete (total)");
            self.complete(pass, reversed, &reading.staging)?
        };
        on_event(SessionEvent::Completed, &LiveMap(None));
        Ok(output)
    }

    /// Pass 1: align and accumulate, frames decoding ahead. Restarted once,
    /// reversed, if the reverse check trips. Returns how frames were read
    /// (pass 2 keeps to it) and whether it ran reversed.
    fn pass_one(
        &self,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
        cancel: &Cancel,
    ) -> Result<(Pass, Reading, bool)> {
        let mut reversed = self.recorded.as_ref().is_some_and(|g| g.reversed);
        let order = self.order(reversed);
        // The read-ahead is decided before the first decode and kept by the
        // restart and pass 2.
        let read = self.to_read(&order);
        let mut reading = self.reading(read.first().unwrap_or(&order[0]))?;
        let preparer = self.new_aligner().map(|a| a.preparer());
        let mut frames = self.prefetcher(&reading, &read, preparer, false, cancel);
        let pass = loop {
            match self.accumulate(&mut reading, reversed, &mut frames, on_event, cancel, None)? {
                Some(pass) => break pass,
                None => {
                    tracing::info!(
                        "reverse check: the second frame magnifies the first; restarting with the frames reversed"
                    );
                    reversed = true;
                    // Wait for the dropped stacker's GPU work so its
                    // buffers free before the next attempt allocates.
                    self.ctx.wait_all().map_err(CoreError::from)?;
                    self.gpu_check()?;
                    let order = self.to_read(&self.order(true));
                    if let Some(files) = &self.files {
                        files.plan(&order);
                    }
                    frames = frames.reorder(order);
                    on_event(SessionEvent::Reversed, &LiveMap(None));
                }
            }
        };
        Ok((pass, reading, reversed))
    }

    /// This thread's GPU error or the device's loss ([`GpuContext::check`]).
    fn gpu_check(&self) -> Result<()> {
        self.ctx.check().map_err(|e| CoreError::from(e).into())
    }

    /// A stacker for a pass over `total` frames.
    fn new_stacker(&self, total: u32) -> Stacker {
        Stacker::new(self.ctx.clone(), self.config, total, self.is_linear())
    }

    /// A fresh aligner when aligning and not replaying; one per pass 1
    /// attempt (it holds the reference).
    fn new_aligner(&self) -> Option<Aligner> {
        match self.options.alignment {
            Alignment::On(settings) if self.recorded.is_none() => {
                Some(Aligner::with_grid_config(settings.mode, settings.grid))
            }
            _ => None,
        }
    }

    /// The frames of `order` pass 1 reads: all but those a replayed
    /// alignment left out.
    pub(super) fn to_read(&self, order: &[PathBuf]) -> Vec<PathBuf> {
        let out: Vec<&PathBuf> =
            self.recorded.iter().flat_map(|g| &g.left_out).map(|l| &self.paths[l.index]).collect();
        order.iter().filter(|p| !out.contains(p)).cloned().collect()
    }

    /// The frames in processing order.
    fn order(&self, reversed: bool) -> Vec<PathBuf> {
        let mut order = self.paths.clone();
        if reversed {
            order.reverse();
        }
        order
    }

    /// One attempt at pass 1 over `frames`; `None` when the reverse check
    /// trips and the pass must restart reversed.
    ///
    /// A frame that can't be read or aligned is left out ([`LeftOut`]), up
    /// to [`left_out_limit`]; the first that stacks is the reference. A
    /// frame of another size stops the stack (another shoot, or turned).
    /// A `snapper` takes its plan's snapshots.
    fn accumulate(
        &self,
        reading: &mut Reading,
        reversed: bool,
        frames: &mut io::FramePrefetcher<LoadedFrame>,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
        cancel: &Cancel,
        mut snapper: Option<&mut Snapper>,
    ) -> Result<Option<Pass>> {
        // The pass's head: from here until the GPU is handed its first frame.
        let mut head = Some(cpu_scope("until the first frame is fed"));
        let total = self.paths.len() as u32;
        let order = self.order(reversed);
        let n = order.len();
        let mut recorded = self.recorded.as_ref().map(|g| g.transforms.clone());
        if reversed && let Some(t) = &mut recorded {
            t.reverse();
        }
        // A replayed stack leaves out what it left out.
        let mut recorded_out = vec![None; n];
        for l in self.recorded.iter().flat_map(|g| &g.left_out) {
            recorded_out[flip(reversed, n, l.index)] = Some(l.reason.clone());
        }
        let mut aligner = self.new_aligner();
        let mut stacker = self.new_stacker(total);
        let mut transforms = Vec::with_capacity(n);
        let mut left_out: Vec<LeftOut> = Vec::new();
        let mut stacked = 0;
        // The deepest frame stacked, which the output's depth follows.
        let mut depth = FrameDepth::Eight;
        // The first frame that stacks, once it has.
        let mut reference: Option<Reference> = None;
        // What the stacked frames' loads noted, with how many frames.
        let mut notes: Vec<(io::LoadNote, usize)> = Vec::new();

        // Size the stacker from the first header so allocation overlaps the
        // decode; the frame is checked against it when it arrives.
        let mut allocated = None;
        let first_read = (0..n).find(|&i| recorded_out[i].is_none()).unwrap_or(0);
        let first = self.file(&order[first_read]);
        if first.is_ok() {
            // The first conversion is made: the rest are about its size.
            self.check_conversion_space()?;
        }
        if let Ok(first) = first
            && let Some(io::Header { width: w, height: h, .. }) =
                io::header_info(&first, self.options.raw_backend.camera_raw())
        {
            // Checked already, unless the reversed restart has another first
            // frame or this is a conversion's header.
            self.ensure_fits(reading, FrameShape::new(w, h, reading.depth))?;
            let _t = cpu_scope("allocate ahead");
            stacker.allocate(w, h, reading.depth)?;
            allocated = Some((w, h));
        }

        // A conversion is let go once its frame is done, unless pass 2 reads
        // it again or the reverse check may still restart the pass.
        let files = self.files.clone().filter(|_| !self.config.needs_blend());
        let checks_reverse = aligner.is_some() && !reversed;
        let mut released = 0;
        let mut release_through = |i: usize| {
            if let Some(files) = &files {
                for path in &order[released..=i] {
                    files.release(path);
                }
            }
            released = i + 1;
        };
        for i in 0..n {
            check_cancel(cancel)?;
            let attempt = match &recorded_out[i] {
                // Never read ([`Self::to_read`]).
                Some(reason) => Err(reason.clone()),
                None => {
                    let placing = match (aligner.as_mut(), &recorded) {
                        (Some(aligner), _) => Placing::Align(aligner),
                        (None, Some(recorded)) => Placing::Recorded(recorded[i]),
                        (None, None) => Placing::AsIs,
                    };
                    // The reason in the user's words; the error to the log.
                    self.take_frame(frames, &order[i], placing, reference.as_ref())?.map_err(|e| {
                        tracing::warn!("{} can't be stacked: {e}", order[i].display());
                        crate::words::explain::left_out_reason(&e, &self.words)
                    })
                }
            };
            let TakenFrame { staged, transform, profile, notes: frame_notes } = match attempt {
                Ok(frame) => frame,
                Err(reason) => {
                    // A frame failed by the cancel isn't left out.
                    check_cancel(cancel)?;
                    // Nor one failed by a lost GPU: the stack stops with the
                    // device's error.
                    self.gpu_check()?;
                    tracing::warn!("leaving {} out of the stack: {reason}", order[i].display());
                    left_out.push(LeftOut { index: i, reason });
                    // Too many left out, or too few frames left to stack
                    // (two, or the only one of a one-frame stack).
                    if left_out.len() > left_out_limit(n) || n - left_out.len() < 2.min(n) {
                        let first = &left_out[0];
                        return Err(Error::TooManyLeftOut {
                            count: left_out.len(),
                            total: n,
                            first: file_label(&order[first.index]),
                            reason: first.reason.clone(),
                        });
                    }
                    stacker.skip_frame()?;
                    transforms.push(None);
                    if !(checks_reverse && stacked < 2) {
                        release_through(i);
                    }
                    self.after_frame(i, stacked, &stacker, snapper.as_deref_mut(), on_event)?;
                    continue;
                }
            };
            if reference.is_none() {
                let size = (staged.width(), staged.height());
                // The reference sizes the stack unless a header already did.
                if allocated != Some(size) {
                    self.ensure_fits(reading, FrameShape::new(size.0, size.1, reading.depth))?;
                    // A left-out frame of another size may have sized it.
                    if allocated.is_some() {
                        stacker = self.new_stacker(total);
                        for _ in 0..i {
                            stacker.skip_frame()?;
                        }
                    }
                    stacker.allocate(size.0, size.1, reading.depth)?;
                    allocated = Some(size);
                }
                // Read now: its conversion may be let go by the end.
                let metadata = if self.is_linear() {
                    let _t = cpu_scope("reference metadata");
                    let file = self.file(&order[i])?;
                    Some(load_metadata_source(&file, self.options.camera_profile)?)
                } else {
                    None
                };
                reference = Some(Reference { size, profile, metadata });
            }
            depth = depth.max(staged.depth());
            if checks_reverse && stacked == 1 && transform.as_ref().is_some_and(magnified) {
                return Ok(None);
            }
            transforms.push(transform);
            drop(head.take());
            reading.headroom.check(&self.ctx)?;
            {
                let _t = cpu_scope("accumulate");
                stacker.accumulate(&staged, transform.as_ref())?;
                self.gpu_check()?;
            }
            stacked += 1;
            if !(checks_reverse && stacked < 2) {
                release_through(i);
            }
            for note in frame_notes {
                match notes.iter_mut().find(|(n, _)| *n == note) {
                    Some((_, count)) => *count += 1,
                    None => notes.push((note, 1)),
                }
            }
            self.after_frame(i, stacked, &stacker, snapper.as_deref_mut(), on_event)?;
        }
        release_through(n - 1);

        let crops = self.options.alignment.crops();
        let crop = match (&aligner, &self.recorded) {
            _ if !crops => None,
            (Some(aligner), _) => aligner.crop(),
            (None, Some(recorded)) => recorded.crop,
            (None, None) => None,
        };
        // At least one frame stacks, or the stack failed as too many left out.
        let reference =
            reference.ok_or_else(|| Error::InvalidState("no frame was stacked".into()))?;
        let mut notes: Vec<String> =
            notes.into_iter().map(|(note, n)| load_note(&note, n, &self.words)).collect();
        // Every frame aligned, so a missing crop means under a quarter of
        // the frame is covered by all.
        if crops && aligner.is_some() && crop.is_none() {
            notes.push(say!(self.words, "note-uncropped"));
        }
        Ok(Some(Pass { order, stacker, transforms, crop, depth, reference, left_out, notes }))
    }

    /// After frame `i` of pass 1: its snapshot, if asked for and anything
    /// stacked, and the progress event.
    fn after_frame(
        &self,
        i: usize,
        stacked: u32,
        stacker: &Stacker,
        snapper: Option<&mut Snapper>,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
    ) -> Result<()> {
        let current = i as u32 + 1;
        if let Some(s) = snapper
            && stacked > 0
            && s.plan.after.contains(&current)
        {
            let shot = self.snapshot(stacker, s.region, current, stacked)?;
            (s.sink)(shot)?;
        }
        let total = self.paths.len() as u32;
        let live = LiveMap::of(&self.ctx, &self.options, stacker);
        on_event(SessionEvent::Accumulated { current, total }, &live);
        Ok(())
    }

    /// The next frame of pass 1, checked against `reference` and placed as
    /// `placing` says. The inner `Err` leaves it out; a frame of another
    /// size fails the stack.
    fn take_frame(
        &self,
        frames: &mut io::FramePrefetcher<LoadedFrame>,
        path: &Path,
        placing: Placing<'_>,
        reference: Option<&Reference>,
    ) -> Result<std::result::Result<TakenFrame, Error>> {
        let frame = match next_frame(frames) {
            Ok(frame) => frame,
            Err(e) => return Ok(Err(e)),
        };
        let size = (frame.staged.width(), frame.staged.height());
        if let Some(reference) = reference
            && size != reference.size
        {
            return Err(Error::FrameSize { file: file_label(path), size, first: reference.size });
        }
        let transform = match placing {
            Placing::Align(aligner) => {
                let prepared = match frame.prepared {
                    Some(Ok(prepared)) => prepared,
                    Some(Err(e)) => return Ok(Err(e.into())),
                    None => {
                        return Ok(Err(AlignmentError::ProcessingFailed(
                            "the frame was not prepared for alignment".into(),
                        )
                        .into()));
                    }
                };
                let _t = cpu_scope("align");
                match self.align_pool.install(|| aligner.align_prepared(prepared)) {
                    Ok(aligned) => Some(frame_alignment(aligned)),
                    Err(e) => return Ok(Err(e.into())),
                }
            }
            Placing::Recorded(transform) => transform,
            Placing::AsIs => None,
        };
        Ok(Ok(TakenFrame {
            staged: frame.staged,
            transform,
            profile: frame.profile,
            notes: frame.notes,
        }))
    }

    /// Frames in `order`, each decoded, prepared by `preparer` and staged on
    /// a prefetch thread, so the calling thread only runs the aligner's step
    /// and feeds the GPU. The window is [`Self::reading`]'s. With
    /// `last_read`, each file is let go once decoded
    /// ([`StackConversions::release`]); the first frame picks the file cache
    /// use ([`file_cache_for`]).
    fn prefetcher(
        &self,
        reading: &Reading,
        order: &[PathBuf],
        preparer: Option<FramePreparer>,
        last_read: bool,
        cancel: &Cancel,
    ) -> io::FramePrefetcher<LoadedFrame> {
        let ctx = self.ctx.clone();
        let staging = reading.staging.clone();
        let align_pool = self.align_pool.clone();
        let files = self.files.clone();
        if let Some(files) = &files {
            files.plan(order);
        }
        let frames = order.len() as u64;
        let cache = Arc::new(std::sync::OnceLock::new());
        let raw = self.options.raw_backend.camera_raw();
        let space = reading.space.clone();
        // Each decode gets its share of the cores the aligner leaves
        // (ADR-143) and watches the window's token.
        let codec_threads = (crate::engine::work::cores()
            .saturating_sub(SessionOptions::align_threads())
            / reading.ahead.window().max(1))
        .max(1);
        let loader = move |path: &Path, stop: &Cancel| {
            crate::engine::work::within(stop, codec_threads, || {
                let cache_for = |file: &Path| *cache.get_or_init(|| file_cache_for(frames, file));
                let into = io::Target::Stack(&space);
                let io::Frame { image, profile, data_area, notes } = match &files {
                    Some(files) => {
                        let file = files.file(path)?;
                        let frame = io::load_frame(&file, cache_for(&file), raw, into);
                        if last_read {
                            files.release(path);
                        }
                        frame?
                    }
                    None => io::load_frame(path, cache_for(path), raw, into)?,
                };
                // The decode can't be stopped; what follows is skipped once
                // the frame isn't wanted.
                stop.check()?;
                let stage = || {
                    let _t = cpu_scope("stage upload (worker)");
                    StagedUpload::with_pool(&ctx, &image, Some(&staging))
                };
                // The staging copy runs beside the prep, on its own thread
                // rather than the busy align pool.
                let (staged, prepared) = match preparer {
                    Some(p) => std::thread::scope(|scope| {
                        let staged = crate::engine::work::thread("stage-upload")
                            .spawn_scoped(scope, stage)
                            .map_err(CoreError::from)?;
                        let prepared = {
                            let _t = cpu_scope("prepare alignment (worker)");
                            align_pool
                                .install(|| p.prepare(&image))
                                .map(|p| p.with_data_area(data_area))
                        };
                        let staged = staged.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
                        Ok::<_, CoreError>((staged, Some(prepared)))
                    })?,
                    None => (stage(), None),
                };
                Ok(LoadedFrame { staged: staged?, prepared, profile, notes })
            })
        };
        let window = reading.ahead.window();
        io::FramePrefetcher::with_loader(order.to_vec(), window, Arc::new(loader), cancel)
    }

    /// Crop and read back the result and package the stack. The stacker
    /// drops once the cropped copy exists, before the readback and display
    /// render allocate.
    fn complete(self, pass: Pass, reversed: bool, staging: &StagingPool) -> Result<SessionOutput> {
        let Pass { stacker, transforms, crop, depth, reference, left_out, notes, .. } = pass;
        // No frame is staged from here: free the staging pool's idle buffers.
        self.ctx.wait_all().map_err(CoreError::from)?;
        staging.release_idle();
        let result_texture = stacker
            .result_texture()
            .ok_or_else(|| Error::InvalidState("no result texture".into()))?;
        let (full_w, full_h) = (result_texture.width(), result_texture.height());
        let crop = crop.filter(|r| trims(r, full_w, full_h));
        let (width, height) = crop.map_or((full_w, full_h), |r| (r.width, r.height));

        // With Rgba16Unorm, the crop is packed to it with the CPU's rounding:
        // the readback source and render input, read after the stacker is
        // gone. Elsewhere the rgba32f crop is read back now and, for a
        // linear stack, copied to render from.
        let region = crop.unwrap_or(Rect { x: 0, y: 0, width: full_w, height: full_h });
        let readback = match &self.common.packer {
            Some(packer) => {
                let _t = cpu_scope("complete: pack");
                Readback::Packed(packer.pack(&self.ctx, result_texture, region))
            }
            None => {
                let _t = cpu_scope("complete: download + convert");
                Readback::Downloaded(download_rgb(&self.ctx, result_texture, region)?)
            }
        };

        // The frame map takes the result's crop: the viewer refits when the
        // image size changes, so an uncropped map would reset the zoom on
        // every toggle. Only the CPU copy is kept.
        let map_timer = cpu_scope("complete: frame map download");
        let frame_map = match stacker.frame_map() {
            Some(planes) => {
                debug_assert_eq!((planes.width(), planes.height()), (full_w, full_h));
                Some(download_planes(&self.ctx, planes, region)?)
            }
            None => None,
        };
        drop(map_timer);

        // A linear stack renders its display from the cropped result; an
        // sRGB stack needs no texture past the stacker.
        let finish = match reference.metadata {
            Some(metadata) => Finish::Linear {
                metadata,
                cropped: match (&readback, crop) {
                    (Readback::Packed(packed), _) => packed.clone(),
                    (Readback::Downloaded(_), Some(r)) => {
                        crop_texture(&self.ctx, result_texture, r)
                    }
                    (Readback::Downloaded(_), None) => result_texture.clone(),
                },
            },
            None => Finish::Srgb { profile: reference.profile },
        };
        // Nothing of the stacker is needed now: dropping it keeps its ~0.9 GB
        // from coexisting with the readback and display render.
        drop(stacker);
        let pixels = match readback {
            Readback::Downloaded(pixels) => pixels,
            Readback::Packed(packed) => {
                let _t = cpu_scope("complete: download + convert");
                download_rgb(&self.ctx, &packed, Rect { x: 0, y: 0, width, height })?
            }
        };
        let (color, display) = match finish {
            Finish::Linear { metadata, cropped } => {
                let render_params = render_params_or_identity(&metadata);
                let display = LinearDisplay { ctx: self.ctx.clone(), cropped };
                (StackColor::Linear { metadata, render_params }, Some(display))
            }
            Finish::Srgb { profile } => {
                let sixteen_bit = depth == FrameDepth::Sixteen;
                (StackColor::Srgb { sixteen_bit, profile }, None)
            }
        };

        let stack = CompletedStack {
            width,
            height,
            pixels,
            frame_map,
            color,
            paths: self.paths,
            geometry: geometry(transforms, left_out, crop, reversed),
            config: self.config,
            options: self.options,
            notes,
            contributions: Default::default(),
        };
        // Counted here, off the thread that shows the result.
        stack.frame_contributions();
        Ok(SessionOutput { stack, display })
    }
}

/// How [`StackingSession::complete`] reads the cropped result back.
enum Readback {
    /// Packed to rgba16 on the GPU, read back once the stacker is gone.
    Packed(wgpu::Texture),
    /// Read back already, quantized from the rgba32f result: 16-bit RGB.
    Downloaded(Vec<u16>),
}

/// What the result's color comes from, with what each kind needs.
enum Finish {
    /// The reference's color science, and the cropped result to render.
    Linear { metadata: deptheos_dng::DngImage, cropped: wgpu::Texture },
    /// The reference's color space.
    Srgb { profile: Option<io::IccProfile> },
}

/// Pass 1's alignment, from pass order into the paths' given order.
fn geometry(
    mut transforms: Vec<Option<FrameAlignment>>,
    left_out: Vec<LeftOut>,
    crop: Option<Rect>,
    reversed: bool,
) -> StackGeometry {
    let n = transforms.len();
    if reversed {
        transforms.reverse();
    }
    let mut left_out: Vec<LeftOut> =
        left_out.into_iter().map(|l| LeftOut { index: flip(reversed, n, l.index), ..l }).collect();
    left_out.sort_by_key(|l| l.index);
    StackGeometry { transforms, crop, reversed, left_out }
}

/// Pass 1's outcome, in processing order, with what its frames went into.
struct Pass {
    order: Vec<PathBuf>,
    stacker: Stacker,
    transforms: Vec<Option<FrameAlignment>>,
    /// The valid area, before checking that it trims anything.
    crop: Option<Rect>,
    /// The deepest frame stacked.
    depth: FrameDepth,
    reference: Reference,
    /// Frames left out, by index into `order`.
    left_out: Vec<LeftOut>,
    /// The stack's notices: load notes and a crop it couldn't make.
    notes: Vec<String>,
}

impl Pass {
    /// Whether frame `index` (into `order`) was left out.
    fn is_left_out(&self, index: usize) -> bool {
        self.left_out.iter().any(|l| l.index == index)
    }
}

/// How pass 1 gives a frame its alignment.
enum Placing<'a> {
    /// Align it to the reference.
    Align(&'a mut Aligner),
    /// Replay its recorded alignment (`None`: the reference, or unaligned).
    Recorded(Option<FrameAlignment>),
    /// Stack it as it is: aligning is off.
    AsIs,
}

/// A frame ready to stack.
struct TakenFrame {
    staged: StagedUpload,
    transform: Option<FrameAlignment>,
    /// Its color space; `None` for sRGB.
    profile: Option<io::IccProfile>,
    /// What its load noted.
    notes: Vec<io::LoadNote>,
}

/// Pass 1's reference, the first frame that stacks: later frames must match
/// its size, and the result takes its color space.
struct Reference {
    size: (u32, u32),
    /// Its color space; `None` for sRGB.
    profile: Option<io::IccProfile>,
    /// Its color science, for a linear stack (read while its file is there).
    metadata: Option<deptheos_dng::DngImage>,
}

/// A frame as the prefetch threads hand it over, its decoded image gone.
struct LoadedFrame {
    staged: StagedUpload,
    /// The aligner's per-frame prep; its error surfaces at the step.
    prepared: Option<std::result::Result<PreparedFrame, AlignmentError>>,
    /// The color space of a standard image's values; `None` for sRGB.
    profile: Option<io::IccProfile>,
    /// What the load had to assume or could not keep.
    notes: Vec<io::LoadNote>,
}

fn next_frame(frames: &mut io::FramePrefetcher<LoadedFrame>) -> Result<LoadedFrame> {
    let _t = cpu_scope("wait for decoded frame");
    Ok(frames
        .next()
        .ok_or_else(|| Error::InvalidState("prefetcher ran out of frames".into()))??)
}

/// The reference's color science, from metadata only: a DNG's own profile,
/// or LibRaw's with a matching installed Adobe DCP profile.
fn load_metadata_source(first: &Path, profile: DcpPreference) -> Result<deptheos_dng::DngImage> {
    if io::is_dng(first) {
        return Ok(deptheos_dng::DngImage::read_metadata(first).map_err(CoreError::from)?);
    }
    let metadata = deptheos_raw::read_metadata(first).map_err(CoreError::from)?;
    Ok(crate::engine::preview::dng_metadata_from_raw(&metadata, profile)?)
}

/// The aligner's per-frame result as the stackers' input type.
fn frame_alignment(a: AlignedFrame) -> FrameAlignment {
    FrameAlignment::new(a.transform, a.brightness)
}

pub(crate) fn check_cancel(cancel: &Cancel) -> Result<()> {
    if cancel.is_canceled() { Err(Error::Canceled) } else { Ok(()) }
}

/// Run `body`, killing `files`' conversions as soon as `cancel` fires; the
/// failure that follows is the cancel ([`canceled_conversion`]).
fn stop_conversions_on_cancel<T>(
    files: Option<Arc<StackConversions>>,
    cancel: &Cancel,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _hook = files.map(|files| cancel.on_cancel(move || files.stop()));
    body().map_err(|e| canceled_conversion(e, cancel))
}

/// `e` as the cancel when it is a conversion's failure or a codec's stop
/// and `cancel` is set; any other error is kept.
fn canceled_conversion(e: Error, cancel: &Cancel) -> Error {
    match e {
        Error::Core(CoreError::Converter(_) | CoreError::Canceled) if cancel.is_canceled() => {
            Error::Canceled
        }
        e => e,
    }
}

/// Refuse a stack of `paths` that mixes frames decoding to linear camera
/// RGB with standard images, camera RAW decoded by `backend`.
pub fn check_same_kind(paths: &[PathBuf], backend: RawBackend) -> Result<()> {
    let Some(first) = paths.first() else { return Ok(()) };
    let kind = FrameKind::of(first, backend);
    match paths.iter().find(|p| FrameKind::of(p, backend) != kind) {
        Some(other) => Err(mixed_kinds(first, other, backend)),
        None => Ok(()),
    }
}

/// `other` can't join a stack whose kind `first` set.
pub(crate) fn mixed_kinds(first: &Path, other: &Path, backend: RawBackend) -> Error {
    Error::MixedKinds {
        file: file_label(other),
        kind: FrameKind::of(other, backend),
        first: file_label(first),
    }
}

/// Whether cropping to `area` would actually trim a `width`×`height` image.
fn trims(area: &Rect, width: u32, height: u32) -> bool {
    area.width > 0
        && area.height > 0
        && !(area.x == 0 && area.y == 0 && area.width == width && area.height == height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reverse check ignores the aligner's wavering around 1.
    #[test]
    fn the_reverse_check_has_a_deadband() {
        let scaled = |s: f32| {
            FrameAlignment::from_transform(nalgebra::Affine2::from_matrix_unchecked(
                nalgebra::Matrix3::new(s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0),
            ))
        };
        assert!(!magnified(&scaled(1.0)));
        assert!(!magnified(&scaled(1.0005)), "within the deadband");
        assert!(magnified(&scaled(1.002)));
        assert!(!magnified(&scaled(0.98)), "shrinking");
    }

    /// Only a conversion's failure is taken for the cancel.
    #[test]
    fn cancel_hides_only_a_conversion_it_stopped() {
        let converter = || {
            Error::Core(CoreError::Converter(deptheos_core::ConverterError::Stopped(
                PathBuf::from("a.CR3"),
            )))
        };
        let real = || Error::InvalidState("out of memory".into());
        let cancel = Cancel::root();
        cancel.cancel();
        assert!(matches!(canceled_conversion(converter(), &cancel), Error::Canceled));
        assert!(matches!(canceled_conversion(real(), &cancel), Error::InvalidState(_)));
        let running = Cancel::root();
        assert!(matches!(
            canceled_conversion(converter(), &running),
            Error::Core(CoreError::Converter(_))
        ));
    }
}
