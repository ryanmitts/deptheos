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

/// [`largest`] over the pixels `neutral` draws below the tone map's
/// shoulder (0.95 display-linear), which an edited curve always takes.
fn largest_below_shoulder(neutral: &[f32], a: &[f32], b: &[f32]) -> f64 {
    let below = |p: &[f32]| p[..3].iter().all(|&v| v < 0.977);
    let (n, a, b) = (neutral.as_chunks::<4>().0, a.as_chunks::<4>().0, b.as_chunks::<4>().0);
    n.iter()
        .zip(a.iter().zip(b))
        .filter(|(n, _)| below(&n[..]))
        .flat_map(|(_, (a, b))| (0..3).map(|c| f64::from((a[c] - b[c]).abs()) * 65535.0))
        .fold(0.0, f64::max)
}

fn mean(pixels: &[f32]) -> f64 {
    let rgb = pixels.as_chunks::<4>().0.iter().flat_map(|p| &p[..3]);
    rgb.map(|&v| f64::from(v)).sum::<f64>() / (pixels.len() / 4 * 3) as f64
}

/// The slider named `name` set to `v`. By name, since several sliders
/// share one spec.
fn set(name: &str, v: f64) -> Adjustments {
    let neutral = Adjustments::NEUTRAL;
    match name {
        "exposure" => Adjustments { exposure: v, ..neutral },
        "contrast" => Adjustments { contrast: v, ..neutral },
        "whites" => Adjustments { whites: v, ..neutral },
        "blacks" => Adjustments { blacks: v, ..neutral },
        "vibrance" => Adjustments { vibrance: v, ..neutral },
        "saturation" => Adjustments { saturation: v, ..neutral },
        "temperature" => {
            let white_balance = WhiteBalance::Relative { temperature: v, tint: 0.0 };
            Adjustments { white_balance, ..neutral }
        }
        "tint" => {
            let white_balance = WhiteBalance::Relative { temperature: 0.0, tint: v };
            Adjustments { white_balance, ..neutral }
        }
        _ => unreachable!("{name} has no field"),
    }
}

/// The sliders the resolve step draws, and whether a linear stack takes
/// them there: its white balance is the SDK's, in its params.
const SLIDERS: [(&str, SliderSpec, bool); 8] = [
    ("vibrance", slider::VIBRANCE, true),
    ("saturation", slider::SATURATION, true),
    ("exposure", slider::EXPOSURE, true),
    ("contrast", slider::CONTRAST, true),
    ("whites", slider::WHITES, true),
    ("blacks", slider::BLACKS, true),
    ("temperature", slider::TEMPERATURE_RELATIVE, false),
    ("tint", slider::TINT_RELATIVE, false),
];

/// How far the first step from neutral may move a picture against the
/// step after it, which shows where the change is smooth: a slider whose
/// first step clips or unclips a range of values jumps by far more. The
/// tone map's shoulder takes white to 0.9875 at any edit, as lightcraft's
/// does, so where it applies only the tones below it are held to this.
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
                let first = at(&set(name, spec.default + step));
                let second = at(&set(name, spec.default + 2.0 * step));
                let tone_map = matches!(name, "contrast" | "whites" | "blacks")
                    && (kind == "standard" || name == "whites");
                let (gap, next) = match tone_map {
                    true => (
                        largest_below_shoulder(&neutral, &neutral, &first),
                        largest_below_shoulder(&neutral, &first, &second),
                    ),
                    false => (largest(&neutral, &first), largest(&first, &second)),
                };
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

/// The gray ramp's mean under `adjustments`, by kind.
fn gray_mean(ctx: &GpuContext, color: Color<'_>, adjustments: &Adjustments, pixels: &[u16]) -> f64 {
    let constants = resolve_adjusted(color, adjustments, OutputSpace::Srgb);
    mean(&draw(ctx, &constants, pixels)[..WIDTH as usize * 4])
}

/// Contrast darkens the shadows and lightens the highlights; Whites lifts
/// the light tones and Blacks the dark ones, each sign its own way.
#[test]
fn contrast_whites_and_blacks_move_their_tones() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let params = linear_params();
    // The gray ramp's dark and light quarters, as their own pictures.
    let quarter = |from: u32| -> Vec<u16> {
        let width = WIDTH as usize;
        (0..HEIGHT as usize * width)
            .flat_map(|i| {
                let x = from as usize + (i % width) / 4;
                let v = (x * 257).min(65_535) as u16;
                [v, v, v]
            })
            .collect()
    };
    let (dark, light) = (quarter(0), quarter(WIDTH * 3 / 4));
    for (kind, color) in kinds(&params) {
        let at = |pixels: &[u16], a: Adjustments| gray_mean(&ctx, color, &a, pixels);
        let neutral = Adjustments::NEUTRAL;
        let (dark0, light0) = (at(&dark, neutral.clone()), at(&light, neutral.clone()));
        let contrast = Adjustments { contrast: 60.0, ..neutral.clone() };
        assert!(at(&dark, contrast.clone()) < dark0, "{kind}: contrast in the shadows");
        assert!(at(&light, contrast) > light0, "{kind}: contrast in the highlights");
        let whites = |whites| Adjustments { whites, ..neutral.clone() };
        assert!(at(&light, whites(60.0)) > light0, "{kind}: whites up");
        assert!(at(&light, whites(-60.0)) < light0, "{kind}: whites down");
        let blacks = |blacks| Adjustments { blacks, ..neutral.clone() };
        assert!(at(&dark, blacks(60.0)) > dark0, "{kind}: blacks up");
        assert!(at(&dark, blacks(-60.0)) < dark0, "{kind}: blacks down");
    }
}

