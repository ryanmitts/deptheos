//! End-to-end depth stacking on synthetic frames: the depth map's picks
//! and the blended result, the focus map's warp, the pyramid blend and its
//! warp kernel, left-out frames, sizes down to a pixel, the stacker's
//! refusals, determinism, and the output held to its reference.

use std::sync::Arc;

use crate::common::download_texture;
use crate::golden;
use deptheos_core::depth::{DepthConfig, DepthStacker};
use deptheos_core::{FrameAlignment, StackInfo};
use deptheos_gpu::GpuContext;
use image::{DynamicImage, ImageBuffer, Rgb};

use crate::common::{
    blur_half, download_depth_map, download_result, golden_alignment, golden_frames, mean_abs_diff,
};

/// The two-frame sRGB stack most tests here build.
const TWO_SRGB: StackInfo = StackInfo { total_frames: 2, is_linear: false };

const SIZE: u32 = 512;

/// One slot of a stack: a frame and its alignment, or `None` for a frame
/// left out.
type Slot<'a> = Option<(&'a DynamicImage, Option<&'a FrameAlignment>)>;

/// `frames` as slots, each stacked and unaligned.
fn unaligned(frames: &[DynamicImage]) -> Vec<Slot<'_>> {
    frames.iter().map(|f| Some((f, None))).collect()
}

/// Run a whole depth stack over `slots`, an sRGB stack of as many frames,
/// and return the finished stacker.
fn stack(ctx: &Arc<GpuContext>, config: DepthConfig, slots: &[Slot]) -> DepthStacker {
    let info = StackInfo { total_frames: slots.len() as u32, is_linear: false };
    let mut stacker = DepthStacker::new(ctx.clone(), config, info);
    for slot in slots {
        match slot {
            Some((frame, alignment)) => stacker.accumulate(*frame, *alignment).expect("accumulate"),
            None => stacker.skip_frame().expect("leave out"),
        }
    }
    stacker.end_pass_one().expect("finalize depth map");
    for (i, slot) in slots.iter().enumerate() {
        if let Some((frame, alignment)) = slot {
            stacker.blend_frame(*frame, i as u32, *alignment).expect("blend frame");
        }
    }
    stacker.finalize().expect("finalize");
    stacker
}

/// The finished stack's result as RGBA floats.
fn result(ctx: &GpuContext, stacker: &DepthStacker) -> Vec<f32> {
    let (bytes, ..) = download_result(ctx, stacker.result_texture());
    bytemuck::cast_slice(&bytes).to_vec()
}

#[test]
fn depth_stacks_two_half_blurred_frames() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    let scene = crate::common::sharp_scene(SIZE);
    let frames = [
        DynamicImage::ImageRgb8(blur_half(&scene, true)), // sharp right
        DynamicImage::ImageRgb8(blur_half(&scene, false)), // sharp left
    ];
    let stacker = stack(&ctx, DepthConfig::default(), &unaligned(&frames));

    // Depth map sanity: frame 0 is sharp on the right (depth 0), frame 1 on
    // the left (depth 1). Sample away from the center seam and borders.
    let (depth_bytes, dw, _) = download_depth_map(&ctx, &stacker);
    let depth_at = |x: u32, y: u32| depth_bytes[((y * dw + x) * 4) as usize] as f64 / 255.0;
    let mut left_sum = 0.0;
    let mut right_sum = 0.0;
    let mut n = 0u32;
    for y in (64..SIZE - 64).step_by(16) {
        left_sum += depth_at(SIZE / 4, y);
        right_sum += depth_at(3 * SIZE / 4, y);
        n += 1;
    }
    let left_mean = left_sum / n as f64;
    let right_mean = right_sum / n as f64;
    assert!(
        left_mean > 0.6,
        "left half should map to frame 1 (sharp left); mean depth {left_mean:.3}"
    );
    assert!(
        right_mean < 0.4,
        "right half should map to frame 0 (sharp right); mean depth {right_mean:.3}"
    );

    let (bytes, width, height) = download_result(&ctx, stacker.result_texture());
    assert_eq!((width, height), (SIZE, SIZE));

    let pixels: &[f32] = bytemuck::cast_slice(&bytes);

    // The blend recovers the sharp scene; depth blending is softer than
    // pyramid WTA, so the tolerance is looser.
    let diff = mean_abs_diff(pixels, &scene);
    assert!(diff < 0.08, "stacked result deviates from sharp scene: mean abs diff {diff:.4}");

    // The collapse writes every pixel opaque (`collapse_level`), so this
    // checks it covered the whole result.
    assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 1.0));
}

