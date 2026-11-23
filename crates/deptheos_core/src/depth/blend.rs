//! Pass 2: each frame's Laplacian pyramid blended per level by depth
//! proximity, then collapsed into the result.

use deptheos_gpu::gpu_utils::{
    create_bind_group, dispatch_compute, dispatch_compute_textures, dispatch_compute_with, tex,
};
use tracing::{debug, info};
use wgpu::Texture;

use super::curves::blend_pyramid_levels;
use super::params::BlendParams;
use super::{AccumResources, DepthStacker, State};
use crate::accum::{FinishBuffers, create_accum, level_sizes};
use crate::error::{CoreError, Result};
use crate::ledger::SecondPass;
use crate::transform::{FrameAlignment, WarpParams};
use crate::upload::{FrameInput, Source};

pub(super) struct BlendResources {
    /// Which frames have been blended.
    pass: SecondPass,
    width: u32,
    height: u32,
    /// Full-resolution depth map, level 0 of the depth pyramid.
    pub(super) depth_index: Texture,
    source: Source, // reused uploader target from accumulation
    /// Frame Gaussian pyramid; the collapse reuses it, level 0 the result.
    gauss: Vec<Texture>,
    /// Accumulator pyramid: weighted Laplacian sums (Gaussian residual on
    /// top), weight in alpha, row-major (`deptheos::accum`).
    acc: Vec<wgpu::Buffer>,
    warp_bg: wgpu::BindGroup,
    warp_params_buffer: wgpu::Buffer,  // rewritten each frame
    down_bgs: Vec<wgpu::BindGroup>,    // gauss[l] → gauss[l+1]
    blend_bgs: Vec<wgpu::BindGroup>,   // levels below the top, with `blend_level`
    blend_top_bg: wgpu::BindGroup,     // the top level, with `blend_top`
    blend_params_buffer: wgpu::Buffer, // rewritten each frame
}

impl DepthStacker {
    /// Set up the blend phase once `finished`, the map's finalization, is
    /// done, keeping only the full-resolution depth map and the source.
    pub(super) fn begin_blend(
        &mut self,
        accum: AccumResources,
        depth_full: Texture,
        finished: wgpu::SubmissionIndex,
    ) -> Result<()> {
        // Pass 1's scratch drops before the pyramids allocate; wgpu frees it
        // only once the finalization using it is done, so wait for that.
        let (width, height, source) = accum.into_blend_inputs();
        self.ctx.wait_for(finished)?;
        let levels = blend_pyramid_levels(width, height);
        info!("depth blend: {levels}-level pyramid");

        let (gauss, acc, depth_pyramid) = self.blend_targets(width, height, levels);
        self.build_depth_pyramid(&depth_full, &depth_pyramid, &acc);

        let device = &self.ctx.device;
        let p = &self.pipelines;
        let view = |t: &Texture| t.create_view(&Default::default());
        let gauss_v: Vec<_> = gauss.iter().map(view).collect();
        let mut depth_v = vec![view(&depth_full)];
        depth_v.extend(depth_pyramid.iter().map(view));

        let warp_params_buffer =
            self.ctx.create_uniform_buffer(&WarpParams::identity(false), "blend_warp_params");
        let warp_bg = create_bind_group(
            device,
            &p.blend_warp,
            0,
            "blend_warp_bg",
            &[
                tex(&view(source.texture())),
                tex(&gauss_v[0]),
                warp_params_buffer.as_entire_binding(),
            ],
        );
        let down_bgs = (0..levels - 1)
            .map(|l| {
                create_bind_group(
                    device,
                    &p.gauss_down2,
                    0,
                    "blend_down_bg",
                    &[tex(&gauss_v[l]), tex(&gauss_v[l + 1])],
                )
            })
            .collect();
        let blend_params_buffer = self.ctx.create_uniform_buffer(
            &BlendParams { scaled_frame_idx: 0.0, dist_below: 0.0, dist_above: 0.0, _pad: 0.0 },
            "blend_params",
        );
        let top = levels - 1;
        let blend_bgs = (0..top)
            .map(|l| {
                create_bind_group(
                    device,
                    &p.blend_level,
                    0,
                    "blend_level_bg",
                    &[
                        tex(&gauss_v[l]),
                        tex(&gauss_v[l + 1]),
                        tex(&depth_v[l]),
                        acc[l].as_entire_binding(),
                        blend_params_buffer.as_entire_binding(),
                    ],
                )
            })
            .collect();
        let blend_top_bg = create_bind_group(
            device,
            &p.blend_top,
            0,
            "blend_top_bg",
            &[
                tex(&gauss_v[top]),
                tex(&depth_v[top]),
                acc[top].as_entire_binding(),
                blend_params_buffer.as_entire_binding(),
            ],
        );

        self.state = State::Blending(BlendResources {
            pass: SecondPass::new(&self.intake.frames),
            width,
            height,
            depth_index: depth_full,
            source,
            gauss,
            acc,
            warp_bg,
            warp_params_buffer,
            down_bgs,
            blend_bgs,
            blend_top_bg,
            blend_params_buffer,
        });
        Ok(())
    }

