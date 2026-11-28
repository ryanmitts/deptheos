//! The engine every front end drives: one headless GPU context (ADR-100),
//! the pipelines every preview shares, and display images built on the CPU
//! ([`crate::engine::present`]). Shared as `Arc<Engine>`; each method runs
//! on the calling thread.
//!
//! Only CPU data crosses back ([`PresentedImage`],
//! [`CpuImage`](crate::CpuImage), [`CompletedStack`]), so a shell needs no
//! GPU types.

pub(crate) mod colormap;
pub mod generation;
pub mod logs;
pub mod look;
pub mod memory;
pub mod orient;
pub mod present;
pub(crate) mod preview;
pub mod store;
pub mod work;

#[cfg(test)]
mod handoff_tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use deptheos_core::converter::parallel_jobs as converter_parallel_jobs;
use deptheos_core::{CommonPipelines, FrameAlignment, io};
use deptheos_gpu::profile::cpu_scope;
use deptheos_gpu::{DeviceChoice, GpuContext, PooledTexture};
use deptheos_render::dng_render::DngRenderPipeline;
use image::math::Rect;
use image::{DynamicImage, RgbImage};

use crate::library::adjustments::RenderAdjustments;
use crate::library::history::THUMBNAIL_SIDE;
use crate::lock;
use crate::runs::session::{
    CompletedStack, DcpPreference, SaveOptions, SessionEvent, SessionOptions, StackingSession,
    download_gray, download_rgb, mixed_kinds, render_params_or_identity,
};
use crate::runs::stacker::StackerConfig;
use crate::words::Words;
use crate::{Converted, DngCache, RawBackend, StackConversions};
use crate::{Error, RenderUse};
use deptheos_render::dng_render::Constants;
use deptheos_render::reframe::Size;
use deptheos_render::{Adjustments, Color, Draw, Reframe, resolve};

use crate::engine::orient::Orient;
use look::Look;
use present::{
    PresentedImage, ToDisplay, conversion, display_space, present_map, present_rgb, present_rgb16,
    present_texture,
};

type Result<T> = std::result::Result<T, Error>;

/// Builds rayon's global pool once with 8 MiB stacks, as the codecs that
/// decode on it need more than the default 2 MiB. A pool started earlier
/// keeps its stacks, with a warning.
fn build_rayon_pool() {
    static BUILT: std::sync::Once = std::sync::Once::new();
    BUILT.call_once(|| {
        if let Err(e) = rayon::ThreadPoolBuilder::new()
            .stack_size(crate::engine::work::STACK_SIZE)
            .build_global()
        {
            tracing::warn!("rayon's pool was started before the engine: {e}");
        }
    });
}

/// Longest side of the live map, in logical pixels; a shell scales it by
/// its display and passes it to [`Engine::stack`]. A full-size readback per
/// frame would cost more than the frame's GPU work.
pub const LIVE_PREVIEW_SIDE: u32 = 1024;

/// What is drawn over a compare source ([`Engine::load_compare`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overlay {
    #[default]
    None,
    /// The pixels the result took from the frame as they are, the rest
    /// dimmed to gray, in proportion where blended; read off
    /// [`CompletedStack::frame_map`].
    Contribution,
}

/// What [`Engine::paint_source`] brings into a stack.
pub enum SourceInput<'a> {
    /// A source frame, placed by
    /// [`StackGeometry::frame_alignment`](crate::StackGeometry::frame_alignment).
    Frame { path: &'a Path, alignment: FrameAlignment },
    /// Another render's unpainted pixels, placed and exposure-matched by
    /// [`CompletedStack::alignment_to`].
    Render { other: &'a CompletedStack, pixels: &'a [u16], alignment: FrameAlignment },
}

/// A texture in a draw's colors as far as the GPU takes them, and the
/// conversion left for the CPU ([`Engine::colored`]).
struct Colored {
    texture: PooledTexture,
    to: ToDisplay,
}