/// The focus metric is measured on each frame's own grid and only the
/// finished map is warped, so an identity alignment reproduces the
/// unaligned path bit for bit.
///
/// A wiring test: it catches a mis-bound texture or uniform, a grid
/// transform not exactly the identity, or a half-pixel offset in
/// `warp_common.wgsl`. It does not measure the benefit of warping the map.
#[test]
fn identity_transform_matches_unaligned_path() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    let scene = crate::common::sharp_scene(SIZE);
    let frame_a = DynamicImage::ImageRgb8(blur_half(&scene, true));
    let frame_b = DynamicImage::ImageRgb8(blur_half(&scene, false));
    let frames = [&frame_a, &frame_b];

    let depth_map_with = |transform: Option<deptheos_core::FrameAlignment>| {
        let config = DepthConfig::default();
        let mut stacker = DepthStacker::new(ctx.clone(), config, TWO_SRGB);
        for frame in frames {
            stacker.accumulate(frame, transform.as_ref()).expect("accumulate");
        }
        // The WTA map, before finalization, which runs identically on both.
        download_depth_map(&ctx, &stacker)
    };

    let (unaligned, w0, h0) = depth_map_with(None);
    let (identity, w1, h1) = depth_map_with(Some(deptheos_core::FrameAlignment::identity()));

    assert_eq!((w0, h0), (w1, h1));
    let differing = unaligned.iter().zip(identity.iter()).filter(|(a, b)| a != b).count();
    assert_eq!(
        differing,
        0,
        "the identity warp is not a no-op: {differing} of {} bytes differ",
        unaligned.len()
    );
}

/// A frame scores zero focus where it has no data, so it cannot win those
/// pixels even when it is sharper everywhere else.
///
/// The map sampler returns zero out of range: edge-clamping would smear a
/// full-strength edge focus across the margin, into the WTA, the
/// confidence histogram and the smoothing.
///
/// The sharp frame sits 64 px to the right, so columns past `SIZE - 64`
/// have no data from it. Smoothing is off to isolate the WTA.
#[test]
fn frames_score_zero_focus_outside_their_own_data() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    const SHIFT: u32 = 64;
    let scene = crate::common::sharp_scene(SIZE);
    let blurred = DynamicImage::ImageRgb8(blur_half(&blur_half(&scene, true), false));
    let sharp = DynamicImage::ImageRgb8(scene.clone());
    let shift = deptheos_core::Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
        1.0,
        0.0,
        SHIFT as f32,
        0.0,
        1.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ));

    let config = DepthConfig { smoothing: 0.0, ..Default::default() };
    let mut stacker = DepthStacker::new(ctx.clone(), config, TWO_SRGB);
    stacker.accumulate(&blurred, None).expect("accumulate blurred");
    stacker.accumulate(&sharp, Some(&shift.into())).expect("accumulate sharp");
    stacker.end_pass_one().expect("finalize depth map");

    let (depth, dw, _) = download_depth_map(&ctx, &stacker);
    let depth_at = |x: u32, y: u32| depth[((y * dw + x) * 4) as usize] as f64 / 255.0;
    let mean_over = |xs: std::ops::Range<u32>| {
        let mut sum = 0.0;
        let mut n = 0u32;
        for y in (64..SIZE - 64).step_by(8) {
            for x in xs.clone().step_by(8) {
                sum += depth_at(x, y);
                n += 1;
            }
        }
        sum / n as f64
    };

    // Well inside the valid area: the sharp frame (index 1, depth 1.0) wins.
    let valid_mean = mean_over(64..SIZE - SHIFT - 16);
    // Past the sharp frame's right edge it has no data at all.
    let margin_mean = mean_over(SIZE - SHIFT + 16..SIZE);

    assert!(
        valid_mean > 0.8,
        "sharp frame should win where it has data; mean depth {valid_mean:.3}"
    );
    assert!(
        margin_mean < 0.2,
        "frame with no data won its invalid margin; mean depth {margin_mean:.3}"
    );
}

