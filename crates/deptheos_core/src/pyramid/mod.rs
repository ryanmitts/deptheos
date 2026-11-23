//! Pyramid focus stacking (Burt & Adelson 1983 Laplacian pyramid, soft WTA).
//!
//! One `accumulate` per frame, `collapse` for the stack so far after any
//! of them, then `finalize` (the stages are `State`'s).

pub(crate) mod pipeline;

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{
    create_bind_group, dispatch_compute, dispatch_compute_with, encode_clear, tex,
};
use pipeline::PyramidPipelines;
use std::sync::Arc;
use tracing::{debug, info};
use wgpu::Texture;

use crate::StackInfo;
use crate::accum::{FinishBuffers, check_accum_fits, create_accum, level_sizes};
use crate::error::{CoreError, Result};
use crate::focus::{BlurParams, blur_half_width, iir_blur_sigma};
use crate::plane::FramePlane;
use crate::scalar::ScalarMap;
use crate::stacker::{self, Intake};
use crate::transform::{FrameAlignment, WarpParams};
use crate::upload::{FrameDepth, FrameInput, Source};

// Wrapped in a module so the inner allow covers the layout-check fns the
// encase derive generates.
mod params {
    #![allow(dead_code)]

    /// Per-level constants: the soft-WTA exponent and the margin mask's inputs.
    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct LevelParams {
        pub(super) exponent: f32,
        /// `2^level`: full-resolution pixels per pixel of this level.
        pub(super) level_scale: f32,
        /// The full-resolution frame size, for the margin mask.
        pub(super) src_w: f32,
        pub(super) src_h: f32,
        /// Where the level reads the frame's focus prior ([`prior_source`]).
        pub(super) prior_source: u32,
    }

    /// The frame being accumulated: its plane in the frame map.
    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct FrameParams {
        pub(super) plane: f32,
    }
}
use params::{FrameParams, LevelParams};

#[derive(Clone, Copy, Debug)]
pub struct PyramidConfig {
    /// Smoothness of the focus blend, 1.0 (sharp) to 10.0 (smooth): the WTA
    /// exponent schedule across levels.
    pub smoothness: f32,
}

/// Inclusive `smoothness` range, enforced: past 13 the WTA exponent
/// (`13 − s`) goes negative and the result NaN.
pub const SMOOTHNESS_RANGE: (f32, f32) = (1.0, 10.0);
/// The app's slider step for `smoothness`.
pub const SMOOTHNESS_STEP: f32 = 0.1;

impl PyramidConfig {
    pub const DEFAULT_SMOOTHNESS: f32 = 4.0;

    /// Reject values that would produce a degenerate or NaN result.
    pub fn validate(&self) -> Result<()> {
        crate::error::Setting::Smoothness.check(self.smoothness, SMOOTHNESS_RANGE)
    }
}

impl Default for PyramidConfig {
    fn default() -> Self {
        Self { smoothness: Self::DEFAULT_SMOOTHNESS }
    }
}

/// The level the winner record is decided at (the shader's `focus_wta`):
/// level 1 holds 2–4 px detail, where level 0's noise would decide flat
/// regions at random. Clamped below the coarsest level, which has no metric.
const RECORD_LEVEL: usize = 1;

/// The focus prior's exponent: every level's focus is multiplied by
/// `prior^PRIOR_EXPONENT`, the frame's fine-scale sharpness at
/// [`RECORD_LEVEL`], so coarse levels don't pick a blurred bright edge and
/// leave a glow. 0 is the plain per-level pick; large values tend to one
/// frame per area (ADR-120). The shader's `PRIOR_EXPONENT` matches.
pub(crate) const PRIOR_EXPONENT: f32 = 0.75;

/// The shader's `PRIOR_FROM_*`: the recording level computes the prior,
/// coarser levels read their level of it, and level 0 upsamples it.
fn prior_source(level: usize, record_level: usize) -> u32 {
    match level.cmp(&record_level) {
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Less => {
            debug_assert_eq!(record_level, level + 1, "the prior upsamples by one level only");
            2
        }
    }
}