/// A finished stack with its display and half-size map ready to show,
/// both turned and flipped by `reframe`, its saved one ([`Orient`]).
pub struct PresentedStack {
    pub stack: CompletedStack,
    pub display: Option<PresentedImage>,
    pub map: Option<PresentedImage>,
    pub reframe: Reframe,
}

impl PresentedStack {
    /// See [`PresentedImage::thumbnail`].
    pub fn thumbnail(&self) -> Option<RgbImage> {
        self.display.as_ref()?.thumbnail()
    }
}

impl PresentedImage {
    /// A render thumbnail in sRGB, whatever the screen's space: the
    /// smallest level still [`THUMBNAIL_SIDE`] on its long side (the store
    /// reduces the rest), or the full image when smaller.
    pub fn thumbnail(&self) -> Option<RgbImage> {
        let level = self
            .levels
            .iter()
            .rev()
            .find(|l| l.width.max(l.height) >= THUMBNAIL_SIDE)
            .or(self.levels.first())?;
        Some(present::in_srgb(level.assemble().to_rgb(), self.space()))
    }
}

/// The whole of a `stack`-sized image.
fn whole(stack: &CompletedStack) -> Rect {
    Rect { x: 0, y: 0, width: stack.width, height: stack.height }
}

/// The one engine of a process; see the module.
pub struct Engine {
    gpu: Arc<GpuContext>,
    /// Compiled once, shared with every session and save.
    common: Arc<CommonPipelines>,
    /// The color render, which the stackers never use.
    color: DngRenderPipeline,
    /// Adobe-converted frames, made on first use; a failure to make it is
    /// not kept, so a later conversion retries.
    dng_cache: Mutex<Option<Arc<DngCache>>>,
    /// The camera profile RAW previews use, matching the shell's stacks.
    camera_profile: Mutex<DcpPreference>,
    /// The language of the engine's sentences (ADR-161).
    words: Words,
    /// Each render's stored picture with paint, as it changes.
    generations: generation::Generations,
}

impl Engine {
    /// The engine on a new headless context on the fastest GPU.
    pub fn new(words: Words) -> Result<Arc<Self>> {
        Self::with_device(&DeviceChoice::Automatic, words)
    }

    /// The engine on a new headless context on the device `choice` names.
    pub fn with_device(choice: &DeviceChoice, words: Words) -> Result<Arc<Self>> {
        let gpu = GpuContext::with_device(choice).map_err(deptheos_core::CoreError::from)?;
        let engine = Self::with_gpu(Arc::new(gpu), words);
        // A shared buffer that failed (out of memory) would otherwise fail
        // every later stack with a confusing error on another thread.
        engine.gpu.check().map_err(deptheos_core::CoreError::from)?;
        Ok(engine)
    }

    /// The engine on an existing context; also builds rayon's pool.
    pub fn with_gpu(gpu: Arc<GpuContext>, words: Words) -> Arc<Self> {
        build_rayon_pool();
        Arc::new(Self {
            common: Arc::new(CommonPipelines::new(&gpu)),
            color: DngRenderPipeline::new(&gpu),
            gpu,
            dng_cache: Mutex::new(None),
            camera_profile: Mutex::new(DcpPreference::default()),
            words,
            generations: generation::Generations::default(),
        })
    }

    pub fn gpu(&self) -> &Arc<GpuContext> {
        &self.gpu
    }

    /// Each render's generation: paint raises it, Adjust's GPU copies
    /// follow it.
    pub fn generations(&self) -> &generation::Generations {
        &self.generations
    }

    /// The language the engine's sentences are worded in.
    pub fn words(&self) -> &Words {
        &self.words
    }

    pub fn common(&self) -> &Arc<CommonPipelines> {
        &self.common
    }

    /// The color render every view and export goes through.
    pub fn color(&self) -> &DngRenderPipeline {
        &self.color
    }

    /// Colors RAW previews with the profile the shell's stacks use
    /// ([`SessionOptions::camera_profile`]).
    pub fn set_camera_profile(&self, profile: DcpPreference) {
        *lock(&self.camera_profile) = profile;
    }