/// The pyramid blend is an exact decomposition: two identical frames give
/// the input back. Pins the σ = 0.666 down/up pair, the dropped-tap edge
/// renormalization and the collapse, independently of any depth map.
#[test]
fn pyramid_blend_of_identical_frames_reproduces_the_frame() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    // Odd size so every level drops an edge tap; a gradient plus the checker
    // for low and high frequencies.
    const W: u32 = 301;
    const H: u32 = 203;
    let scene = ImageBuffer::from_fn(W, H, |x, y| {
        let on = ((x / 5) + (y / 7)) % 2 == 0;
        let g = (x * 255 / W) as u8;
        if on { Rgb([220, g, 60]) } else { Rgb([30, g, 180]) }
    });
    let frame = DynamicImage::ImageRgb8(scene.clone());

    let stacker = stack(&ctx, DepthConfig::default(), &unaligned(&[frame.clone(), frame]));
    let result = result(&ctx, &stacker);
    assert_eq!(result.len(), (W * H * 4) as usize);
    let mut max_err = 0.0f32;
    for (i, p) in scene.pixels().enumerate() {
        for c in 0..3 {
            max_err = max_err.max((result[i * 4 + c] - p[c] as f32 / 255.0).abs());
        }
    }
    assert!(max_err < 2.0 / 255.0, "pyramid round trip is lossy: max err {max_err}");
}

/// Keys cubic with `B = 0, C = 0.6121`, the blend warp's kernel.
fn keys_weight(x: f32) -> f32 {
    const C: f32 = 0.6121;
    let a = x.abs();
    if a < 1.0 {
        (2.0 - C) * a * a * a - (3.0 - C) * a * a + 1.0
    } else if a < 2.0 {
        -C * a * a * a + 5.0 * C * a * a - 8.0 * C * a + 4.0 * C
    } else {
        0.0
    }
}

/// CPU twin of `keys_sample` in `warp_common.wgsl`: position clamped to
/// the pixel-center range, 4×4 taps from `floor − 1`, out-of-image taps
/// dropped and the rest renormalized.
fn keys_sample_cpu(img: &ImageBuffer<Rgb<u8>, Vec<u8>>, x: f32, y: f32, c: usize) -> f32 {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let sx = x.clamp(0.0, (w - 1) as f32);
    let sy = y.clamp(0.0, (h - 1) as f32);
    let bx = sx.floor() as i32 - 1;
    let by = sy.floor() as i32 - 1;
    let mut sum = 0.0;
    let mut wsum = 0.0;
    for dy in 0..4 {
        let py = by + dy;
        if py < 0 || py >= h {
            continue;
        }
        for dx in 0..4 {
            let px = bx + dx;
            if px < 0 || px >= w {
                continue;
            }
            let wgt = keys_weight(sx - px as f32) * keys_weight(sy - py as f32);
            sum += img.get_pixel(px as u32, py as u32)[c] as f32 / 255.0 * wgt;
            wsum += wgt;
        }
    }
    (sum / wsum).clamp(0.0, 1.0)
}

