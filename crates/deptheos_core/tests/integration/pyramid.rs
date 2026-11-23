//! End-to-end pyramid stacking on synthetic frames: two of one checkerboard,
//! each blurred on a different half, stack close to the sharp scene.

use crate::common::download_texture;
use crate::golden;
use deptheos_core::pyramid::{PyramidConfig, PyramidStacker};
use deptheos_core::{FrameDepth, StackInfo};
use deptheos_gpu::GpuContext;
use image::{DynamicImage, ImageBuffer, Rgb};

use crate::common::{blur_half, download_result, golden_alignment, golden_frames, mean_abs_diff};

const SIZE: u32 = 512;

/// An sRGB stack of `total_frames` frames.
fn srgb(total_frames: u32) -> StackInfo {
    StackInfo { total_frames, is_linear: false }
}

#[test]
fn stacks_two_half_blurred_frames() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    let scene = crate::common::sharp_scene(SIZE);
    let frame_a = blur_half(&scene, true); // sharp right half
    let frame_b = blur_half(&scene, false); // sharp left half

    let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(2));
    stacker.accumulate(&DynamicImage::ImageRgb8(frame_a), None).expect("accumulate frame A");
    stacker.accumulate(&DynamicImage::ImageRgb8(frame_b), None).expect("accumulate frame B");
    stacker.finalize().expect("finalize");

    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    assert_eq!((width, height), (SIZE, SIZE));

    let pixels: &[f32] = bytemuck::cast_slice(&bytes);

    // Much closer to the sharp scene than a blurred frame (~0.25 mean abs);
    // soft WTA is not pixel-exact, so allow 0.05.
    let diff = mean_abs_diff(pixels, &scene);
    assert!(diff < 0.05, "stacked result deviates from sharp scene: mean abs diff {diff:.4}");

    // Alpha must be exactly 1 after the finishing collapse pass.
    assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 1.0));
}

/// The winner record names, per full-resolution pixel, the frame the
/// result took it from, as that frame's plane: frame A (plane 0) on its
/// sharp right half, frame B (plane 1) on its sharp left half.
#[test]
fn records_which_frame_won_each_pixel() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    let scene = crate::common::sharp_scene(SIZE);
    let frame_a = blur_half(&scene, true); // sharp right half
    let frame_b = blur_half(&scene, false); // sharp left half

    let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(2));
    stacker.accumulate(&DynamicImage::ImageRgb8(frame_a), None).expect("accumulate frame A");
    stacker.accumulate(&DynamicImage::ImageRgb8(frame_b), None).expect("accumulate frame B");
    stacker.finalize().expect("finalize");

    let winners = stacker.frame_map().expect("winners");
    assert_eq!((winners.width(), winners.height()), (SIZE, SIZE));
    let bytes = download_texture(&ctx, winners).unwrap();
    let planes: &[f32] = bytemuck::cast_slice(&bytes);
    assert!(planes.iter().all(|&p| p == 0.0 || p == 1.0), "two frames: two planes");

    // The share of each half that went to the frame sharp there: the record
    // is decided at a coarser level, so only a band at the seam and the
    // border is in doubt.
    let share = |left: bool, plane: f32| {
        let (n, hit) = planes.iter().enumerate().fold((0, 0), |(n, hit), (i, &p)| {
            if ((i as u32 % SIZE) < SIZE / 2) == left {
                (n + 1, hit + (p == plane) as u32)
            } else {
                (n, hit)
            }
        });
        hit as f64 / n as f64
    };
    let (a_right, b_left) = (share(false, 0.0), share(true, 1.0));
    assert!(a_right > 0.95, "frame A owns its sharp half: {a_right:.3}");
    assert!(b_left > 0.95, "frame B owns its sharp half: {b_left:.3}");
}