    fn camera_profile(&self) -> DcpPreference {
        *lock(&self.camera_profile)
    }

    /// The cache of Adobe conversions, made now if need be.
    fn dng_cache(&self) -> Result<Arc<DngCache>> {
        let mut slot = lock(&self.dng_cache);
        match &*slot {
            Some(cache) => Ok(cache.clone()),
            None => {
                // A folder that can't be made is a disk error, explained as one.
                let cache = DngCache::for_this_process(None).map_err(|e| {
                    tracing::warn!("couldn't make the DNG cache: {e}");
                    Error::Core(deptheos_core::CoreError::Io(e))
                })?;
                Ok(slot.insert(Arc::new(cache)).clone())
            }
        }
    }

    /// For a process about to exit: deletes the converted frames and leaks
    /// the engine, so its GPU device is never destroyed (that aborts on
    /// Mesa's Venus driver, in QEMU and UTM VMs); the OS frees it at exit.
    pub fn keep_until_exit(self: Arc<Self>) {
        if let Some(cache) = lock(&self.dng_cache).take() {
            cache.clear();
            // A clone held elsewhere leaves the lock file to the next sweep.
            drop(cache);
        }
        tracing::debug!("engine kept until exit, its GPU device not destroyed");
        std::mem::forget(self);
    }

    /// A file for display, as stored: RAW and DNG render on the GPU through
    /// their own color science; anything else is shown without the GPU.
    pub fn load_preview(&self, path: &Path, backend: RawBackend) -> Result<PresentedImage> {
        if !backend.decodes_linear(path) {
            let frame =
                io::load_frame(path, io::FileCache::Use, backend.camera_raw(), io::Target::Own)?;
            let to = conversion(frame.profile.as_ref(), display_space());
            return Ok(present_image(&frame.image, &to));
        }
        let (image, metadata) = preview::decode_linear_with_metadata(path, self.camera_profile())?;
        let params = render_params_or_identity(&metadata);
        let linear = self.upload(&image)?;
        let draw = resolve(Color::Linear(&params), None, display_space());
        self.show(self.colored(linear, draw))
    }

    /// A source frame in `stack`'s color space and geometry, warped and
    /// cropped; linear for linear stacks (paint blends in linear light).
    /// `frame_index` indexes `stack.paths`; `None` (not in the stack) is
    /// only cropped. A file of the other kind than the stack is refused.
    pub(crate) fn aligned_source(
        &self,
        path: &Path,
        frame_index: Option<usize>,
        stack: &CompletedStack,
    ) -> Result<wgpu::Texture> {
        let uploaded = self.upload_frame(path, stack)?;
        let transform =
            frame_index.and_then(|i| stack.geometry.transforms.get(i)).and_then(|t| t.as_ref());
        Ok(self.common.warper.warp(&self.gpu, &uploaded, transform, stack.geometry.crop))
    }

    /// A frame aligned to `look`'s stack and drawn in `look`, with
    /// `overlay`, for display. A frame left out, or a render without a frame
    /// map, has no contribution to show.
    pub fn load_compare(
        &self,
        path: &Path,
        frame_index: Option<usize>,
        look: Look<'_>,
        overlay: Overlay,
    ) -> Result<PresentedImage> {
        let stack = look.stack();
        let aligned = self.aligned_source(path, frame_index, stack)?;
        let Colored { texture, to } = self.colored(aligned, look.resolve());
        let texture = match (overlay, frame_index, stack.frame_map.as_ref()) {
            (Overlay::Contribution, Some(index), Some(map))
                if !stack.geometry.is_left_out(index) =>
            {
                let _t = cpu_scope("contribution");
                let lit = self.common.contribution.light(
                    &self.gpu,
                    &texture,
                    map,
                    stack.frame_plane(index),
                );
                PooledTexture::unpooled(lit)
            }
            _ => texture,
        };
        self.show(Colored { texture, to })
    }

