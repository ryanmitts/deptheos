//! Scalar maps the stackers update in place ([`ScalarMap`]): `f32`s in a
//! row-major storage buffer, not a read-write storage texture, which the
//! iOS Simulator's Metal device lacks.

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{create_bind_group, create_compute_pipeline, dispatch_compute};

/// A `width`×`height` map of `f32`, row-major in one storage buffer.
pub(crate) struct ScalarMap {
    pub buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
}

/// Bytes per pixel of a [`ScalarMap`].
pub(crate) const SCALAR_BYTES: u64 = 4;

impl ScalarMap {
    /// A new map, uncleared.
    pub fn new(ctx: &GpuContext, width: u32, height: u32, label: &str) -> Self {
        let size = u64::from(width) * u64::from(height) * SCALAR_BYTES;
        let buffer = ctx.create_storage_buffer(label, size, wgpu::BufferUsages::COPY_DST);
        Self { buffer, width, height }
    }

    pub fn binding(&self) -> wgpu::BindingResource<'_> {
        self.buffer.as_entire_binding()
    }
}

mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct FillParams {
        pub(super) value: f32,
        pub(super) width: u32,
    }
}

/// Sets every value of a [`ScalarMap`] to one not zero (`scalar.wgsl`).
pub(crate) struct FillPipeline {
    pipeline: wgpu::ComputePipeline,
}

impl FillPipeline {
    pub fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module(include_str!("scalar.wgsl"), "scalar");
        Self { pipeline: create_compute_pipeline(&ctx.device, &module, "fill") }
    }

    /// Encode setting every value of `map` to `value`.
    pub fn encode(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        map: &ScalarMap,
        value: f32,
    ) {
        let params =
            ctx.create_uniform_buffer(&params::FillParams { value, width: map.width }, "fill");
        let bg = create_bind_group(
            &ctx.device,
            &self.pipeline,
            0,
            "fill",
            &[map.binding(), params.as_entire_binding()],
        );
        dispatch_compute(encoder, &self.pipeline, &bg, "fill", map.width, map.height);
    }
}
