//! The uploader on a failed GPU: an error, never a panic; and the stackers'
//! source at either depth.

use std::sync::Arc;

use crate::common::download_texture;
use deptheos_core::depth::{DepthConfig, DepthStacker};
use deptheos_core::pyramid::{PyramidConfig, PyramidStacker};
use deptheos_core::upload::{ImageUploader, StagedUpload};
use deptheos_core::wavelet::{WaveletConfig, WaveletStacker};
use deptheos_core::{FrameDepth, StackInfo};
use deptheos_gpu::GpuContext;
use image::{DynamicImage, ImageBuffer, Rgb};

use crate::common::{golden_alignment, golden_frames};

fn frame(size: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(ImageBuffer::from_pixel(size, size, Rgb([10, 20, 30])))
}

/// Once the device is gone, an upload is an error, whether it reuses the
/// staging buffer or grows it, and stays one.
#[test]
fn an_upload_on_a_destroyed_device_is_an_error() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let mut uploader = ImageUploader::new(&ctx);
    uploader.prepare(&ctx, &frame(32)).expect("upload on a working device");

    ctx.device.destroy();
    assert!(uploader.prepare(&ctx, &frame(32)).is_err(), "reusing the buffer");
    assert!(uploader.prepare(&ctx, &frame(64)).is_err(), "growing the buffer");
    assert!(uploader.prepare(&ctx, &frame(64)).is_err(), "after a failed grow");
}

/// Which stacker [`stack`] runs.
#[derive(Clone, Copy, Debug)]
enum Method {
    Pyramid,
    Depth,
    Wavelet,
}

/// `frames` staged and stacked by `method` sized for `depth`, the second
/// aligned: the result and the frame map, as bytes.
fn stack(
    ctx: &Arc<GpuContext>,
    method: Method,
    depth: FrameDepth,
    frames: &[DynamicImage],
) -> [Vec<u8>; 2] {
    let (w, h) = (frames[0].width(), frames[0].height());
    let aligned = golden_alignment();
    let alignment = |i: usize| (i == 1).then_some(&aligned);
    let info = StackInfo { total_frames: frames.len() as u32, is_linear: false };
    let staged = |i: usize| StagedUpload::with_pool(ctx, &frames[i], None).unwrap();
    let read = |result: Option<&wgpu::Texture>, map: Option<&wgpu::Texture>| {
        [
            download_texture(ctx, result.unwrap()).unwrap(),
            download_texture(ctx, map.unwrap()).unwrap(),
        ]
    };
    match method {
        Method::Pyramid => {
            let mut s = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), info);
            s.allocate(w, h, depth).unwrap();
            for i in 0..frames.len() {
                s.accumulate(&staged(i), alignment(i)).unwrap();
            }
            s.finalize().unwrap();
            read(s.result_texture(), s.frame_map())
        }
        Method::Depth => {
            let mut s = DepthStacker::new(ctx.clone(), DepthConfig::default(), info);
            s.allocate(w, h, depth).unwrap();
            for i in 0..frames.len() {
                s.accumulate(&staged(i), alignment(i)).unwrap();
            }
            s.end_pass_one().unwrap();
            for i in 0..frames.len() {
                s.blend_frame(&staged(i), i as u32, alignment(i)).unwrap();
            }
            s.finalize().unwrap();
            read(s.result_texture(), s.frame_map())
        }
        Method::Wavelet => {
            let mut s = WaveletStacker::new(ctx.clone(), WaveletConfig::default(), info);
            s.allocate(w, h, depth).unwrap();
            for i in 0..frames.len() {
                s.accumulate(&staged(i), alignment(i)).unwrap();
            }
            s.end_pass_one().unwrap();
            for i in 0..frames.len() {
                s.blend_frame(&staged(i), i as u32, alignment(i)).unwrap();
            }
            s.finalize().unwrap();
            read(s.result_texture(), s.frame_map())
        }
    }
}

/// 8-bit frames stack bit for bit as their 16-bit copies, on every stacker,
/// and so does a mix in a stack sized for 16 bits: Rgba16Unorm holds an
/// 8-bit value exactly. Skipped where the 16-bit source is Rgba32Float,
/// which can differ in the last bit. A software GPU (lavapipe, WARP) reads an
/// 8-bit texel to a float with its own rounding, so there the result need only
/// be within a tenth of a 16-bit step, and a frame map may differ where frames tie.
#[test]
fn eight_bit_frames_stack_as_their_sixteen_bit_copies() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    if ctx.source_format() != wgpu::TextureFormat::Rgba16Unorm {
        eprintln!("skipping: the 16-bit source is {:?}", ctx.source_format());
        return;
    }
    let software = ctx.device_info().is_software();
    let floats = |v: &[u8]| -> Vec<f32> {
        v.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
    };
    let same = |narrow: &[Vec<u8>; 2], wide: &[Vec<u8>; 2]| {
        if !software {
            return narrow == wide;
        }
        let (a, b) = (floats(&narrow[0]), floats(&wide[0]));
        a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 0.1 / 65535.0)
    };
    let eight = golden_frames(517, 389).to_vec();
    let sixteen: Vec<_> = eight.iter().map(|f| DynamicImage::ImageRgb16(f.to_rgb16())).collect();
    let mut mixed = eight.clone();
    mixed[1] = sixteen[1].clone();
    for method in [Method::Pyramid, Method::Depth, Method::Wavelet] {
        let wide = stack(&ctx, method, FrameDepth::Sixteen, &sixteen);
        let narrow = stack(&ctx, method, FrameDepth::Eight, &eight);
        assert!(same(&narrow, &wide), "{method:?}: 8-bit frames");
        let mixed = stack(&ctx, method, FrameDepth::Sixteen, &mixed);
        assert!(mixed == wide, "{method:?}: a 16-bit frame among them");
    }
}

/// A 16-bit frame given to a stack sized for 8-bit frames is an error, on
/// every stacker, not stacked with its low bits lost.
#[test]
fn a_deeper_frame_than_the_stack_was_sized_for_is_refused() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let eight = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(64, 64, Rgb([10, 20, 30])));
    let sixteen = DynamicImage::ImageRgb16(eight.to_rgb16());
    let info = StackInfo { total_frames: 2, is_linear: false };
    let mut p = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), info);
    p.allocate(64, 64, FrameDepth::Eight).unwrap();
    p.accumulate(&eight, None).unwrap();
    assert!(p.accumulate(&sixteen, None).is_err(), "pyramid");
    let mut d = DepthStacker::new(ctx.clone(), DepthConfig::default(), info);
    d.allocate(64, 64, FrameDepth::Eight).unwrap();
    d.accumulate(&eight, None).unwrap();
    assert!(d.accumulate(&sixteen, None).is_err(), "depth");
    let mut w = WaveletStacker::new(ctx.clone(), WaveletConfig::default(), info);
    w.allocate(64, 64, FrameDepth::Eight).unwrap();
    w.accumulate(&eight, None).unwrap();
    assert!(w.accumulate(&sixteen, None).is_err(), "wavelet");
}
