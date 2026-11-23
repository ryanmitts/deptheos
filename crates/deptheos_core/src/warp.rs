//! Standalone GPU warp: bring a whole texture into a stack's geometry.
//!
//! The stackers don't keep their warped frames, so a frame shown beside the
//! result is warped and cropped again here, in one dispatch.

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::dispatch_compute_textures;
use image::math::Rect;
use nalgebra::{Matrix3, Vector2};
use wgpu::Texture;

use crate::pipelines::SHARED_SHADER_MODULES;
use crate::transform::{Affine2, FrameAlignment, WarpParams};

pub struct TextureWarper {
    pipeline: wgpu::ComputePipeline,
}

impl TextureWarper {
    pub fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(
            include_str!("warp.wgsl"),
            "warp",
            SHARED_SHADER_MODULES,
        );
        let pipeline =
            deptheos_gpu::gpu_utils::create_compute_pipeline(&ctx.device, &module, "warp_texture");
        Self { pipeline }
    }

    /// Resample `source` through the alignment (reference → frame) and crop
    /// to `crop` (reference coordinates), into a new texture in
    /// `ctx.source_format()`, clamped to [0, 1], in the source's encoding.
    /// `None` is identity / no crop.
    ///
    /// The crop is folded into the transform: output pixel `c` samples
    /// `M · (c + origin)`.
    pub fn warp(
        &self,
        ctx: &GpuContext,
        source: &Texture,
        alignment: Option<&FrameAlignment>,
        crop: Option<Rect>,
    ) -> Texture {
        let (width, height, ox, oy) = match crop {
            Some(r) => (r.width, r.height, r.x as f32, r.y as f32),
            None => (source.width(), source.height(), 0.0, 0.0),
        };
        let base = alignment.map(|a| a.transform).unwrap_or_else(Affine2::identity);
        let shifted = Affine2::from_matrix_unchecked(
            base.matrix() * Matrix3::new_translation(&Vector2::new(ox, oy)),
        );
        let params = ctx.create_uniform_buffer(
            &WarpParams::from_alignment(
                &FrameAlignment::new(shifted, alignment.map_or(1.0, |a| a.brightness)),
                false,
            ),
            "warp_texture_params",
        );

        let output = ctx.create_texture(
            width,
            height,
            ctx.source_format(),
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "warped_texture",
        );
        let mut encoder = ctx.encoder("warp_texture");
        dispatch_compute_textures(
            &mut encoder,
            &ctx.device,
            &self.pipeline,
            "warp_texture",
            &[source, &output],
            &[params.as_entire_binding()],
            width,
            height,
        );
        ctx.submit(encoder);
        output
    }
}