    /// [`Self::load_compare`] as a full-size image, for the CLI.
    pub fn compare_rgb(
        &self,
        path: &Path,
        frame_index: Option<usize>,
        look: Look<'_>,
        overlay: Overlay,
    ) -> Result<RgbImage> {
        let presented = self.load_compare(path, frame_index, look, overlay)?;
        let level = presented
            .levels
            .first()
            .ok_or_else(|| Error::InvalidState("the compare source has no pixels".into()))?;
        Ok(present::in_srgb(level.assemble().to_rgb(), presented.space()))
    }

    /// [`Self::load_compare`] for another render: `other`'s stack in
    /// `base`'s geometry, drawn in `other`, aligned through a shared frame
    /// ([`CompletedStack::alignment_to`]).
    pub fn load_render_compare(
        &self,
        base: &CompletedStack,
        other: Look<'_>,
    ) -> Result<PresentedImage> {
        let stack = other.stack();
        // Placed only: its own exposure, to compare it as it came out.
        let alignment =
            base.alignment_to(stack).map(|a| FrameAlignment::from_transform(a.transform));
        let warped = self.warp_render(base, stack, &stack.pixels, alignment, RenderUse::Compare)?;
        self.show(self.colored(warped, other.resolve()))
    }

    /// `pixels` of `other` warped into `base`'s geometry (`alignment`
    /// `None`: no shared frame), edge pixels repeating where it has no data.
    /// Both must be linear or both not.
    fn warp_render(
        &self,
        base: &CompletedStack,
        other: &CompletedStack,
        pixels: &[u16],
        alignment: Option<FrameAlignment>,
        purpose: RenderUse,
    ) -> Result<wgpu::Texture> {
        if base.is_linear() != other.is_linear() {
            return Err(Error::MixedRenders { purpose });
        }
        let alignment = alignment.ok_or(Error::NoSharedFrame)?;
        let uploaded = self.upload_rgb16(pixels, other.width, other.height)?;
        let area = base.geometry.crop.unwrap_or_else(|| whole(base));
        Ok(self.common.warper.warp(&self.gpu, &uploaded, Some(&alignment), Some(area)))
    }

    /// A paint source: `input` warped into `look`'s stack's geometry and
    /// exposure, with its pixels (RGB u16, the stack's kind) when
    /// `with_pixels`, and its display in `look`. Without pixels the display
    /// starts at half size, for scrubbing at a quarter of the memory.
    pub fn paint_source(
        &self,
        look: Look<'_>,
        input: SourceInput<'_>,
        with_pixels: bool,
    ) -> Result<(Option<Vec<u16>>, PresentedImage)> {
        let stack = look.stack();
        let warped = match input {
            SourceInput::Frame { path, alignment } => {
                let uploaded = self.upload_frame(path, stack)?;
                self.common.warper.warp(&self.gpu, &uploaded, Some(&alignment), stack.geometry.crop)
            }
            // Exposure-matched: it becomes the stack's pixels where painted.
            SourceInput::Render { other, pixels, alignment } => {
                self.warp_render(stack, other, pixels, Some(alignment), RenderUse::Paint)?
            }
        };
        let pixels =
            if with_pixels { Some(download_rgb(&self.gpu, &warped, whole(stack))?) } else { None };
        let display = self.show(self.colored(warped, look.resolve()))?;
        Ok((pixels, if with_pixels { display } else { display.below_full_size() }))
    }

    /// `pixels` (`look`'s stack's own or painted) shown in `look`.
    pub fn present_pixels(&self, look: Look<'_>, pixels: &[u16]) -> Result<PresentedImage> {
        let stack = look.stack();
        match look.resolve() {
            Draw::Stored { profile, output } => {
                let to = conversion(profile, output);
                Ok(present_rgb16(pixels, stack.width, stack.height, &to))
            }
            Draw::Rendered(constants) => {
                let texture = self.upload_for(&constants, pixels, stack.width, stack.height)?;
                let texture = self.color.render(&self.gpu, &texture, &constants);
                self.show(Colored { texture, to: ToDisplay::drawn(constants.output()) })
            }
        }
    }