/// Off-grid, the blend warp is the Keys 0.6121 bicubic, not Lanczos: a
/// half-pixel shift is where the two differ most. Two identical frames
/// collapse losslessly, so the result is the warped frame itself, compared
/// with the CPU twin, edges included.
#[test]
fn blend_warp_is_keys_bicubic_off_grid() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    const W: u32 = 257;
    const H: u32 = 193;
    // Hard edges plus a ramp: the cubic's overshoot is part of what is pinned.
    let scene = ImageBuffer::from_fn(W, H, |x, y| {
        let on = ((x / 9) + (y / 6)) % 2 == 0;
        let g = (y * 255 / H) as u8;
        if on { Rgb([235, g, 40]) } else { Rgb([20, g, 200]) }
    });
    let frame = DynamicImage::ImageRgb8(scene.clone());

    // dst → src: src = dst + (0.5, 0.5).
    let shift = nalgebra::Vector2::new(0.5f32, 0.5);
    let alignment = deptheos_core::FrameAlignment::from_transform(
        deptheos_core::Affine2::from_matrix_unchecked(nalgebra::Matrix3::new_translation(&shift)),
    );

    let slot = Some((&frame, Some(&alignment)));
    let stacker = stack(&ctx, DepthConfig::default(), &[slot, slot]);
    let result = result(&ctx, &stacker);
    let (w, h) = (W, H);
    assert_eq!(result.len(), (w * h * 4) as usize);
    let mut max_err = 0.0f32;
    let mut max_tent_gap = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                let expected = keys_sample_cpu(&scene, x as f32 + 0.5, y as f32 + 0.5, c);
                max_err = max_err.max((result[i + c] - expected).abs());
                // The cubic must differ from the 2×2 average (a tent kernel)
                // somewhere, or the scene does not tell the kernels apart.
                let x1 = (x + 1).min(W - 1);
                let y1 = (y + 1).min(H - 1);
                let tent = (scene.get_pixel(x, y)[c] as f32
                    + scene.get_pixel(x1, y)[c] as f32
                    + scene.get_pixel(x, y1)[c] as f32
                    + scene.get_pixel(x1, y1)[c] as f32)
                    / (4.0 * 255.0);
                max_tent_gap = max_tent_gap.max((expected - tent).abs());
            }
        }
    }
    assert!(max_tent_gap > 0.02, "scene does not exercise the kernel shape");
    assert!(
        max_err < 2.5 / 255.0,
        "blend warp is not the Keys bicubic: max err {max_err} ({:.2}/255)",
        max_err * 255.0
    );
}

/// The same depth map twice gives the same bytes. Metal does not order a
/// storage write and a read-back of it within one invocation without a
/// fence, so a sweep reading its own writes drifts by ±1 in a few runs out
/// of 40. Each run builds the full map and is compared to the first.
#[test]
fn depth_map_is_deterministic() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    const RUNS: usize = 24;

    let scene = crate::common::sharp_scene(SIZE);
    let frame_a = DynamicImage::ImageRgb8(blur_half(&scene, true));
    let frame_b = DynamicImage::ImageRgb8(blur_half(&scene, false));

    let build = || {
        let config = DepthConfig::default();
        let mut stacker = DepthStacker::new(ctx.clone(), config, TWO_SRGB);
        stacker.accumulate(&frame_a, None).expect("accumulate");
        stacker.accumulate(&frame_b, None).expect("accumulate");
        stacker.end_pass_one().expect("finalize depth map");
        download_depth_map(&ctx, &stacker).0
    };
    let first = build();
    let differing_runs = (1..RUNS).map(|_| build()).filter(|map| *map != first).count();
    assert_eq!(differing_runs, 0, "{differing_runs} of {} rebuilds differ", RUNS - 1);
}

