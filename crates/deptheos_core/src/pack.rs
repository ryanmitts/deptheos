//! Exact u16 quantization on the GPU, and the result pack built on it.
//!
//! [`U16Packer`] crops and quantizes an rgba32float stack into one
//! rgba16unorm texture, half the bytes, for both display and readback,
//! rounding as [`crate::io::quantize_u16`] does bit for bit
//! (`shaders/quantize.wgsl`).
//!
//! Requires `FormatSupport::rgba16unorm_storage`; callers keep the
//! rgba32float path elsewhere.

use std::sync::OnceLock;

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{WORKGROUP_SIZE, create_bind_group, dispatch_compute_with, tex};
use glam::UVec4;
use image::math::Rect;

use crate::error::{CoreError, Result};
use crate::io::quantize_u16;

/// The `deptheos::quantize` shader module, for `create_shader_module_composed`.
pub const QUANTIZE_MODULE: (&str, &str) =
    (include_str!("shaders/quantize.wgsl"), "deptheos::quantize");

/// Entry k is the smallest f32 that [`quantize_u16`] maps to k or above,
/// found by binary search over the bits (positive floats order like them).
pub(crate) fn u16_thresholds() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = vec![0.0f32; 65536];
        let mut lo = 0u32;
        for (k, entry) in table.iter_mut().enumerate().skip(1) {
            let mut hi = 1.0f32.to_bits();
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if usize::from(quantize_u16(f32::from_bits(mid))) >= k {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            *entry = f32::from_bits(lo);
        }
        table
    })
}

/// Bind group 1 of a shader that imports `deptheos::quantize`, built once.
pub fn threshold_bind_group(ctx: &GpuContext, pipeline: &wgpu::ComputePipeline) -> wgpu::BindGroup {
    let buffer = ctx.create_buffer_with_data(
        "u16_thresholds",
        bytemuck::cast_slice(u16_thresholds()),
        wgpu::BufferUsages::STORAGE,
    );
    create_bind_group(&ctx.device, pipeline, 1, "u16_thresholds", &[buffer.as_entire_binding()])
}

pub struct U16Packer {
    pipeline: wgpu::ComputePipeline,
    thresholds: wgpu::BindGroup,
}

impl U16Packer {
    /// Fails on adapters without Rgba16Unorm storage.
    pub fn new(ctx: &GpuContext) -> Result<Self> {
        if !ctx.format_support.rgba16unorm_storage {
            return Err(CoreError::InvalidState(
                "this GPU cannot store Rgba16Unorm; check source_format()".into(),
            ));
        }
        let module = ctx.create_shader_module_composed(
            include_str!("pack.wgsl"),
            "pack",
            &[QUANTIZE_MODULE],
        );
        let pipeline =
            deptheos_gpu::gpu_utils::create_compute_pipeline(&ctx.device, &module, "pack_u16");
        let thresholds = threshold_bind_group(ctx, &pipeline);
        Ok(Self { pipeline, thresholds })
    }

    /// `rect` of `source` as a new rgba16unorm texture, quantized exactly as
    /// [`quantize_u16`].
    pub fn pack(&self, ctx: &GpuContext, source: &wgpu::Texture, rect: Rect) -> wgpu::Texture {
        let output = ctx.create_texture(
            rect.width,
            rect.height,
            wgpu::TextureFormat::Rgba16Unorm,
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "packed_u16",
        );
        let params = ctx.create_uniform_buffer(&UVec4::new(rect.x, rect.y, 0, 0), "pack_params");
        let src_view = source.create_view(&Default::default());
        let dst_view = output.create_view(&Default::default());
        let group0 = create_bind_group(
            &ctx.device,
            &self.pipeline,
            0,
            "pack_u16",
            &[tex(&src_view), tex(&dst_view), params.as_entire_binding()],
        );
        let mut encoder = ctx.encoder("pack_u16");
        dispatch_compute_with(
            &mut encoder,
            &self.pipeline,
            &[&group0, &self.thresholds],
            "pack_u16",
            rect.width,
            rect.height,
            WORKGROUP_SIZE,
        );
        ctx.submit(encoder);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::rgba_row_to_rgb_u16;
    use deptheos_gpu::gpu_utils::download_texture_with;

    fn next_down(v: f32) -> f32 {
        f32::from_bits(v.to_bits() - 1)
    }

    fn next_up(v: f32) -> f32 {
        f32::from_bits(v.to_bits() + 1)
    }

    /// Every threshold and the floats either side of it, the exact-tie values
    /// (k + 0.5) / 65535 and theirs, out-of-range values, and a spread of
    /// ordinary ones.
    fn adversarial_values() -> Vec<f32> {
        let mut values = vec![-1.0, -0.0, 0.0, 1.0, 2.0, f32::MIN_POSITIVE, 0.5, next_down(0.5)];
        for &t in &u16_thresholds()[1..] {
            values.extend([t, next_down(t), next_up(t)]);
        }
        for k in 0..65535u32 {
            let tie = (k as f32 + 0.5) / 65535.0;
            values.extend([tie, next_down(tie), next_up(tie)]);
        }
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            values.push((state >> 40) as f32 / (1u64 << 24) as f32);
        }
        values
    }

