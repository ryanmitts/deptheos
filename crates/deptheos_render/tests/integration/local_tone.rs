//! The sliders that read neighbors take planes of the whole frame: a
//! region, a band and the whole picture draw alike, across tiles, for both
//! kinds; each slider moves its tones; and the gaps in dark lettering keep
//! their tone.

use deptheos_dng::{DngImage, RenderParams};
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;
use deptheos_render::dng_render::{
    Constants, DngRenderPipeline, PreparedRender, Region, Source, TILE,
};
use deptheos_render::{Adjustments, Color, OutputSpace, Sharpening, resolve_adjusted};
use image::{DynamicImage, ImageBuffer, Rgb};

/// Wider than a tile, so regions cross a tile's edge.
const FRAME: [u32; 2] = [TILE + 52, 300];

fn frame() -> Vec<u16> {
    let [w, h] = FRAME;
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let block = (x / 97 + y / 61) % 3;
            for c in 0..3u32 {
                let hash = (x.wrapping_mul(73_856_093)
                    ^ y.wrapping_mul(19_349_663)
                    ^ c.wrapping_mul(83_492_791))
                    % 4096;
                let level = [1500, 20_000, 52_000][block as usize] + x * 4 + c * 900;
                pixels.push((level + hash * 2).min(65_535) as u16);
            }
        }
    }
    pixels
}

fn file_params() -> RenderParams {
    let path = crate::common::core_fixture("converter.dng");
    let read = DngImage::read_with_pixels(
        &path,
        deptheos_dng::FileCache::Use,
        deptheos_dng::Budget::unlimited(),
    )
    .expect("read DNG");
    let deptheos_dng::DngRead::Linear(source, _) = read else { panic!("a linear DNG") };
    source.render_params().expect("render params")
}

fn adjusted(highlights: f64, shadows: f64) -> Adjustments {
    Adjustments { highlights, shadows, exposure: 0.4, ..Adjustments::NEUTRAL }
}

fn upload(ctx: &GpuContext, size: [u32; 2], pixels: Vec<u16>) -> wgpu::Texture {
    let buffer = ImageBuffer::<Rgb<u16>, _>::from_raw(size[0], size[1], pixels).unwrap();
    crate::common::upload(ctx, &DynamicImage::ImageRgb16(buffer))
}

fn read(ctx: &GpuContext, texture: &wgpu::Texture, region: Region, at: [u32; 2]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let origin = [region.x - at[0], region.y - at[1]];
    download_texture_with(ctx, texture, origin, [region.width, region.height], |row| {
        bytes.extend_from_slice(row)
    })
    .unwrap();
    bytes
}

fn draw(
    ctx: &GpuContext,
    pipeline: &DngRenderPipeline,
    prepared: &PreparedRender,
    source: &wgpu::Texture,
    region: Region,
) -> wgpu::Texture {
    let output = DngRenderPipeline::create_output(ctx, region.width, region.height);
    pipeline.draw(ctx, prepared, Source::whole(source), region, &output);
    output
}

/// Each region equals that region of the whole picture, bit for bit,
/// whatever band the base was read in; and the sliders change the picture.
fn regions_draw_as_the_whole(ctx: &GpuContext, constants: &Constants, plain: &Constants) {
    let pipeline = DngRenderPipeline::new(ctx);
    let source = upload(ctx, FRAME, frame());
    let whole = Region::whole(FRAME[0], FRAME[1]);
    let prepare = |band: u32| {
        let prepared = PreparedRender::new(ctx, constants);
        pipeline.with_planes(ctx, prepared, Source::whole(&source), band)
    };
    let full = read(ctx, &draw(ctx, &pipeline, &prepare(FRAME[1]), &source, whole), whole, [0, 0]);
    let unadjusted = PreparedRender::new(ctx, plain);
    assert!(full != read(ctx, &draw(ctx, &pipeline, &unadjusted, &source, whole), whole, [0, 0]));

    let banded = prepare(7);
    let regions = [
        Region { x: 0, y: 0, width: 31, height: 17 },
        Region { x: TILE - 40, y: 13, width: 90, height: 170 },
        Region { x: TILE + 2, y: 250, width: 50, height: 50 },
        Region { x: 0, y: 0, width: FRAME[0], height: 64 },
        Region { x: 0, y: 64, width: FRAME[0], height: 64 },
        whole,
    ];
    for region in regions {
        let drawn = read(
            ctx,
            &draw(ctx, &pipeline, &banded, &source, region),
            region,
            [region.x, region.y],
        );
        assert!(drawn == read_region(&full, region), "{region:?} differs");
    }
}

/// `region` of a whole picture's bytes.
fn read_region(full: &[u8], region: Region) -> Vec<u8> {
    let texel = full.len() / (FRAME[0] * FRAME[1]) as usize;
    let row = FRAME[0] as usize * texel;
    (region.y..region.y + region.height)
        .flat_map(|y| {
            let start = y as usize * row + region.x as usize * texel;
            full[start..start + region.width as usize * texel].iter().copied()
        })
        .collect()
}