/// Levels go down until the shorter side is about this many pixels.
pub(crate) const LEVEL_FLOOR_PX: f32 = 16.0;
/// At least two levels: the focus map lives on every level but the
/// coarsest, so a single level would have none.
pub(crate) const MIN_LEVELS: usize = 2;
/// The focus blur half-width the shader's tile holds (`FW_HALO`).
const FOCUS_HALO: u32 = 3;
/// The best-focus start value, below every key (masked keys are in [-1, 0)).
const FOCUS_SENTINEL: f32 = -2.0;
/// The finest level's WTA exponent is `EXPONENT_BASE - smoothness`.
pub(crate) const EXPONENT_BASE: f32 = 13.0;
/// The coarsest level's exponent is the finest's times this.
pub(crate) const COARSEST_EXPONENT_RATIO: f32 = 0.5;
/// Neither end of the exponent schedule goes below this.
const MIN_EXPONENT: f32 = 1.0;
/// The focus blur's IIR radius is `(trunc(smoothness) - 1) / BLUR_RADIUS_DIVISOR`.
const BLUR_RADIUS_DIVISOR: f32 = 5.0;

/// Level count for a frame size, down to about [`LEVEL_FLOOR_PX`]: the
/// coarsest level is a plain average, so a higher floor turns exposure drift
/// into low-frequency haze over more of the image. At least [`MIN_LEVELS`].
fn num_levels(width: u32, height: u32) -> usize {
    let min_dim = width.min(height);
    (((min_dim as f32 / LEVEL_FLOOR_PX).log2().floor() as i32 + 1).max(MIN_LEVELS as i32)) as usize
}

/// Gaussian blur over the focus metric, the same σ at every level: radius
/// (trunc(smoothness) − 1)/5 through `focus::iir_blur_sigma` (σ ≈ 0.71–0.82).
/// Below smoothness 2, half-width 0, the shader's "no blur".
pub(crate) fn focus_blur(smoothness: f32) -> BlurParams {
    let blur_s = (smoothness.trunc() - 1.0).max(0.0);
    if blur_s > 0.0 {
        let sigma = iir_blur_sigma(blur_s / BLUR_RADIUS_DIVISOR);
        BlurParams { sigma, half_width: blur_half_width(sigma) }
    } else {
        BlurParams { sigma: 0.0, half_width: 0 }
    }
}

/// WTA exponent schedule: smoothness 1..10 → finest 12..3, coarsest half
/// that (at least 1), linear between. `num_levels` ≥ 2.
pub(crate) fn exponent_schedule(smoothness: f32, num_levels: usize) -> Vec<f32> {
    let finest_exponent = (EXPONENT_BASE - smoothness).max(MIN_EXPONENT);
    let coarsest_exponent = (finest_exponent * COARSEST_EXPONENT_RATIO).max(MIN_EXPONENT);
    (0..num_levels)
        .map(|level| {
            let t = level as f32 / (num_levels - 1) as f32; // num_levels ≥ 2
            finest_exponent + (coarsest_exponent - finest_exponent) * t
        })
        .collect()
}

/// GPU resources for the accumulation phase. Buffers only a pass reads
/// (the soft-WTA sums, focus bests, the prior pyramid) live only in the bind
/// groups, which keep them alive.
struct AccumResources {
    // Uploader target, warped into scratch_gaussian[0].
    source: Source,
    /// The Gaussian pyramid, finest first; the collapse writes the result
    /// to level 0.
    scratch_gaussian: Vec<Texture>,
    /// The winner record so far, as the winning frame's plane (r32float,
    /// full resolution), decided at [`RECORD_LEVEL`].
    winners: Texture,
    /// The level the record is decided at.
    record_level: usize,
    /// The focus prior's downsamples from the recording level, one a level.
    prior_downsample_bgs: Vec<wgpu::BindGroup>,

    // Pre-created bind groups (these texture pairings never change)
    warp_bg: wgpu::BindGroup,
    warp_params_buffer: wgpu::Buffer,  // rewritten each frame
    frame_params_buffer: wgpu::Buffer, // rewritten each frame
    downsample_bgs: Vec<wgpu::BindGroup>,
    focus_wta_bgs: Vec<wgpu::BindGroup>,
    accumulate_sum_bg: wgpu::BindGroup,
    normalize_coarsest_bg: wgpu::BindGroup,
    laplacian_add_bgs: Vec<wgpu::BindGroup>,
}

impl AccumResources {
    /// The frame size, level 0's.
    fn size(&self) -> (u32, u32) {
        let base = &self.scratch_gaussian[0];
        (base.width(), base.height())
    }

    /// The pyramid's levels, the coarsest last.
    fn levels(&self) -> usize {
        self.scratch_gaussian.len()
    }
}