    /// `texture` of stored pixels in `draw`'s colors as far as the GPU
    /// takes them, with the conversion left for the CPU.
    fn colored(&self, texture: wgpu::Texture, draw: Draw<'_>) -> Colored {
        match draw {
            Draw::Rendered(constants) => Colored {
                texture: self.color.render(&self.gpu, &texture, &constants),
                to: ToDisplay::drawn(constants.output()),
            },
            Draw::Stored { profile, output } => Colored {
                texture: PooledTexture::unpooled(texture),
                to: conversion(profile, output),
            },
        }
    }

    /// A [`Colored`] texture read back for display.
    fn show(&self, colored: Colored) -> Result<PresentedImage> {
        self.present(&colored.texture, &colored.to)
    }

    /// Runs a stack on the calling thread. With [`SessionOptions::live_map`],
    /// `on_live_map` gets the frame map so far
    /// ([`crate::runs::stacker::Stacker::render_live`]), `live_side` on its
    /// long side, after each frame and after pass 1.
    ///
    /// The live map is read back on its own thread, since a readback waits
    /// for the whole queue and would serialize the aligner with the GPU. A
    /// single-slot mailbox drops stale maps, never frames. The thread is
    /// joined before this returns.
    #[allow(clippy::too_many_arguments)]
    pub fn stack(
        &self,
        paths: Vec<PathBuf>,
        config: StackerConfig,
        options: SessionOptions,
        adjustments: &Adjustments,
        live_side: u32,
        cancel: &deptheos_core::Cancel,
        mut on_event: impl FnMut(SessionEvent),
        on_live_map: impl Fn(PresentedImage) + Sync,
    ) -> Result<PresentedStack> {
        // Past the session's run too: its result is read back after.
        let _stacking = crate::engine::work::stacking();
        let session = self.session(paths, config, options)?;
        // The stackers bring their own uploaders; free the preview one's.
        self.common.uploader().release_resources();
        let mailbox = LiveMapMailbox::default();
        let output = std::thread::scope(|scope| {
            // Closes on every exit, a panic included, so the scope can join.
            let _close = CloseOnDrop(&mailbox);
            crate::engine::work::thread("live-map")
                .spawn_scoped(scope, || {
                    while let Some(map) = mailbox.take() {
                        let _t = cpu_scope("live map readback (worker)");
                        match self.live_map_image(&map) {
                            Ok(image) => on_live_map(image),
                            Err(e) => tracing::warn!("live map readback failed: {e}"),
                        }
                    }
                })
                .map_err(|e| Error::Core(e.into()))?;
            session.run(
                &mut |event, live| {
                    let changed =
                        matches!(event, SessionEvent::Accumulated { .. } | SessionEvent::Finalized);
                    if changed {
                        let _t = cpu_scope("live map render");
                        if let Some(map) = live.render(live_side) {
                            mailbox.offer(map);
                        }
                    }
                    on_event(event);
                },
                cancel,
            )
        })?;
        let display = match (output.display, Look::shown(&output.stack, adjustments).resolve()) {
            (Some(display), Draw::Rendered(constants)) => Some(Colored {
                texture: display.render(&self.color, &constants),
                to: ToDisplay::drawn(constants.output()),
            }),
            _ => None,
        };
        let saved = RenderAdjustments { adjustments: adjustments.clone(), reframe: Reframe::NONE };
        Ok(self.presented(output.stack, &saved, display))
    }

    /// A session over `paths`, ready to run; every stack starts here. With
    /// the Adobe backend, frames convert into this engine's cache as the
    /// session reads them, started only after [`StackingSession::new`] has
    /// refused a mixed stack. A cancel kills running conversions.
    pub fn session(
        &self,
        paths: Vec<PathBuf>,
        config: StackerConfig,
        options: SessionOptions,
    ) -> Result<StackingSession> {
        let conversions_of = paths.clone();
        let mut session = StackingSession::new(
            self.gpu.clone(),
            self.common.clone(),
            paths,
            config,
            options,
            self.words.clone(),
        )?;
        if let Some(conversions) = self.start_conversions(&conversions_of, options.raw_backend)? {
            session = session.with_frame_files(conversions);
        }
        Ok(session)
    }

