//! The adjustment sliders on both kinds of stack: each one's first step
//! from neutral, its direction, and how exposure meets white.

use deptheos_adjust::slider::{self, SliderSpec};
use deptheos_dng::RenderParams;
use deptheos_gpu::GpuContext;
use deptheos_render::dng_render::{Constants, DngRenderPipeline};
use deptheos_render::{Adjustments, Color, OutputSpace, WhiteBalance, resolve_adjusted};
use image::{DynamicImage, ImageBuffer, Rgb};

use crate::common::{download_rgba, upload};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 64;

/// A gray ramp, a ramp of each channel, saturated colors and a ramp past
/// a linear stack's white (its camera white is below 1).
fn gradient() -> Vec<u16> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let v = (x * 257) as u16;
            pixels.extend(match y / 16 {
                0 => [v, v, v],
                1 => [v, 0, 0],
                2 => [v / 4, v, v / 2],
                _ => [65_535, v, 65_535 - v],
            });
        }
    }
    pixels
}

/// A linear stack's color science: a camera white below 1, a black level,
/// a curved tone curve and a positive baseline.
fn linear_params() -> RenderParams {
    RenderParams {
        camera_white: [1.0, 0.9, 0.95],
        black_level: 256.0,
        baseline_exposure: 0.35,
        shadow_scale: 1.0,
        tone_curve: (0..=256)
            .map(|i| {
                let x = i as f32 / 256.0;
                x * x * (3.0 - 2.0 * x) * 0.6 + x * 0.4
            })
            .collect(),
        ..RenderParams::srgb_identity()
    }
}

/// `pixels` drawn through `constants`, as RGBA floats.
fn draw(ctx: &GpuContext, constants: &Constants, pixels: &[u16]) -> Vec<f32> {
    let buffer = ImageBuffer::<Rgb<u16>, _>::from_raw(WIDTH, HEIGHT, pixels.to_vec()).unwrap();
    let input = upload(ctx, &DynamicImage::ImageRgb16(buffer));
    let output = DngRenderPipeline::new(ctx).render(ctx, &input, constants);
    download_rgba(ctx, &output)
}

/// Both kinds' colors: a linear stack, and a standard one in sRGB.
fn kinds(params: &RenderParams) -> [(&'static str, Color<'_>); 2] {
    [("linear", Color::Linear(params)), ("standard", Color::Standard(None))]
}

/// The largest gap between two drawn pictures, in 16-bit codes.
fn largest(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(a, b)| f64::from((a - b).abs()) * 65535.0).fold(0.0, f64::max)
}

fn mean(pixels: &[f32]) -> f64 {
    let rgb = pixels.as_chunks::<4>().0.iter().flat_map(|p| &p[..3]);
    rgb.map(|&v| f64::from(v)).sum::<f64>() / (pixels.len() / 4 * 3) as f64
}

/// Each slider's field, by its spec, set to `v`.
fn set(spec: &SliderSpec, v: f64) -> Adjustments {
    let neutral = Adjustments::NEUTRAL;
    match spec {
        s if s == &slider::EXPOSURE => Adjustments { exposure: v, ..neutral },
        s if s == &slider::TEMPERATURE_RELATIVE => {
            let white_balance = WhiteBalance::Relative { temperature: v, tint: 0.0 };
            Adjustments { white_balance, ..neutral }
        }
        s if s == &slider::TINT_RELATIVE => {
            let white_balance = WhiteBalance::Relative { temperature: 0.0, tint: v };
            Adjustments { white_balance, ..neutral }
        }
        _ => unreachable!("{spec:?} has no field"),
    }
}

/// The sliders the resolve step draws, and whether a linear stack takes
/// them there: its white balance is the SDK's, in its params.
const SLIDERS: [(&str, SliderSpec, bool); 3] = [
    ("exposure", slider::EXPOSURE, true),
    ("temperature", slider::TEMPERATURE_RELATIVE, false),
    ("tint", slider::TINT_RELATIVE, false),
];

/// How far the first step from neutral may move a picture against the
/// step after it, which shows where the change is smooth: a slider whose
/// first step clips or unclips a range of values jumps by far more.
const FIRST_STEP_BOUND: f64 = 2.0;