/// Where a stack is.
///
/// Lifecycle: `Uninitialized` → `Accumulating` (the first
/// [`PyramidStacker::allocate`] whose textures all allocate; it stays
/// `Uninitialized` if one fails) → `Finalized`
/// ([`PyramidStacker::finalize`]). Changed only by the [`PyramidStacker`]'s
/// methods.
// One instance per stacker, so the size spread between variants is harmless.
#[allow(clippy::large_enum_variant)]
enum State {
    Uninitialized,
    Accumulating(AccumResources),
    Finalized {
        /// The stacked image (rgba32float, full size).
        result: Texture,
        /// The frame map: each pixel the chosen frame's [`FramePlane`]
        /// value (r32float, full size).
        winners: Texture,
    },
}

impl State {
    /// Whether the stack still takes frames, and so can leave one out.
    fn in_first_pass(&self) -> bool {
        matches!(self, State::Uninitialized | State::Accumulating(_))
    }
}

pub struct PyramidStacker {
    ctx: Arc<GpuContext>,
    /// Built once per device ([`GpuContext::programs`]).
    pipelines: Arc<PyramidPipelines>,
    config: PyramidConfig,
    info: StackInfo,
    state: State,
    intake: Intake,
}

/// The warp ingest's bind group: `source` resampled into the pyramid's
/// base, `base`.
fn warp_bind_group(
    ctx: &GpuContext,
    pipelines: &PyramidPipelines,
    source: &Source,
    base: &Texture,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let src = source.texture().create_view(&Default::default());
    let dst = base.create_view(&Default::default());
    create_bind_group(
        &ctx.device,
        &pipelines.warp_rgba,
        0,
        "pyramid_warp_bg",
        &[tex(&src), tex(&dst), params.as_entire_binding()],
    )
}

impl PyramidStacker {
    pub fn new(ctx: Arc<GpuContext>, config: PyramidConfig, info: StackInfo) -> Self {
        Self {
            pipelines: ctx.programs(PyramidPipelines::new),
            intake: Intake::new(&ctx, info.total_frames),
            ctx,
            config,
            info,
            state: State::Uninitialized,
        }
    }

    /// Leave the next frame out, keeping its index so later frames keep theirs.
    pub fn skip_frame(&mut self) -> Result<()> {
        self.intake.skip(self.state.in_first_pass())
    }

    /// Size the pyramid ahead of the first [`Self::accumulate`], so a caller
    /// can allocate while the first frame decodes. Does nothing once sized.
    pub fn allocate(&mut self, width: u32, height: u32, depth: FrameDepth) -> Result<()> {
        if !matches!(self.state, State::Uninitialized) {
            return Ok(());
        }
        self.config.validate()?;
        self.intake.check_size(&self.ctx, width, height)?;
        check_accum_fits(&self.ctx, width, height)?;
        let accum = self.allocate_and_init(width, height, depth);
        self.state = State::Accumulating(stacker::allocated(&self.ctx, accum)?);
        debug!("pyramid stacker: accumulating");
        Ok(())
    }

    /// The peak GPU memory of a `width`×`height` stack of `depth` frames:
    /// [`Self::BYTES_PER_PIXEL`] and the source texture, as
    /// `stacker::gpu_bytes` counts.
    pub fn gpu_bytes(ctx: &GpuContext, width: u32, height: u32, depth: FrameDepth) -> u64 {
        let held = stacker::pixels(width, height) * Self::BYTES_PER_PIXEL;
        stacker::gpu_bytes(ctx, width, height, depth, held)
    }

    /// What a stack holds a pixel besides its source, Metal's 1/16 texture
    /// padding included: each level's Gaussian scratch and accumulator, the
    /// focus bests and priors, and the winner record.
    pub const BYTES_PER_PIXEL: u64 = 56;

    /// Accumulate one frame with an optional alignment. The first call sizes
    /// the pyramid (unless [`Self::allocate`] did); later frames must match.
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

        // The warp always runs: it is also the ingest that sRGB-encodes.
        let warp = WarpParams::for_frame(alignment, self.info.is_linear);
        self.ctx.write_uniform_buffer(&r.warp_params_buffer, &warp);
        let plane = FramePlane::new(index, self.info.total_frames).plane;
        self.ctx.write_uniform_buffer(&r.frame_params_buffer, &FrameParams { plane });

