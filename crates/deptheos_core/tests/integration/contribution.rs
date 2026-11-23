//! Contribution over a frame: a frame's share of the result, read off a
//! frame map, kept as it is with the rest dimmed to gray.

use deptheos_core::contribution::ContributionOverlay;
use deptheos_core::{FramePlane, PlaneRamp, UNLIT};
use deptheos_gpu::GpuContext;
use image::{ImageBuffer, Luma, Rgb};

use crate::common::{download_rgb, upload_rgb8 as upload};

const SIZE: u32 = 256;

/// Frame `index`'s ramp in a `total`-frame stack with every frame stacked.
fn all(index: u32, total: u32) -> PlaneRamp {
    PlaneRamp::new(index, total, |_| true)
}

/// Whether a pixel is as its source has it: one the frame supplied whole.
fn is_lit(src: &Rgb<u8>, out: [f32; 3]) -> bool {
    let src = src.0.map(|c| c as f32 / 255.0);
    out.iter().zip(src).all(|(o, s)| (o - s).abs() < 0.01)
}

/// Fraction of lit pixels on each half: (left, right).
fn lit_halves(source: &ImageBuffer<Rgb<u8>, Vec<u8>>, lit: &[[f32; 3]]) -> (f64, f64) {
    let mut counts = [0u32; 2];
    for (i, (p, out)) in source.pixels().zip(lit).enumerate() {
        let x = i as u32 % SIZE;
        if is_lit(p, *out) {
            counts[(x >= SIZE / 2) as usize] += 1;
        }
    }
    let half = (SIZE * SIZE / 2) as f64;
    (counts[0] as f64 / half, counts[1] as f64 / half)
}

/// The share a flat gray pixel was lit by: it shows at
/// `gray·(UNLIT + (1 − UNLIT)·share)`, gray in every channel.
fn share(gray: f32, out: [f32; 3]) -> f32 {
    let [r, g, b] = out;
    assert!((r - g).abs() < 0.01 && (g - b).abs() < 0.01, "a gray pixel stays gray: {out:?}");
    (g / gray - UNLIT) / (1.0 - UNLIT)
}

#[test]
fn contribution_follows_the_frame_map() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let overlay = ContributionOverlay::new(&ctx);
    let flat = ImageBuffer::from_pixel(SIZE, SIZE, Rgb([128u8; 3]));
    let source = upload(&ctx, &flat);

    // Two frames: the result's left half from the first, the right from
    // the second. Each frame lights its half, whole, and nothing else.
    let halves =
        ImageBuffer::from_fn(SIZE, SIZE, |x, _| Luma([if x < SIZE / 2 { 0u16 } else { 65535 }]));
    let first = overlay.light(&ctx, &source, &halves, all(0, 2));
    assert_eq!(first.format(), ctx.source_format());
    assert_eq!(lit_halves(&flat, &download_rgb(&ctx, &first)), (1.0, 0.0));
    let second = overlay.light(&ctx, &source, &halves, all(1, 2));
    assert_eq!(lit_halves(&flat, &download_rgb(&ctx, &second)), (0.0, 1.0));

    // Three frames over a ramp (the map value is the column over 255): the
    // middle frame's share is whole at its plane (0.5), half way to a
    // neighbor's plane it is half, and at the neighbors' planes nothing.
    let ramp = ImageBuffer::from_fn(SIZE, SIZE, |x, _| Luma([x as u16 * 257]));
    let middle = overlay.light(&ctx, &source, &ramp, all(1, 3));
    let pixels = download_rgb(&ctx, &middle);
    let share_at = |x: u32| share(128.0 / 255.0, pixels[x as usize]);
    for x in [127, 128] {
        assert!((share_at(x) - 1.0).abs() < 0.02, "x {x}: {}", share_at(x));
    }
    for x in [64, 191] {
        assert!((share_at(x) - 0.5).abs() < 0.03, "x {x}: {}", share_at(x));
    }
    for x in [0, 255] {
        assert!(share_at(x).abs() < 0.02, "x {x}: {}", share_at(x));
    }

    // The middle frame left out: the first frame's share reaches across it
    // to the last frame's plane, as the blend's does.
    let first = overlay.light(&ctx, &source, &ramp, PlaneRamp::new(0, 3, |j| j != 1));
    let pixels = download_rgb(&ctx, &first);
    let share_at = |x: u32| share(128.0 / 255.0, pixels[x as usize]);
    for (x, want) in [(0, 1.0), (128, 0.5), (191, 0.25), (255, 0.0)] {
        assert!((share_at(x) - want).abs() < 0.03, "x {x}: {} not {want}", share_at(x));
    }
}

/// The 16-bit frame map keeps neighboring frames of a long stack apart,
/// which 8 bits could not: in a 600-frame stack whose left half came from
/// frame 150 and right half from frame 151 (one 8-bit code for both),
/// frame 150 lights exactly the left half.
#[test]
fn a_16_bit_frame_map_separates_neighbors_in_a_long_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let overlay = ContributionOverlay::new(&ctx);
    let flat = ImageBuffer::from_pixel(SIZE, SIZE, Rgb([128u8; 3]));
    let source = upload(&ctx, &flat);
    let (left, right) = (FramePlane::new(150, 600), FramePlane::new(151, 600));
    let code8 = |p: FramePlane| (p.plane * 255.0).round() as u8;
    assert_eq!(code8(left), code8(right), "the case 8 bits cannot tell apart");
    let code = |p: FramePlane| Luma([(p.plane * 65535.0).round() as u16]);
    let map =
        ImageBuffer::from_fn(SIZE, SIZE, |x, _| code(if x < SIZE / 2 { left } else { right }));
    let lit = overlay.light(&ctx, &source, &map, all(150, 600));
    assert_eq!(lit_halves(&flat, &download_rgb(&ctx, &lit)), (1.0, 0.0));
}