#[test]
fn a_region_with_highlights_and_shadows_draws_as_the_whole_picture() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let params = file_params();
    let linear = |a: &Adjustments| *resolve_adjusted(Color::Linear(&params), a, OutputSpace::Srgb);
    regions_draw_as_the_whole(&ctx, &linear(&adjusted(-60.0, 70.0)), &linear(&adjusted(0.0, 0.0)));
    let standard = |a: &Adjustments| *resolve_adjusted(Color::Standard(None), a, OutputSpace::Srgb);
    regions_draw_as_the_whole(
        &ctx,
        &standard(&adjusted(-60.0, 70.0)),
        &standard(&adjusted(0.0, 0.0)),
    );
}

/// Each slider that reads neighbors, alone and with the others.
#[test]
fn a_region_with_each_local_slider_draws_as_the_whole_picture() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let params = file_params();
    let plain = adjusted(0.0, 0.0);
    let cases = [
        Adjustments { clarity: 45.0, ..plain.clone() },
        Adjustments { texture: -60.0, ..plain.clone() },
        Adjustments { sharpening: sharpening(60.0, 1.4, 50.0), ..plain.clone() },
        Adjustments {
            clarity: -70.0,
            texture: 80.0,
            sharpening: sharpening(150.0, 0.6, 0.0),
            ..adjusted(-30.0, 40.0)
        },
    ];
    for a in &cases {
        let linear =
            |a: &Adjustments| *resolve_adjusted(Color::Linear(&params), a, OutputSpace::Srgb);
        regions_draw_as_the_whole(&ctx, &linear(a), &linear(&plain));
        let standard =
            |a: &Adjustments| *resolve_adjusted(Color::Standard(None), a, OutputSpace::Srgb);
        regions_draw_as_the_whole(&ctx, &standard(a), &standard(&plain));
    }
}

/// The green channel of a standard stack's draw, in linear light.
fn linear_green(ctx: &GpuContext, size: [u32; 2], pixels: &[u16], a: &Adjustments) -> Vec<f64> {
    let pipeline = DngRenderPipeline::new(ctx);
    let source = upload(ctx, size, pixels.to_vec());
    let constants = resolve_adjusted(Color::Standard(None), a, OutputSpace::Srgb);
    let rendered = pipeline.render(ctx, &source, &constants);
    let rgba = crate::common::download_rgba(ctx, &rendered);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let v = f64::from(p[1]);
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        })
        .collect()
}

/// Shadows raises a dark area and Highlights lowers a bright one.
#[test]
fn shadows_lift_dark_and_highlights_lower_bright() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let size = [600, 400];
    let pixels: Vec<u16> = (0..size[0] * size[1])
        .flat_map(|i| [if i % size[0] < 300 { 4000 } else { 60_000 }; 3])
        .collect();
    let neutral = linear_green(&ctx, size, &pixels, &Adjustments::NEUTRAL.clone());
    let shadows = Adjustments { shadows: 60.0, ..Adjustments::NEUTRAL };
    let highlights = Adjustments { highlights: -60.0, ..Adjustments::NEUTRAL };
    let lifted = linear_green(&ctx, size, &pixels, &shadows);
    let lowered = linear_green(&ctx, size, &pixels, &highlights);
    let (dark, bright) = (200 * 600 + 100, 200 * 600 + 500);
    assert!(lifted[dark] > neutral[dark] * 1.2, "{} {}", lifted[dark], neutral[dark]);
    assert!(lowered[bright] < neutral[bright] * 0.9, "{} {}", lowered[bright], neutral[bright]);
}

/// Mean log2 of `drawn` over `neutral` within `x` of a row 400 wide.
fn lift_over(drawn: &[f64], neutral: &[f64], x: std::ops::Range<usize>) -> f64 {
    let n = (200..300).count() * x.len();
    let sum: f64 = (200..300)
        .flat_map(|y| x.clone().map(move |x| y * 400 + x))
        .map(|i| (drawn[i] / neutral[i]).log2())
        .sum();
    sum / n as f64
}

/// A gray step, 400 by 400: dark on the left, light on the right.
fn step() -> ([u32; 2], Vec<u16>) {
    let size = [400, 400];
    let pixels = (0..400 * 400).flat_map(|i| [if i % 400 < 200 { 9000 } else { 30_000 }; 3]);
    (size, pixels.collect())
}

/// Clarity raised darkens the dark side of an edge and lightens the light
/// side, more beside the edge than far from it; lowered, it softens both.
#[test]
fn clarity_raises_and_lowers_contrast_at_an_edge() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (size, pixels) = step();
    let neutral = linear_green(&ctx, size, &pixels, &Adjustments::NEUTRAL);
    let at = |clarity| {
        let a = Adjustments { clarity, ..Adjustments::NEUTRAL };
        let drawn = linear_green(&ctx, size, &pixels, &a);
        (lift_over(&drawn, &neutral, 185..195), lift_over(&drawn, &neutral, 205..215))
    };
    let (dark, light) = at(80.0);
    eprintln!("clarity +80 at the edge: dark {dark:.3}, light {light:.3} stops");
    assert!(dark < -0.02 && light > 0.02, "{dark} {light}");
    let (dark, light) = at(-80.0);
    assert!(dark > 0.02 && light < -0.02, "{dark} {light}");
}

