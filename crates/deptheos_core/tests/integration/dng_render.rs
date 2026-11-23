//! GPU DNG render parity: the WGSL pipeline matches the DNG SDK's CPU
//! renderer on the same pixels and color science, above 60 dB PSNR.
//!
//! Synthetic metadata (`from_raw_metadata`) carries no HueSatMap/LookTable;
//! the LUT paths are tested in `tests/integration/dcp.rs`. Both render from
//! the metadata this crate carries, so `gpu_render_matches_sdk_render_of_file`
//! checks the whole route: a real DNG read as the app reads it, against the
//! SDK's render of the file itself.

use deptheos_core::dcp::{self, DcpPreference};
use deptheos_core::dng_render::DngRenderPipeline;
use deptheos_dng::{DngImage, RenderParams};
use deptheos_gpu::GpuContext;
use deptheos_raw::{RawDngColor, RawMetadata, TestMetadata};
use image::{DynamicImage, ImageBuffer, Rgb};

use crate::common::download_rgba;

const SIZE: u32 = 256;

/// Plausible camera color science, shaped like what LibRaw parses from a
/// real file: daylight WB multipliers and a typical XYZ→camera matrix.
fn synthetic_metadata() -> TestMetadata {
    let mut color = RawDngColor {
        illuminant: 21, // D65
        ..Default::default()
    };
    #[rustfmt::skip]
    let xyz_to_cam = [
        1.3426, -0.6334, -0.1177,
        -0.4244, 1.2136, 0.2371,
        -0.0580, 0.1303, 0.6122,
        0.0, 0.0, 0.0,
    ];
    color.color_matrix = Some(xyz_to_cam);

    TestMetadata {
        make: "Synthetic".into(),
        model: "TestCam".into(),
        cam_mul: [2.2, 1.0, 1.6, 1.0],
        raw_colors: 3,
        dng_color: [color, RawDngColor::default()],
        baseline_exposure: Some(0.35),
        ..Default::default()
    }
}

/// Linear gradient sweeping a wide range of colors and intensities.
pub(crate) fn synthetic_linear_rgb16() -> Vec<u16> {
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = x as f32 / (SIZE - 1) as f32;
            let fy = y as f32 / (SIZE - 1) as f32;
            let r = fx;
            let g = fy;
            let b = (fx + fy) * 0.5;
            for c in [r, g, b] {
                pixels.push((c * 65535.0 + 0.5) as u16);
            }
        }
    }
    pixels
}

fn upload_linear(ctx: &GpuContext, pixels: &[u16]) -> wgpu::Texture {
    let buffer =
        ImageBuffer::<Rgb<u16>, _>::from_raw(SIZE, SIZE, pixels.to_vec()).expect("buffer size");
    crate::common::upload(ctx, &DynamicImage::ImageRgb16(buffer))
}

fn psnr_db(gpu_rgba_f32: &[f32], cpu_rgb_u16: &[u16]) -> f64 {
    let mut sum_sq = 0.0f64;
    let mut count = 0u64;
    for (i, chunk) in gpu_rgba_f32.as_chunks::<4>().0.iter().enumerate() {
        for c in 0..3 {
            let gpu = chunk[c] as f64;
            let cpu = cpu_rgb_u16[i * 3 + c] as f64 / 65535.0;
            sum_sq += (gpu - cpu) * (gpu - cpu);
            count += 1;
        }
    }
    let mse = sum_sq / count as f64;
    if mse <= 0.0 {
        return f64::INFINITY;
    }
    -10.0 * mse.log10()
}

fn assert_gpu_matches_cpu(metadata: &RawMetadata, label: &str) {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    let source = DngImage::from_raw_metadata(metadata).expect("metadata synthesis");
    assert_renders_like_sdk(&ctx, &source, label);
}

/// Renders [`synthetic_linear_rgb16`] through `source`'s color science on
/// the GPU and checks it against the SDK's CPU render of the same pixels.
pub(crate) fn assert_renders_like_sdk(ctx: &GpuContext, source: &DngImage, label: &str) {
    let linear = synthetic_linear_rgb16();

    // Reference: the Adobe SDK's own CPU renderer.
    let cpu = source.render_linear_to_srgb(&linear, SIZE, SIZE, 3).expect("CPU render");
    let cpu_pixels = cpu.pixels();

    // GPU: same pixels + the same metadata's render params.
    let render_params = source.render_params().expect("render params");
    let input = upload_linear(ctx, &linear);
    let pipeline = DngRenderPipeline::new(ctx);
    let output = pipeline.render(ctx, &input, &render_params);
    let gpu_pixels = &download_rgba(ctx, &output)[..];

    let psnr = psnr_db(gpu_pixels, cpu_pixels);
    println!("GPU vs SDK CPU render PSNR ({label}): {psnr:.1} dB");
    assert!(psnr > 60.0, "{label}: PSNR {psnr:.1} dB below the 60 dB parity bar");
}

#[test]
fn gpu_render_matches_sdk_cpu_render() {
    assert_gpu_matches_cpu(&synthetic_metadata().into(), "dng_color");
}

