//! End-to-end wavelet stacking on synthetic frames: two frames of a
//! checkerboard, each blurred on a different half.

use crate::common::download_texture;
use crate::golden;
use deptheos_core::wavelet::{WaveletConfig, WaveletStacker};
use deptheos_core::{FrameAlignment, StackInfo};
use deptheos_gpu::GpuContext;
use image::DynamicImage;

use crate::common::{
    Rgb8Image, blur_half, download_result, golden_alignment, golden_frames, mean_abs_diff,
};

const SIZE: u32 = 512;

fn info(total_frames: u32) -> StackInfo {
    StackInfo { total_frames, is_linear: false }
}

/// Both passes over `frames`, each aligned by its entry in `alignments`,
/// as linear frames when `is_linear`.
fn stack(
    ctx: &std::sync::Arc<GpuContext>,
    config: WaveletConfig,
    frames: &[DynamicImage],
    alignments: &[Option<&FrameAlignment>],
    is_linear: bool,
) -> WaveletStacker {
    assert_eq!(frames.len(), alignments.len(), "an alignment for each frame");
    let info = StackInfo { total_frames: frames.len() as u32, is_linear };
    let mut stacker = WaveletStacker::new(ctx.clone(), config, info);
    for (frame, &alignment) in frames.iter().zip(alignments) {
        stacker.accumulate(frame, alignment).expect("accumulate");
    }
    stacker.end_pass_one().expect("merge");
    for (i, (frame, &alignment)) in frames.iter().zip(alignments).enumerate() {
        stacker.blend_frame(frame, i as u32, alignment).expect("reassign");
    }
    stacker.finalize().expect("finalize");
    stacker
}

fn half_blurred(scene: &Rgb8Image) -> [DynamicImage; 2] {
    [
        DynamicImage::ImageRgb8(blur_half(scene, true)), // sharp right
        DynamicImage::ImageRgb8(blur_half(scene, false)), // sharp left
    ]
}

fn planes(ctx: &GpuContext, stacker: &WaveletStacker) -> Vec<f32> {
    let bytes = download_texture(ctx, stacker.frame_map().expect("frame map")).expect("download");
    bytemuck::cast_slice(&bytes).to_vec()
}

#[test]
fn stacks_two_half_blurred_frames() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene = crate::common::sharp_scene(SIZE);
    let frames = half_blurred(&scene);
    let stacker = stack(&ctx, WaveletConfig::default(), &frames, &[None; 2], false);

    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    assert_eq!((width, height), (SIZE, SIZE));
    let pixels: &[f32] = bytemuck::cast_slice(&bytes);
    // A blurred half differs from sharp by ~0.25 mean abs.
    let diff = mean_abs_diff(pixels, &scene);
    assert!(diff < 0.03, "stack deviates from the sharp scene: {diff:.4}");
    assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 1.0));

    // The frame map: frame 0 (plane 0) on the right, frame 1 (plane 1) on
    // the left, away from the seam.
    let map = planes(&ctx, &stacker);
    let share = |x0: u32, x1: u32, plane: f32| {
        let (mut hit, mut all) = (0, 0);
        for y in 16..SIZE - 16 {
            for x in x0..x1 {
                all += 1;
                hit += (map[(y * SIZE + x) as usize] == plane) as u32;
            }
        }
        hit as f64 / all as f64
    };
    assert!(share(16, SIZE / 2 - 32, 1.0) > 0.95, "left half from frame 1");
    assert!(share(SIZE / 2 + 32, SIZE - 16, 0.0) > 0.95, "right half from frame 0");
}

/// Odd, non-power-of-two frames go through the padding, with both
/// consistency filters and denoise on. The checkerboard is cropped off its
/// grid, or every frame's detail is zero at its squares' centers and the
/// vote swaps on rounding.
#[test]
fn stacks_odd_sizes_with_every_filter() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene =
        image::imageops::crop_imm(&crate::common::sharp_scene(SIZE), 2, 2, 509, 381).to_image();
    let frames = half_blurred(&scene);
    for consistency in 0..=2 {
        let config = WaveletConfig { denoise: 2.0, consistency };
        let stacker = stack(&ctx, config, &frames, &[None; 2], false);
        let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
        assert_eq!((width, height), (509, 381));
        let diff = mean_abs_diff(bytemuck::cast_slice(&bytes), &scene);
        assert!(diff < 0.03, "consistency {consistency}: deviates by {diff:.4}");
    }
}