        let mut encoder = self.ctx.encoder("pyramid_accumulate");

        // Unpack the upload into source, warp into the pyramid base.
        prep.dispatch_to_rgba(&self.ctx, &mut encoder, r.source.texture());
        let (width, height) = r.size();
        dispatch_compute(
            &mut encoder,
            &self.pipelines.warp_rgba,
            &r.warp_bg,
            "warp_ingest",
            width,
            height,
        );

        let last = r.levels() - 1;
        for level in 0..last {
            let dst_size = r.scratch_gaussian[level + 1].size();
            // 8×8 workgroups (`downsample` in the shader).
            dispatch_compute_with(
                &mut encoder,
                &self.pipelines.downsample,
                &[&r.downsample_bgs[level]],
                "downsample",
                dst_size.width,
                dst_size.height,
                8,
            );
        }

        // The recording level runs first: it writes the focus prior, which
        // is pyramided down before the other levels read it.
        let focus_wta = |encoder: &mut wgpu::CommandEncoder, level: usize| {
            let size = r.scratch_gaussian[level].size();
            dispatch_compute(
                encoder,
                &self.pipelines.focus_wta,
                &r.focus_wta_bgs[level],
                "focus_wta",
                size.width,
                size.height,
            );
        };
        focus_wta(&mut encoder, r.record_level);
        // Prior level `l` is the size of the pyramid's level `l`.
        let prior_levels = &r.scratch_gaussian[r.record_level + 1..];
        for (dst, bg) in prior_levels.iter().zip(&r.prior_downsample_bgs) {
            let size = dst.size();
            dispatch_compute(
                &mut encoder,
                &self.pipelines.prior_downsample,
                bg,
                "prior_downsample",
                size.width,
                size.height,
            );
        }
        for level in (0..last).filter(|&l| l != r.record_level) {
            focus_wta(&mut encoder, level);
        }

        // Coarsest level: mask-weighted running sum, averaged at finalize.
        let coarse_size = r.scratch_gaussian[last].size();
        dispatch_compute(
            &mut encoder,
            &self.pipelines.accumulate_sum,
            &r.accumulate_sum_bg,
            "accumulate_sum",
            coarse_size.width,
            coarse_size.height,
        );

