//! Upload and download for the render tests.

use std::path::{Path, PathBuf};

use deptheos_core::upload::ImageUploader;
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;
use image::DynamicImage;

/// `image` uploaded as a stacker's frame is.
pub fn upload(ctx: &GpuContext, image: &DynamicImage) -> wgpu::Texture {
    let mut uploader = ImageUploader::new(ctx);
    let prep = uploader.prepare(ctx, image).unwrap();
    let texture = ImageUploader::create_target(ctx, prep.width, prep.height, "test_source");
    let mut encoder = ctx.encoder("test_upload");
    prep.dispatch_to_rgba(ctx, &mut encoder, &texture);
    ctx.submit(encoder);
    texture
}

/// A render output (Rgba16Unorm or Rgba32Float, per the adapter) as RGBA
/// floats.
pub fn download_rgba(ctx: &GpuContext, texture: &wgpu::Texture) -> Vec<f32> {
    let mut bytes = Vec::new();
    let size = [texture.width(), texture.height()];
    download_texture_with(ctx, texture, [0, 0], size, |row| bytes.extend_from_slice(row)).unwrap();
    match texture.format() {
        wgpu::TextureFormat::Rgba16Unorm => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b) as f32 / 65535.0)
            .collect(),
        wgpu::TextureFormat::Rgba32Float => bytemuck::cast_slice(&bytes).to_vec(),
        other => panic!("unexpected output format {other:?}"),
    }
}

/// A file of `deptheos_core`'s test fixtures (its `known.rs` says what each
/// is).
pub fn core_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../deptheos_core/tests/fixtures").join(name)
}