/// Reassignment copies source pixels, so a one-frame stack is the frame;
/// linear frames too, which are matched in sRGB but stored as they are.
#[test]
fn a_single_frame_stack_is_the_frame() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frame = DynamicImage::ImageRgb8(blur_half(&crate::common::sharp_scene(256), true));
    for is_linear in [false, true] {
        let frames = std::slice::from_ref(&frame);
        let stacker = stack(&ctx, WaveletConfig::default(), frames, &[None], is_linear);
        let (bytes, _, _) = download_result(&ctx, stacker.result_texture());
        let diff = mean_abs_diff(bytemuck::cast_slice(&bytes), frame.as_rgb8().unwrap());
        assert!(diff < 1e-6, "linear {is_linear}: differs from the frame by {diff}");
    }
}

/// An identity alignment goes through the warp and footprint paths and
/// must give what no alignment gives.
#[test]
fn identity_alignment_matches_unaligned() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frames = half_blurred(&crate::common::sharp_scene(256));
    let identity = FrameAlignment::identity();
    let plain = stack(&ctx, WaveletConfig::default(), &frames, &[None; 2], false);
    let aligned = stack(&ctx, WaveletConfig::default(), &frames, &[Some(&identity); 2], false);
    let a = download_result(&ctx, plain.result_texture()).0;
    let b = download_result(&ctx, aligned.result_texture()).0;
    assert_eq!(a, b);
}

/// A shifted frame's edge smear never wins: where frame 1 has no data
/// (its warp lands off the frame), the result comes from frame 0.
#[test]
fn edge_smear_loses_to_data() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene = crate::common::sharp_scene(256);
    let frames =
        [DynamicImage::ImageRgb8(blur_half(&scene, true)), DynamicImage::ImageRgb8(scene.clone())];
    // Frame 1 sampled 40 px to the right: reference x > 215 is off it.
    let shift = FrameAlignment::from_transform(nalgebra::Affine2::from_matrix_unchecked(
        nalgebra::Matrix3::new(1.0, 0.0, 40.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
    ));
    let stacker = stack(&ctx, WaveletConfig::default(), &frames, &[None, Some(&shift)], false);
    let map = planes(&ctx, &stacker);
    for y in 0..256 {
        for x in 220..256 {
            assert_eq!(map[y * 256 + x], 0.0, "({x}, {y}) came from the smear");
        }
    }
}

/// The transform mirrors each frame at all four edges, so with a frame
/// brighter on one side the left and top edges take the sharp frame as
/// the right and bottom do, not a step wrapped in from the far side.
#[test]
fn every_edge_takes_the_sharp_frame() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    // Padded on both axes, so the right and bottom edges are mirrored too.
    let (w, h) = (509u32, 381u32);
    let noise = |x: u32, y: u32| {
        let mut v = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2C1B_3C6D);
        v ^= v >> 12;
        (v & 0x3f) as i32 - 32
    };
    // Fine detail over a ramp, dark at the top left and bright at the
    // bottom right.
    let sharp = Rgb8Image::from_fn(w, h, |x, y| {
        let ramp = 30 + (190 * (x + y) / (w + h)) as i32;
        image::Rgb([(ramp + noise(x, y)).clamp(0, 255) as u8; 3])
    });
    let blurred = Rgb8Image::from_fn(w, h, |x, y| {
        let mut sum = 0u32;
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as u32;
                let sy = (y as i32 + dy).clamp(0, h as i32 - 1) as u32;
                sum += sharp.get_pixel(sx, sy)[0] as u32;
            }
        }
        image::Rgb([(sum / 25) as u8; 3])
    });
    let frames = [DynamicImage::ImageRgb8(sharp), DynamicImage::ImageRgb8(blurred)];
    let stacker = stack(&ctx, WaveletConfig::default(), &frames, &[None; 2], false);
    let map = planes(&ctx, &stacker);
    // The share of the pixels within 6 of an edge that took the blurred
    // frame (plane 1).
    let near = |edge: &str, x: u32, y: u32| match edge {
        "left" => x < 6,
        "top" => y < 6,
        "right" => x >= w - 6,
        _ => y >= h - 6,
    };
    for edge in ["left", "top", "right", "bottom"] {
        let (mut all, mut hit) = (0, 0);
        for y in 0..h {
            for x in (0..w).filter(|&x| near(edge, x, y)) {
                all += 1;
                hit += (map[(y * w + x) as usize] != 0.0) as u32;
            }
        }
        let share = hit as f64 / all as f64;
        assert!(share < 0.02, "the {edge} edge took the blurred frame at {:.1}%", share * 100.0);
    }
}