/// The picture while a stack runs is the winner record so far, as planes.
/// Reduced, it takes one pixel of each footprint, so it shows no plane
/// between two frames'; at full size it is the finished frame map.
#[test]
fn the_live_picture_is_the_winner_record_so_far() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene = crate::common::sharp_scene(SIZE);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let live = |stacker: &PyramidStacker, side: u32| -> Vec<u8> {
        let picture = stacker.render_live(side, format).expect("live picture");
        assert_eq!((picture.width(), picture.height()), (side, side));
        let rgba = download_texture(&ctx, &picture).unwrap();
        rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
    };

    let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(2));
    assert!(stacker.render_live(SIZE, format).is_err(), "no picture before a frame");
    stacker.accumulate(&DynamicImage::ImageRgb8(blur_half(&scene, true)), None).unwrap();
    assert!(live(&stacker, SIZE).iter().all(|&v| v == 0), "one frame: its plane everywhere");
    stacker.accumulate(&DynamicImage::ImageRgb8(blur_half(&scene, false)), None).unwrap();

    let small = live(&stacker, SIZE / 3);
    assert!(small.iter().all(|&v| v == 0 || v == 255), "a pixel is picked, not averaged");
    assert!(small.contains(&0) && small.contains(&255), "both frames show");

    let full = live(&stacker, SIZE);
    stacker.finalize().expect("finalize");
    let bytes = download_texture(&ctx, stacker.frame_map().expect("winners")).unwrap();
    let planes: &[f32] = bytemuck::cast_slice(&bytes);
    let map: Vec<u8> = planes.iter().map(|p| (p * 255.0).round() as u8).collect();
    assert_eq!(full, map, "the last picture is the frame map");
    assert_eq!(live(&stacker, SIZE), map, "and still is once finalized");
}

/// The linear path encodes to sRGB for the pyramid math and decodes back,
/// so a frame stacked with itself returns the original values.
#[test]
fn linear_round_trip_preserves_values() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    // 16-bit, with dark values where the sRGB curve is steepest.
    let frame = ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(256, 256, |x, y| {
        image::Rgb([(x * 257) as u16, (y * 257) as u16, ((x + y) * 128) as u16])
    });
    let source = DynamicImage::ImageRgb16(frame);

    let mut stacker = PyramidStacker::new(
        ctx.clone(),
        PyramidConfig::default(),
        StackInfo { total_frames: 2, is_linear: true },
    );
    stacker.accumulate(&source, None).expect("accumulate 1");
    stacker.accumulate(&source, None).expect("accumulate 2");
    stacker.finalize().expect("finalize");

    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    let pixels: &[f32] = bytemuck::cast_slice(&bytes);
    let reference = source.to_rgb16();

    let mut worst = 0.0f64;
    for (i, p) in reference.pixels().enumerate() {
        let base = i * 4;
        for c in 0..3 {
            let expected = p[c] as f64 / 65535.0;
            worst = worst.max((pixels[base + c] as f64 - expected).abs());
        }
    }
    assert_eq!((width, height), (256, 256));
    assert!(worst < 0.01, "linear round trip drifted by {worst:.5} (max channel error)");
}

/// The ingest warp reproduces the source exactly under an identity
/// transform: a single frame collapses losslessly, so any deviation is the
/// resampler. `lanczos_sample` places pixel *j*'s center at *j*, as
/// `bilinear_sample` and the aligner do.
#[test]
fn identity_warp_reproduces_source() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    // A half-pixel error blends the tones at every square boundary.
    let scene = crate::common::sharp_scene(SIZE);

    let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(1));
    stacker.accumulate(&DynamicImage::ImageRgb8(scene.clone()), None).expect("accumulate");
    stacker.finalize().expect("finalize");

    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    assert_eq!((width, height), (SIZE, SIZE));
    let pixels: &[f32] = bytemuck::cast_slice(&bytes);

    let mut worst = 0.0f64;
    for (i, p) in scene.pixels().enumerate() {
        let base = i * 4;
        for c in 0..3 {
            worst = worst.max((pixels[base + c] as f64 - p[c] as f64 / 255.0).abs());
        }
    }
    assert!(
        worst < 0.02,
        "identity warp altered the source by {worst:.5} (max channel error) — \
         check the sampler's pixel-center convention in warp_common.wgsl"
    );
}

