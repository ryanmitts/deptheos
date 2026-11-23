use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{create_compute_pipeline, create_tiled_compute_pipeline};
use wgpu::ComputePipeline;

use crate::map::{MapPipelines, MapReduce};
use crate::scalar::FillPipeline;

pub struct PyramidPipelines {
    pub warp_rgba: ComputePipeline,
    pub downsample: ComputePipeline,
    pub laplacian_add: ComputePipeline,
    pub focus_wta: ComputePipeline,
    pub accumulate_sum: ComputePipeline,
    pub normalize: ComputePipeline,
    pub prior_downsample: ComputePipeline,
    /// The picture while a stack runs: the winner record's planes.
    pub live_map: MapPipelines,
    /// Starts the running bests at `FOCUS_SENTINEL`.
    pub fill: FillPipeline,
}

impl PyramidPipelines {
    pub fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(
            include_str!("pyramid.wgsl"),
            "pyramid",
            crate::pipelines::SHARED_SHADER_MODULES,
        );
        let device = &ctx.device;
        let make = |name| create_compute_pipeline(device, &module, name);

        Self {
            warp_rgba: make("warp_rgba"),
            // Tiled kernel: every tile entry is written before it is read.
            downsample: create_tiled_compute_pipeline(device, &module, "downsample"),
            laplacian_add: make("laplacian_add"),
            // Tiled kernel: every tile entry it reads is written first.
            focus_wta: create_tiled_compute_pipeline(device, &module, "focus_wta"),
            accumulate_sum: make("accumulate_sum"),
            normalize: make("normalize_coarsest"),
            prior_downsample: make("prior_downsample"),
            live_map: MapPipelines::new(ctx, MapReduce::Pick),
            fill: FillPipeline::new(ctx),
        }
    }
}
