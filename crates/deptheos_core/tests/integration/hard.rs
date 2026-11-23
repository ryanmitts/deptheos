//! Hard frames on synthetic scenes, kept out of the end-to-end cases: a
//! frame both moved (turned, magnified as focus breathing does, shifted)
//! and focused elsewhere, at a size of two primes, which no power-of-two
//! pyramid or tile divides. Each aligner finds the motion, and each
//! method stacks the pair at the frames' size, as sharp as the scene.

use deptheos_aligner::{AlignMode, Aligner};
use deptheos_core::depth::{DepthConfig, DepthStacker};
use deptheos_core::pyramid::{PyramidConfig, PyramidStacker};
use deptheos_core::wavelet::{WaveletConfig, WaveletStacker};
use deptheos_core::{FrameAlignment, StackInfo};
use deptheos_gpu::GpuContext;
use image::{DynamicImage, ImageBuffer, Rgb};
use nalgebra::{Affine2, Matrix3, Point2};

use crate::common::{Rgb8Image, align, blur_half, download_result};

/// Two primes.
const W: u32 = 509;
const H: u32 = 383;
/// The source's margin round the reference frame, so the moved frame has
/// no border of its own.
const MARGIN: u32 = 96;

/// The motion of the second frame: a frame pixel `p` shows the reference's
/// point `motion · p`. Turned 1.5°, magnified 3% about the middle, moved
/// (23, −17) px.
fn motion() -> Affine2<f32> {
    let (c, s) = (1.5f32.to_radians().cos() * 1.03, 1.5f32.to_radians().sin() * 1.03);
    let (cx, cy) = (W as f32 / 2.0, H as f32 / 2.0);
    let m = Matrix3::new(
        c,
        -s,
        cx - c * cx + s * cy + 23.0,
        s,
        c,
        cy - s * cx - c * cy - 17.0,
        0.0,
        0.0,
        1.0,
    );
    Affine2::from_matrix_unchecked(m)
}

/// A natural scene (the aligner's fruits), larger than the frames: ECC
/// needs aperiodic structure.
fn source() -> Rgb8Image {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../deptheos_aligner/fixtures/fruits.jpg");
    image::open(path)
        .expect("fixture")
        .resize_exact(W + 2 * MARGIN, H + 2 * MARGIN, image::imageops::FilterType::Triangle)
        .to_rgb8()
}

/// A frame of `source` through `motion`: pixel `p` sampled (bilinear) at
/// the reference's `motion · p`.
fn frame(source: &Rgb8Image, motion: &Affine2<f32>) -> Rgb8Image {
    ImageBuffer::from_fn(W, H, |x, y| {
        let q = motion.transform_point(&Point2::new(x as f32, y as f32));
        let (sx, sy) = (q.x + MARGIN as f32, q.y + MARGIN as f32);
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = (sx - x0, sy - y0);
        let at = |dx: f32, dy: f32| {
            let px = (x0 + dx).clamp(0.0, (source.width() - 1) as f32) as u32;
            let py = (y0 + dy).clamp(0.0, (source.height() - 1) as f32) as u32;
            source.get_pixel(px, py).0.map(f32::from)
        };
        let (a, b, c, d) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0));
        Rgb(std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            (top + (bottom - top) * fy).round() as u8
        }))
    })
}

/// The reference, sharp; and the pair stacked: the reference sharp on its
/// right, the moved frame sharp on its left.
fn scene() -> (Rgb8Image, [DynamicImage; 2]) {
    let source = source();
    let sharp = frame(&source, &Affine2::identity());
    let moved = frame(&source, &motion());
    let frames = [
        DynamicImage::ImageRgb8(blur_half(&sharp, true)),
        DynamicImage::ImageRgb8(blur_half(&moved, false)),
    ];
    (sharp, frames)
}

/// Where `found` puts the frames' corners and middle, against the truth:
/// the most it is off by, in pixels. An aligner's transform takes a
/// reference point to where the frame shows it, the inverse of `motion`
/// as it is written here.
fn worst_error(found: &Affine2<f32>) -> f32 {
    let truth = motion().inverse();
    let (w, h) = (W as f32, H as f32);
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h), (w / 2.0, h / 2.0)]
        .into_iter()
        .map(|(x, y)| {
            let p = Point2::new(x, y);
            (found.transform_point(&p) - truth.transform_point(&p)).norm()
        })
        .fold(0.0, f32::max)
}