/// The R8Unorm map (what completion downloads) is byte for byte the gray
/// of the Rgba8Unorm one (the live preview), at full size and reduced.
#[test]
fn r8_map_matches_rgba8_map() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    if ctx.map_format() != wgpu::TextureFormat::R8Unorm {
        eprintln!("skipping: no R8Unorm storage on this adapter");
        return;
    }
    let scene = crate::common::sharp_scene(SIZE);
    let mut stacker = DepthStacker::new(ctx.clone(), DepthConfig::default(), TWO_SRGB);
    for left in [true, false] {
        stacker
            .accumulate(&DynamicImage::ImageRgb8(blur_half(&scene, left)), None)
            .expect("accumulate");
    }
    stacker.end_pass_one().expect("finalize depth map");

    for max_side in [u32::MAX, SIZE / 3] {
        let download = |format| {
            let tex = stacker.render_live(max_side, format).expect("render map");
            download_texture(&ctx, &tex).expect("download map")
        };
        let rgba = download(wgpu::TextureFormat::Rgba8Unorm);
        let r8 = download(wgpu::TextureFormat::R8Unorm);
        assert_eq!(r8.len() * 4, rgba.len());
        assert!(r8.iter().zip(rgba.as_chunks::<4>().0).all(|(a, p)| *a == p[0]), "{max_side:?}");
    }
}

/// A one-frame stack is that frame: every pixel at its plane (0), the
/// blend giving it full weight.
#[test]
fn a_single_frame_stack_is_the_frame() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene = blur_half(&crate::common::sharp_scene(64), true);
    let frames = [DynamicImage::ImageRgb8(scene.clone())];
    let stacker = stack(&ctx, DepthConfig::default(), &unaligned(&frames));
    let (result, (depth, ..)) = (result(&ctx, &stacker), download_depth_map(&ctx, &stacker));
    let gray = depth.as_chunks::<4>().0.iter().map(|p| p[0]);
    assert!(gray.into_iter().all(|g| g == 0), "every pixel at plane 0");
    let mut max_err = 0.0f32;
    for (i, p) in scene.pixels().enumerate() {
        for c in 0..3 {
            max_err = max_err.max((result[i * 4 + c] - p[c] as f32 / 255.0).abs());
        }
    }
    assert!(max_err < 2.0 / 255.0, "single-frame result differs: max err {max_err}");
}

/// Frames down to a single pixel stack: the map grid, the smoothing sweeps
/// and the blend pyramid all reduce to a pixel or two.
#[test]
fn tiny_frames_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    for size in [1, 7, 24] {
        let frame =
            DynamicImage::ImageRgb8(ImageBuffer::from_pixel(size, size, Rgb([90, 120, 30])));
        let stacker = stack(&ctx, DepthConfig::default(), &unaligned(&[frame.clone(), frame]));
        let result = result(&ctx, &stacker);
        assert_eq!(stacker.result_texture().unwrap().width(), size);
        for (c, want) in result[..3].iter().zip([90.0, 120.0, 30.0]) {
            assert!((c - want / 255.0).abs() < 1e-3, "{size}px: {:?}", &result[..3]);
        }
        assert!(result.iter().all(|v| v.is_finite()), "{size}px: non-finite result");
    }
}

/// The stacker refuses frames beyond the stack's declared size, in either
/// pass, instead of placing them past the last plane.
#[test]
fn frames_beyond_the_stack_size_are_refused() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frame = DynamicImage::ImageRgb8(crate::common::sharp_scene(32));
    let mut stacker = DepthStacker::new(ctx, DepthConfig::default(), TWO_SRGB);
    stacker.accumulate(&frame, None).unwrap();
    stacker.accumulate(&frame, None).unwrap();
    assert!(stacker.accumulate(&frame, None).is_err());
    stacker.end_pass_one().unwrap();
    assert!(stacker.blend_frame(&frame, 2, None).is_err());
    stacker.blend_frame(&frame, 1, None).unwrap();
}