    /// Converts the frames `backend` converts now, held until the result
    /// drops, for several sessions over them (a sweep) or to time a stack
    /// without conversions. `Ok(None)` means `cancel` stopped it.
    pub fn convert_first(
        &self,
        paths: &[PathBuf],
        backend: RawBackend,
        cancel: &deptheos_core::Cancel,
        on_progress: impl FnMut(u32, u32),
    ) -> Result<Option<Converted>> {
        if !paths.iter().any(|p| backend.converts(p)) {
            return Ok(Some(Converted::default()));
        }
        let cache = self.dng_cache()?;
        let held =
            cache.hold_all(paths, backend, converter_parallel_jobs(), cancel, on_progress)?;
        Ok(held)
    }

    /// The conversions a stack of `paths` needs, started now; `None` when
    /// there are none.
    fn start_conversions(
        &self,
        paths: &[PathBuf],
        backend: RawBackend,
    ) -> Result<Option<Arc<StackConversions>>> {
        if !paths.iter().any(|p| backend.converts(p)) {
            return Ok(None);
        }
        let cache = self.dng_cache()?;
        let jobs = converter_parallel_jobs();
        Ok(Some(Arc::new(StackConversions::start(cache, paths.to_vec(), backend, jobs))))
    }

    /// A finished stack read from disk, made ready to show under
    /// `adjustments` as [`Self::stack`] does.
    pub fn present_stack(
        &self,
        stack: CompletedStack,
        adjustments: &Adjustments,
    ) -> Result<PresentedStack> {
        let saved = RenderAdjustments { adjustments: adjustments.clone(), reframe: Reframe::NONE };
        self.present_saved(stack, &saved)
    }

    /// A render read from the Library, made ready to show under what it
    /// was saved with ([`Look::shown_saved`]).
    pub fn present_saved(
        &self,
        stack: CompletedStack,
        saved: &RenderAdjustments,
    ) -> Result<PresentedStack> {
        let display = match Look::shown_saved(&stack, saved).resolve() {
            Draw::Rendered(constants) => {
                let _t = cpu_scope("present: display render");
                let (w, h) = (stack.width, stack.height);
                let input = self.upload_for(&constants, &stack.pixels, w, h)?;
                let texture = self.color.render(&self.gpu, &input, &constants);
                Some(Colored { texture, to: ToDisplay::drawn(constants.output()) })
            }
            Draw::Stored { .. } => None,
        };
        Ok(self.presented(stack, saved, display))
    }

    /// The display alone of `stack` under what it was saved with, turned
    /// and flipped as [`Self::present_saved`] hands it over: a display made
    /// again after the app let go of it.
    pub fn present_display(
        &self,
        stack: &CompletedStack,
        saved: &RenderAdjustments,
    ) -> Result<PresentedImage> {
        let display = self.present_pixels(Look::shown_saved(stack, saved), &stack.pixels)?;
        let orient = Orient::of(saved.reframe, Size { width: stack.width, height: stack.height });
        Ok(orient.presented(display))
    }

    /// Save `look`'s stack, its format from the extension: `.dng` writes
    /// the linear pixels (linear stacks only), others drawn in `look`.
    /// JPEG, PNG and TIFF carry the reference's EXIF and the stack's XMP
    /// ([`crate::library::export_tags`]).
    pub fn save(&self, look: Look<'_>, path: &Path, save: SaveOptions) -> Result<()> {
        self.save_pixels(look, &look.stack().pixels, path, save)
    }

    /// [`Self::save`] with `pixels` (retouched) in place of the stack's own.
    pub fn save_pixels(
        &self,
        look: Look<'_>,
        pixels: &[u16],
        path: &Path,
        save: SaveOptions,
    ) -> Result<()> {
        let tags = crate::library::export_tags::export_metadata(look);
        crate::runs::session::export(self, look, pixels, path, save, &tags)
    }