    /// The blend's `levels`-level Gaussian pyramid, its accumulators, and
    /// levels 1.. of the depth pyramid (level 0 is the full-size map).
    fn blend_targets(
        &self,
        width: u32,
        height: u32,
        levels: usize,
    ) -> (Vec<Texture>, Vec<wgpu::Buffer>, Vec<Texture>) {
        let rgba_usage =
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let mut gauss = Vec::with_capacity(levels);
        let mut acc = Vec::with_capacity(levels);
        let mut depth_pyramid = Vec::with_capacity(levels - 1);
        for (level, (w, h)) in level_sizes(width, height, levels).enumerate() {
            gauss.push(self.ctx.create_texture(
                w,
                h,
                wgpu::TextureFormat::Rgba32Float,
                rgba_usage | wgpu::TextureUsages::COPY_SRC,
                &format!("blend_gauss_{level}"),
            ));
            acc.push(create_accum(&self.ctx, w, h, &format!("blend_acc_{level}")));
            if level > 0 {
                depth_pyramid.push(self.ctx.create_texture(
                    w,
                    h,
                    wgpu::TextureFormat::R32Float,
                    rgba_usage,
                    &format!("blend_depth_{level}"),
                ));
            }
        }
        (gauss, acc, depth_pyramid)
    }

    /// Build the depth pyramid once, down from `depth_full`, and clear the
    /// accumulators.
    fn build_depth_pyramid(
        &self,
        depth_full: &Texture,
        depth_pyramid: &[Texture],
        acc: &[wgpu::Buffer],
    ) {
        let mut encoder = self.ctx.encoder("depth_begin_blend");
        for level in 1..=depth_pyramid.len() {
            let src = if level == 1 { depth_full } else { &depth_pyramid[level - 2] };
            let dst = &depth_pyramid[level - 1];
            let size = dst.size();
            dispatch_compute_textures(
                &mut encoder,
                &self.ctx.device,
                &self.pipelines.box_down2_r,
                "box_down2_r",
                &[src, dst],
                &[],
                size.width,
                size.height,
            );
        }
        for a in acc {
            encoder.clear_buffer(a, 0, None);
        }
        self.ctx.submit(encoder);
    }