        self.ctx.submit(encoder);
        self.intake.frames.push_stacked(index);
        Ok(())
    }

    /// The stack so far, collapsed into the per-frame scratch, leaving the
    /// stacker accumulating. The texture (rgba32float, full size) is
    /// overwritten by the next [`Self::accumulate`], so read it back first.
    /// A collapse after `k` frames equals a `k`-frame stack of them, up to
    /// float rounding. [`CoreError::NoFrames`] before any frame was stacked.
    pub fn collapse(&self) -> Result<&Texture> {
        if self.intake.frames.stacked_count() == 0 {
            return Err(CoreError::NoFrames);
        }
        let State::Accumulating(accum) = &self.state else {
            return Err(CoreError::InvalidState("collapse() requires Accumulating state".into()));
        };

        let last = accum.levels() - 1;
        let mut encoder = self.ctx.encoder("pyramid_collapse");

        // Coarsest level: mask-weighted average into scratch_gaussian[last].
        let coarse_size = accum.scratch_gaussian[last].size();
        dispatch_compute(
            &mut encoder,
            &self.pipelines.normalize,
            &accum.normalize_coarsest_bg,
            "normalize_coarsest",
            coarse_size.width,
            coarse_size.height,
        );

        // G[i] = normalize(L[i]) + Upsample(G[i+1]); the level-0 pass also
        // finishes, so `scratch_gaussian[0]` is the result.
        for level in (0..last).rev() {
            let fine_size = accum.scratch_gaussian[level].size();
            dispatch_compute(
                &mut encoder,
                &self.pipelines.laplacian_add,
                &accum.laplacian_add_bgs[level],
                "laplacian_add",
                fine_size.width,
                fine_size.height,
            );
        }

        self.ctx.submit(encoder);
        Ok(&accum.scratch_gaussian[0])
    }

    /// [`Self::collapse`], then Finalized, keeping only the result and the
    /// winner record.
    pub fn finalize(&mut self) -> Result<()> {
        self.collapse()?;
        // A failure is reported before the state moves on, leaving the
        // accumulators in place.
        self.ctx.check()?;
        let State::Accumulating(accum) = std::mem::replace(&mut self.state, State::Uninitialized)
        else {
            unreachable!("collapse checked the state");
        };
        self.intake.finish();

        self.state = State::Finalized {
            result: accum.scratch_gaussian.into_iter().next().expect("level 0"),
            winners: accum.winners,
        };
        debug!("pyramid stacker: finalized");
        Ok(())
    }

    pub fn result_texture(&self) -> Option<&Texture> {
        match &self.state {
            State::Finalized { result, .. } => Some(result),
            _ => None,
        }
    }

    /// The picture while the stack runs: the winner record so far (8-bit, in
    /// `format`, its longest side capped at `max_side`). Valid from the first
    /// accumulated frame.
    pub fn render_live(&self, max_side: u32, format: wgpu::TextureFormat) -> Result<Texture> {
        let winners = match &self.state {
            State::Accumulating(r) if self.intake.frames.stacked_count() > 0 => &r.winners,
            State::Finalized { winners, .. } => winners,
            _ => return Err(CoreError::NoFrames),
        };
        let (live, _, _) =
            self.pipelines.live_map.render(&self.ctx, winners, Some(max_side), format)?;
        Ok(live)
    }

    /// The frame map (r32float, full resolution, [`FramePlane`] values);
    /// `None` before [`Self::finalize`].
    pub fn frame_map(&self) -> Option<&Texture> {
        match &self.state {
            State::Finalized { winners, .. } => Some(winners),
            _ => None,
        }
    }

    fn allocate_and_init(&self, width: u32, height: u32, depth: FrameDepth) -> AccumResources {
        let num_levels = num_levels(width, height);
        // Below the coarsest, which has no metric.
        let record_level = RECORD_LEVEL.min(num_levels - 2);

        info!("allocating pyramid: {width}x{height}, {num_levels} levels");

        let rgba_usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST;
        // The winner record is cleared by a render pass.
        let clear_usage = rgba_usage | wgpu::TextureUsages::RENDER_ATTACHMENT;

        let source = Source::new(&self.ctx, width, height, depth, "pyramid_source");

        let mut scratch_gaussian = Vec::with_capacity(num_levels);
        let mut best_rgb_pyramid = Vec::with_capacity(num_levels);
        let mut best_focus_pyramid = Vec::with_capacity(num_levels);

        for (level, (w, h)) in level_sizes(width, height, num_levels).enumerate() {
            scratch_gaussian.push(self.ctx.create_texture(
                w,
                h,
                wgpu::TextureFormat::Rgba32Float,
                rgba_usage,
                &format!("scratch_gaussian_{level}"),
            ));
            best_rgb_pyramid.push(create_accum(&self.ctx, w, h, &format!("best_rgb_{level}")));
            // The coarsest level has no focus metric.
            if level + 1 < num_levels {
                best_focus_pyramid.push(ScalarMap::new(
                    &self.ctx,
                    w,
                    h,
                    &format!("best_focus_{level}"),
                ));
            }
        }
        let last = num_levels - 1;
        let winners = self.ctx.create_texture(
            width,
            height,
            wgpu::TextureFormat::R32Float,
            clear_usage,
            "pyramid_winners",
        );
        let record_size = scratch_gaussian[record_level].size();
        let winners_best = ScalarMap::new(
            &self.ctx,
            record_size.width,
            record_size.height,
            "pyramid_winners_best",
        );

        let prior = self.prior_chain(&scratch_gaussian, record_level, last);

        let blur = focus_blur(self.config.smoothness);
        assert!(
            blur.half_width <= FOCUS_HALO,
            "focus blur half-width {} exceeds the tile halo",
            blur.half_width
        );
        let focus_blur_params_buffer = self.ctx.create_uniform_buffer(&blur, "focus_blur_params");

        let level_params_buffers = self.level_params(num_levels, record_level, width, height);

        let warp_params_buffer =
            self.ctx.create_uniform_buffer(&WarpParams::identity(false), "pyramid_warp_params");
        let warp_bg = warp_bind_group(
            &self.ctx,
            &self.pipelines,
            &source,
            &scratch_gaussian[0],
            &warp_params_buffer,
        );

        let mut downsample_bgs = Vec::with_capacity(last);
        for level in 0..last {
            let src = scratch_gaussian[level].create_view(&Default::default());
            let dst = scratch_gaussian[level + 1].create_view(&Default::default());
            downsample_bgs.push(create_bind_group(
                &self.ctx.device,
                &self.pipelines.downsample,
                0,
                &format!("downsample_bg_{level}"),
                &[tex(&src), tex(&dst)],
            ));
        }

        let frame_params_buffer =
            self.ctx.create_uniform_buffer(&FrameParams { plane: 0.0 }, "pyramid_frame_params");
        let winners_view = winners.create_view(&Default::default());
        let mut focus_wta_bgs = Vec::with_capacity(last);
        for level in 0..last {
            let fine = scratch_gaussian[level].create_view(&Default::default());
            let coarse = scratch_gaussian[level + 1].create_view(&Default::default());
            // The recording level writes the prior; level 0 reads the
            // recording level's, the others their own.
            let (prior_out, prior_in) = if level == record_level {
                (&prior.views[0], &prior.placeholder)
            } else {
                (&prior.placeholder, &prior.views[level.saturating_sub(record_level)])
            };
            focus_wta_bgs.push(create_bind_group(
                &self.ctx.device,
                &self.pipelines.focus_wta,
                0,
                &format!("focus_wta_bg_{level}"),
                &[
                    tex(&fine),                                      // fw_gauss_fine
                    tex(&coarse),                                    // fw_gauss_coarse
                    best_rgb_pyramid[level].as_entire_binding(),     // fw_best_rgb
                    best_focus_pyramid[level].binding(),             // fw_best_focus
                    level_params_buffers[level].as_entire_binding(), // fw_level
                    warp_params_buffer.as_entire_binding(),          // fw_warp
                    focus_blur_params_buffer.as_entire_binding(),    // fw_blur
                    frame_params_buffer.as_entire_binding(),         // fw_frame
                    tex(&winners_view),                              // fw_winner
                    winners_best.binding(),                          // fw_winner_best
                    tex(prior_out),                                  // fw_prior_out
                    tex(prior_in),                                   // fw_prior_in
                ],
            ));
        }

        let coarse_gauss = scratch_gaussian[last].create_view(&Default::default());
        let accumulate_sum_bg = create_bind_group(
            &self.ctx.device,
            &self.pipelines.accumulate_sum,
            0,
            "accumulate_sum_bg",
            &[
                tex(&coarse_gauss),
                best_rgb_pyramid[last].as_entire_binding(),
                level_params_buffers[last].as_entire_binding(),
                warp_params_buffer.as_entire_binding(),
            ],
        );

        let finish = FinishBuffers::new(&self.ctx, self.info.is_linear);
        let normalize_coarsest_bg = create_bind_group(
            &self.ctx.device,
            &self.pipelines.normalize,
            0,
            "normalize_coarsest",
            &[
                best_rgb_pyramid[last].as_entire_binding(),
                tex(&coarse_gauss),
                finish.for_level(last).as_entire_binding(),
            ],
        );
        let mut laplacian_add_bgs = Vec::with_capacity(last);
        for level in 0..last {
            let coarse = scratch_gaussian[level + 1].create_view(&Default::default());
            let fine = scratch_gaussian[level].create_view(&Default::default());
            laplacian_add_bgs.push(create_bind_group(
                &self.ctx.device,
                &self.pipelines.laplacian_add,
                0,
                &format!("laplacian_add_{level}"),
                &[
                    best_rgb_pyramid[level].as_entire_binding(),
                    tex(&coarse),
                    tex(&fine),
                    finish.for_level(level).as_entire_binding(),
                ],
            ));
        }

        self.clear_accumulators(&best_rgb_pyramid, &best_focus_pyramid, &winners, &winners_best);

        AccumResources {
            source,
            scratch_gaussian,
            winners,
            record_level,
            prior_downsample_bgs: prior.downsample_bgs,
            warp_bg,
            warp_params_buffer,
            frame_params_buffer,
            downsample_bgs,
            focus_wta_bgs,
            accumulate_sum_bg,
            normalize_coarsest_bg,
            laplacian_add_bgs,
        }
    }

    /// The focus prior per level from the recording level to the one above
    /// the coarsest, its downsample chain, and a 1×1 placeholder for the
    /// binding a level does not use.
    fn prior_chain(
        &self,
        scratch_gaussian: &[Texture],
        record_level: usize,
        last: usize,
    ) -> PriorChain {
        let prior_usage =
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let prior_pyramid: Vec<Texture> = (record_level..last)
            .map(|level| {
                let s = scratch_gaussian[level].size();
                self.ctx.create_texture(
                    s.width,
                    s.height,
                    wgpu::TextureFormat::R32Float,
                    prior_usage,
                    &format!("prior_{level}"),
                )
            })
            .collect();
        let prior_placeholder = self.ctx.create_texture(
            1,
            1,
            wgpu::TextureFormat::R32Float,
            prior_usage,
            "prior_placeholder",
        );
        let prior_views: Vec<wgpu::TextureView> =
            prior_pyramid.iter().map(|t| t.create_view(&Default::default())).collect();
        let placeholder_view = prior_placeholder.create_view(&Default::default());
        let prior_downsample_bgs: Vec<wgpu::BindGroup> = prior_views
            .windows(2)
            .map(|pair| {
                create_bind_group(
                    &self.ctx.device,
                    &self.pipelines.prior_downsample,
                    0,
                    "prior_downsample_bg",
                    &[tex(&pair[0]), tex(&pair[1])],
                )
            })
            .collect();
        PriorChain {
            views: prior_views,
            placeholder: placeholder_view,
            downsample_bgs: prior_downsample_bgs,
        }
    }

    /// One [`LevelParams`] buffer per level, the coarsest included (mask
    /// fields only).
    fn level_params(
        &self,
        num_levels: usize,
        record_level: usize,
        width: u32,
        height: u32,
    ) -> Vec<wgpu::Buffer> {
        let exponents = exponent_schedule(self.config.smoothness, num_levels);
        let mut level_params_buffers = Vec::with_capacity(num_levels);
        for (level, exponent) in exponents.into_iter().enumerate() {
            level_params_buffers.push(self.ctx.create_uniform_buffer(
                &LevelParams {
                    exponent,
                    level_scale: (1u32 << level) as f32,
                    src_w: width as f32,
                    src_h: height as f32,
                    prior_source: prior_source(level, record_level),
                },
                &format!("level_params_{level}"),
            ));
        }
        level_params_buffers
    }

    /// best_rgb starts at zero, best_focus at `FOCUS_SENTINEL`.
    fn clear_accumulators(
        &self,
        best_rgb_pyramid: &[wgpu::Buffer],
        best_focus_pyramid: &[ScalarMap],
        winners: &Texture,
        winners_best: &ScalarMap,
    ) {
        let mut encoder = self.ctx.encoder("init_accumulators");
        for buffer in best_rgb_pyramid {
            encoder.clear_buffer(buffer, 0, None);
        }
        let fill = &self.pipelines.fill;
        for map in best_focus_pyramid {
            fill.encode(&self.ctx, &mut encoder, map, FOCUS_SENTINEL);
        }
        // The record's best starts at the sentinel, so the first frame writes
        // every pixel; the texture's clear only defines it.
        encode_clear(&mut encoder, winners, wgpu::Color::TRANSPARENT);
        fill.encode(&self.ctx, &mut encoder, winners_best, FOCUS_SENTINEL);
        self.ctx.submit(encoder);
    }
}

