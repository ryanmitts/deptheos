//! Clipping counted on the GPU over a drawn picture: how many pixels of a
//! window show each channel at 8-bit 0 or 255, from every pixel, without
//! reading the picture back.

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{
    WORKGROUP_SIZE, create_bind_group, create_compute_pipeline, dispatch_compute_with,
    download_buffer, tex,
};

use super::Region;

pub(super) struct Clip {
    pipeline: wgpu::ComputePipeline,
}

impl Clip {
    pub(super) fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(include_str!("clip.wgsl"), "clip", &[]);
        Self { pipeline: create_compute_pipeline(&ctx.device, &module, "clip") }
    }
}

/// Pixels of a window at 8-bit 0 (`shadows`) and 255 (`highlights`), per
/// channel, red first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clipped {
    pub shadows: [u32; 3],
    pub highlights: [u32; 3],
}

impl super::DngRenderPipeline {
    /// The clipping of `window` of `picture`, a drawn output in display
    /// values.
    pub fn clipped(
        &self,
        ctx: &GpuContext,
        picture: &wgpu::Texture,
        window: Region,
    ) -> deptheos_gpu::Result<Clipped> {
        let counts = ctx.create_storage_buffer(
            "clip_counts",
            6 * 4,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        ctx.queue.write_buffer(&counts, 0, &[0; 24]);
        let area = ctx.create_buffer_with_data(
            "clip_window",
            &[window.x, window.y, window.width, window.height]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
            wgpu::BufferUsages::UNIFORM,
        );
        let view = picture.create_view(&Default::default());
        let group = create_bind_group(
            &ctx.device,
            &self.clip.pipeline,
            0,
            "clip",
            &[tex(&view), counts.as_entire_binding(), area.as_entire_binding()],
        );
        let mut encoder = ctx.encoder("clip");
        dispatch_compute_with(
            &mut encoder,
            &self.clip.pipeline,
            &[&group],
            "clip",
            window.width,
            window.height,
            WORKGROUP_SIZE,
        );
        ctx.submit(encoder);
        let bytes = download_buffer(ctx, &counts, 24)?;
        let n = |i: usize| u32::from_le_bytes(bytes[4 * i..][..4].try_into().expect("4 bytes"));
        Ok(Clipped { shadows: [n(0), n(1), n(2)], highlights: [n(3), n(4), n(5)] })
    }
}