#[test]
fn each_aligner_finds_a_hard_motion() {
    let (_, frames) = scene();
    for mode in AlignMode::ALL {
        let mut chain = Aligner::new(mode);
        let mut last = Affine2::identity();
        for frame in &frames {
            last = align(&mut chain, frame).unwrap_or_else(|e| panic!("{mode:?}: {e}")).transform;
        }
        let e = worst_error(&last);
        eprintln!("{mode:?}: off by {e:.3} px");
        assert!(e < 1.0, "{mode:?}: off by {e:.2} px");
    }
}

/// The mean difference from the sharp reference, 0 to 1, inside the area
/// both frames cover.
fn interior_diff(result: &[f32], sharp: &Rgb8Image) -> f64 {
    let inset = 48;
    let mut total = 0.0;
    let mut n = 0.0;
    for y in inset..H - inset {
        for x in inset..W - inset {
            let i = ((y * W + x) * 4) as usize;
            let p = sharp.get_pixel(x, y);
            for c in 0..3 {
                total += (f64::from(result[i + c]) - f64::from(p[c]) / 255.0).abs();
                n += 1.0;
            }
        }
    }
    total / n
}

#[test]
fn every_method_stacks_hard_frames_of_prime_size() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (sharp, frames) = scene();
    let mut chain = Aligner::new(AlignMode::Ecc);
    let alignments: Vec<FrameAlignment> =
        frames.iter().map(|f| align(&mut chain, f).expect("align").transform.into()).collect();
    let info = StackInfo { total_frames: 2, is_linear: false };
    // A blurred half differs from the sharp scene by about this much.
    let blurred = {
        let soft = frames[0].as_rgb8().unwrap();
        let px: Vec<f32> = soft
            .pixels()
            .flat_map(|p| [p[0], p[1], p[2], 255].map(|v| f32::from(v) / 255.0))
            .collect();
        interior_diff(&px, &sharp)
    };

    let mut pyramid = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), info);
    for (frame, a) in frames.iter().zip(&alignments) {
        pyramid.accumulate(frame, Some(a)).expect("accumulate");
    }
    pyramid.finalize().expect("finalize");
    let pyramid = download_result(&ctx, pyramid.result_texture());

    let mut depth = DepthStacker::new(ctx.clone(), DepthConfig::default(), info);
    for (frame, a) in frames.iter().zip(&alignments) {
        depth.accumulate(frame, Some(a)).expect("accumulate");
    }
    depth.end_pass_one().expect("depth map");
    for (i, (frame, a)) in frames.iter().zip(&alignments).enumerate() {
        depth.blend_frame(frame, i as u32, Some(a)).expect("blend");
    }
    depth.finalize().expect("finalize");
    let depth = download_result(&ctx, depth.result_texture());

    let mut wavelet = WaveletStacker::new(ctx.clone(), WaveletConfig::default(), info);
    for (frame, a) in frames.iter().zip(&alignments) {
        wavelet.accumulate(frame, Some(a)).expect("accumulate");
    }
    wavelet.end_pass_one().expect("merge");
    for (i, (frame, a)) in frames.iter().zip(&alignments).enumerate() {
        wavelet.blend_frame(frame, i as u32, Some(a)).expect("reassign");
    }
    wavelet.finalize().expect("finalize");
    let wavelet = download_result(&ctx, wavelet.result_texture());

    for (name, (bytes, width, height)) in
        [("pyramid", pyramid), ("depth", depth), ("wavelet", wavelet)]
    {
        assert_eq!((width, height), (W, H), "{name}: the frames' size");
        let diff = interior_diff(bytemuck::cast_slice(&bytes), &sharp);
        eprintln!("{name}: {diff:.4} from the sharp scene, a blurred half {blurred:.4}");
        assert!(diff < blurred / 4.0, "{name}: {diff:.4} from the sharp scene");
    }
}
