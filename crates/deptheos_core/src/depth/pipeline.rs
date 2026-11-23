use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{create_compute_pipeline, create_tiled_compute_pipeline};
use wgpu::ComputePipeline;

use crate::focus::FocusKernels;
use crate::map::{MapPipelines, MapReduce};
use crate::scalar::FillPipeline;

pub struct DepthPipelines {
    // Accumulation
    pub focus: FocusKernels,
    pub depth_wta_accumulate: ComputePipeline,

    // Finalization
    pub opacity_diff: ComputePipeline,
    pub histogram_accumulate: ComputePipeline,
    pub histogram_percentile: ComputePipeline,
    pub normalize_opacity: ComputePipeline,
    pub iir_sweep: ComputePipeline,
    pub depth_feather: ComputePipeline,
    pub upsample_bilinear: ComputePipeline,

    // Blend
    pub blend_warp: ComputePipeline,
    pub gauss_down2: ComputePipeline,
    pub box_down2_r: ComputePipeline,
    pub blend_level: ComputePipeline,
    pub blend_top: ComputePipeline,
    pub collapse_level: ComputePipeline,
    pub blend_resolve_top: ComputePipeline,

    // Visualization
    pub map: MapPipelines,

    /// Starts the WTA's best focus at −1.
    pub fill: FillPipeline,
}

impl DepthPipelines {
    pub fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(
            include_str!("depth.wgsl"),
            "depth",
            crate::pipelines::SHARED_SHADER_MODULES,
        );
        let device = &ctx.device;
        let make = |name| create_compute_pipeline(device, &module, name);

        Self {
            focus: FocusKernels::new(ctx, &module),
            depth_wta_accumulate: make("depth_wta_accumulate"),
            opacity_diff: make("opacity_diff"),
            histogram_accumulate: make("histogram_accumulate"),
            histogram_percentile: make("histogram_percentile"),
            normalize_opacity: make("normalize_opacity"),
            iir_sweep: make("iir_sweep"),
            depth_feather: make("depth_feather"),
            upsample_bilinear: make("upsample_bilinear"),
            blend_warp: make("blend_warp"),
            // Tiled kernel: every tile entry is written before it is read.
            gauss_down2: create_tiled_compute_pipeline(device, &module, "gauss_down2"),
            box_down2_r: make("box_down2_r"),
            blend_level: make("blend_level"),
            blend_top: make("blend_top"),
            collapse_level: make("collapse_level"),
            blend_resolve_top: make("blend_resolve_top"),
            map: MapPipelines::new(ctx, MapReduce::Average),
            fill: FillPipeline::new(ctx),
        }
    }
}