/// The blend pass takes each stacked frame once: a repeat would double its
/// weight, and finishing early would leave its share empty.
#[test]
fn the_blend_pass_takes_each_frame_once() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frame = DynamicImage::ImageRgb8(crate::common::sharp_scene(32));
    let mut stacker = DepthStacker::new(ctx, DepthConfig::default(), TWO_SRGB);
    stacker.accumulate(&frame, None).unwrap();
    stacker.end_pass_one().unwrap();
    assert!(stacker.skip_frame().is_err(), "left out after the map was made");
    assert!(stacker.finalize().is_err(), "nothing blended");
    stacker.blend_frame(&frame, 0, None).unwrap();
    assert!(stacker.blend_frame(&frame, 0, None).is_err(), "blended twice");
    stacker.finalize().unwrap();
    assert!(stacker.result_texture().is_some());
}

/// A left-out frame drops out completely: two frames with an empty slot
/// between them give the plain two-frame stack's map and result. The
/// confidence's mean focus divides by the frames stacked, and feathering
/// pulls toward the stacked frames' planes.
#[test]
fn a_left_out_frame_leaves_the_two_frame_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene = crate::common::sharp_scene(128);
    let a = DynamicImage::ImageRgb8(blur_half(&scene, true));
    let b = DynamicImage::ImageRgb8(blur_half(&scene, false));

    // Feathering at the default, at its hardest and off.
    for blend_range in [DepthConfig::DEFAULT_BLEND_RANGE, 1.0, 10.0] {
        let config = DepthConfig { blend_range, ..DepthConfig::default() };
        let run = |slots: &[Slot]| {
            let stacker = stack(&ctx, config, slots);
            (download_depth_map(&ctx, &stacker).0, result(&ctx, &stacker))
        };
        let (depth_skip, result_skip) = run(&[Some((&a, None)), None, Some((&b, None))]);
        let (depth, result) = run(&[Some((&a, None)), Some((&b, None))]);
        assert_eq!(depth_skip, depth, "blend range {blend_range}: depth map differs");
        assert_eq!(result_skip, result, "blend range {blend_range}: result differs");
    }
}

/// Left-out slots at either end leave the two frames ½ apart instead of
/// 1, which the blend and feathering scale to.
#[test]
fn left_out_end_frames_leave_the_two_frame_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let scene = crate::common::sharp_scene(128);
    let a = DynamicImage::ImageRgb8(blur_half(&scene, true));
    let b = DynamicImage::ImageRgb8(blur_half(&scene, false));
    let (a, b) = (Some((&a, None)), Some((&b, None)));
    let run = |slots: &[Slot]| result(&ctx, &stack(&ctx, DepthConfig::default(), slots));
    let plain = run(&[a, b]);
    for (name, result) in [("last", run(&[a, b, None])), ("first", run(&[None, a, b]))] {
        let worst = result.iter().zip(&plain).map(|(s, p)| (s - p).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "{name} slot left out: result differs by up to {worst}");
    }
}

/// The depth stacker's outputs on a fixed stack against its reference
/// (`golden`): the result and the full-resolution map. Four frames of
/// 289×257 (odd at every blend level), one aligned with an exposure
/// correction so part has no data, one left out; at the default settings
/// and at the hardest feather with no smoothing.
#[test]
fn depth_output_is_pinned() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let frames = golden_frames(289, 257);
    let aligned = golden_alignment();
    // Frame 2 left out.
    let slots = [
        Some((&frames[0], None)),
        Some((&frames[1], Some(&aligned))),
        None,
        Some((&frames[3], None)),
    ];

    let run = |config: DepthConfig| {
        let stacker = stack(&ctx, config, &slots);
        let (result, ..) = download_result(&ctx, stacker.result_texture());
        let depth = download_texture(&ctx, stacker.frame_map().unwrap()).unwrap();
        (result, depth)
    };
    let runs = [
        ("default", run(DepthConfig::default())),
        ("hard", run(DepthConfig { blend_range: 1.0, smoothing: 0.0, ..DepthConfig::default() })),
    ];
    golden::check(&golden::outputs("depth", 289, 257, &runs));
}