/// A frame cannot win pixels it has no data for. Frame B is sharp but
/// shifted so a right-hand strip falls outside it; the warp's edge smear
/// there would out-score A's defocused content, so the margin mask must
/// give the strip to A, and the covered region to B.
#[test]
fn shifted_frame_cannot_win_outside_its_footprint() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    const SHIFT: u32 = 64;
    let scene = crate::common::sharp_scene(SIZE);
    // Blur both halves: A is defocused everywhere.
    let frame_a = blur_half(&blur_half(&scene, true), false);
    let frame_b = scene.clone();
    // Reference → frame: src_x = x + SHIFT, so x ≥ SIZE − SHIFT has no
    // data in B.
    let shift = deptheos_core::Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
        1.0,
        0.0,
        SHIFT as f32, //
        0.0,
        1.0,
        0.0, //
        0.0,
        0.0,
        1.0,
    ));

    let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(2));
    stacker.accumulate(&DynamicImage::ImageRgb8(frame_a.clone()), None).expect("accumulate A");
    stacker
        .accumulate(&DynamicImage::ImageRgb8(frame_b), Some(&shift.into()))
        .expect("accumulate B");
    stacker.finalize().expect("finalize");

    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    assert_eq!((width, height), (SIZE, SIZE));
    let pixels: &[f32] = bytemuck::cast_slice(&bytes);

    // Stay clear of the footprint boundary: the coarsest level's mask fade
    // spans tens of pixels either side.
    const GUARD: u32 = 24;
    let mut outside = (0.0f64, 0u64); // vs frame A
    let mut inside = (0.0f64, 0u64); // vs shifted scene (B's data)
    for y in GUARD..SIZE - GUARD {
        for x in GUARD..SIZE {
            let base = ((y * SIZE + x) * 4) as usize;
            if x >= SIZE - SHIFT + GUARD {
                let p = frame_a.get_pixel(x, y);
                for c in 0..3 {
                    outside.0 += (pixels[base + c] as f64 - p[c] as f64 / 255.0).abs();
                    outside.1 += 1;
                }
            } else if x < SIZE - SHIFT - GUARD {
                let p = scene.get_pixel(x + SHIFT, y);
                for c in 0..3 {
                    inside.0 += (pixels[base + c] as f64 - p[c] as f64 / 255.0).abs();
                    inside.1 += 1;
                }
            }
        }
    }
    let outside = outside.0 / outside.1 as f64;
    let inside = inside.0 / inside.1 as f64;
    assert!(
        inside < 0.05,
        "covered region deviates from frame B's data: mean abs diff {inside:.4}"
    );
    assert!(
        outside < 0.05,
        "uncovered strip deviates from frame A: mean abs diff {outside:.4} — \
         the shifted frame's smeared margin won pixels it has no data for"
    );
}

/// Where no frame has data (outside every footprint, and the first pixel
/// of each coarse level when all frames shift the same way) the stack
/// falls back to the order-independent blend; otherwise the collapse
/// spreads a band along the boundary. Two frames with one shift, stacked
/// in both orders, agree everywhere.
#[test]
fn common_shift_leaves_no_band_at_the_boundary() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    const SHIFT: u32 = 24;
    let scene = crate::common::sharp_scene(SIZE);
    let frame_a = DynamicImage::ImageRgb8(scene.clone());
    let blurred = blur_half(&blur_half(&scene, true), false);
    let frame_b = DynamicImage::ImageRgb8(ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
        let p = blurred.get_pixel(x, y);
        Rgb([p[0].saturating_add(77), p[1].saturating_add(77), p[2].saturating_add(77)])
    }));
    // Reference → frame: src = dst − SHIFT.
    let shift = deptheos_core::Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
        1.0,
        0.0,
        -(SHIFT as f32), //
        0.0,
        1.0,
        -(SHIFT as f32), //
        0.0,
        0.0,
        1.0,
    ));

    let stack = |first: &DynamicImage, second: &DynamicImage| -> Vec<f32> {
        let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(2));
        stacker.accumulate(first, Some(&shift.into())).expect("accumulate first");
        stacker.accumulate(second, Some(&shift.into())).expect("accumulate second");
        stacker.finalize().expect("finalize");
        let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
        assert_eq!((width, height), (SIZE, SIZE));
        bytemuck::cast_slice(&bytes).to_vec()
    };
    let ab = stack(&frame_a, &frame_b);
    let ba = stack(&frame_b, &frame_a);

    let mut worst = (0.0f32, 0u32, 0u32);
    for (i, (p, q)) in ab.chunks(4).zip(ba.chunks(4)).enumerate() {
        for c in 0..3 {
            let d = (p[c] - q[c]).abs();
            if d > worst.0 {
                worst = (d, i as u32 % SIZE, i as u32 / SIZE);
            }
        }
    }
    assert!(
        worst.0 < 0.01,
        "stack depends on frame order: max channel difference {:.4} at ({}, {})",
        worst.0,
        worst.1,
        worst.2
    );
}

