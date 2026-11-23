//! Depth focus stacking (bandpass focus metric + guided depth-map smoothing + blend).
//!
//! Three phases:
//! 1. `accumulate` per frame: gray → focus metric (`crate::focus`) → WTA
//!    into a depth map and an opacity (confidence) map, on a half-resolution
//!    map grid.
//! 2. `end_pass_one`: confidence, guided IIR smoothing (see
//!    [`DepthConfig`]), feathering toward the stacked frames' planes.
//! 3. `blend_frame` per frame, then `finalize`: each frame's Laplacian
//!    pyramid (σ = 0.666, 2:1) is blended per level by depth proximity
//!    against a box-averaged pyramid of the depth map, then collapsed. At
//!    coarse levels a block's depth is the average under it, so the frames
//!    either side of a depth edge blend there, which hides seams.

mod blend;
mod curves;
mod pipeline;

use blend::BlendResources;
pub(crate) use curves::{MAP_SCALE, iir_post_blur_sigma};
use curves::{SMOOTHING_PASSES, feather_power, iir_pre_blur_sigma, iir_smoothing_k1, map_grid};
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{
    create_bind_group, dispatch_compute, dispatch_compute_textures, dispatch_compute_with,
    encode_clear, tex,
};
use pipeline::DepthPipelines;
use std::sync::Arc;
use tracing::{debug, info};
use wgpu::Texture;

use crate::StackInfo;
use crate::accum::check_accum_fits;
use crate::error::{CoreError, Result, Setting};
use crate::focus::{FocusBindings, FocusUniforms, iir_sigma};
use crate::ledger::FrameLedger;
use crate::plane::{FramePlane, stacked_neighbors};
use crate::scalar::ScalarMap;
use crate::stacker::{self, Intake};
use crate::transform::{FrameAlignment, WarpParams};
use crate::upload::{FrameDepth, FrameInput, Source};

/// Depth-stacker parameters.
///
/// Everything before the blend runs on the map grid ([`map_grid`], scale
/// `min(0.5, 6000/longest side)`); the depth map is then upsampled
/// bilinearly for the full-resolution blend. Smoothing is a two-sided
/// exponential blur whose coefficient goes per pixel from weak to strong by
/// confidence value (no gradient term), run twice, cascaded.
///
/// Parameters are in map pixels. `smoothing`: weak arg = smoothing·3/128,
/// strong = weak·64. `radius`: focus pre-blur arg = radius·1.2, post-blur
/// arg = radius·4.8, through `focus::iir_blur_sigma` (σ ≈ 3 and ≈ 11.7 at
/// the default). `blend_range`: feather power = 1 + (10 − blend_range)/3.
#[derive(Clone, Copy, Debug)]
pub struct DepthConfig {
    /// Focus-metric radius ([`RADIUS_RANGE`]): how large a patch picks its
    /// frame together.
    pub radius: f32,
    /// Depth map smoothing strength ([`SMOOTHING_RANGE`]); 0 is off.
    pub smoothing: f32,
    /// Blend range ([`BLEND_RANGE`]). 10 leaves depths as they are; lower
    /// pulls them harder toward the nearest frame (power 4.33 at 0).
    pub blend_range: f32,
}

/// Inclusive `blend_range` range.
pub const BLEND_RANGE: (f32, f32) = (0.0, 10.0);
/// Inclusive `smoothing` range; 0 turns the depth-map smoothing off.
pub const SMOOTHING_RANGE: (f32, f32) = (0.0, 20.0);
/// Inclusive `radius` range.
pub const RADIUS_RANGE: (f32, f32) = (1.0, 50.0);
/// The app's slider steps for the settings above.
pub const BLEND_RANGE_STEP: f32 = 0.1;
pub const SMOOTHING_STEP: f32 = 0.1;
pub const RADIUS_STEP: f32 = 1.0;

impl DepthConfig {
    pub const DEFAULT_RADIUS: f32 = 8.0;
    pub const DEFAULT_SMOOTHING: f32 = 4.0;
    pub const DEFAULT_BLEND_RANGE: f32 = 3.0;