/// [`PyramidStacker::prior_chain`]'s views and downsample chain, which hold
/// its textures.
struct PriorChain {
    views: Vec<wgpu::TextureView>,
    placeholder: wgpu::TextureView,
    downsample_bgs: Vec<wgpu::BindGroup>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An earlier frame's failure, reported by `finalize`, leaves the
    /// stacker accumulating.
    #[test]
    fn an_earlier_frame_s_failure_leaves_it_accumulating() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let info = StackInfo { total_frames: 2, is_linear: false };
        let mut stacker = PyramidStacker::new(ctx, PyramidConfig::default(), info);
        let frame = image::DynamicImage::new_rgb8(64, 48);
        stacker.accumulate(&frame, None).unwrap();
        let State::Accumulating(r) = &stacker.state else { unreachable!() };
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        r.source.texture().destroy();
        // The frame's work fails on the GPU, unchecked until `finalize`.
        stacker.accumulate(&frame, None).unwrap();
        assert!(stacker.finalize().is_err());
        assert!(matches!(stacker.state, State::Accumulating(_)));
    }

    #[test]
    fn the_shader_s_prior_exponent_is_the_stacker_s() {
        let line = format!("const PRIOR_EXPONENT: f32 = {PRIOR_EXPONENT:?};");
        assert!(include_str!("pyramid.wgsl").contains(&line), "pyramid.wgsl should have {line}");
    }
}