    /// Blend one frame, `frame_index` being its place in pass 1. The blend
    /// is a sum weighted by index alone, so frames may come in any order,
    /// each once.
    pub fn blend_frame<'a>(
        &mut self,
        frame: impl Into<FrameInput<'a>>,
        frame_index: u32,
        alignment: Option<&FrameAlignment>,
    ) -> Result<()> {
        let frame = frame.into();
        let State::Blending(r) = &mut self.state else {
            return Err(CoreError::InvalidState("blend_frame() requires Blending state".into()));
        };
        let ramp = self.intake.frames.plane_ramp(frame_index);
        let prep = self.intake.second_pass(&self.ctx, frame, frame_index, &r.pass, &r.source)?;
        self.ctx.write_uniform_buffer(
            &r.blend_params_buffer,
            &BlendParams {
                scaled_frame_idx: ramp.plane,
                dist_below: ramp.below,
                dist_above: ramp.above,
                _pad: 0.0,
            },
        );
        // No sRGB step: the blend keeps linear input linear. Brightness
        // applies here.
        self.ctx
            .write_uniform_buffer(&r.warp_params_buffer, &WarpParams::for_frame(alignment, false));

        let mut encoder = self.ctx.encoder("depth_blend_frame");
        prep.dispatch_to_rgba(&self.ctx, &mut encoder, r.source.texture());
        dispatch_compute(
            &mut encoder,
            &self.pipelines.blend_warp,
            &r.warp_bg,
            "blend_warp",
            r.width,
            r.height,
        );
        for (l, bg) in r.down_bgs.iter().enumerate() {
            let size = r.gauss[l + 1].size();
            // 8×8 workgroups (`gauss_down2` in the shader).
            dispatch_compute_with(
                &mut encoder,
                &self.pipelines.gauss_down2,
                &[bg],
                "gauss_down2",
                size.width,
                size.height,
                8,
            );
        }
        let p = &self.pipelines;
        for (l, bg) in r.blend_bgs.iter().enumerate() {
            let size = r.gauss[l].size();
            dispatch_compute(
                &mut encoder,
                &p.blend_level,
                bg,
                "blend_level",
                size.width,
                size.height,
            );
        }
        let top = r.gauss.last().expect("blend pyramid has at least one level").size();
        dispatch_compute(
            &mut encoder,
            &p.blend_top,
            &r.blend_top_bg,
            "blend_top",
            top.width,
            top.height,
        );
        self.ctx.submit(encoder);
        r.pass.mark(frame_index);
        Ok(())
    }

    /// Normalize the accumulators and collapse the pyramid into the result.
    pub fn finalize(&mut self) -> Result<()> {
        let State::Blending(blend) = &self.state else {
            return Err(CoreError::InvalidState("finalize() requires Blending state".into()));
        };
        // A frame not blended would leave its share of the picture empty.
        blend.pass.check_complete(&self.intake.frames)?;
        // A frame's earlier failure leaves the stacker blending.
        self.ctx.check()?;
        let State::Blending(blend) = self.state.take() else {
            unreachable!("checked above");
        };
        // No frame comes after: the upload buffer goes with the blend.
        self.intake.finish();

        let device = &self.ctx.device;
        let p = &self.pipelines;
        let mut encoder = self.ctx.encoder("depth_finalize_blend");

        // The blend keeps the source's encoding: no decode at the finish.
        let finish = FinishBuffers::new(&self.ctx, false);
        let top = blend.acc.len() - 1;

        // The top accumulator, normalized, into the frame pyramid's top
        // level; for a single level that is the result.
        {
            let size = blend.gauss[top].size();
            dispatch_compute_textures(
                &mut encoder,
                device,
                &p.blend_resolve_top,
                "blend_resolve_top",
                &[&blend.gauss[top]],
                &[blend.acc[top].as_entire_binding(), finish.for_level(top).as_entire_binding()],
                size.width,
                size.height,
            );
        }

        // Collapse coarse → fine into the frame pyramid.
        for l in (0..top).rev() {
            let dst = &blend.gauss[l];
            let size = dst.size();
            dispatch_compute_textures(
                &mut encoder,
                device,
                &p.collapse_level,
                "collapse_level",
                &[&blend.gauss[l + 1], dst],
                &[blend.acc[l].as_entire_binding(), finish.for_level(l).as_entire_binding()],
                size.width,
                size.height,
            );
        }
        self.ctx.submit(encoder);
        // The collapse wrote over the frame pyramid, so a failure is final.
        if let Err(e) = self.ctx.check() {
            self.state = State::Failed;
            debug!("depth stacker: failed");
            return Err(e.into());
        }

        let result = blend.gauss.into_iter().next().expect("blend pyramid has at least one level");

        self.state = State::Finalized { result, depth_map: blend.depth_index };
        debug!("depth stacker: finalized");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StackInfo;
    use crate::depth::DepthConfig;
    use deptheos_gpu::GpuContext;

    /// A depth stacker blending a stack of two blank frames, the first
    /// blended; the frame.
    fn blending() -> Option<(DepthStacker, image::DynamicImage)> {
        let ctx = GpuContext::new_or_skip()?;
        let info = StackInfo { total_frames: 2, is_linear: false };
        let mut stacker = DepthStacker::new(ctx, DepthConfig::default(), info);
        let frame = image::DynamicImage::new_rgb8(64, 48);
        stacker.accumulate(&frame, None).unwrap();
        stacker.accumulate(&frame, None).unwrap();
        stacker.end_pass_one().unwrap();
        stacker.blend_frame(&frame, 0, None).unwrap();
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        Some((stacker, frame))
    }

    /// An earlier frame's failure, reported by `finalize`, leaves the
    /// stacker blending: nothing is collapsed.
    #[test]
    fn an_earlier_frame_s_failure_leaves_it_blending() {
        let Some((mut stacker, frame)) = blending() else { return };
        let State::Blending(r) = &stacker.state else { unreachable!() };
        r.source.texture().destroy();
        // The frame's work fails on the GPU, unchecked until `finalize`.
        stacker.blend_frame(&frame, 1, None).unwrap();
        assert!(stacker.finalize().is_err());
        assert!(matches!(stacker.state, State::Blending(_)));
    }

    /// A failure in the collapse leaves the stacker failed: it refuses a
    /// second `finalize` and more frames, and has no result.
    #[test]
    fn finalize_failing_late_refuses_further_use() {
        let Some((mut stacker, frame)) = blending() else { return };
        stacker.blend_frame(&frame, 1, None).unwrap();
        let State::Blending(r) = &stacker.state else { unreachable!() };
        stacker.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        r.acc[0].destroy();
        assert!(stacker.finalize().is_err());
        assert!(matches!(stacker.state, State::Failed));
        assert!(matches!(stacker.finalize(), Err(CoreError::InvalidState(_))));
        assert!(matches!(stacker.blend_frame(&frame, 1, None), Err(CoreError::InvalidState(_))));
        assert!(stacker.result_texture().is_none());
        assert!(stacker.render_live(64, wgpu::TextureFormat::Rgba8Unorm).is_err());
    }
}