#[test]
fn a_left_out_frame_is_not_reassigned() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frames = half_blurred(&crate::common::sharp_scene(128));
    let mut stacker = WaveletStacker::new(ctx.clone(), WaveletConfig::default(), info(3));
    stacker.accumulate(&frames[0], None).unwrap();
    stacker.skip_frame().unwrap();
    stacker.accumulate(&frames[1], None).unwrap();
    assert!(stacker.accumulate(&frames[1], None).is_err(), "a 4th frame of 3");
    stacker.end_pass_one().unwrap();
    assert!(stacker.blend_frame(&frames[1], 1, None).is_err());
    stacker.blend_frame(&frames[0], 0, None).unwrap();
    stacker.blend_frame(&frames[1], 2, None).unwrap();
    stacker.finalize().unwrap();
    let map = planes(&ctx, &stacker);
    assert!(map.iter().all(|&p| p == 0.0 || p == 1.0), "only frames 0 and 2 (planes 0, 1)");
}

/// The picture while a stack runs names frames, as planes: in pass 1 the
/// frame the finest detail came from so far, held through pass 2, then
/// the frame map. Reduced, it takes one pixel of each footprint, so it
/// shows no plane between two frames'.
#[test]
fn the_live_picture_names_the_frame_the_finest_detail_came_from() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frames = half_blurred(&crate::common::sharp_scene(SIZE));
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let live = |stacker: &WaveletStacker, side: u32| -> Vec<u8> {
        let picture = stacker.render_live(side, format).expect("live picture");
        assert_eq!((picture.width(), picture.height()), (side, side));
        let rgba = download_texture(&ctx, &picture).unwrap();
        rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
    };

    let mut stacker = WaveletStacker::new(ctx.clone(), WaveletConfig::default(), info(2));
    assert!(stacker.render_live(32, format).is_err(), "no picture before a frame");
    stacker.accumulate(&frames[0], None).unwrap();
    assert!(live(&stacker, SIZE / 2).iter().all(|&v| v == 0), "one frame: its plane everywhere");
    stacker.accumulate(&frames[1], None).unwrap();

    // The planes are kept at half the frame's size. Frame 1 (plane 1) is
    // sharp on the left, frame 0 on the right.
    let half = SIZE / 2;
    let pass_one = live(&stacker, half);
    assert!(pass_one.iter().all(|&v| v == 0 || v == 255), "two frames: two planes");
    let share = |left: bool, plane: u8| {
        let side =
            pass_one.iter().enumerate().filter(|(i, _)| ((*i as u32 % half) < half / 2) == left);
        let (n, hit) = side.fold((0, 0), |(n, hit), (_, &v)| (n + 1, hit + (v == plane) as u32));
        hit as f64 / n as f64
    };
    assert!(share(true, 255) > 0.9, "frame 1 on its sharp half: {:.3}", share(true, 255));
    assert!(share(false, 0) > 0.9, "frame 0 on its sharp half: {:.3}", share(false, 0));

    let small = live(&stacker, 32);
    assert!(small.iter().all(|&v| v == 0 || v == 255), "a pixel is picked, not averaged");
    assert!(small.contains(&0) && small.contains(&255), "both frames show");

    stacker.end_pass_one().unwrap();
    assert_eq!(live(&stacker, half), pass_one, "pass 2 holds pass 1's picture");
    stacker.blend_frame(&frames[0], 0, None).unwrap();
    stacker.blend_frame(&frames[1], 1, None).unwrap();
    assert_eq!(live(&stacker, half), pass_one, "until the stack is finalized");
    stacker.finalize().unwrap();
    let map: Vec<u8> = planes(&ctx, &stacker).iter().map(|p| (p * 255.0).round() as u8).collect();
    assert_eq!(live(&stacker, SIZE), map, "then it is the frame map");
}

/// Reassignment does not depend on the order the frames come in, ties
/// included (duplicate frames tie everywhere).
#[test]
fn blend_order_does_not_matter() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let [a, b] = half_blurred(&crate::common::sharp_scene(256));
    let frames = [a.clone(), b, a];
    let run = |order: &[usize]| {
        let mut stacker = WaveletStacker::new(ctx.clone(), WaveletConfig::default(), info(3));
        for f in &frames {
            stacker.accumulate(f, None).unwrap();
        }
        stacker.end_pass_one().unwrap();
        for &i in order {
            stacker.blend_frame(&frames[i], i as u32, None).unwrap();
        }
        stacker.finalize().unwrap();
        (download_result(&ctx, stacker.result_texture()).0, planes(&ctx, &stacker))
    };
    assert_eq!(run(&[0, 1, 2]), run(&[2, 1, 0]));
}