#[test]
fn each_sliders_first_step_from_neutral_is_small() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (pixels, params) = (gradient(), linear_params());
    for (kind, color) in kinds(&params) {
        let at = |adjustments: &Adjustments| {
            draw(&ctx, &resolve_adjusted(color, adjustments, OutputSpace::Srgb), &pixels)
        };
        let neutral = at(&Adjustments::NEUTRAL);
        for (name, spec, _) in SLIDERS.into_iter().filter(|s| kind == "standard" || s.2) {
            for step in [spec.step, -spec.step] {
                let first = at(&set(&spec, spec.default + step));
                let second = at(&set(&spec, spec.default + 2.0 * step));
                let (gap, next) = (largest(&neutral, &first), largest(&first, &second));
                assert!(gap > 0.0, "{kind} {name} {step:+}: no change");
                assert!(
                    gap <= FIRST_STEP_BOUND * next,
                    "{kind} {name} {step:+}: {gap:.0} codes, then {next:.0}"
                );
            }
        }
    }
}

#[test]
fn exposure_brightens_up_and_darkens_down() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (pixels, params) = (gradient(), linear_params());
    for (kind, color) in kinds(&params) {
        let at = |exposure| {
            let adjustments = Adjustments { exposure, ..Adjustments::NEUTRAL };
            mean(&draw(&ctx, &resolve_adjusted(color, &adjustments, OutputSpace::Srgb), &pixels))
        };
        let (darker, neutral, brighter) = (at(-1.0), at(0.0), at(1.0));
        assert!(darker < neutral && neutral < brighter, "{kind}: {darker} {neutral} {brighter}");
    }
}

/// A standard stack's white balance: warmer and cooler, magenta and green,
/// on the gray ramp, and white never past 1.
#[test]
fn white_balance_warms_and_tints_a_standard_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pixels = gradient();
    let gray_mean = |temperature, tint| {
        let white_balance = WhiteBalance::Relative { temperature, tint };
        let adjustments = Adjustments { white_balance, ..Adjustments::NEUTRAL };
        let constants = resolve_adjusted(Color::Standard(None), &adjustments, OutputSpace::Srgb);
        let gray = &draw(&ctx, &constants, &pixels)[..WIDTH as usize * 4];
        let channel = |c: usize| {
            gray.as_chunks::<4>().0.iter().map(|p| f64::from(p[c])).sum::<f64>() / f64::from(WIDTH)
        };
        [channel(0), channel(1), channel(2)]
    };
    let [r, _, b] = gray_mean(50.0, 0.0);
    assert!(r > b, "warmer: {r} {b}");
    let [r, _, b] = gray_mean(-50.0, 0.0);
    assert!(r < b, "cooler: {r} {b}");
    let [r, g, b] = gray_mean(0.0, 50.0);
    assert!(g < r && g < b, "magenta: {r} {g} {b}");
    let [r, g, b] = gray_mean(0.0, -50.0);
    assert!(g > r && g > b, "green: {r} {g} {b}");
}

/// Raised exposure keeps the highlights apart on the way into white: a
/// gray ramp stays increasing and below white, where a clip flattens it.
#[test]
fn the_shoulder_brings_raised_highlights_under_white() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (pixels, params) = (gradient(), linear_params());
    for (kind, color) in kinds(&params) {
        let adjustments = Adjustments { exposure: 2.0, ..Adjustments::NEUTRAL };
        let constants = resolve_adjusted(color, &adjustments, OutputSpace::Srgb);
        let gray: Vec<f32> = draw(&ctx, &constants, &pixels)[..WIDTH as usize * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[1])
            .collect();
        let top = gray[gray.len() - 1];
        assert!(top < 1.0, "{kind}: white reached");
        assert!(gray.windows(2).all(|w| w[1] >= w[0]), "{kind}: the ramp turns back");
        let distinct = gray[gray.len() * 3 / 4..].windows(2).filter(|w| w[1] > w[0]).count();
        assert!(distinct > gray.len() / 8, "{kind}: the top quarter is flat ({distinct})");
        // Without the shoulder the raised top clips flat at white.
        let clipped = draw(&ctx, &constants.clone().without_shoulder(), &pixels);
        assert_eq!(clipped[(WIDTH as usize - 1) * 4 + 1], 1.0, "{kind}");
    }
}