fn sharpening(amount: f64, radius: f64, masking: f64) -> Sharpening {
    Sharpening { amount, radius, masking, ..Sharpening::NONE }
}

/// Sharpening darkens the dark side of an edge and lightens the light side
/// right beside it, a pixel or two, and leaves flat areas as they are.
#[test]
fn sharpening_steepens_an_edge() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (size, pixels) = step();
    let neutral = linear_green(&ctx, size, &pixels, &Adjustments::NEUTRAL);
    let a = Adjustments { sharpening: sharpening(80.0, 1.0, 0.0), ..Adjustments::NEUTRAL };
    let drawn = linear_green(&ctx, size, &pixels, &a);
    let (dark, light) =
        (lift_over(&drawn, &neutral, 198..200), lift_over(&drawn, &neutral, 200..202));
    let flat = lift_over(&drawn, &neutral, 20..60);
    eprintln!("sharpening 80 at the edge: dark {dark:.3}, light {light:.3}, flat {flat:.4} stops");
    assert!(dark < -0.05 && light > 0.05 && flat.abs() < 1e-3, "{dark} {light} {flat}");
}

/// Texture raised strengthens fine detail and lowered smooths it: the
/// spread of a fine pattern's log brightness grows and shrinks.
#[test]
fn texture_strengthens_and_smooths_fine_detail() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let size = [400u32, 400];
    let pixels: Vec<u16> = (0..400 * 400u32)
        .flat_map(|i| {
            let (x, y) = (i % 400, i / 400);
            let hash = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 6000;
            [(14_000 + hash) as u16; 3]
        })
        .collect();
    let spread = |texture| {
        let a = Adjustments { texture, ..Adjustments::NEUTRAL };
        let logs: Vec<f64> =
            linear_green(&ctx, size, &pixels, &a).iter().map(|v| v.max(1e-6).log2()).collect();
        let mean = logs.iter().sum::<f64>() / logs.len() as f64;
        (logs.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / logs.len() as f64).sqrt()
    };
    let (neutral, raised, lowered) = (spread(0.0), spread(80.0), spread(-80.0));
    eprintln!("spread in stops: neutral {neutral:.4}, +80 {raised:.4}, -80 {lowered:.4}");
    assert!(raised > neutral * 1.3 && lowered < neutral * 0.8, "{neutral} {raised} {lowered}");
}

/// Dark lettering on a bright panel: strokes 4 px wide with gaps of 3, in
/// letters of three strokes. Shadows lifts the strokes, and the gaps
/// between them within 0.05 stops of the open panel: the gaps keep the
/// panel's tone (a base of the plain median lifts them about 0.7).
#[test]
fn shadows_keep_the_gaps_in_dark_lettering() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let size = [2700u32, 1200];
    // Gap, stroke and open panel, as (x, y) tests.
    let stroke = |x: u32, y: u32| {
        let (lx, ly) = (x % 48, y % 64);
        (100..2600).contains(&x) && (100..1100).contains(&y) && ly < 28 && lx < 18 && lx % 7 < 4
    };
    let gap = |x: u32, y: u32| {
        let (lx, ly) = (x % 48, y % 64);
        (100..2600).contains(&x) && (100..1100).contains(&y) && ly < 28 && lx < 18 && lx % 7 >= 4
    };
    let pixels: Vec<u16> = (0..size[1])
        .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
        .flat_map(|(x, y)| [if stroke(x, y) { 2500 } else { 50_000 }; 3])
        .collect();
    let neutral = linear_green(&ctx, size, &pixels, &Adjustments::NEUTRAL);
    let lifted =
        linear_green(&ctx, size, &pixels, &Adjustments { shadows: 100.0, ..Adjustments::NEUTRAL });
    let lift = |test: &dyn Fn(u32, u32) -> bool| {
        let (mut sum, mut n) = (0.0, 0);
        for y in 0..size[1] {
            for x in 0..size[0] {
                if test(x, y) {
                    let i = (y * size[0] + x) as usize;
                    sum += (lifted[i] / neutral[i]).log2();
                    n += 1;
                }
            }
        }
        sum / f64::from(n)
    };
    let panel = |x: u32, y: u32| x < 60 || y < 60 || x >= 2640 || y >= 1140;
    let (strokes, gaps, open) = (lift(&stroke), lift(&gap), lift(&panel));
    eprintln!("lift in stops: strokes {strokes:.3}, gaps {gaps:.3}, open panel {open:.3}");
    assert!(strokes > open + 0.1, "strokes {strokes} panel {open}");
    assert!(gaps - open < 0.05, "gaps {gaps} panel {open}");
}
