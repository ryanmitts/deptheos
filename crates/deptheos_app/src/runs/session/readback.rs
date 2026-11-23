//! Reading a stack's GPU results back: the RGB result, the 8-bit map and
//! the frame map, and the crop they are cut from.

use deptheos_core::{CoreError, io};
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_rows;
use image::GrayImage;
use image::math::Rect;
use rayon::prelude::*;

use super::Gray16Image;
use crate::{Error, Result};

/// Download `rect` of an Rgba16Unorm or Rgba32Float result as interleaved
/// RGB u16, rows converted from the mapping in parallel.
pub(crate) fn download_rgb(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    rect: Rect,
) -> Result<Vec<u16>> {
    let format = texture.format();
    let row_len = rect.width as usize * 3;
    let mut rgb = vec![0u16; row_len * rect.height as usize];
    download_texture_rows(
        ctx,
        texture,
        [rect.x, rect.y],
        [rect.width, rect.height],
        |top, mapped, stride, row_bytes| {
            rgb[top as usize * row_len..]
                .par_chunks_mut(row_len)
                .zip(mapped.par_chunks(stride))
                .for_each(|(out, row)| io::rgba_row_to_rgb_u16(&row[..row_bytes], format, out))
        },
    )
    .map_err(CoreError::from)?;
    Ok(rgb)
}

/// Download `rect` of an 8-bit grayscale visualization (R8Unorm, or
/// Rgba8Unorm with the gray in every channel) as a single-channel image.
pub(crate) fn download_gray(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    rect: Rect,
) -> Result<GrayImage> {
    let (w, h) = (rect.width, rect.height);
    let row_len = w as usize;
    let mut gray = vec![0u8; row_len * h as usize];
    let rgba = texture.format() == wgpu::TextureFormat::Rgba8Unorm;
    download_texture_rows(
        ctx,
        texture,
        [rect.x, rect.y],
        [w, h],
        |top, mapped, stride, row_bytes| {
            let gray = &mut gray[top as usize * row_len..];
            gray.par_chunks_mut(row_len).zip(mapped.par_chunks(stride)).for_each(|(out, row)| {
                let row = &row[..row_bytes];
                if rgba {
                    for (o, p) in out.iter_mut().zip(row.as_chunks::<4>().0) {
                        *o = p[0];
                    }
                } else {
                    out.copy_from_slice(row);
                }
            });
        },
    )
    .map_err(CoreError::from)?;
    GrayImage::from_raw(w, h, gray)
        .ok_or_else(|| Error::InvalidState("map buffer size mismatch".into()))
}

/// Download `rect` of an r32float map of frame planes in [0, 1] as a
/// 16-bit image, each plane quantized to `round(plane × 65535)`.
pub(super) fn download_planes(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    rect: Rect,
) -> Result<Gray16Image> {
    let (w, h) = (rect.width, rect.height);
    let row_len = w as usize;
    let mut planes = vec![0u16; row_len * h as usize];
    download_texture_rows(
        ctx,
        texture,
        [rect.x, rect.y],
        [w, h],
        |top, mapped, stride, row_bytes| {
            let planes = &mut planes[top as usize * row_len..];
            planes.par_chunks_mut(row_len).zip(mapped.par_chunks(stride)).for_each(|(out, row)| {
                for (o, &b) in out.iter_mut().zip(row[..row_bytes].as_chunks::<4>().0) {
                    *o = (f32::from_ne_bytes(b).clamp(0.0, 1.0) * 65535.0).round() as u16;
                }
            });
        },
    )
    .map_err(CoreError::from)?;
    Gray16Image::from_raw(w, h, planes)
        .ok_or_else(|| Error::InvalidState("frame map buffer size mismatch".into()))
}

/// Copy a rectangle of a texture into a fresh displayable, readable one.
pub(super) fn crop_texture(ctx: &GpuContext, source: &wgpu::Texture, rect: Rect) -> wgpu::Texture {
    let cropped = ctx.create_texture(
        rect.width,
        rect.height,
        source.format(),
        wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        "cropped_result",
    );
    let mut encoder = ctx.encoder("crop_result");
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d { x: rect.x, y: rect.y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &cropped,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d { width: rect.width, height: rect.height, depth_or_array_layers: 1 },
    );
    ctx.submit(encoder);
    cropped
}
