//! The 8-bit rendering of the stackers' live pictures (the pyramid's
//! winner record, the depth map, the wavelet's winners and frame map), one
//! kernel for all of them.

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{create_bind_group, create_compute_pipeline, dispatch_compute, tex};
use wgpu::Texture;

use crate::error::{CoreError, Result};

/// How a map rendered smaller reduces each pixel's footprint (`map.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapReduce {
    /// The box average: the depth map, whose in-between values are blends.
    Average,
    /// The center texel: a map naming one frame a pixel, where an average
    /// of two planes would name a third frame.
    Pick,
}

mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct MapParams {
        pub(super) reduce: u32,
    }
}

/// The map kernel: Rgba8Unorm output always, R8Unorm where the adapter
/// stores it (`GpuContext::map_format`).
pub(crate) struct MapPipelines {
    rgba8: wgpu::ComputePipeline,
    r8: Option<wgpu::ComputePipeline>,
    params: wgpu::Buffer,
}

impl MapPipelines {
    pub fn new(ctx: &GpuContext, reduce: MapReduce) -> Self {
        let module = ctx.create_shader_module(include_str!("map.wgsl"), "map");
        let reduce = match reduce {
            MapReduce::Average => 0,
            MapReduce::Pick => 1,
        };
        Self {
            rgba8: create_compute_pipeline(&ctx.device, &module, "map_to_rgba8"),
            r8: ctx
                .format_support
                .r8_storage
                .then(|| create_compute_pipeline(&ctx.device, &module, "map_to_r8")),
            params: ctx.create_uniform_buffer(&params::MapParams { reduce }, "map_params"),
        }
    }

    fn for_format(&self, format: wgpu::TextureFormat) -> Result<&wgpu::ComputePipeline> {
        match format {
            wgpu::TextureFormat::Rgba8Unorm => Ok(&self.rgba8),
            wgpu::TextureFormat::R8Unorm => self.r8.as_ref().ok_or_else(|| {
                CoreError::InvalidState("this GPU cannot store R8Unorm; use map_format()".into())
            }),
            other => Err(CoreError::InvalidState(format!("{other:?} is not a map format"))),
        }
    }

    /// Render `map` to a new 8-bit texture in `format`, its longest side
    /// capped at `max_side` (`None`: full size).
    pub fn render(
        &self,
        ctx: &GpuContext,
        map: &Texture,
        max_side: Option<u32>,
        format: wgpu::TextureFormat,
    ) -> Result<(Texture, u32, u32)> {
        let pipeline = self.for_format(format)?;
        let (width, height) = map_size(map.width(), map.height(), max_side);
        let output = ctx.create_texture(
            width,
            height,
            format,
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "map_u8",
        );
        let (view, output_view) =
            (map.create_view(&Default::default()), output.create_view(&Default::default()));
        let resources = [tex(&view), tex(&output_view), self.params.as_entire_binding()];
        let bind_group = create_bind_group(&ctx.device, pipeline, 0, "map_u8", &resources);
        let mut encoder = ctx.encoder("map_u8");
        dispatch_compute(&mut encoder, pipeline, &bind_group, "map_u8", width, height);
        ctx.submit(encoder);
        Ok((output, width, height))
    }
}

/// The size a map renders at with its longest side capped; sides ≥ 1.
pub(crate) fn map_size(width: u32, height: u32, max_side: Option<u32>) -> (u32, u32) {
    match max_side {
        Some(side) if width.max(height) > side.max(1) => {
            let s = side.max(1) as f64 / width.max(height) as f64;
            (
                ((width as f64 * s).round() as u32).max(1),
                ((height as f64 * s).round() as u32).max(1),
            )
        }
        _ => (width, height),
    }
}

#[cfg(test)]
mod tests {
    use super::map_size;

    #[test]
    fn map_size_caps_the_longest_side() {
        assert_eq!(map_size(7000, 6300, None), (7000, 6300));
        assert_eq!(map_size(800, 600, Some(1024)), (800, 600));
        assert_eq!(map_size(8192, 5464, Some(1024)), (1024, 683));
        assert_eq!(map_size(100, 4000, Some(1024)), (26, 1024));
        assert_eq!(map_size(5000, 1, Some(10)), (10, 1));
    }
}
