//! Wavelet focus stacking, after Petteri Aimonen's focus-stack
//! (<https://github.com/PetteriAimonen/focus-stack>), streamed so that a
//! stack of any length holds one frame's worth of coefficients.
//!
//! Lifecycle, like the depth stacker's two passes:
//! 1. `accumulate` per frame: the frame's luma goes through a complex
//!    Daubechies-6 wavelet; each detail coefficient keeps the frame whose
//!    3×3 energy there is highest (winner-take-all), the coarsest lowpass
//!    band is averaged over the frames.
//! 2. `end_pass_one`: optional consistency filters and denoise on the
//!    winners, then the inverse transform gives a merged gray target.
//! 3. `blend_frame` per frame, any order: each pixel keeps the color of
//!    the frame whose luma is closest to the target.
//! 4. `finalize`: the result and the frame map.
//!
//! Running state is 32-bit (half floats overflow at the coarse levels), and
//! frames are weighted by their footprint, so a warped frame's edge smear
//! never wins.

mod pipeline;

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{
    create_bind_group, dispatch_compute, dispatch_compute_textures, tex,
};
use pipeline::WaveletPipelines;
use std::sync::Arc;
use tracing::{debug, info};
use wgpu::Texture;

use crate::StackInfo;
use crate::accum::check_binding_fits;
use crate::error::{CoreError, Result};
use crate::ledger::SecondPass;
use crate::plane::FramePlane;
use crate::stacker::{self, Intake};
use crate::transform::{FrameAlignment, WarpParams};
use crate::upload::{FrameDepth, FrameInput, Source};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveletConfig {
    /// Soft threshold on the merged detail coefficients' energy, on a
    /// 0–255 scale; 0 is off.
    pub denoise: f32,
    /// The consistency filters: 0 off, 1 the 2-of-3 subband vote, 2 that
    /// and the neighbor outlier filter. A coefficient can only be swapped
    /// for its runner-up. Doubles the winners' memory.
    pub consistency: u32,
}

/// Inclusive `denoise` range and the app's slider step.
pub const DENOISE_RANGE: (f32, f32) = (0.0, 10.0);
pub const DENOISE_STEP: f32 = 0.1;
/// The largest `consistency`.
pub const MAX_CONSISTENCY: u32 = 2;

impl WaveletConfig {
    pub const DEFAULT_DENOISE: f32 = 1.0;

    pub fn validate(&self) -> Result<()> {
        use crate::error::Setting;
        Setting::Denoise.check(self.denoise, DENOISE_RANGE)?;
        Setting::Consistency.check(self.consistency as f32, (0.0, MAX_CONSISTENCY as f32))
    }

    /// Whether each coefficient keeps its runner-up, for the consistency
    /// filters to swap in.
    fn runner_up(&self) -> bool {
        self.consistency > 0
    }
}

impl Default for WaveletConfig {
    fn default() -> Self {
        Self { denoise: Self::DEFAULT_DENOISE, consistency: 0 }
    }
}

/// The transform stops when the shorter side reaches this many pixels.
const MIN_COARSE: u32 = 8;
/// The reflection padding needs a few pixels to mirror.
const MIN_SIDE: u32 = 4;
/// At most this many levels.
const MAX_LEVELS: u32 = 9;
/// Bytes of one vec4f state entry.
const ENTRY: u64 = 16;

/// Level count and padded size for a frame: halve the shorter side until
/// it is at most [`MIN_COARSE`] (at most [`MAX_LEVELS`] times, at least
/// once); pad each side up to a multiple of `2^levels`.
fn plan(width: u32, height: u32) -> (usize, u32, u32) {
    let mut size = width.min(height);
    let mut levels = 0u32;
    while size > MIN_COARSE && levels < MAX_LEVELS {
        size /= 2;
        levels += 1;
    }
    let levels = levels.max(1);
    let align = 1u32 << levels;
    (levels as usize, width.next_multiple_of(align), height.next_multiple_of(align))
}

mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct WaveletDims {
        pub(super) src_width: u32,
        pub(super) src_height: u32,
        pub(super) dst_width: u32,
        pub(super) dst_height: u32,
        pub(super) direction: u32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct WtaParams {
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) frame: f32,
        pub(super) runner_up: u32,
        pub(super) scale: f32,
        pub(super) src_w: f32,
        pub(super) src_h: f32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct LiveParams {
        pub(super) width: u32,
        pub(super) step: f32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct LlParams {
        pub(super) scale: f32,
        pub(super) src_w: f32,
        pub(super) src_h: f32,
        pub(super) width: u32,
        pub(super) height: u32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct LevelDims {
        pub(super) width: u32,
        pub(super) height: u32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct DenoiseParams {
        pub(super) threshold: f32,
        pub(super) width: u32,
        pub(super) height: u32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct ReassignParams {
        pub(super) plane: f32,
        pub(super) src_w: f32,
        pub(super) src_h: f32,
    }
}
use params::*;

/// One level's per-frame dispatch state.
struct Level {
    /// This level's coefficient grid (half its input).
    half_w: u32,
    half_h: u32,
    /// Its input's height (the horizontal pass's output height).
    full_h: u32,
    /// Its coefficient grid's size, for the merge's passes.
    dims: wgpu::Buffer,
    /// Each coefficient's winner per subband, and its runner-up (a
    /// placeholder without the consistency filter): `winner_index`.
    best: wgpu::Buffer,
    runner: wgpu::Buffer,
    /// The WTA's uniforms, rewritten each frame with its index.
    wta: WtaParams,
    wta_params: wgpu::Buffer,
    fd_h_bg: wgpu::BindGroup,
    fd_v_low_bg: wgpu::BindGroup,
    fd_v_high_bg: wgpu::BindGroup,
    wta_bg: wgpu::BindGroup,
}

struct AccumResources {
    width: u32,
    height: u32,
    padded_width: u32,
    padded_height: u32,
    /// The uploader's target, kept for the reassign pass.
    source: Source,
    scratch: Scratch,
    levels: Vec<Level>,
    /// The coarsest band's running (re, im, count) (`ll_accumulate`).
    ll_state: wgpu::Buffer,
    warp_buffer: wgpu::Buffer,
    ingest_bg: wgpu::BindGroup,
    ll_bg: wgpu::BindGroup,
    /// The live picture ([`WaveletStacker::live_planes`]), r32float, half
    /// the frame's size; pass 2 shows it as pass 1 ended.
    live: Texture,
    live_bg: wgpu::BindGroup,
}

impl AccumResources {
    /// What the merge's last step and the reassign pass need; the rest is
    /// dropped here (destructuring with `..` would keep it to the caller's
    /// scope end).
    fn into_merge_inputs(self) -> (u32, u32, Source, Texture, Texture) {
        (self.width, self.height, self.source, self.scratch.padded_luma, self.live)
    }
}

/// The forward transform's scratch, rg32float, reused by the inverse in
/// [`WaveletStacker::end_pass_one`]. Level `l` writes `ll[l % 2]`.
struct Scratch {
    padded_luma: Texture,
    temp_low: Texture,
    temp_high: Texture,
    ll: [Texture; 2],
    detail: [Texture; 3],
}

impl Scratch {
    /// The coarsest lowpass band a `levels`-level transform ends on.
    fn coarsest_ll(&self, levels: usize) -> &Texture {
        &self.ll[(levels - 1) % 2]
    }
}

struct ReassignResources {
    /// Which frames have been reassigned from.
    pass: SecondPass,
    width: u32,
    height: u32,
    source: Source,
    merged: Texture,
    /// Pass 1's picture as it ended, shown until the result lands.
    live: Texture,
    /// Each pixel's closest frame so far: (r, g, b, key), and its plane.
    pick: wgpu::Buffer,
    plane: wgpu::Buffer,
    warp_buffer: wgpu::Buffer,
    /// The reassign's uniforms, rewritten each frame with its plane.
    params: ReassignParams,
    params_buffer: wgpu::Buffer,
    bg: wgpu::BindGroup,
    /// The pass's latest submission, which `finalize` waits for.
    last: wgpu::SubmissionIndex,
}

/// Where a stack is.
///
/// Lifecycle: `Uninitialized` → `Accumulating` (the first
/// [`WaveletStacker::allocate`] whose textures all allocate) →
/// `Reassigning` ([`WaveletStacker::end_pass_one`], or `Failed` if it fails
/// after merging the winners in place) → `Finalized`
/// ([`WaveletStacker::finalize`], or `Failed` if it fails once the reassign
/// state is gone). An earlier frame's failure leaves the state as it was.
/// Changed only by the [`WaveletStacker`]'s methods; pass 2's frames are a
/// [`SecondPass`] inside `Reassigning`.
// One instance per stacker, so the size spread between variants is harmless.
#[allow(clippy::large_enum_variant)]
enum State {
    Uninitialized,
    Accumulating(AccumResources),
    Reassigning(ReassignResources),
    Finalized {
        /// The stacked image (rgba32float, full size).
        result: Texture,
        /// The frame map: each pixel's [`FramePlane`] value (r32float).
        planes: Texture,
    },
    /// A late failure of `end_pass_one` or `finalize`; nothing runs from here.
    Failed,
}

impl State {
    /// Whether the stack still takes first-pass frames.
    fn in_first_pass(&self) -> bool {
        matches!(self, State::Uninitialized | State::Accumulating(_))
    }
}

pub struct WaveletStacker {
    ctx: Arc<GpuContext>,
    /// Built once per device ([`GpuContext::programs`]).
    pipelines: Arc<WaveletPipelines>,
    config: WaveletConfig,
    info: StackInfo,
    state: State,
    intake: Intake,
}

impl WaveletStacker {
    pub fn new(ctx: Arc<GpuContext>, config: WaveletConfig, info: StackInfo) -> Self {
        Self {
            pipelines: ctx.programs(WaveletPipelines::new),
            intake: Intake::new(&ctx, info.total_frames),
            ctx,
            config,
            info,
            state: State::Uninitialized,
        }
    }

    /// Leave the next frame out, keeping its index.
    pub fn skip_frame(&mut self) -> Result<()> {
        self.intake.skip(self.state.in_first_pass())
    }

    /// Size the stacker before the first frame (see
    /// `PyramidStacker::allocate`). Does nothing once sized.
    pub fn allocate(&mut self, width: u32, height: u32, depth: FrameDepth) -> Result<()> {
        if !matches!(self.state, State::Uninitialized) {
            return Ok(());
        }
        self.config.validate()?;
        if width.min(height) < MIN_SIDE {
            return Err(CoreError::FrameTooSmall { width, height, min_side: MIN_SIDE });
        }
        self.intake.check_size(&self.ctx, width, height)?;
        self.check_state_fits(width, height)?;
        let accum = self.allocate_and_init(width, height, depth);
        self.state = State::Accumulating(stacker::allocated(&self.ctx, accum)?);
        debug!("wavelet stacker: accumulating");
        Ok(())
    }

    /// Refuse frames whose largest state buffer (the finest winners or the
    /// reassigned colors) exceeds one binding (`accum::check_accum_fits`).
    fn check_state_fits(&self, width: u32, height: u32) -> Result<()> {
        let (_, pw, ph) = plan(width, height);
        let px = |w: u32, h: u32| u64::from(w) * u64::from(h);
        let bytes = (px(pw / 2, ph / 2) * 3).max(px(width, height)) * ENTRY;
        check_binding_fits(&self.ctx, [width, height], bytes, "wavelet state buffer")
    }

    /// The peak GPU memory of a `width`×`height` stack (the accumulate
    /// phase): [`Self::bytes_per_padded_pixel`] over the padded frame and
    /// the source, as `stacker::gpu_bytes` counts.
    pub fn gpu_bytes(
        ctx: &GpuContext,
        config: &WaveletConfig,
        width: u32,
        height: u32,
        depth: FrameDepth,
    ) -> u64 {
        let (_, pw, ph) = plan(width, height);
        let held = stacker::pixels(pw, ph) * Self::bytes_per_padded_pixel(config);
        stacker::gpu_bytes(ctx, width, height, depth, held)
    }

    /// What the accumulate phase holds a padded pixel besides the source,
    /// Metal's 1/16 texture padding included: scratch, live picture and
    /// winners (twice with the runner-up).
    pub fn bytes_per_padded_pixel(config: &WaveletConfig) -> u64 {
        if config.runner_up() { 61 } else { 45 }
    }

    pub fn accumulate<'a>(
        &mut self,
        frame: impl Into<FrameInput<'a>>,
        alignment: Option<&FrameAlignment>,
    ) -> Result<()> {
        let frame = frame.into();
        self.allocate(frame.width(), frame.height(), frame.depth())?;
        let State::Accumulating(r) = &self.state else {
            return Err(CoreError::InvalidState("expected Accumulating state".into()));
        };
        let (index, prep) = self.intake.first_pass(&self.ctx, frame, &r.source)?;

        let warp = WarpParams::for_frame(alignment, self.info.is_linear);
        self.ctx.write_uniform_buffer(&r.warp_buffer, &warp);
        for level in &r.levels {
            let params = WtaParams { frame: index as f32, ..level.wta };
            self.ctx.write_uniform_buffer(&level.wta_params, &params);
        }

        let p = &self.pipelines;
        let mut encoder = self.ctx.encoder("wavelet_accumulate");
        prep.dispatch_to_rgba(&self.ctx, &mut encoder, r.source.texture());
        dispatch_compute(
            &mut encoder,
            &p.ingest,
            &r.ingest_bg,
            "ingest",
            r.padded_width,
            r.padded_height,
        );
        let last = r.levels.len() - 1;
        for (i, level) in r.levels.iter().enumerate() {
            let (hw, hh) = (level.half_w, level.half_h);
            dispatch_compute(
                &mut encoder,
                &p.forward,
                &level.fd_h_bg,
                "forward_h",
                hw,
                level.full_h,
            );
            dispatch_compute(&mut encoder, &p.forward, &level.fd_v_low_bg, "forward_v", hw, hh);
            dispatch_compute(&mut encoder, &p.forward, &level.fd_v_high_bg, "forward_v", hw, hh);
            dispatch_compute(&mut encoder, &p.wta, &level.wta_bg, "wta", hw, hh);
            if i == last {
                dispatch_compute(&mut encoder, &p.ll_accumulate, &r.ll_bg, "ll_accumulate", hw, hh);
            }
        }
        self.ctx.submit(encoder);
        self.intake.frames.push_stacked(index);
        Ok(())
    }

    /// Filter the winners and reconstruct the merged target for the
    /// reassign pass, releasing the winners and the forward scratch.
    pub fn end_pass_one(&mut self) -> Result<()> {
        if self.intake.frames.stacked_count() == 0 {
            return Err(CoreError::NoFrames);
        }
        let State::Accumulating(r) = &self.state else {
            return Err(CoreError::InvalidState(
                "end_pass_one() requires Accumulating state".into(),
            ));
        };
        // A frame's failure is reported before the merge.
        self.ctx.check()?;
        // Pass 2's picture, before the merge changes the winners.
        self.live_planes(r);
        let merged = self.encode_merge(r);

        let State::Accumulating(r) = std::mem::replace(&mut self.state, State::Uninitialized)
        else {
            unreachable!("checked above");
        };
        // The merge changed the winners in place (a retry would denoise
        // twice), so a failure from here on leaves the stacker failed.
        if let Err(e) = self.reassign_from(r, merged) {
            self.state = State::Failed;
            debug!("wavelet stacker: failed");
            return Err(e);
        }
        debug!("wavelet stacker: reassigning");
        info!("wavelet merge finished: {} frames", self.intake.frames.stacked_count());
        Ok(())
    }

    /// The merged target and reassign state from `r` and its `merged`
    /// submission, waiting for each drop to be freed before the next
    /// allocation.
    fn reassign_from(&mut self, r: AccumResources, merged: wgpu::SubmissionIndex) -> Result<()> {
        self.ctx.check()?;
        let (width, height, source, padded_luma, live) = r.into_merge_inputs();
        let (merged, extracted) = self.extract_merged(&padded_luma, width, height, merged)?;
        drop(padded_luma);
        self.begin_reassign(width, height, source, merged, live, extracted)?;
        Ok(self.ctx.check()?)
    }

    /// Filter and denoise the winners in place and invert the transform
    /// into the padded luma.
    fn encode_merge(&self, r: &AccumResources) -> wgpu::SubmissionIndex {
        let mut encoder = self.ctx.encoder("wavelet_merge");
        self.encode_filters(&mut encoder, r);
        self.encode_inverse(&mut encoder, r);
        self.ctx.submit(encoder)
    }

    /// The merged luma (r32float, frame size) and its submission, made once
    /// the winners are freed.
    fn extract_merged(
        &self,
        padded_luma: &Texture,
        width: u32,
        height: u32,
        merged: wgpu::SubmissionIndex,
    ) -> Result<(Texture, wgpu::SubmissionIndex)> {
        // wgpu frees a dropped buffer only once its work is done.
        self.ctx.wait_for(merged)?;
        // Copied from by `merged_target`'s callers.
        let usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let merged = self.ctx.create_texture(
            width,
            height,
            wgpu::TextureFormat::R32Float,
            usage,
            "wavelet_merged",
        );
        let mut encoder = self.ctx.encoder("wavelet_extract");
        dispatch_compute_textures(
            &mut encoder,
            &self.ctx.device,
            &self.pipelines.extract_real,
            "extract_real",
            &[padded_luma, &merged],
            &[],
            width,
            height,
        );
        let extracted = self.ctx.submit(encoder);
        Ok((merged, extracted))
    }

    fn encode_filters(&self, encoder: &mut wgpu::CommandEncoder, r: &AccumResources) {
        let p = &self.pipelines;
        let device = &self.ctx.device;

        // The detail scratch holds the neighbor filter's snapshot.
        let d = &r.scratch.detail;
        // Levels are independent.
        for level in r.levels.iter().rev() {
            let dims = &level.dims;
            if self.config.consistency >= 1 {
                dispatch_compute_textures(
                    encoder,
                    device,
                    &p.subband_consistency,
                    "subband_consistency",
                    &[],
                    &[
                        level.best.as_entire_binding(),
                        level.runner.as_entire_binding(),
                        dims.as_entire_binding(),
                    ],
                    level.half_w,
                    level.half_h,
                );
            }
            if self.config.consistency >= 2 {
                // From a snapshot: the filter writes what it reads.
                dispatch_compute_textures(
                    encoder,
                    device,
                    &p.snapshot_frames,
                    "snapshot_frames",
                    &[&d[0], &d[1], &d[2]],
                    &[level.best.as_entire_binding(), dims.as_entire_binding()],
                    level.half_w,
                    level.half_h,
                );
                dispatch_compute_textures(
                    encoder,
                    device,
                    &p.neighbor_consistency,
                    "neighbor_consistency",
                    &[&d[0], &d[1], &d[2]],
                    &[
                        level.best.as_entire_binding(),
                        level.runner.as_entire_binding(),
                        dims.as_entire_binding(),
                    ],
                    level.half_w,
                    level.half_h,
                );
            }
            // `denoise` is on a 0–255 scale, luma 0–1: energy scales by 1/255².
            if self.config.denoise > 0.0 {
                let params = self.ctx.create_uniform_buffer(
                    &DenoiseParams {
                        threshold: self.config.denoise / 65025.0,
                        width: level.half_w,
                        height: level.half_h,
                    },
                    "wavelet_denoise",
                );
                dispatch_compute_textures(
                    encoder,
                    device,
                    &p.denoise,
                    "denoise",
                    &[],
                    &[level.best.as_entire_binding(), params.as_entire_binding()],
                    level.half_w,
                    level.half_h,
                );
            }
        }
    }

    /// The inverse transform into the scratch's padded luma.
    fn encode_inverse(&self, encoder: &mut wgpu::CommandEncoder, r: &AccumResources) {
        let p = &self.pipelines;
        let device = &self.ctx.device;

        // Coarse to fine. Level L writes the padded luma at L = 0, else the
        // ping-pong its forward pass did not write.
        let s = &r.scratch;
        let n = r.levels.len();
        let out = |l: usize| -> &Texture { if l == 0 { &s.padded_luma } else { &s.ll[1 - l % 2] } };
        let coarsest_in = s.coarsest_ll(n);
        {
            let coarsest = &r.levels[n - 1];
            let (w, h) = (coarsest.half_w, coarsest.half_h);
            dispatch_compute_textures(
                encoder,
                device,
                &p.ll_normalize,
                "ll_normalize",
                &[coarsest_in],
                &[r.ll_state.as_entire_binding(), coarsest.dims.as_entire_binding()],
                w,
                h,
            );
        }
        for l in (0..n).rev() {
            let level = &r.levels[l];
            let input = if l == n - 1 { coarsest_in } else { out(l + 1) };
            dispatch_compute_textures(
                encoder,
                device,
                &p.inverse_vertical,
                "inverse_vertical",
                &[input, &s.temp_low, &s.temp_high],
                &[level.best.as_entire_binding(), level.dims.as_entire_binding()],
                level.half_w,
                level.half_h * 2,
            );
            dispatch_compute_textures(
                encoder,
                device,
                &p.inverse_horizontal,
                "inverse_horizontal",
                &[&s.temp_low, &s.temp_high, out(l)],
                &[level.dims.as_entire_binding()],
                level.half_w * 2,
                level.half_h * 2,
            );
        }
    }

    /// Set up the reassign pass on `merged` once `extracted` is done.
    /// Accumulating → Reassigning.
    fn begin_reassign(
        &mut self,
        width: u32,
        height: u32,
        source: Source,
        merged: Texture,
        live: Texture,
        extracted: wgpu::SubmissionIndex,
    ) -> Result<()> {
        let p = &self.pipelines;
        let device = &self.ctx.device;
        // Wait, or the reassign state (20 B/px) lands on top of the padded
        // luma, which wgpu frees only once its work is done.
        self.ctx.wait_for(extracted)?;

        let pixels = u64::from(width) * u64::from(height);
        let pick = storage_buffer(&self.ctx, pixels * ENTRY, "wavelet_reassign_pick");
        let plane = storage_buffer(&self.ctx, pixels * 4, "wavelet_reassign_plane");
        let warp_buffer =
            self.ctx.create_uniform_buffer(&WarpParams::identity(false), "wavelet_reassign_warp");
        let params = ReassignParams { plane: 0.0, src_w: width as f32, src_h: height as f32 };
        let params_buffer = self.ctx.create_uniform_buffer(&params, "wavelet_reassign_params");
        let bg = {
            let views = [source.texture(), &merged].map(|t| t.create_view(&Default::default()));
            create_bind_group(
                device,
                &p.reassign,
                0,
                "wavelet_reassign_bg",
                &[
                    tex(&views[0]),                    // cr_src
                    tex(&views[1]),                    // cr_merged
                    pick.as_entire_binding(),          // cr_pick
                    plane.as_entire_binding(),         // cr_plane
                    warp_buffer.as_entire_binding(),   // cr_warp
                    params_buffer.as_entire_binding(), // cr_params
                ],
            )
        };
        let dims =
            self.ctx.create_uniform_buffer(&LevelDims { width, height }, "wavelet_reassign_dims");
        let mut encoder = self.ctx.encoder("wavelet_reassign_init");
        dispatch_compute_textures(
            &mut encoder,
            device,
            &p.reassign_init,
            "reassign_init",
            &[],
            &[pick.as_entire_binding(), plane.as_entire_binding(), dims.as_entire_binding()],
            width,
            height,
        );
        let last = self.ctx.submit(encoder);
        self.state = State::Reassigning(ReassignResources {
            pass: SecondPass::new(&self.intake.frames),
            width,
            height,
            source,
            merged,
            live,
            pick,
            plane,
            warp_buffer,
            params,
            params_buffer,
            bg,
            last,
        });
        Ok(())
    }

    /// Reassign colors from one frame, in any order; `frame_index` is its
    /// accumulation index.
    pub fn blend_frame<'a>(
        &mut self,
        frame: impl Into<FrameInput<'a>>,
        frame_index: u32,
        alignment: Option<&FrameAlignment>,
    ) -> Result<()> {
        let frame = frame.into();
        let State::Reassigning(r) = &mut self.state else {
            return Err(CoreError::InvalidState("blend_frame() requires end_pass_one()".into()));
        };
        let prep = self.intake.second_pass(&self.ctx, frame, frame_index, &r.pass, &r.source)?;
        self.ctx.write_uniform_buffer(
            &r.warp_buffer,
            &WarpParams::for_frame(alignment, self.info.is_linear),
        );
        self.ctx.write_uniform_buffer(
            &r.params_buffer,
            &ReassignParams {
                plane: FramePlane::new(frame_index, self.info.total_frames).plane,
                ..r.params
            },
        );
        let mut encoder = self.ctx.encoder("wavelet_reassign");
        prep.dispatch_to_rgba(&self.ctx, &mut encoder, r.source.texture());
        dispatch_compute(
            &mut encoder,
            &self.pipelines.reassign,
            &r.bg,
            "reassign",
            r.width,
            r.height,
        );
        r.last = self.ctx.submit(encoder);
        r.pass.mark(frame_index);
        Ok(())
    }

    /// The result and frame map from the reassigned colors.
    pub fn finalize(&mut self) -> Result<()> {
        let State::Reassigning(r) = &self.state else {
            return Err(CoreError::InvalidState("finalize() requires the reassign pass".into()));
        };
        // A frame not reassigned from leaves its pixels without a color.
        r.pass.check_complete(&self.intake.frames)?;
        // A frame's failure is reported before the finish.
        self.ctx.check()?;
        let State::Reassigning(r) = std::mem::replace(&mut self.state, State::Uninitialized) else {
            unreachable!("checked above");
        };
        // Free the pass's inputs before the result is made, once the last
        // reassign is done with them.
        let ReassignResources {
            width, height, merged, pick, plane, source, live, bg, last, ..
        } = r;
        drop((source, merged, live, bg));
        self.intake.finish();
        if let Err(e) = self.ctx.wait_for(last) {
            self.state = State::Failed;
            debug!("wavelet stacker: failed");
            return Err(e.into());
        }
        let usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST;
        let result = self.ctx.create_texture(
            width,
            height,
            wgpu::TextureFormat::Rgba32Float,
            usage,
            "wavelet_result",
        );
        let planes = self.ctx.create_texture(
            width,
            height,
            wgpu::TextureFormat::R32Float,
            usage,
            "wavelet_frame_map",
        );
        let mut encoder = self.ctx.encoder("wavelet_finish");
        dispatch_compute_textures(
            &mut encoder,
            &self.ctx.device,
            &self.pipelines.finish,
            "finish",
            &[&result, &planes],
            &[pick.as_entire_binding(), plane.as_entire_binding()],
            width,
            height,
        );
        self.ctx.submit(encoder);
        // The reassign state is gone, so a failure leaves it failed.
        if let Err(e) = self.ctx.check() {
            self.state = State::Failed;
            debug!("wavelet stacker: failed");
            return Err(e.into());
        }
        self.state = State::Finalized { result, planes };
        debug!("wavelet stacker: finalized");
        Ok(())
    }

    pub fn result_texture(&self) -> Option<&Texture> {
        match &self.state {
            State::Finalized { result, .. } => Some(result),
            _ => None,
        }
    }

    /// The picture while the stack runs (8-bit, in `format`, its longest
    /// side capped at `max_side`): pass 1's estimate of the frame map
    /// ([`Self::live_planes`]), then the frame map once finalized. Valid
    /// from the first accumulated frame.
    pub fn render_live(&self, max_side: u32, format: wgpu::TextureFormat) -> Result<Texture> {
        let render = |planes: &Texture| {
            let (live, _, _) =
                self.pipelines.live_map.render(&self.ctx, planes, Some(max_side), format)?;
            Ok(live)
        };
        match &self.state {
            State::Accumulating(r) if self.intake.frames.stacked_count() > 0 => {
                self.live_planes(r);
                render(&r.live)
            }
            State::Reassigning(r) => render(&r.live),
            State::Finalized { planes, .. } => render(planes),
            _ => Err(CoreError::NoFrames),
        }
    }

    /// Write the finest level's strongest winners as planes into `r.live`.
    fn live_planes(&self, r: &AccumResources) {
        let mut encoder = self.ctx.encoder("wavelet_live_planes");
        dispatch_compute(
            &mut encoder,
            &self.pipelines.live_planes,
            &r.live_bg,
            "live_planes",
            r.live.width(),
            r.live.height(),
        );
        self.ctx.submit(encoder);
    }

    /// The merged gray target (r32float, frame size, luma 0..1), from
    /// [`Self::end_pass_one`] until [`Self::finalize`].
    pub fn merged_target(&self) -> Option<&Texture> {
        match &self.state {
            State::Reassigning(r) => Some(&r.merged),
            _ => None,
        }
    }

    /// The frame map ([`FramePlane`] values, r32float); `None` before
    /// [`Self::finalize`].
    pub fn frame_map(&self) -> Option<&Texture> {
        match &self.state {
            State::Finalized { planes, .. } => Some(planes),
            _ => None,
        }
    }

    fn allocate_and_init(&self, width: u32, height: u32, depth: FrameDepth) -> AccumResources {
        let (num_levels, pw, ph) = plan(width, height);
        info!("allocating wavelet: {width}x{height} (padded {pw}x{ph}), {num_levels} levels");

        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let rg = |w, h, label: &str| {
            self.ctx.create_texture(w, h, wgpu::TextureFormat::Rg32Float, usage, label)
        };

        let source = Source::new(&self.ctx, width, height, depth, "wavelet_source");
        let scratch = Scratch {
            padded_luma: rg(pw, ph, "wavelet_padded_luma"),
            temp_low: rg(pw / 2, ph, "wavelet_temp_low"),
            temp_high: rg(pw / 2, ph, "wavelet_temp_high"),
            ll: [rg(pw / 2, ph / 2, "wavelet_ll_0"), rg(pw / 2, ph / 2, "wavelet_ll_1")],
            detail: [
                rg(pw / 2, ph / 2, "wavelet_lh"),
                rg(pw / 2, ph / 2, "wavelet_hl"),
                rg(pw / 2, ph / 2, "wavelet_hh"),
            ],
        };
        let (ll_w, ll_h) = (pw >> num_levels, ph >> num_levels);
        let ll_state = storage_buffer(
            &self.ctx,
            u64::from(ll_w) * u64::from(ll_h) * ENTRY,
            "wavelet_ll_state",
        );

        let device = &self.ctx.device;
        let p = &self.pipelines;
        let view = |t: &Texture| t.create_view(&Default::default());

        let warp_buffer =
            self.ctx.create_uniform_buffer(&WarpParams::identity(false), "wavelet_warp");
        let ingest_params =
            self.ctx.create_uniform_buffer(&LevelDims { width, height }, "wavelet_ingest");
        let ingest_bg = {
            let (s, d) = (view(source.texture()), view(&scratch.padded_luma));
            create_bind_group(
                device,
                &p.ingest,
                0,
                "wavelet_ingest_bg",
                &[
                    tex(&s),
                    tex(&d),
                    warp_buffer.as_entire_binding(),
                    ingest_params.as_entire_binding(),
                ],
            )
        };

        let mut levels = Vec::with_capacity(num_levels);
        let (mut cw, mut ch) = (pw, ph);
        for level in 0..num_levels {
            levels.push(self.level(&scratch, &warp_buffer, (width, height), level, cw, ch));
            (cw, ch) = (cw / 2, ch / 2);
        }

        let ll_params = self.ctx.create_uniform_buffer(
            &LlParams {
                scale: (1u32 << num_levels) as f32,
                src_w: width as f32,
                src_h: height as f32,
                width: ll_w,
                height: ll_h,
            },
            "wavelet_ll_params",
        );
        let ll_bg = {
            let c = view(scratch.coarsest_ll(num_levels));
            create_bind_group(
                device,
                &p.ll_accumulate,
                0,
                "wavelet_ll_bg",
                &[
                    tex(&c),
                    ll_state.as_entire_binding(),
                    ll_params.as_entire_binding(),
                    warp_buffer.as_entire_binding(),
                ],
            )
        };

        let live = self.ctx.create_texture(
            width.div_ceil(2),
            height.div_ceil(2),
            wgpu::TextureFormat::R32Float,
            usage,
            "wavelet_live",
        );
        let live_params = self.ctx.create_uniform_buffer(
            &LiveParams {
                width: levels[0].half_w,
                step: FramePlane::new(0, self.info.total_frames).step,
            },
            "wavelet_live_params",
        );
        let live_bg = {
            let l = view(&live);
            create_bind_group(
                device,
                &p.live_planes,
                0,
                "wavelet_live_bg",
                &[tex(&l), levels[0].best.as_entire_binding(), live_params.as_entire_binding()],
            )
        };

        // wgpu zero-initializes: every best starts at 0 energy.
        AccumResources {
            width,
            height,
            padded_width: pw,
            padded_height: ph,
            source,
            scratch,
            levels,
            ll_state,
            warp_buffer,
            ingest_bg,
            ll_bg,
            live,
            live_bg,
        }
    }

    /// Level `level`'s running bests and bind groups (forward passes and
    /// WTA), its input `cw`×`ch`.
    fn level(
        &self,
        t: &Scratch,
        warp: &wgpu::Buffer,
        (src_w, src_h): (u32, u32),
        level: usize,
        cw: u32,
        ch: u32,
    ) -> Level {
        let device = &self.ctx.device;
        let p = &self.pipelines;
        let view = |texture: &Texture| texture.create_view(&Default::default());
        let (hw, hh) = (cw / 2, ch / 2);
        let entries = u64::from(hw) * u64::from(hh) * 3;
        let best = storage_buffer(&self.ctx, entries * ENTRY, &format!("wavelet_best_{level}"));
        // Never read without the consistency filter: one entry binds.
        let runner_entries = if self.config.runner_up() { entries } else { 1 };
        let runner =
            storage_buffer(&self.ctx, runner_entries * ENTRY, &format!("wavelet_runner_{level}"));
        let dims = |src_w, src_h, dst_w, dst_h, direction| {
            self.ctx.create_uniform_buffer(
                &WaveletDims {
                    src_width: src_w,
                    src_height: src_h,
                    dst_width: dst_w,
                    dst_height: dst_h,
                    direction,
                },
                "wavelet_dims",
            )
        };
        let h_dims = dims(cw, ch, hw, ch, 0);
        let v_dims = dims(hw, ch, hw, hh, 1);
        let input = if level == 0 { &t.padded_luma } else { &t.ll[(level - 1) % 2] };
        let forward_bg = |src: &Texture, lo: &Texture, hi: &Texture, d: &wgpu::Buffer| {
            let (s, l, h) = (view(src), view(lo), view(hi));
            create_bind_group(
                device,
                &p.forward,
                0,
                "wavelet_forward_bg",
                &[tex(&s), tex(&l), tex(&h), d.as_entire_binding()],
            )
        };
        let fd_h_bg = forward_bg(input, &t.temp_low, &t.temp_high, &h_dims);
        let fd_v_low_bg = forward_bg(&t.temp_low, &t.ll[level % 2], &t.detail[0], &v_dims);
        let fd_v_high_bg = forward_bg(&t.temp_high, &t.detail[1], &t.detail[2], &v_dims);

        let wta = WtaParams {
            width: hw,
            height: hh,
            frame: 0.0,
            runner_up: self.config.runner_up() as u32,
            scale: (2u32 << level) as f32,
            src_w: src_w as f32,
            src_h: src_h as f32,
        };
        let wta_params = self.ctx.create_uniform_buffer(&wta, "wavelet_wta_params");
        let level_dims = self
            .ctx
            .create_uniform_buffer(&LevelDims { width: hw, height: hh }, "wavelet_level_dims");
        let wta_bg = {
            let views = [&t.detail[0], &t.detail[1], &t.detail[2]].map(view);
            create_bind_group(
                device,
                &p.wta,
                0,
                "wavelet_wta_bg",
                &[
                    tex(&views[0]),                 // wta_lh
                    tex(&views[1]),                 // wta_hl
                    tex(&views[2]),                 // wta_hh
                    best.as_entire_binding(),       // wta_best
                    runner.as_entire_binding(),     // wta_runner
                    wta_params.as_entire_binding(), // wta_params
                    warp.as_entire_binding(),       // wta_warp
                ],
            )
        };
        Level {
            half_w: hw,
            half_h: hh,
            full_h: ch,
            dims: level_dims,
            best,
            runner,
            wta,
            wta_params,
            fd_h_bg,
            fd_v_low_bg,
            fd_v_high_bg,
            wta_bg,
        }
    }
}

/// A storage buffer of `bytes`, zeroed, nothing but bound.
fn storage_buffer(ctx: &GpuContext, bytes: u64, label: &str) -> wgpu::Buffer {
    ctx.create_storage_buffer(label, bytes, wgpu::BufferUsages::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure after the merge changed the winners in place leaves the
    /// stacker failed: it refuses more frames and a second `end_pass_one`.
    #[test]
    fn end_pass_one_failing_late_refuses_further_use() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let info = StackInfo { total_frames: 2, is_linear: false };
        let mut stacker = WaveletStacker::new(ctx, WaveletConfig::default(), info);
        let frame = image::DynamicImage::new_rgb8(64, 48);
        stacker.accumulate(&frame, None).unwrap();
        // With the frame's work done and the inverse's scratch destroyed,
        // the merge fails after changing the winners.
        let State::Accumulating(r) = &stacker.state else { unreachable!() };
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        r.scratch.temp_low.destroy();
        assert!(stacker.end_pass_one().is_err());
        assert!(matches!(stacker.state, State::Failed));
        assert!(matches!(stacker.end_pass_one(), Err(CoreError::InvalidState(_))));
        assert!(matches!(stacker.accumulate(&frame, None), Err(CoreError::InvalidState(_))));
        assert!(stacker.blend_frame(&frame, 0, None).is_err());
        assert!(stacker.render_live(64, wgpu::TextureFormat::Rgba8Unorm).is_err());
    }

    /// A stacker reassigning two blank frames, the first done; the frame.
    fn reassigning() -> Option<(WaveletStacker, image::DynamicImage)> {
        let ctx = GpuContext::new_or_skip()?;
        let info = StackInfo { total_frames: 2, is_linear: false };
        let mut stacker = WaveletStacker::new(ctx, WaveletConfig::default(), info);
        let frame = image::DynamicImage::new_rgb8(64, 48);
        stacker.accumulate(&frame, None).unwrap();
        stacker.accumulate(&frame, None).unwrap();
        stacker.end_pass_one().unwrap();
        stacker.blend_frame(&frame, 0, None).unwrap();
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        Some((stacker, frame))
    }

    /// An earlier frame's failure, reported by `finalize`, leaves the
    /// stacker reassigning: nothing is finished.
    #[test]
    fn an_earlier_frame_s_failure_leaves_it_reassigning() {
        let Some((mut stacker, frame)) = reassigning() else { return };
        let State::Reassigning(r) = &stacker.state else { unreachable!() };
        r.source.texture().destroy();
        // The frame's work fails on the GPU, unchecked until `finalize`.
        stacker.blend_frame(&frame, 1, None).unwrap();
        assert!(stacker.finalize().is_err());
        assert!(matches!(stacker.state, State::Reassigning(_)));
    }

    /// A failure once the reassign state is gone leaves the stacker
    /// failed: it refuses a second `finalize` and more frames, and has no
    /// result.
    #[test]
    fn finalize_failing_late_refuses_further_use() {
        let Some((mut stacker, frame)) = reassigning() else { return };
        stacker.blend_frame(&frame, 1, None).unwrap();
        let State::Reassigning(r) = &stacker.state else { unreachable!() };
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        r.pick.destroy();
        assert!(stacker.finalize().is_err());
        assert!(matches!(stacker.state, State::Failed));
        assert!(matches!(stacker.finalize(), Err(CoreError::InvalidState(_))));
        assert!(matches!(stacker.blend_frame(&frame, 1, None), Err(CoreError::InvalidState(_))));
        assert!(stacker.result_texture().is_none());
        assert!(stacker.render_live(64, wgpu::TextureFormat::Rgba8Unorm).is_err());
    }

    /// The merged target (row-major luma) after pass 1 of `frames`, with no
    /// filter or denoise.
    fn merged_target(
        ctx: &Arc<GpuContext>,
        frames: &[image::DynamicImage],
        alignments: &[Option<FrameAlignment>],
    ) -> Vec<f32> {
        let info = StackInfo { total_frames: frames.len() as u32, is_linear: false };
        let config = WaveletConfig { denoise: 0.0, consistency: 0 };
        let mut stacker = WaveletStacker::new(ctx.clone(), config, info);
        for (frame, alignment) in frames.iter().zip(alignments) {
            stacker.accumulate(frame, alignment.as_ref()).unwrap();
        }
        stacker.end_pass_one().unwrap();
        let merged = stacker.merged_target().unwrap();
        let mut target = Vec::new();
        let size = [merged.width(), merged.height()];
        deptheos_gpu::gpu_utils::download_texture_with(ctx, merged, [0, 0], size, |row| {
            target.extend_from_slice(bytemuck::cast_slice(row))
        })
        .unwrap();
        target
    }

    /// With one frame nothing is merged, so the inverse gives back the
    /// frame's luma, its edges included: the forward transform's boundary
    /// and the inverse's agree.
    #[test]
    fn the_transform_reconstructs_a_frame() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        // Padded on both axes; detail over a ramp, so every edge differs.
        let (w, h) = (509, 381);
        let frame = image::RgbImage::from_fn(w, h, |x, y| {
            let v = (x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77)) >> 26;
            image::Rgb([(20 + 160 * (x + y) / (w + h) + v) as u8; 3])
        });
        let target = merged_target(&ctx, &[image::DynamicImage::ImageRgb8(frame.clone())], &[None]);
        let worst = frame
            .pixels()
            .zip(&target)
            .map(|(p, t)| (p[0] as f32 / 255.0 - t).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-4, "the target is off the frame by up to {worst}");
    }

    /// A flat `side`² gray frame of `value`.
    fn flat(side: u32, value: u8) -> image::DynamicImage {
        let frame = image::RgbImage::from_pixel(side, side, image::Rgb([value; 3]));
        image::DynamicImage::ImageRgb8(frame)
    }

    /// Frames without data at a coefficient count in the coarsest band only
    /// where no frame has data, however many they are.
    #[test]
    fn frames_without_data_leave_the_coarsest_band_alone() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        const SIDE: u32 = 256;
        // Frame 0 at 0.2 on the reference; 99 frames at 0.9 sampled from
        // far off themselves, so none has data anywhere on it.
        let away = FrameAlignment::from_transform(nalgebra::Affine2::from_matrix_unchecked(
            nalgebra::Matrix3::new(1.0, 0.0, 4.0 * SIDE as f32, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
        ));
        let mut frames = vec![flat(SIDE, 51)];
        let mut alignments = vec![None];
        frames.extend((0..99).map(|_| flat(SIDE, 230)));
        alignments.extend((0..99).map(|_| Some(away)));
        let target = merged_target(&ctx, &frames, &alignments);
        let worst = target.iter().map(|v| (v - 0.2).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "the target is off frame 0's 0.2 by up to {worst}");
        // With no frame on the reference, the frames are averaged alike.
        let target = merged_target(&ctx, &[flat(SIDE, 230), flat(SIDE, 50)], &[Some(away); 2]);
        let worst = target.iter().map(|v| (v - 140.0 / 255.0).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "the smear's average is off by up to {worst}");
    }

    #[test]
    fn the_shader_weighs_luma_as_the_aligner_does() {
        let [r, g, b] = deptheos_aligner::REC709_LUMA;
        let line = format!("const LUMA: vec3f = vec3f({r}, {g}, {b});");
        assert!(include_str!("wavelet.wgsl").contains(&line), "wavelet.wgsl should have {line}");
    }

    #[test]
    fn plan_pads_to_the_coarsest_block() {
        assert_eq!(plan(512, 512), (6, 512, 512));
        assert_eq!(plan(509, 381), (6, 512, 384));
        assert_eq!(plan(6000, 4000), (9, 6144, 4096));
        assert_eq!(plan(20, 9), (1, 20, 10));
    }
}