/// Camera RAW files (RAF/CR3/…) carry no dng_color entries; color science
/// falls back to LibRaw's Adobe cam_xyz matrix.
#[test]
fn gpu_render_matches_sdk_via_cam_xyz_fallback() {
    let mut metadata = synthetic_metadata();
    metadata.cam_xyz[..9].copy_from_slice(&metadata.dng_color[0].color_matrix.take().unwrap()[..9]);
    assert!(metadata.dng_color[0].color_matrix.is_none());
    assert_gpu_matches_cpu(&metadata.into(), "cam_xyz fallback");
}

#[test]
fn srgb_identity_params_encode_linear_input() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    let linear = synthetic_linear_rgb16();
    let input = upload_linear(&ctx, &linear);
    let pipeline = DngRenderPipeline::new(&ctx);
    let output = pipeline.render(&ctx, &input, &RenderParams::srgb_identity());
    let gpu_pixels = &download_rgba(&ctx, &output)[..];

    // Identity params reduce the pipeline to the sRGB transfer function.
    let mut max_err = 0.0f32;
    for (i, chunk) in gpu_pixels.as_chunks::<4>().0.iter().enumerate() {
        for c in 0..3 {
            let x = linear[i * 3 + c] as f32 / 65535.0;
            let expected =
                if x <= 0.003_130_8 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
            max_err = max_err.max((chunk[c] - expected).abs());
        }
    }
    assert!(max_err < 5e-3, "identity render deviates from sRGB encode by {max_err}");
}

/// The camera of the converter DNG fixture (`known.rs`; linear, Adobe
/// Standard embedded), whose camera-matching profiles ship with Camera Raw.
const SAMPLE_MODEL: &str = "Canon EOS R5 Mark II";
/// The PSNR bar with the Camera Standard profile: the GPU's interpolation
/// of its sRGB-encoded look table departs from the SDK's most in deep
/// shadows, up to 0.8% of a sample (57.8 dB here). Every sample stays
/// within 1%.
const CAMERA_STANDARD: f64 = 55.0;

#[test]
fn gpu_render_matches_sdk_render_of_file() {
    let path = crate::known::fixture("converter.dng");
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    // The file's own profile.
    assert_file_render_matches(&ctx, &path, None, "embedded profile", 60.0);

    // A camera-matching profile in its place: the sRGB-encoded LookTable,
    // DefaultBlackRender and BaselineExposureOffset most of them carry.
    match dcp::find_profile(SAMPLE_MODEL, DcpPreference::CameraStandard)
        .filter(|p| p.to_string_lossy().contains("Camera Standard"))
    {
        Some(dcp) => {
            assert_file_render_matches(&ctx, &path, Some(&dcp), "Camera Standard", CAMERA_STANDARD)
        }
        None => eprintln!("skipping the Camera Standard case: profile not installed"),
    }
}

fn assert_file_render_matches(
    ctx: &GpuContext,
    path: &std::path::Path,
    dcp: Option<&std::path::Path>,
    label: &str,
    bar: f64,
) {
    // The app's route: linear pixels and metadata, the profile applied to
    // the metadata, render params, GPU render.
    let read = DngImage::read_with_pixels(
        path,
        deptheos_dng::FileCache::Use,
        deptheos_dng::Budget::unlimited(),
    )
    .expect("read DNG");
    let deptheos_dng::DngRead::Linear(mut source, linear) = read else {
        panic!("{label}: a mosaic, read as metadata only")
    };
    if let Some(dcp) = dcp {
        source.apply_dcp(dcp).expect("apply DCP");
    }
    let render_params = source.render_params().expect("render params");
    let (width, height) = (source.width(), source.height());

    // A center crop: the render is per pixel.
    let (x, y) = ((width - SIZE) / 2, (height - SIZE) / 2);
    let mut crop = Vec::with_capacity((SIZE * SIZE * 3) as usize);
    for row in y..y + SIZE {
        let start = ((row * width + x) * 3) as usize;
        crop.extend_from_slice(&linear[start..start + (SIZE * 3) as usize]);
    }
    let input = upload_linear(ctx, &crop);
    let output = DngRenderPipeline::new(ctx).render(ctx, &input, &render_params);
    let gpu_pixels = &download_rgba(ctx, &output)[..];

    let sdk = deptheos_dng::sdk_render_file_srgb(path, dcp, [x, y, SIZE, SIZE])
        .expect("SDK render of the file");

    let psnr = psnr_db(gpu_pixels, &sdk);
    println!("GPU vs SDK render of {} ({label}) PSNR: {psnr:.1} dB", path.display());
    assert!(psnr > bar, "{label}: PSNR {psnr:.1} dB below the {bar} dB parity bar");
    let worst = gpu_pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(sdk.as_chunks::<3>().0)
        .flat_map(|(g, s)| (0..3).map(move |c| (g[c] - f32::from(s[c]) / 65535.0).abs()))
        .fold(0f32, f32::max);
    assert!(worst < 0.01, "{label}: a sample {worst:.4} from the SDK's");
}
