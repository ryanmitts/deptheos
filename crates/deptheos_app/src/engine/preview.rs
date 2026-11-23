//! Decode and upload helpers for previews and compare sources. RAW and DNG
//! decode linear and render with their own color science, so a preview
//! matches the stack; other formats upload directly.

use std::path::Path;

use deptheos_core::Result;
use deptheos_core::dcp::DcpPreference;
use deptheos_core::io;
use deptheos_core::upload::{ImageUploader, PreparedUpload};
use deptheos_gpu::GpuContext;
use image::DynamicImage;

use crate::Error;

/// Upload an image to a `GpuContext::source_format()` texture with `uploader`.
pub(crate) fn upload_texture(
    ctx: &GpuContext,
    uploader: &mut ImageUploader,
    image: &DynamicImage,
) -> Result<wgpu::Texture> {
    let prep = uploader.prepare(ctx, image)?;
    Ok(unpack_prepared(ctx, prep))
}

/// Upload interleaved RGB u16 pixels, read in place, with `uploader`.
pub(crate) fn upload_rgb16_texture(
    ctx: &GpuContext,
    uploader: &mut ImageUploader,
    pixels: &[u16],
    width: u32,
    height: u32,
) -> Result<wgpu::Texture> {
    let prep = uploader.prepare_rgb16(ctx, pixels, width, height)?;
    Ok(unpack_prepared(ctx, prep))
}

fn unpack_prepared(ctx: &GpuContext, prep: PreparedUpload<'_>) -> wgpu::Texture {
    let texture = ImageUploader::create_target(ctx, prep.width, prep.height, "display_upload");
    let mut encoder = ctx.encoder("display_upload");
    prep.dispatch_to_rgba(ctx, &mut encoder, &texture);
    ctx.submit(encoder);
    texture
}

/// Decodes a camera RAW or DNG as linear camera RGB with the metadata that
/// renders it, in one pass; camera RAW takes a matching installed DCP.
pub fn decode_linear_with_metadata(
    path: &Path,
    profile: DcpPreference,
) -> std::result::Result<(DynamicImage, deptheos_dng::DngImage), Error> {
    let linear = io::decode_linear(path)?;
    let metadata = match linear.color {
        io::LinearColor::Dng(dng) => dng,
        io::LinearColor::Raw(meta) => dng_metadata_from_raw(&meta, profile)?,
    };
    Ok((linear.image, metadata))
}

/// DNG color science for a camera RAW frame: LibRaw's metadata, upgraded
/// by the installed DCP `profile` picks when one matches.
pub(crate) fn dng_metadata_from_raw(
    meta: &deptheos_raw::RawMetadata,
    profile: DcpPreference,
) -> Result<deptheos_dng::DngImage> {
    let mut metadata =
        deptheos_dng::DngImage::from_raw_metadata(meta).map_err(deptheos_core::CoreError::from)?;
    deptheos_core::dcp::apply_profile(&mut metadata, meta, profile);
    Ok(metadata)
}