/// A dark gradient under lifted and lowered Blacks climbs in steps with no
/// flat run and no jump, as a table coarse in the shadows would show.
#[test]
fn a_dark_gradient_under_blacks_shows_no_banding() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let params = linear_params();
    // The darkest 6% of codes, 16 codes a step.
    let ramp: Vec<u16> = (0..HEIGHT * WIDTH)
        .flat_map(|i| {
            let v = (256 + (i % WIDTH) * 16) as u16;
            [v, v, v]
        })
        .collect();
    for (kind, color) in kinds(&params) {
        for blacks in [100.0, 40.0, -40.0] {
            let adjustments = Adjustments { blacks, ..Adjustments::NEUTRAL };
            let constants = resolve_adjusted(color, &adjustments, OutputSpace::Srgb);
            let row: Vec<f64> = draw(&ctx, &constants, &ramp)[..WIDTH as usize * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| f64::from(p[1]) * 65535.0)
                .collect();
            let steps: Vec<f64> = row.windows(2).map(|w| w[1] - w[0]).collect();
            // From the first step off black: every step rises, none flat,
            // and none more than twice both its neighbors (a jump).
            let Some(first) = steps.iter().position(|&d| d > 0.0) else { continue };
            let rising = &steps[first + 1..];
            assert!(rising.iter().all(|&d| d > 0.0), "{kind} blacks {blacks}: a flat run");
            for w in rising.windows(3) {
                assert!(
                    w[1] <= 2.0 * w[0].max(w[2]),
                    "{kind} blacks {blacks}: a jump of {:.0} between {:.0} and {:.0}",
                    w[1],
                    w[0],
                    w[2]
                );
            }
        }
    }
}

/// The spread of a drawn row's channels: how colorful it is.
fn colorfulness(pixels: &[f32]) -> f64 {
    let rgb = pixels.as_chunks::<4>().0.iter();
    rgb.map(|p| f64::from(p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum()
}

/// Saturation and Vibrance raise color and lower it; at -100 Saturation
/// leaves gray.
#[test]
fn vibrance_and_saturation_move_color() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (pixels, params) = (gradient(), linear_params());
    // The colored rows: a ramp of red, and a muted green.
    let colored = WIDTH as usize * 16 * 4..WIDTH as usize * 48 * 4;
    for (kind, color) in kinds(&params) {
        let at = |vibrance, saturation| {
            let adjustments = Adjustments { vibrance, saturation, ..Adjustments::NEUTRAL };
            let constants = resolve_adjusted(color, &adjustments, OutputSpace::Srgb);
            colorfulness(&draw(&ctx, &constants, &pixels)[colored.clone()])
        };
        let neutral = at(0.0, 0.0);
        assert!(at(0.0, 40.0) > neutral && at(0.0, -40.0) < neutral, "{kind}: saturation");
        assert!(at(40.0, 0.0) > neutral && at(-40.0, 0.0) < neutral, "{kind}: vibrance");
        assert!(at(0.0, -100.0) < neutral * 0.01, "{kind}: saturation -100 leaves color");
    }
}

/// Whites, Vibrance and Saturation are one stage on both kinds: the same
/// linear values in give the same picture out, a linear stack with sRGB's
/// matrix and no curve, a standard one stored in sRGB.
#[test]
fn the_shared_stages_draw_alike_on_both_kinds() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let identity = RenderParams::srgb_identity();
    let linear: Vec<u16> = gradient();
    let encoded: Vec<u16> = linear
        .iter()
        .map(|&v| {
            let v = f64::from(v) / 65535.0;
            let e = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            (e * 65535.0).round() as u16
        })
        .collect();
    let neutral = Adjustments::NEUTRAL;
    let all = [
        Adjustments { whites: 60.0, ..neutral.clone() },
        Adjustments { whites: -60.0, ..neutral.clone() },
        Adjustments { vibrance: 60.0, ..neutral.clone() },
        Adjustments { saturation: -50.0, ..neutral.clone() },
    ];
    for adjustments in all {
        let a = draw(
            &ctx,
            &resolve_adjusted(Color::Linear(&identity), &adjustments, OutputSpace::Srgb),
            &linear,
        );
        let b = draw(
            &ctx,
            &resolve_adjusted(Color::Standard(None), &adjustments, OutputSpace::Srgb),
            &encoded,
        );
        // The gray and two colored rows: the two matrices to ProPhoto
        // differ in their fourth decimal, and the stored codes round apart.
        let rows = WIDTH as usize * 48 * 4;
        let gap = largest(&a[..rows], &b[..rows]);
        assert!(gap < 80.0, "{adjustments:?}: {gap:.0} codes apart");
    }
}