/// Denoise and each consistency filter change the merged target, and so
/// the frame each pixel takes its color from.
#[test]
fn settings_change_the_target() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    // The same scene with different noise per frame: the winners scatter,
    // so both filters find dissenters and outliers.
    let scene = crate::common::sharp_scene(256);
    let mut seed = 1u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 24) as i32 - 128
    };
    let frames: Vec<DynamicImage> = (0..4)
        .map(|_| {
            let mut f = scene.clone();
            for p in f.pixels_mut() {
                let n = noise() / 4;
                p.0 = p.0.map(|c| (c as i32 + n).clamp(0, 255) as u8);
            }
            DynamicImage::ImageRgb8(f)
        })
        .collect();
    let target = |config: WaveletConfig| {
        let mut stacker = WaveletStacker::new(ctx.clone(), config, info(4));
        for f in &frames {
            stacker.accumulate(f, None).unwrap();
        }
        stacker.end_pass_one().unwrap();
        for (i, f) in frames.iter().enumerate() {
            stacker.blend_frame(f, i as u32, None).unwrap();
        }
        stacker.finalize().unwrap();
        planes(&ctx, &stacker)
    };
    let plain = target(WaveletConfig { denoise: 0.0, consistency: 0 });
    assert_ne!(plain, target(WaveletConfig { denoise: 8.0, consistency: 0 }), "denoise");
    let one = target(WaveletConfig { denoise: 0.0, consistency: 1 });
    let two = target(WaveletConfig { denoise: 0.0, consistency: 2 });
    assert_ne!(plain, one, "subband vote");
    assert_ne!(one, two, "neighbor filter");
}

/// Steps out of order are refused, and leave the stacker usable.
#[test]
fn steps_out_of_order_are_refused() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frames = half_blurred(&crate::common::sharp_scene(128));
    let mut stacker = WaveletStacker::new(ctx.clone(), WaveletConfig::default(), info(2));
    assert!(stacker.end_pass_one().is_err(), "merge before any frame");
    stacker.accumulate(&frames[0], None).unwrap();
    assert!(stacker.blend_frame(&frames[0], 0, None).is_err(), "blend before the merge");
    assert!(stacker.finalize().is_err(), "finalize before the merge");
    stacker.accumulate(&frames[1], None).unwrap();
    stacker.end_pass_one().unwrap();
    assert!(stacker.end_pass_one().is_err(), "merge twice");
    assert!(stacker.skip_frame().is_err(), "left out after the merge");
    assert!(stacker.accumulate(&frames[0], None).is_err(), "accumulate after the merge");
    assert!(stacker.finalize().is_err(), "finalize with nothing reassigned");
    let wrong_size = DynamicImage::new_rgb8(64, 64);
    assert!(stacker.blend_frame(&wrong_size, 0, None).is_err(), "a frame of another size");
    stacker.blend_frame(&frames[0], 0, None).unwrap();
    assert!(stacker.blend_frame(&frames[0], 0, None).is_err(), "reassigned from twice");
    assert!(stacker.finalize().is_err(), "finalize with a frame to come");
    stacker.blend_frame(&frames[1], 1, None).unwrap();
    stacker.finalize().unwrap();
    assert!(stacker.finalize().is_err(), "finalize twice");
    assert!(stacker.result_texture().is_some());
}

/// The wavelet stacker's outputs on a fixed stack against its reference
/// (`golden`): the result and the frame map. Four frames of 203×150 (padded
/// on both axes), one aligned with an exposure correction so part has no
/// data, one left out; at the default settings, with the 2-of-3 vote, and
/// with both consistency filters and no denoise.
#[test]
fn wavelet_output_is_pinned() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frames = golden_frames(203, 150);
    let aligned = golden_alignment();
    let alignments = [None, Some(&aligned), None, None];
    let stacked = [0usize, 1, 3];

    let run = |config: WaveletConfig| {
        let mut stacker = WaveletStacker::new(ctx.clone(), config, info(4));
        for i in 0..4 {
            if stacked.contains(&i) {
                stacker.accumulate(&frames[i], alignments[i]).unwrap();
            } else {
                stacker.skip_frame().unwrap();
            }
        }
        stacker.end_pass_one().unwrap();
        for &i in &stacked {
            stacker.blend_frame(&frames[i], i as u32, alignments[i]).unwrap();
        }
        stacker.finalize().unwrap();
        let (result, ..) = download_result(&ctx, stacker.result_texture());
        let map = download_texture(&ctx, stacker.frame_map().unwrap()).unwrap();
        (result, map)
    };
    let runs = [
        ("default", run(WaveletConfig::default())),
        ("consistency1", run(WaveletConfig { consistency: 1, ..WaveletConfig::default() })),
        ("consistency2-nodenoise", run(WaveletConfig { denoise: 0.0, consistency: 2 })),
    ];
    golden::check(&golden::outputs("wavelet", 203, 150, &runs));
}
