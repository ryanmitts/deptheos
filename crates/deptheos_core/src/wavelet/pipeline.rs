use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::create_compute_pipeline;
use wgpu::ComputePipeline;

use crate::map::{MapPipelines, MapReduce};

pub struct WaveletPipelines {
    pub ingest: ComputePipeline,
    pub forward: ComputePipeline,
    pub wta: ComputePipeline,
    pub ll_accumulate: ComputePipeline,
    pub ll_normalize: ComputePipeline,
    pub subband_consistency: ComputePipeline,
    pub snapshot_frames: ComputePipeline,
    pub neighbor_consistency: ComputePipeline,
    pub denoise: ComputePipeline,
    pub inverse_vertical: ComputePipeline,
    pub inverse_horizontal: ComputePipeline,
    pub extract_real: ComputePipeline,
    pub reassign_init: ComputePipeline,
    pub reassign: ComputePipeline,
    pub finish: ComputePipeline,
    pub live_planes: ComputePipeline,
    /// The picture while a stack runs: the winners' planes.
    pub live_map: MapPipelines,
}

impl WaveletPipelines {
    pub fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(
            include_str!("wavelet.wgsl"),
            "wavelet",
            crate::pipelines::SHARED_SHADER_MODULES,
        );
        let make = |name| create_compute_pipeline(&ctx.device, &module, name);
        Self {
            ingest: make("ingest"),
            forward: make("forward"),
            wta: make("wta"),
            ll_accumulate: make("ll_accumulate"),
            ll_normalize: make("ll_normalize"),
            subband_consistency: make("subband_consistency"),
            snapshot_frames: make("snapshot_frames"),
            neighbor_consistency: make("neighbor_consistency"),
            denoise: make("denoise"),
            inverse_vertical: make("inverse_vertical"),
            inverse_horizontal: make("inverse_horizontal"),
            extract_real: make("extract_real"),
            reassign_init: make("reassign_init"),
            reassign: make("reassign"),
            finish: make("finish"),
            live_planes: make("live_planes"),
            live_map: MapPipelines::new(ctx, MapReduce::Pick),
        }
    }
}