    #[test]
    fn pack_matches_cpu_quantization_exactly() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        if !ctx.format_support.rgba16unorm_storage {
            eprintln!("skipping: no Rgba16Unorm storage");
            return;
        }
        let values = adversarial_values();
        // Three channels per pixel; alpha gets a value too, to show it is dropped.
        let width = 1024u32;
        let pixels = values.len().div_ceil(3) as u32;
        let height = pixels.div_ceil(width);
        let mut texels = vec![0.25f32; (width * height * 4) as usize];
        for (i, &v) in values.iter().enumerate() {
            texels[i / 3 * 4 + i % 3] = v;
        }
        let source = ctx.create_texture(
            width,
            height,
            wgpu::TextureFormat::Rgba32Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            "pack_test_src",
        );
        ctx.queue.write_texture(
            source.as_image_copy(),
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 16),
                rows_per_image: None,
            },
            source.size(),
        );

        let packer = U16Packer::new(&ctx).unwrap();
        let whole = Rect { x: 0, y: 0, width, height };
        let packed = packer.pack(&ctx, &source, whole);
        assert_eq!(packed.format(), wgpu::TextureFormat::Rgba16Unorm);
        let mut gpu = Vec::new();
        download_texture_with(&ctx, &packed, [0, 0], [width, height], |row| {
            push_row(row, packed.format(), &mut gpu)
        })
        .unwrap();

        let mut mismatches = 0;
        for (i, &v) in values.iter().enumerate() {
            let want = quantize_u16(texels[i / 3 * 4 + i % 3]);
            if gpu[i] != want {
                if mismatches < 10 {
                    eprintln!("value {v:e} ({:#x}): gpu {} cpu {want}", v.to_bits(), gpu[i]);
                }
                mismatches += 1;
            }
        }
        assert_eq!(mismatches, 0, "{mismatches} of {} values differ from the CPU", values.len());

        // A cropped pack reads from the rect's origin.
        let rect = Rect { x: 3, y: 1, width: 5, height: 2 };
        let cropped = packer.pack(&ctx, &source, rect);
        let mut gpu = Vec::new();
        download_texture_with(&ctx, &cropped, [0, 0], [rect.width, rect.height], |row| {
            push_row(row, cropped.format(), &mut gpu)
        })
        .unwrap();
        let mut cpu = Vec::new();
        for y in rect.y..rect.y + rect.height {
            let start = ((y * width + rect.x) * 4) as usize;
            let row = &texels[start..start + rect.width as usize * 4];
            push_row(bytemuck::cast_slice(row), wgpu::TextureFormat::Rgba32Float, &mut cpu);
        }
        assert_eq!(gpu, cpu);
    }

    /// Append one RGBA row to `rgb` as RGB u16.
    fn push_row(row: &[u8], format: wgpu::TextureFormat, rgb: &mut Vec<u16>) {
        let start = rgb.len();
        rgb.resize(start + row.len() / format.block_copy_size(None).unwrap() as usize * 3, 0);
        rgba_row_to_rgb_u16(row, format, &mut rgb[start..]);
    }

    #[test]
    fn thresholds_bound_every_code() {
        let t = u16_thresholds();
        assert_eq!(t[0], 0.0);
        for (k, &entry) in t.iter().enumerate().skip(1) {
            assert_eq!(usize::from(quantize_u16(entry)), k, "entry {k}");
            let below = f32::from_bits(entry.to_bits() - 1);
            assert_eq!(usize::from(quantize_u16(below)), k - 1, "below entry {k}");
        }
    }
}