    /// Reject values outside the slider ranges, which bound the blur widths.
    pub fn validate(&self) -> Result<()> {
        Setting::Radius.check(self.radius, RADIUS_RANGE)?;
        Setting::Smoothing.check(self.smoothing, SMOOTHING_RANGE)?;
        Setting::BlendRange.check(self.blend_range, BLEND_RANGE)
    }
}

impl Default for DepthConfig {
    fn default() -> Self {
        Self {
            radius: Self::DEFAULT_RADIUS,
            smoothing: Self::DEFAULT_SMOOTHING,
            blend_range: Self::DEFAULT_BLEND_RANGE,
        }
    }
}

mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct ConfidenceParams {
        pub(super) width: u32,
        pub(super) stacked_frames: u32,
        pub(super) total_pixels: u32,
        pub(super) gamma: f32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct DepthWtaParams {
        pub(super) plane: f32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct IirParams {
        pub(super) k1_weak: f32,
        pub(super) k1_strong: f32,
        pub(super) horizontal: u32,
        pub(super) backward: u32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct DepthFeatherParams {
        pub(super) step_size: f32,
        pub(super) power: f32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct BlendParams {
        pub(super) scaled_frame_idx: f32,
        pub(super) dist_below: f32,
        pub(super) dist_above: f32,
        pub(super) _pad: f32,
    }
}
use params::*;

struct AccumResources {
    width: u32,
    height: u32,
    /// Map grid ([`map_grid`]); every map below lives on it.
    map_width: u32,
    map_height: u32,

    // The WTA updates these two in place, so they are buffers; it only
    // writes the depth map.
    best_focus: ScalarMap,
    depth_index: Texture, // r32f — full precision avoids blend banding
    opacity_map: ScalarMap,
    // Per frame, the metric's working maps (gray rows lives on in the bind
    // groups); after the last frame, the finalization's scratch.
    /// The metric's gray; the smoothing's forward result.
    map_a: Texture,
    /// The metric's scratch; the confidence guide.
    map_b: Texture,
    /// The frame's focus map; the smoothing's horizontal result, then the
    /// feathered depth map.
    map_c: Texture,
    source: Source, // uploader target (reused in blend phase)

    // Static per-frame bind groups (texture pairings never change)
    focus_bgs: FocusBindings,
    wta_bg: wgpu::BindGroup,
    wta_params_buffer: wgpu::Buffer,  // rewritten each frame
    warp_params_buffer: wgpu::Buffer, // rewritten each frame
}

impl AccumResources {
    /// What the blend phase keeps: the frame size and the uploader target.
    fn into_blend_inputs(self) -> (u32, u32, Source) {
        (self.width, self.height, self.source)
    }
}

/// Where a stack is.
///
/// Lifecycle: `Uninitialized` → `Accumulating` (the first
/// [`DepthStacker::allocate`] whose textures all allocate; it stays
/// `Uninitialized` if one fails) → `Blending` ([`DepthStacker::end_pass_one`],
/// or `Failed` if it fails after changing the accumulators) → `Finalized`
/// ([`DepthStacker::finalize`], or `Failed` if its collapse fails). An
/// earlier frame's failure, reported by either, leaves the state as it
/// was. Changed only by the [`DepthStacker`]'s methods; pass 2's frames
/// are a [`SecondPass`](crate::ledger::SecondPass) inside `Blending`.
// One State per stacker, so boxing the large variants gains nothing.
#[allow(clippy::large_enum_variant)]
enum State {
    Uninitialized,
    Accumulating(AccumResources),
    Blending(BlendResources),
    Finalized {
        /// The stacked image (rgba32float, full size).
        result: Texture,
        /// The full-resolution depth map (r32float, [`FramePlane`] values):
        /// the stack's frame map.
        depth_map: Texture,
    },
    /// `end_pass_one` or `finalize` failed after changing what it can't
    /// redo; nothing runs from here.
    Failed,
}

impl State {
    fn in_first_pass(&self) -> bool {
        matches!(self, State::Uninitialized | State::Accumulating(_))
    }

    /// Take the state out, leaving `Uninitialized` until the caller sets
    /// the next.
    fn take(&mut self) -> State {
        std::mem::replace(self, State::Uninitialized)
    }
}

/// The confidence map's response curve after the percentile stretch.
const CONFIDENCE_GAMMA: f32 = 0.5;
/// Bins of the confidence histogram (atomic u32 each).
const HISTOGRAM_BINS: u32 = 1024;

pub struct DepthStacker {
    ctx: Arc<GpuContext>,
    /// Built once per device ([`GpuContext::programs`]).
    pipelines: Arc<DepthPipelines>,
    config: DepthConfig,
    info: StackInfo,
    state: State,
    intake: Intake,
}

/// For each step between frame `i` and `i + 1`, the stacked frames either
/// side of it (`depth_feather`'s `df_spans`), so feathering never pulls
/// toward a left-out frame. `[0, 0]` where one side has none.
fn feather_spans(frames: &FrameLedger, total: u32) -> Vec<[u32; 2]> {
    stacked_neighbors(total, |j| frames.stacked(j))
        .windows(2)
        .map(|pair| match (pair[0][0], pair[1][1]) {
            (Some(below), Some(above)) => [below, above],
            _ => [0, 0],
        })
        .collect()
}

impl DepthStacker {
    pub fn new(ctx: Arc<GpuContext>, config: DepthConfig, info: StackInfo) -> Self {
        Self {
            pipelines: ctx.programs(DepthPipelines::new),
            intake: Intake::new(&ctx, info.total_frames),
            ctx,
            config,
            info,
            state: State::Uninitialized,
        }
    }

    /// Leave the next frame out: its index and plane stay, empty, and its
    /// neighbors blend across the gap.
    pub fn skip_frame(&mut self) -> Result<()> {
        self.intake.skip(self.state.in_first_pass())
    }

    /// Size the stacker ahead of the first [`Self::accumulate`], which
    /// otherwise does it. Does nothing once sized.
    pub fn allocate(&mut self, width: u32, height: u32, depth: FrameDepth) -> Result<()> {
        if !matches!(self.state, State::Uninitialized) {
            return Ok(());
        }
        self.config.validate()?;
        self.intake.check_size(&self.ctx, width, height)?;
        // Checked before pass 1, though only the blend allocates them.
        check_accum_fits(&self.ctx, width, height)?;
        let accum = self.allocate_and_init(width, height, depth);
        self.state = State::Accumulating(stacker::allocated(&self.ctx, accum)?);
        debug!("depth stacker: accumulating");
        Ok(())
    }

    /// Peak GPU memory, in the blend pass: [`Self::BYTES_PER_PIXEL`] and
    /// the source (pass 1 is freed before the blend allocates). The upload
    /// buffer is the caller's (`stacker::gpu_bytes`).
    pub fn gpu_bytes(ctx: &GpuContext, width: u32, height: u32, depth: FrameDepth) -> u64 {
        let held = stacker::pixels(width, height) * Self::BYTES_PER_PIXEL;
        stacker::gpu_bytes(ctx, width, height, depth, held)
    }

    /// Blend bytes per pixel besides the source, with Metal's 1/16 texture
    /// padding: the depth map, and per level the frame Gaussian, the
    /// accumulator and the depth level (a third more above level 0). 49.7
    /// at any checked size.
    pub const BYTES_PER_PIXEL: u64 = 50;

    /// Accumulate one frame: grayscale → bandpass focus metric → WTA.
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
        self.ctx.write_uniform_buffer(
            &r.wta_params_buffer,
            &DepthWtaParams { plane: FramePlane::new(index, self.info.total_frames).plane },
        );
        // The WTA reads the focus map through this warp, mapped onto the map
        // grid by the downscale's own per-axis ratio (a bare 0.5 drifts on
        // odd sizes). No brightness correction: the metric's Weber
        // normalization divides out most of an exposure difference.
        let warp = match alignment {
            Some(a) => {
                let ratio =
                    [r.width as f32 / r.map_width as f32, r.height as f32 / r.map_height as f32];
                WarpParams::on_grid(&a.transform, ratio, false)
            }
            None => WarpParams::identity(false),
        };
        self.ctx.write_uniform_buffer(&r.warp_params_buffer, &warp);

        let mut encoder = self.ctx.encoder("depth_accumulate");

        prep.dispatch_to_rgba(&self.ctx, &mut encoder, r.source.texture());

        let (w, h) = (r.map_width, r.map_height);
        let p = &self.pipelines;
        // The metric runs on the frame's own grid and only the finished
        // focus map is warped, by the WTA: warping first would make it
        // sensitive to subpixel phase (`warped_focus` in depth.wgsl).
        p.focus.encode(&mut encoder, &r.focus_bgs, w, h);
        dispatch_compute(&mut encoder, &p.depth_wta_accumulate, &r.wta_bg, "depth_wta", w, h);

        self.ctx.submit(encoder);
        self.intake.frames.push_stacked(index);
        Ok(())
    }

    /// Finish the depth map and set up the blend pass (Accumulating →
    /// Blending).
    pub fn end_pass_one(&mut self) -> Result<()> {
        if self.intake.frames.stacked_count() == 0 {
            return Err(CoreError::NoFrames);
        }
        if !matches!(self.state, State::Accumulating(_)) {
            return Err(CoreError::InvalidState(
                "end_pass_one() requires Accumulating state".into(),
            ));
        }
        // A frame's earlier failure leaves the stacker accumulating.
        self.ctx.check()?;
        let State::Accumulating(accum) = self.state.take() else {
            unreachable!("checked above");
        };
        let (depth_full, finished) = self.encode_depth_map(&accum);
        let begun = self.begin_blend(accum, depth_full, finished);
        // The accumulators were rewritten in place (a retry would smooth
        // twice), and a failed blend allocation would lose every frame
        // silently: either failure leaves the stacker failed.
        if let Err(e) = begun.and_then(|()| Ok(self.ctx.check()?)) {
            self.state = State::Failed;
            debug!("depth stacker: failed");
            return Err(e);
        }
        debug!("depth stacker: blending");
        Ok(())
    }

    /// Encode and submit the depth-map finalization. Returns the
    /// full-resolution map and the submission.
    fn encode_depth_map(&self, r: &AccumResources) -> (Texture, wgpu::SubmissionIndex) {
        let mut encoder = self.ctx.encoder("depth_finalize");
        self.encode_confidence(&mut encoder, r);
        self.encode_iir(&mut encoder, [&r.depth_index, &r.map_b, &r.map_a, &r.map_c]);
        let depth = self.encode_feather(&mut encoder, r);
        let depth_full = self.encode_upsample(&mut encoder, r, depth);

        (depth_full, self.ctx.submit(encoder))
    }

    /// The confidence map: opacity contrast, then percentile normalization.
    fn encode_confidence(&self, encoder: &mut wgpu::CommandEncoder, r: &AccumResources) {
        let (w, h) = (r.map_width, r.map_height);
        let p = &self.pipelines;
        let device = &self.ctx.device;
        // `clamp(inv_range·(x − offset), 0, 1)^gamma`: the square root lifts
        // mid confidences so more of the subject counts as confident.
        let params = self.ctx.create_uniform_buffer(
            &ConfidenceParams {
                width: w,
                stacked_frames: self.intake.frames.stacked_count(),
                total_pixels: w * h,
                gamma: CONFIDENCE_GAMMA,
            },
            "confidence_params",
        );
        // Both written on the GPU; the stretch is `NormOpacityParams`.
        let histogram = self.ctx.create_storage_buffer(
            "histogram",
            HISTOGRAM_BINS as u64 * 4,
            wgpu::BufferUsages::COPY_DST,
        );
        let stretch =
            self.ctx.create_storage_buffer("confidence_stretch", 8, wgpu::BufferUsages::empty());
        let guide = r.map_b.create_view(&Default::default());

        let bind = |pipeline, label, resources: &[wgpu::BindingResource]| {
            create_bind_group(device, pipeline, 0, label, resources)
        };
        let opacity_diff = bind(
            &p.opacity_diff,
            "opacity_diff",
            &[r.opacity_map.binding(), r.best_focus.binding(), params.as_entire_binding()],
        );
        let histogram_bg = bind(
            &p.histogram_accumulate,
            "histogram_accumulate",
            &[r.opacity_map.binding(), histogram.as_entire_binding(), params.as_entire_binding()],
        );
        let percentile = bind(
            &p.histogram_percentile,
            "histogram_percentile",
            &[
                histogram.as_entire_binding(),
                stretch.as_entire_binding(),
                params.as_entire_binding(),
            ],
        );
        let normalize = bind(
            &p.normalize_opacity,
            "normalize_opacity",
            &[
                r.opacity_map.binding(),
                stretch.as_entire_binding(),
                params.as_entire_binding(),
                tex(&guide),
            ],
        );

        dispatch_compute(encoder, &p.opacity_diff, &opacity_diff, "opacity_diff", w, h);
        encoder.clear_buffer(&histogram, 0, None);
        dispatch_compute(
            encoder,
            &p.histogram_accumulate,
            &histogram_bg,
            "histogram_accumulate",
            w,
            h,
        );
        // One thread: `histogram_percentile` is `@workgroup_size(1)`.
        dispatch_compute_with(
            encoder,
            &p.histogram_percentile,
            &[&percentile],
            "percentile",
            1,
            1,
            1,
        );
        dispatch_compute(encoder, &p.normalize_opacity, &normalize, "normalize_opacity", w, h);
    }

    /// Pull depth toward the stacked frames' planes. Returns the finished
    /// map: `map_c` when feathered, else `depth_index`.
    fn encode_feather<'r>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        r: &'r AccumResources,
    ) -> &'r Texture {
        let (w, h) = (r.map_width, r.map_height);
        let p = &self.pipelines;
        let feather_power = feather_power(self.config.blend_range);
        if feather_power > 1.0 && self.intake.frames.stacked_count() > 1 {
            let step_size = FramePlane::new(0, self.info.total_frames).step;
            let feather_params = self.ctx.create_uniform_buffer(
                &DepthFeatherParams { step_size, power: feather_power },
                "feather_params",
            );
            let spans = self.ctx.create_buffer_with_data(
                "feather_spans",
                bytemuck::cast_slice(&feather_spans(&self.intake.frames, self.info.total_frames)),
                wgpu::BufferUsages::STORAGE,
            );
            let depth = r.depth_index.create_view(&Default::default());
            let out = r.map_c.create_view(&Default::default());
            let bg = create_bind_group(
                &self.ctx.device,
                &p.depth_feather,
                0,
                "depth_feather",
                &[
                    tex(&depth),
                    feather_params.as_entire_binding(),
                    spans.as_entire_binding(),
                    tex(&out),
                ],
            );
            dispatch_compute(encoder, &p.depth_feather, &bg, "depth_feather", w, h);
            return &r.map_c;
        }
        &r.depth_index
    }

    /// `depth` upsampled bilinearly to full resolution.
    fn encode_upsample(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        r: &AccumResources,
        depth: &Texture,
    ) -> Texture {
        let p = &self.pipelines;
        let depth_full = self.ctx.create_texture(
            r.width,
            r.height,
            wgpu::TextureFormat::R32Float,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "depth_index_full",
        );
        dispatch_compute_textures(
            encoder,
            &self.ctx.device,
            &p.upsample_bilinear,
            "upsample_depth",
            &[depth, &depth_full],
            &[],
            r.width,
            r.height,
        );
        depth_full
    }

    /// One sweep, one thread per line; the backward sweep also reads `fwd`.
    fn dispatch_sweep(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        [src, guide, out, fwd]: [&Texture; 4],
        params: &wgpu::Buffer,
        lines: u32,
    ) {
        let pipeline = &self.pipelines.iir_sweep;
        let view = |t: &Texture| t.create_view(&Default::default());
        let [src, guide, out, fwd] = [src, guide, out, fwd].map(view);
        let bg = create_bind_group(
            &self.ctx.device,
            pipeline,
            0,
            "iir_sweep",
            &[tex(&src), tex(&guide), tex(&out), params.as_entire_binding(), tex(&fwd)],
        );
        dispatch_compute_with(encoder, pipeline, &[&bg], "iir_sweep", lines, 1, 64);
    }

    /// Guided IIR smoothing ([`DepthConfig`]): each pass horizontal then
    /// vertical, each averaging a forward and a backward sweep. Confident
    /// pixels keep their depth; unconfident ones inherit their neighbors'.
    ///
    /// `depth` is smoothed in place; `fwd` and `across` are scratch.
    fn encode_iir(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        [depth, guide, fwd, across]: [&Texture; 4],
    ) {
        if self.config.smoothing <= 0.0 {
            return;
        }
        let (w, h) = (depth.width(), depth.height());
        let (k1_weak, k1_strong) = iir_smoothing_k1(self.config.smoothing);
        info!(
            "iir smoothing: weak σ={:.3}, strong σ={:.3} per pass, {SMOOTHING_PASSES} passes",
            iir_sigma(k1_weak),
            iir_sigma(k1_strong),
        );
        let params = |horizontal: bool, backward: bool| {
            self.ctx.create_uniform_buffer(
                &IirParams {
                    k1_weak,
                    k1_strong,
                    horizontal: horizontal as u32,
                    backward: backward as u32,
                },
                "iir_params",
            )
        };
        let [h_fwd, h_back, v_fwd, v_back] =
            [(true, false), (true, true), (false, false), (false, true)]
                .map(|(horizontal, backward)| params(horizontal, backward));

        // Horizontal depth → across, vertical across → depth.
        for _ in 0..SMOOTHING_PASSES {
            for (src, out, fwd_params, back_params, lines) in
                [(depth, across, &h_fwd, &h_back, h), (across, depth, &v_fwd, &v_back, w)]
            {
                self.dispatch_sweep(encoder, [src, guide, fwd, src], fwd_params, lines);
                self.dispatch_sweep(encoder, [src, guide, out, fwd], back_params, lines);
            }
        }
    }

    pub fn result_texture(&self) -> Option<&Texture> {
        match &self.state {
            State::Finalized { result, .. } => Some(result),
            _ => None,
        }
    }

    /// The full-resolution depth map (r32float, [`FramePlane`] values);
    /// `None` before [`Self::finalize`].
    pub fn frame_map(&self) -> Option<&Texture> {
        match &self.state {
            State::Finalized { depth_map, .. } => Some(depth_map),
            _ => None,
        }
    }

    /// Render the depth map as 8-bit gray in `format`
    /// (`GpuContext::map_format`), from the first frame on: the map grid in
    /// pass 1, full size after.
    fn render_map(
        &self,
        max_side: Option<u32>,
        format: wgpu::TextureFormat,
    ) -> Result<(Texture, u32, u32)> {
        let depth = match &self.state {
            State::Accumulating(r) => &r.depth_index,
            State::Blending(r) => &r.depth_index,
            State::Finalized { depth_map, .. } => depth_map,
            State::Uninitialized => return Err(CoreError::NoFrames),
            State::Failed => return Err(CoreError::InvalidState("the stacker failed".into())),
        };
        self.pipelines.map.render(&self.ctx, depth, max_side, format)
    }

    /// The live picture: the depth map as it stands ([`Self::render_map`]).
    pub fn render_live(&self, max_side: u32, format: wgpu::TextureFormat) -> Result<Texture> {
        let (live, _, _) = self.render_map(Some(max_side), format)?;
        Ok(live)
    }

    fn allocate_and_init(&self, width: u32, height: u32, depth: FrameDepth) -> AccumResources {
        info!("allocating depth stacker: {width}x{height}, {} frames", self.info.total_frames);

        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let clear_usage = usage | wgpu::TextureUsages::RENDER_ATTACHMENT;
        let r_format = wgpu::TextureFormat::R32Float;

        let (map_width, map_height, map_scale) = map_grid(width, height);
        info!("map grid {map_width}x{map_height} (scale {map_scale})");

        let best_focus = ScalarMap::new(&self.ctx, map_width, map_height, "best_focus");
        let depth_index =
            self.ctx.create_texture(map_width, map_height, r_format, clear_usage, "depth_index");
        let opacity_map = ScalarMap::new(&self.ctx, map_width, map_height, "opacity_map");
        let source = Source::new(&self.ctx, width, height, depth, "depth_source");
        let gray_rows = self.ctx.create_texture(map_width, height, r_format, usage, "gray_rows");
        let [map_a, map_b, map_c] = ["depth_map_a", "depth_map_b", "depth_map_c"]
            .map(|label| self.ctx.create_texture(map_width, map_height, r_format, usage, label));

        info!(
            "focus metric blur: pre σ={:.3}, post σ={:.3}",
            iir_pre_blur_sigma(self.config.radius),
            iir_post_blur_sigma(self.config.radius)
        );
        let focus_uniforms = FocusUniforms::new(
            &self.ctx,
            iir_pre_blur_sigma(self.config.radius),
            iir_post_blur_sigma(self.config.radius),
            self.info.is_linear,
        );
        let wta_params_buffer = self.ctx.create_uniform_buffer(
            &DepthWtaParams { plane: FramePlane::new(0, self.info.total_frames).plane },
            "wta_params",
        );

        let device = &self.ctx.device;
        let p = &self.pipelines;
        let view = |t: &Texture| t.create_view(&Default::default());

        let focus_bgs = p.focus.bind(
            &self.ctx,
            &focus_uniforms,
            source.texture(),
            &gray_rows,
            [&map_a, &map_b, &map_c],
        );
        let warp_params_buffer =
            self.ctx.create_uniform_buffer(&WarpParams::identity(false), "depth_warp_params");
        // The finished focus map (map_c), warped as the WTA reads it.
        let wta_bg = create_bind_group(
            device,
            &p.depth_wta_accumulate,
            0,
            "wta_bg",
            &[
                tex(&view(&map_c)),                     // wta_focus
                best_focus.binding(),                   // wta_best_focus
                tex(&view(&depth_index)),               // wta_depth_index
                opacity_map.binding(),                  // wta_opacity_map
                wta_params_buffer.as_entire_binding(),  // wta_params
                warp_params_buffer.as_entire_binding(), // wta_warp
            ],
        );

        // best_focus starts at −1, below any focus value, so the first frame
        // wins every pixel.
        let mut encoder = self.ctx.encoder("depth_init_accumulators");
        p.fill.encode(&self.ctx, &mut encoder, &best_focus, -1.0);
        encode_clear(&mut encoder, &depth_index, wgpu::Color::TRANSPARENT);
        encoder.clear_buffer(&opacity_map.buffer, 0, None);
        self.ctx.submit(encoder);

        AccumResources {
            width,
            height,
            map_width,
            map_height,
            best_focus,
            depth_index,
            opacity_map,
            map_a,
            map_b,
            map_c,
            source,
            focus_bgs,
            wta_bg,
            wta_params_buffer,
            warp_params_buffer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::tests::ledger;

    /// Feathering's spans run between stacked frames, across any run of
    /// left-out ones, and are empty past the outermost stacked frames.
    #[test]
    fn feather_spans_skip_left_out_frames() {
        // 0 left out, 1 stacked, 2–3 left out, 4–5 stacked, 6 left out.
        let spans = feather_spans(&ledger("-s--ss-"), 7);
        assert_eq!(spans, [[0, 0], [1, 4], [1, 4], [1, 4], [4, 5], [0, 0]]);
        // Without a left-out frame, each step is its own span.
        let spans = feather_spans(&ledger("sssssss"), 7);
        assert_eq!(spans, (0..6).map(|i| [i, i + 1]).collect::<Vec<_>>());
    }

    /// `rows` (all one length), smoothed by [`DepthStacker::encode_iir`]
    /// at the default smoothing with confidence `guide` everywhere.
    fn smoothed(ctx: &Arc<GpuContext>, rows: &[Vec<f32>], guide: f32) -> Vec<Vec<f32>> {
        let info = StackInfo { total_frames: 2, is_linear: false };
        let stacker = DepthStacker::new(ctx.clone(), DepthConfig::default(), info);
        let (w, h) = (rows[0].len() as u32, rows.len() as u32);
        let usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC;
        let make = |label, values: &[f32]| {
            let t = ctx.create_texture(w, h, wgpu::TextureFormat::R32Float, usage, label);
            ctx.queue.write_texture(
                t.as_image_copy(),
                bytemuck::cast_slice(values),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            t
        };
        let zeros = vec![0.0; (w * h) as usize];
        let depth = make("depth", &rows.concat());
        let guide = make("guide", &vec![guide; (w * h) as usize]);
        let [fwd, across] = ["fwd", "across"].map(|label| make(label, &zeros));
        let mut encoder = ctx.encoder("smoothed");
        stacker.encode_iir(&mut encoder, [&depth, &guide, &fwd, &across]);
        ctx.submit(encoder);
        let mut out = Vec::new();
        deptheos_gpu::gpu_utils::download_texture_with(ctx, &depth, [0, 0], [w, h], |row| {
            out.push(bytemuck::pod_collect_to_vec(row))
        })
        .unwrap();
        out
    }

    /// A step at the right end smooths to the mirror image of the same step
    /// at the left.
    #[test]
    fn smoothing_treats_both_ends_alike() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        const W: usize = 96;
        let left: Vec<f32> = (0..W).map(|x| if x < 2 { 1.0 } else { 0.0 }).collect();
        let right: Vec<f32> = left.iter().rev().copied().collect();
        // Rows alike, so the vertical passes change nothing; no confidence,
        // so the strong smoothing reaches across the step.
        let [left, right] = [left, right].map(|row| smoothed(&ctx, &vec![row; 4], 0.0)[0].clone());
        let worst = (0..W).map(|x| (left[x] - right[W - 1 - x]).abs()).fold(0.0, f32::max);
        assert!(
            worst < 1e-4,
            "the right end smooths unlike the left, by up to {worst} \
             (edge {} against {})",
            right[W - 1],
            left[0]
        );
        // And a constant line stays constant.
        let flat = smoothed(&ctx, &vec![vec![0.5; W]; 4], 0.0);
        assert!(flat.iter().flatten().all(|&v| (v - 0.5).abs() < 1e-5));
    }

    /// An earlier frame's failure, reported by `end_pass_one`, leaves the
    /// stacker accumulating.
    #[test]
    fn an_earlier_frame_s_failure_leaves_it_accumulating() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let info = StackInfo { total_frames: 2, is_linear: false };
        let mut stacker = DepthStacker::new(ctx, DepthConfig::default(), info);
        let frame = image::DynamicImage::new_rgb8(64, 48);
        stacker.accumulate(&frame, None).unwrap();
        let State::Accumulating(r) = &stacker.state else { unreachable!() };
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        r.source.texture().destroy();
        // The frame's work fails on the GPU, unchecked until `end_pass_one`.
        stacker.accumulate(&frame, None).unwrap();
        assert!(stacker.end_pass_one().is_err());
        assert!(matches!(stacker.state, State::Accumulating(_)));
    }

    /// A failure after the finalization rewrote the accumulators leaves the
    /// stacker failed: it refuses more frames and a second `end_pass_one`.
    #[test]
    fn end_pass_one_failing_late_refuses_further_use() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let info = StackInfo { total_frames: 2, is_linear: false };
        let mut stacker = DepthStacker::new(ctx, DepthConfig::default(), info);
        let frame = image::DynamicImage::new_rgb8(64, 48);
        stacker.accumulate(&frame, None).unwrap();
        // With the frame's work done and `depth_index` destroyed, the
        // finalization fails after changing the accumulators.
        let State::Accumulating(r) = &stacker.state else { unreachable!() };
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        r.depth_index.destroy();
        assert!(stacker.end_pass_one().is_err());
        assert!(matches!(stacker.state, State::Failed));
        assert!(matches!(stacker.end_pass_one(), Err(CoreError::InvalidState(_))));
        assert!(matches!(stacker.accumulate(&frame, None), Err(CoreError::InvalidState(_))));
        assert!(stacker.render_map(None, wgpu::TextureFormat::Rgba8Unorm).is_err());
    }
}
