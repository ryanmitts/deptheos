//! The reduced copy of a source: long side at most [`REDUCED_LONG`], in
//! 16-bit float, linear, each texel the mean of the area of the source it
//! covers taken in values as they show, as the viewer's levels take the
//! mean of display values, so its level is the settled picture's
//! (ADR-138): a standard stack's through sRGB's curve, a linear stack's
//! through its own render at neutral ([`Encoding`]). A standard stack's
//! codes are decoded first; drawn with [`Constants::reduced`].

use std::sync::Arc;

use deptheos_core::upload::ImageUploader;
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{
    WORKGROUP_SIZE, create_bind_group, create_compute_pipeline, dispatch_compute_with,
    download_texture_with, tex,
};

use super::{Constants, Kind, lut_buffer};

/// The reduced copy's longest side.
pub const REDUCED_LONG: u32 = 4096;

/// Its format: linear values with no clipping.
pub const REDUCED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Samples of a linear stack's [`Encoding::Response`].
const RESPONSE_SAMPLES: u32 = 1024;

mod sizes {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    /// Mirror of `reduce.wgsl`'s `Sizes`.
    #[derive(Clone, Copy, Debug, encase::ShaderType)]
    pub(super) struct Sizes {
        pub(super) size: glam::UVec4,
        pub(super) flags: glam::UVec4,
        pub(super) white: glam::Vec4,
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

/// How the reduced copy takes its means: in values as they show.
#[derive(Clone, Debug)]
pub enum Encoding {
    /// Through sRGB's curve: a standard stack's, whose stored values are
    /// already the ones shown.
    Srgb,
    /// Through a linear stack's own render, its per-pixel stages: how a
    /// gray of each level from black to the camera's white (`white`, per
    /// channel) shows, rising from 0 at black.
    Response { white: [f32; 3], table: Arc<Vec<f32>> },
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
    /// How a reduced copy of a source `constants` draw takes its means: a
    /// linear stack's response is a gray ramp drawn through their
    /// per-pixel stages.
    pub fn encoding(&self, ctx: &GpuContext, constants: &Constants) -> Encoding {
        let Kind::Linear { uniform, .. } = &constants.kind else { return Encoding::Srgb };
        let of = constants.per_pixel();
        let white = uniform.camera_white.truncate().to_array();
        let n = RESPONSE_SAMPLES;
        let ramp: Vec<u16> = (0..n)
            .flat_map(|i| {
                let x = i as f32 / (n - 1) as f32;
                white.map(|w| (x * w * 65535.0).round() as u16)
            })
            .collect();
        let mut uploader = ImageUploader::new(ctx);
        let prepared = uploader.prepare_rgb16(ctx, &ramp, n, 1).expect("a ramp uploads");
        let input = ImageUploader::create_target(ctx, n, 1, "response_ramp");
        let mut encoder = ctx.encoder("response_ramp");
        prepared.dispatch_to_rgba(ctx, &mut encoder, &input);
        ctx.submit(encoder);
        let shown = self.render(ctx, &input, &of);
        let mut bytes = Vec::new();
        download_texture_with(ctx, &shown, [0, 0], [n, 1], |row| bytes.extend_from_slice(row))
            .expect("read back the ramp");
        let texel = bytes.len() / n as usize;
        let channel = |t: &[u8], c: usize| match texel {
            8 => f32::from(u16::from_le_bytes([t[2 * c], t[2 * c + 1]])) / 65535.0,
            _ => f32::from_le_bytes(t[4 * c..][..4].try_into().unwrap()),
        };
        // The mean of the three, made to rise strictly so it inverts.
        let mut table = Vec::with_capacity(n as usize);
        let mut last = -1.0f32;
        for (i, t) in bytes.chunks_exact(texel).enumerate() {
            let shown = (channel(t, 0) + channel(t, 1) + channel(t, 2)) / 3.0;
            let rising = if i == 0 { 0.0 } else { shown.max(last + 1e-6) };
            table.push(rising);
            last = rising;
        }
        Encoding::Response { white, table: Arc::new(table) }
    }