    /// Saves `stack` with the paint `flattened` answers
    /// ([`crate::paint::PaintSession::flatten`]), or as it is when there is
    /// none. Unreadable paint ([`Error::PaintUnread`]) or a worker gone
    /// fails the save rather than drop the retouching. Blocks until written.
    pub fn save_flattened(
        &self,
        look: Look<'_>,
        flattened: Option<std::sync::mpsc::Receiver<crate::paint::Flattened>>,
        path: &Path,
        save: SaveOptions,
    ) -> Result<()> {
        let pixels = match flattened.map(|rx| rx.recv()) {
            None => None,
            Some(Ok(pixels)) => pixels?,
            Some(Err(_)) => {
                return Err(Error::InvalidState(
                    "the paint worker stopped before the paint was flattened".into(),
                ));
            }
        };
        match pixels {
            Some(pixels) => self.save_pixels(look, &pixels, path, save),
            None => self.save(look, path, save),
        }
    }

    /// A texture read back for display, through `to`.
    pub(crate) fn present(
        &self,
        texture: &wgpu::Texture,
        to: &ToDisplay,
    ) -> Result<PresentedImage> {
        present_texture(&self.gpu, texture, to)
    }

    /// The display and map for `stack` under `adjustments`. A rendered
    /// display (`display`) is read back once and dropped; otherwise the
    /// stack's pixels are drawn.
    fn presented(
        &self,
        stack: CompletedStack,
        saved: &RenderAdjustments,
        display: Option<Colored>,
    ) -> PresentedStack {
        let display = {
            let _t = cpu_scope("present: display");
            match display {
                Some(colored) => self.show(colored),
                None => self.present_pixels(Look::shown_saved(&stack, saved), &stack.pixels),
            }
            .inspect_err(|e| tracing::warn!("the stack's display readback failed: {e}"))
            .ok()
        };
        let _t = cpu_scope("present: map");
        let map = stack.present_map(None);
        drop(_t);
        // Turned and flipped at the hand-off; the crop is the app's window.
        let orient = Orient::of(saved.reframe, Size { width: stack.width, height: stack.height });
        let (display, map) =
            (display.map(|d| orient.presented(d)), map.map(|m| orient.presented(m)));
        PresentedStack { stack, display, map, reframe: saved.reframe }
    }

    /// A live map ([`crate::LiveMap::render`]) read back in the depth map's
    /// colors.
    fn live_map_image(&self, map: &wgpu::Texture) -> Result<PresentedImage> {
        let gray = download_gray(
            &self.gpu,
            map,
            Rect { x: 0, y: 0, width: map.width(), height: map.height() },
        )?;
        Ok(PresentedImage::single(present_map(&gray)))
    }

    /// `path` decoded as `stack`'s frames were and uploaded; a file of the
    /// other kind is refused.
    fn upload_frame(&self, path: &Path, stack: &CompletedStack) -> Result<wgpu::Texture> {
        let backend = stack.options.raw_backend;
        if backend.decodes_linear(path) != stack.is_linear() {
            return Err(mixed_kinds(&stack.paths[0], path, backend));
        }
        let converted = match backend.converts(path) {
            true => Some(self.dng_cache()?.open(path)?),
            false => None,
        };
        let file = converted.as_ref().map_or(path, |c| c.path());
        let space = io::StackSpace::new(stack.profile().cloned());
        let into = io::Target::Stack(&space);
        self.upload(&io::load_frame(file, io::FileCache::Use, backend.camera_raw(), into)?.image)
    }

    /// The uploader keeps its frame-sized buffer between calls, so clicking
    /// through previews reuses one allocation; [`Self::stack`] releases it.
    fn upload(&self, image: &DynamicImage) -> Result<wgpu::Texture> {
        Ok(preview::upload_texture(&self.gpu, &mut self.common.uploader(), image)?)
    }