/// A frame's margin does not bleed into the area it covers. A frame
/// shifted by a multiple of the checker's 16 px period shows the same scene
/// wherever it has data, so the stack reproduces it up to the boundary.
/// Black margins would darken the shifted frame's coarse levels inside its
/// boundary and win as a bright band; edge smear avoids it.
#[test]
fn margin_does_not_bleed_into_covered_area() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    const SHIFT: u32 = 96; // multiple of the 16 px checker period
    let scene = crate::common::sharp_scene(SIZE);
    let shift = deptheos_core::Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
        1.0,
        0.0,
        -(SHIFT as f32), //
        0.0,
        1.0,
        -(SHIFT as f32), //
        0.0,
        0.0,
        1.0,
    ));

    let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(4));
    stacker
        .accumulate(&DynamicImage::ImageRgb8(scene.clone()), None)
        .expect("accumulate identity frame");
    stacker
        .accumulate(&DynamicImage::ImageRgb8(scene.clone()), Some(&shift.into()))
        .expect("accumulate shifted frame");
    stacker.finalize().expect("finalize");

    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    assert_eq!((width, height), (SIZE, SIZE));
    let pixels: &[f32] = bytemuck::cast_slice(&bytes);

    // Mean luminance error per 16-row band, from the boundary inward.
    let mut worst = (0.0f64, 0u32);
    for band in 0..8 {
        let y0 = SHIFT + band * 16;
        let mut total = 0.0f64;
        let mut count = 0u64;
        for y in y0..y0 + 16 {
            for x in SHIFT..SIZE {
                let base = ((y * SIZE + x) * 4) as usize;
                let p = scene.get_pixel(x, y);
                for c in 0..3 {
                    total += (pixels[base + c] as f64 - p[c] as f64 / 255.0).abs();
                    count += 1;
                }
            }
        }
        let err = total / count as f64;
        if err > worst.0 {
            worst = (err, y0);
        }
    }
    assert!(
        worst.0 < 0.03,
        "shifted frame's margin bled into the covered area: rows {}..{} deviate from \
         the scene by {:.4} mean abs",
        worst.1,
        worst.1 + 16,
        worst.0
    );
}

/// Frames under 32 px a side stack: the pyramid keeps at least two levels.
#[test]
fn tiny_frames_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    for size in [1, 7, 24] {
        let frame =
            DynamicImage::ImageRgb8(ImageBuffer::from_pixel(size, size, Rgb([90, 120, 30])));
        let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(2));
        stacker.accumulate(&frame, None).expect("accumulate");
        stacker.accumulate(&frame, None).expect("accumulate");
        for format in [wgpu::TextureFormat::Rgba8Unorm, ctx.map_format()] {
            stacker.render_live(1024, format).expect("live picture");
        }
        stacker.finalize().expect("finalize");
        let (result, w, h) = download_result(&ctx, stacker.result_texture());
        assert_eq!((w, h), (size, size));
        let first: &[f32] = &bytemuck::cast_slice(&result)[..3];
        for (c, want) in first.iter().zip([90.0, 120.0, 30.0]) {
            assert!((c - want / 255.0).abs() < 1e-3, "{size}px: {first:?}");
        }
    }
}

/// A frame wider than the GPU's texture limit is an error before anything
/// is allocated, not a wgpu panic.
#[test]
fn a_frame_past_the_texture_limit_is_an_error() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let too_wide = ctx.device.limits().max_texture_dimension_2d + 1;
    let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(2));
    let err = stacker.allocate(too_wide, 64, FrameDepth::Eight).expect_err("past the limit");
    assert!(err.to_string().contains("texture limit"), "{err}");
    ctx.check().expect("nothing reached wgpu");
}

/// Five frames of one scene, each sharp in its own vertical band, with
/// small different shifts so the transforms matter.
fn banded_frames(n: u32) -> (Vec<DynamicImage>, Vec<deptheos_core::FrameAlignment>) {
    use deptheos_core::{Affine2, FrameAlignment};
    let scene = crate::common::sharp_scene(SIZE);
    let blurred = blur_half(&blur_half(&scene, true), false);
    let band = SIZE / n;
    let frames = (0..n)
        .map(|f| {
            let image = ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
                if x / band == f { *scene.get_pixel(x, y) } else { *blurred.get_pixel(x, y) }
            });
            DynamicImage::ImageRgb8(image)
        })
        .collect();
    let transforms = (0..n)
        .map(|f| {
            #[rustfmt::skip]
            let m = nalgebra::Matrix3::new(
                1.0 + 0.002 * f as f32, 0.0, 0.7 * f as f32,
                0.0, 1.0 + 0.002 * f as f32, -0.4 * f as f32,
                0.0, 0.0, 1.0,
            );
            FrameAlignment::new(Affine2::from_matrix_unchecked(m), 1.0 - 0.01 * f as f32)
        })
        .collect();
    (frames, transforms)
}