    /// The reduced copy of `source`, stored pixels uploaded as `constants`
    /// draw them ([`Constants::linearizer`]), its means taken through
    /// `encoding`.
    pub fn reduce(
        &self,
        ctx: &GpuContext,
        source: &wgpu::Texture,
        constants: &Constants,
        encoding: &Encoding,
    ) -> wgpu::Texture {
        let decode = match &constants.kind {
            Kind::Standard(standard) if !standard.decode.is_empty() => Some(&standard.decode[..]),
            _ => None,
        };
        let [w, h] = reduced_size(source.width(), source.height());
        self.resample_into(ctx, source, [w, h], decode, Means::Shown(encoding))
    }

    /// `source`'s linear values at `size`, each texel taking the mean of
    /// what it covers through `encoding`, as [`Self::reduce`] does: the fit
    /// preview's copy of the reduced copy (ADR-138).
    pub fn resample(
        &self,
        ctx: &GpuContext,
        source: &wgpu::Texture,
        size: [u32; 2],
        encoding: &Encoding,
    ) -> wgpu::Texture {
        self.resample_into(ctx, source, size, None, Means::Shown(encoding))
    }

    /// The local tone's values at `size`, each texel the plain mean of
    /// what it covers, all four channels.
    pub fn resample_planes(
        &self,
        ctx: &GpuContext,
        source: &wgpu::Texture,
        size: [u32; 2],
    ) -> wgpu::Texture {
        self.resample_into(ctx, source, size, None, Means::Plain)
    }

    fn resample_into(
        &self,
        ctx: &GpuContext,
        source: &wgpu::Texture,
        [w, h]: [u32; 2],
        decode: Option<&[f32]>,
        means: Means<'_>,
    ) -> wgpu::Texture {
        let (width, height) = (source.width(), source.height());
        let reduced = ctx.create_texture(
            w,
            h,
            REDUCED_FORMAT,
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "reduced_copy",
        );
        let buffer = lut_buffer(ctx, decode.unwrap_or(&[]), "reduce_decode");
        let (mode, white, table) = match means {
            Means::Plain => (1, [1.0; 3], &[][..]),
            Means::Shown(Encoding::Srgb) => (0, [1.0; 3], &[][..]),
            Means::Shown(Encoding::Response { white, table, .. }) => (2, *white, &table[..]),
        };
        let table_buffer = lut_buffer(ctx, table, "reduce_response");
        let sizes = sizes::Sizes {
            size: glam::UVec4::new(width, height, w, h),
            flags: glam::UVec4::new(u32::from(decode.is_some()), mode, table.len() as u32, 0),
            white: glam::Vec4::new(white[0], white[1], white[2], 0.0),
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
                table_buffer.as_entire_binding(),
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

/// How a resample takes its means.
enum Means<'a> {
    /// Of values as they show.
    Shown(&'a Encoding),
    /// Of the values themselves, all four channels: planes.
    Plain,
}

impl Constants {
    /// These constants' stages that read no neighbors and no position: no
    /// local tone and no vignette.
    fn per_pixel(&self) -> Self {
        let mut constants = self.clone();
        let local = super::stages::TONE_TERM
            | super::stages::CLARITY_TERM
            | super::stages::TEXTURE_TERM
            | super::stages::SHARPEN_TERM;
        let (flags, vignette) = match &mut constants.kind {
            Kind::Linear { uniform, .. } => (&mut uniform.flags, &mut uniform.vignette),
            Kind::Standard(standard) => {
                (&mut standard.uniform.flags, &mut standard.uniform.vignette)
            }
        };
        flags.z &= !local;
        flags.w &= !16;
        *vignette = glam::Vec4::ZERO;
        constants.local = None;
        constants.without_window()
    }

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