    /// Interleaved RGB u16 pixels uploaded, through the same uploader.
    /// Stored `pixels` uploaded as `constants` draw them: as they are, or
    /// linearized on the CPU first.
    pub(crate) fn upload_for(
        &self,
        constants: &Constants,
        pixels: &[u16],
        width: u32,
        height: u32,
    ) -> Result<wgpu::Texture> {
        match constants.linearizer() {
            Some(linearizer) => Ok(linearizer.upload(&self.gpu, pixels, width, height)),
            None => self.upload_rgb16(pixels, width, height),
        }
    }

    fn upload_rgb16(&self, pixels: &[u16], width: u32, height: u32) -> Result<wgpu::Texture> {
        Ok(preview::upload_rgb16_texture(
            &self.gpu,
            &mut self.common.uploader(),
            pixels,
            width,
            height,
        )?)
    }
}

/// A non-RAW file as displayed through `to`; anything but RGB8 or RGB16,
/// or converted, is widened to RGB16 first.
fn present_image(image: &DynamicImage, to: &ToDisplay) -> PresentedImage {
    let (w, h) = (image.width(), image.height());
    if to.convert.is_some() {
        let rgb = image.to_rgb16();
        return present_rgb16(rgb.as_raw(), w, h, to);
    }
    match image {
        DynamicImage::ImageRgb8(i) => present_rgb(i.as_raw(), w, h, |v| v),
        DynamicImage::ImageRgb16(i) => present_rgb(i.as_raw(), w, h, io::u16_to_u8),
        other => present_rgb(other.to_rgb16().as_raw(), w, h, io::u16_to_u8),
    }
}

/// A single-slot mailbox carrying the newest live map to the readback
/// thread.
#[derive(Default)]
struct LiveMapMailbox {
    slot: Mutex<Slot>,
    changed: Condvar,
}

/// What the mailbox holds. Transitions:
///
/// - `Open` or `Waiting` → `Waiting` on [`LiveMapMailbox::offer`], a newer
///   map replacing one not taken.
/// - `Waiting` → `Open` on [`LiveMapMailbox::take`].
/// - `Open` → `Closed` and `Waiting` → `Draining` on
///   [`LiveMapMailbox::close`]; the waiting map is still read back.
/// - `Draining` → `Closed` on `take`.
///
/// Offer and take log at trace, as they run every frame.
#[derive(Default)]
enum Slot {
    #[default]
    Open,
    Waiting(wgpu::Texture),
    Draining(wgpu::Texture),
    Closed,
}

impl LiveMapMailbox {
    /// Replace whatever is waiting with `map`.
    fn offer(&self, map: wgpu::Texture) {
        let mut slot = lock(&self.slot);
        *slot = match std::mem::take(&mut *slot) {
            Slot::Open | Slot::Waiting(_) => Slot::Waiting(map),
            // Not expected after close; still read back if it happens.
            Slot::Draining(_) | Slot::Closed => Slot::Draining(map),
        };
        tracing::trace!("live map: one waiting");
        self.changed.notify_one();
    }

    /// No more maps will arrive; `take` returns `None` once drained.
    fn close(&self) {
        let mut slot = lock(&self.slot);
        *slot = match std::mem::take(&mut *slot) {
            Slot::Waiting(map) | Slot::Draining(map) => {
                tracing::debug!("live map: closed with one still to read back");
                Slot::Draining(map)
            }
            Slot::Open | Slot::Closed => {
                tracing::debug!("live map: closed");
                Slot::Closed
            }
        };
        self.changed.notify_one();
    }

    /// Block for the next map; `None` after `close` with nothing pending.
    fn take(&self) -> Option<wgpu::Texture> {
        let mut slot = lock(&self.slot);
        loop {
            match std::mem::take(&mut *slot) {
                Slot::Waiting(map) => {
                    tracing::trace!("live map: taken");
                    return Some(map);
                }
                Slot::Draining(map) => {
                    *slot = Slot::Closed;
                    tracing::debug!("live map: the last one taken");
                    return Some(map);
                }
                Slot::Closed => {
                    *slot = Slot::Closed;
                    return None;
                }
                Slot::Open => slot = self.changed.wait(slot).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }
}

struct CloseOnDrop<'a>(&'a LiveMapMailbox);

impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}
