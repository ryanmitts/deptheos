//! Contribution over a frame: a source frame with what it supplied to the
//! result kept and the rest dim gray ([`UNLIT`]), no color laid over.
//!
//! The share comes from the stack's frame map ([`crate::FramePlane`]
//! values, [`PlaneRamp::share`]). The map is 16-bit: 8 bits tell
//! neighboring frames apart only up to about 128 frames.

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{create_compute_pipeline, dispatch_compute_textures};
use image::{ImageBuffer, Luma};
use wgpu::Texture;

use crate::pipelines::SHARED_SHADER_MODULES;
use crate::plane::{HALF_LEVEL, PlaneRamp, UNLIT};

mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct ContributionParams {
        pub(super) plane: f32,
        pub(super) below: f32,
        pub(super) above: f32,
        pub(super) unlit: f32,
        pub(super) half_level: f32,
    }
}
use params::*;

pub struct ContributionOverlay {
    pipeline: wgpu::ComputePipeline,
}

impl ContributionOverlay {
    pub fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(
            include_str!("contribution.wgsl"),
            "contribution",
            SHARED_SHADER_MODULES,
        );
        Self { pipeline: create_compute_pipeline(&ctx.device, &module, "contribution_light") }
    }

    /// `image` (a compare source, the result's size) lit for `frame` from
    /// `map`, the 16-bit frame map. A new texture in the image's format.
    pub fn light(
        &self,
        ctx: &GpuContext,
        image: &Texture,
        map: &ImageBuffer<Luma<u16>, Vec<u16>>,
        frame: PlaneRamp,
    ) -> Texture {
        let map = upload_map(ctx, map);
        let output = ctx.create_texture(
            image.width(),
            image.height(),
            ctx.source_format(),
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "contribution_lit",
        );
        let params = ctx.create_uniform_buffer(
            &ContributionParams {
                plane: frame.plane,
                below: frame.below,
                above: frame.above,
                unlit: UNLIT,
                half_level: HALF_LEVEL,
            },
            "contribution_light",
        );
        let mut encoder = ctx.encoder("contribution_light");
        dispatch_compute_textures(
            &mut encoder,
            &ctx.device,
            &self.pipeline,
            "contribution_light",
            &[image, &map, &output],
            &[params.as_entire_binding()],
            output.width(),
            output.height(),
        );
        ctx.submit(encoder);
        output
    }
}

/// `map` as R16Uint, scaled to [0, 1] in the shader: R16Unorm is not a core
/// sampled format.
fn upload_map(ctx: &GpuContext, map: &ImageBuffer<Luma<u16>, Vec<u16>>) -> Texture {
    let (width, height) = map.dimensions();
    let texture = ctx.create_texture(
        width,
        height,
        wgpu::TextureFormat::R16Uint,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "frame_map",
    );
    ctx.queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(map.as_raw()),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 2),
            rows_per_image: None,
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    texture
}
