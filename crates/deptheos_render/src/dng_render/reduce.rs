//! The reduced copy of a source: long side at most [`REDUCED_LONG`], in
//! 16-bit float, each texel the mean of the area of the source it covers in
//! linear values, so its level is the settled picture's (ADR-135). A
//! standard stack's codes are decoded first; drawn with
//! [`Constants::reduced`].

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{
    WORKGROUP_SIZE, create_bind_group, create_compute_pipeline, dispatch_compute_with, tex,
};

use super::{Constants, Kind, lut_buffer};

/// The reduced copy's longest side.
pub const REDUCED_LONG: u32 = 4096;

/// Its format: linear values with no clipping.
pub const REDUCED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

mod sizes {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    /// Mirror of `reduce.wgsl`'s `Sizes`.
    #[derive(Clone, Copy, Debug, encase::ShaderType)]
    pub(super) struct Sizes {
        pub(super) size: glam::UVec4,
        pub(super) flags: glam::UVec4,
    }
}

pub(super) struct Reduce {
    pipeline: wgpu::ComputePipeline,
}

impl Reduce {
    pub(super) fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(include_str!("reduce.wgsl"), "reduce", &[]);
        Self { pipeline: create_compute_pipeline(&ctx.device, &module, "reduce") }
    }
}

/// The reduced copy's size for a `width`×`height` source.
pub fn reduced_size(width: u32, height: u32) -> [u32; 2] {
    let long = width.max(height);
    if long <= REDUCED_LONG {
        return [width, height];
    }
    let scale = f64::from(REDUCED_LONG) / f64::from(long);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, REDUCED_LONG);
    [side(width), side(height)]
}

impl super::DngRenderPipeline {
    /// The reduced copy of `source`, stored pixels uploaded as `constants`
    /// draw them ([`Constants::linearizer`]).
    pub fn reduce(
        &self,
        ctx: &GpuContext,
        source: &wgpu::Texture,
        constants: &Constants,
    ) -> wgpu::Texture {
        let (width, height) = (source.width(), source.height());
        let [w, h] = reduced_size(width, height);
        let reduced = ctx.create_texture(
            w,
            h,
            REDUCED_FORMAT,
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "reduced_copy",
        );
        let decode = match &constants.kind {
            Kind::Standard(standard) if !standard.decode.is_empty() => Some(&standard.decode[..]),
            _ => None,
        };
        let buffer = lut_buffer(ctx, decode.unwrap_or(&[]), "reduce_decode");
        let sizes = sizes::Sizes {
            size: glam::UVec4::new(width, height, w, h),
            flags: glam::UVec4::new(u32::from(decode.is_some()), 0, 0, 0),
        };
        let uniform = ctx.create_uniform_buffer(&sizes, "reduce_sizes");
        let (in_view, out_view) =
            (source.create_view(&Default::default()), reduced.create_view(&Default::default()));
        let group = create_bind_group(
            &ctx.device,
            &self.reduce.pipeline,
            0,
            "reduce",
            &[
                tex(&in_view),
                tex(&out_view),
                buffer.as_entire_binding(),
                uniform.as_entire_binding(),
            ],
        );
        let mut encoder = ctx.encoder("reduce");
        dispatch_compute_with(
            &mut encoder,
            &self.reduce.pipeline,
            &[&group],
            "reduce",
            w,
            h,
            WORKGROUP_SIZE,
        );
        ctx.submit(encoder);
        reduced
    }
}

impl Constants {
    /// These constants drawing from the reduced copy
    /// ([`super::DngRenderPipeline::reduce`]), whose values are already
    /// linear.
    pub fn reduced(mut self) -> Self {
        if let Kind::Standard(standard) = &mut self.kind {
            standard.uniform.flags.x = 1;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_long_side_is_at_most_4096() {
        assert_eq!(reduced_size(8256, 5504), [4096, 2731]);
        assert_eq!(reduced_size(5504, 8256), [2731, 4096]);
        assert_eq!(reduced_size(4096, 100), [4096, 100]);
        assert_eq!(reduced_size(3000, 2000), [3000, 2000]);
        assert_eq!(reduced_size(100_000, 1), [4096, 1]);
    }
}