/// A collapse after `k` frames is the `k`-frame stack: no whole-stack
/// information goes in, so a grow sequence can be read off one pass.
/// Checked in both orders.
#[test]
fn a_collapse_after_k_frames_is_the_k_frame_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let n = 5;
    let (frames, transforms) = banded_frames(n);
    let read = |t: &wgpu::Texture| -> Vec<f32> {
        bytemuck::cast_slice(&download_texture(&ctx, t).unwrap()).to_vec()
    };
    for reverse in [false, true] {
        let mut order: Vec<usize> = (0..n as usize).collect();
        if reverse {
            order.reverse();
        }
        let mut running = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(n));
        assert!(matches!(running.collapse(), Err(deptheos_core::CoreError::NoFrames)));
        for k in 1..=n as usize {
            let i = order[k - 1];
            running.accumulate(&frames[i], Some(&transforms[i])).unwrap();
            let snapshot = read(running.collapse().unwrap());

            let mut separate = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(n));
            for &j in &order[..k] {
                separate.accumulate(&frames[j], Some(&transforms[j])).unwrap();
            }
            separate.finalize().unwrap();
            let expected = read(separate.result_texture().unwrap());
            let worst =
                snapshot.iter().zip(&expected).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-4, "reverse {reverse}, k {k}: max abs diff {worst}");
        }
        // Accumulating goes on after a collapse, and the stack finalizes.
        running.finalize().unwrap();
    }

    // Order does not change the full stack either.
    let full = |order: &[usize]| {
        let mut s = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), srgb(n));
        for &i in order {
            s.accumulate(&frames[i], Some(&transforms[i])).unwrap();
        }
        s.finalize().unwrap();
        read(s.result_texture().unwrap())
    };
    let (forward, backward) = (full(&[0, 1, 2, 3, 4]), full(&[4, 3, 2, 1, 0]));
    let worst = forward.iter().zip(&backward).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "forward vs backward: max abs diff {worst}");
}

/// A left-out frame drops out of the stack completely: two frames with an
/// empty slot between them give a plain two-frame stack's result.
#[test]
fn a_left_out_frame_leaves_the_two_frame_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene = crate::common::sharp_scene(128);
    let a = DynamicImage::ImageRgb8(blur_half(&scene, true));
    let b = DynamicImage::ImageRgb8(blur_half(&scene, false));

    let run = |skip_middle: bool| {
        let mut stacker = PyramidStacker::new(
            ctx.clone(),
            PyramidConfig::default(),
            srgb(if skip_middle { 3 } else { 2 }),
        );
        stacker.accumulate(&a, None).unwrap();
        if skip_middle {
            stacker.skip_frame().unwrap();
        }
        stacker.accumulate(&b, None).unwrap();
        stacker.finalize().unwrap();
        download_result(&ctx, stacker.result_texture()).0
    };
    assert_eq!(run(true), run(false), "result differs");
}

/// The pyramid stacker's outputs on a fixed stack against its reference
/// (`golden`): the result and the winner record. Four frames of 203×150
/// (not a multiple of the 16 px tile), one aligned with an exposure
/// correction so part is masked, one left out; at smoothness 1 (no focus
/// blur) and 4.
#[test]
fn pyramid_output_is_pinned() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frames = golden_frames(203, 150);
    let aligned = golden_alignment();

    let run = |smoothness: f32| {
        let mut stacker = PyramidStacker::new(ctx.clone(), PyramidConfig { smoothness }, srgb(4));
        stacker.accumulate(&frames[0], None).unwrap();
        stacker.accumulate(&frames[1], Some(&aligned)).unwrap();
        stacker.skip_frame().unwrap();
        stacker.accumulate(&frames[3], None).unwrap();
        stacker.finalize().unwrap();
        let (result, ..) = download_result(&ctx, stacker.result_texture());
        let record = download_texture(&ctx, stacker.frame_map().unwrap()).unwrap();
        (result, record)
    };
    let runs = [("smoothness1", run(1.0)), ("smoothness4", run(4.0))];
    golden::check(&golden::outputs("pyramid", 203, 150, &runs));
}
