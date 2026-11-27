//! The drag preview against the picture it settles on (ADR-138): a region
//! is that window of the full draw bit for bit, a fit tick keeps the level
//! of the settled picture at that zoom, the vignette stays where the full
//! draw puts it, and a tick reuses its slot. Both kinds of stack.

use std::path::Path;

use deptheos_render::reframe::Rect;
use deptheos_render::{Adjustments, Draw, Reframe, Sharpening, Vignette, WhiteBalance};

use super::*;
use crate::adjust::preview::{Draft, FlatPicture, FlatSlot, Shown, read_flat};
use crate::runs::session::StackColor;

const SIZE: [u32; 2] = [900, 600];

/// A fine-detail scene: dark lines a pixel or two wide every 7 on a bright
/// gradient, over noise, as pcb's pads and lettering are.
fn scene([w, h]: [u32; 2]) -> Vec<u16> {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let line = x % 7 < 1 + (y / 150) % 2 || y % 11 == 0;
            let base = 0.25 + 0.6 * x as f32 / w as f32;
            for c in 0..3u32 {
                let noise = ((x * 7 + y * 13 + c * 5).wrapping_mul(2_654_435_761) >> 22) as f32;
                let v = if line { 0.04 } else { base * (0.9 + 0.1 * c as f32) };
                pixels.push(((v + (noise / 1024.0 - 0.5) * 0.03).clamp(0.0, 1.0) * 65535.0) as u16);
            }
        }
    }
    pixels
}

fn standard(pixels: Vec<u16>) -> CompletedStack {
    CompletedStack::srgb(SIZE[0], SIZE[1], pixels, Vec::new())
}

/// A linear stack of one synthetic DNG of `pixels`, with its color science.
fn linear(dir: &Path, pixels: Vec<u16>) -> CompletedStack {
    use deptheos_dng::test_dng::{Layout, TestDng};
    let [w, h] = SIZE;
    let dng = TestDng {
        width: w,
        height: h,
        cfa: false,
        big_endian: false,
        layout: Layout::Strips { rows: 16 },
        black: [0; 3],
        white: [65535; 3],
        active: [0, 0, h, w],
        crop: [0, 0, w, h],
    };
    let path = dir.join("scene.dng");
    dng.write(&path, &pixels);
    let (image, metadata) = crate::decode_linear_with_metadata(&path, Default::default()).unwrap();
    let render_params = metadata.render_params().unwrap();
    CompletedStack {
        color: StackColor::Linear { metadata, render_params },
        ..CompletedStack::srgb(w, h, image.to_rgb16().into_raw(), vec![path])
    }
}

/// Both kinds of the scene.
fn stacks(dir: &Path, pixels: Vec<u16>) -> [(&'static str, CompletedStack); 2] {
    [("standard", standard(pixels.clone())), ("linear", linear(dir, pixels))]
}

fn constants(stack: &CompletedStack, adjustments: &Adjustments, reframe: Reframe) -> Constants {
    match Look::adjusting(stack, adjustments, reframe).resolve() {
        Draw::Rendered(constants) => *constants,
        Draw::Stored { .. } => panic!("Adjust draws on the GPU"),
    }
}

/// The settled full picture, as the viewer reads it back.
fn full(engine: &Engine, copies: &mut GpuCopies, constants: &Constants) -> FlatPicture {
    let texture = copies.draw_full(engine, constants);
    let whole = Region::whole(SIZE[0], SIZE[1]);
    read_flat(engine.gpu(), &texture, SIZE, whole, constants.output(), &FlatSlot::default())
        .unwrap()
}

/// `picture`'s window `r`, row by row.
fn window(picture: &FlatPicture, r: Region) -> Vec<u8> {
    let row = picture.width as usize * 4;
    (r.y..r.y + r.height)
        .flat_map(|y| {
            let at = y as usize * row + r.x as usize * 4;
            picture.pixels[at..at + r.width as usize * 4].to_vec()
        })
        .collect()
}

/// `picture` as the mean of the display values each pixel of a `size`
/// copy covers, as the viewer's levels shrink it: red, green and blue.
fn shrunk(picture: &FlatPicture, [w, h]: [u32; 2]) -> Vec<[f64; 3]> {
    let (sx, sy) = (picture.width as f64 / w as f64, picture.height as f64 / h as f64);
    let mut out = vec![[0.0; 3]; (w * h) as usize];
    for (i, o) in out.iter_mut().enumerate() {
        let (gx, gy) = ((i as u32 % w) as f64, (i as u32 / w) as f64);
        let (mut sum, mut area) = ([0.0; 3], 0.0);
        let (x0, x1, y0, y1) = (gx * sx, (gx + 1.0) * sx, gy * sy, (gy + 1.0) * sy);
        for y in y0.floor() as u32..(y1.ceil() as u32).min(picture.height) {
            let hy = (y1.min(f64::from(y) + 1.0) - y0.max(f64::from(y))).max(0.0);
            for x in x0.floor() as u32..(x1.ceil() as u32).min(picture.width) {
                let wx = (x1.min(f64::from(x) + 1.0) - x0.max(f64::from(x))).max(0.0);
                let p = &picture.pixels[((y * picture.width + x) * 4) as usize..][..4];
                for (c, i) in [2, 1, 0].into_iter().enumerate() {
                    sum[c] += f64::from(p[i]) * wx * hy;
                }
                area += wx * hy;
            }
        }
        *o = sum.map(|s| s / area);
    }
    out
}

/// The mean and the largest of how far `a`'s and `b`'s values are apart,
/// and the mean signed difference, in 8-bit codes.
fn apart(a: &[[f64; 3]], b: &FlatPicture) -> (f64, f64, f64) {
    let b: Vec<[f64; 3]> = b
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| [f64::from(p[2]), f64::from(p[1]), f64::from(p[0])])
        .collect();
    assert_eq!(a.len(), b.len());
    let pairs = a.iter().zip(&b).flat_map(|(x, y)| (0..3).map(move |c| y[c] - x[c]));
    let (mut abs, mut worst, mut signed, mut n) = (0.0, 0.0f64, 0.0, 0.0);
    for d in pairs {
        abs += d.abs();
        worst = worst.max(d.abs());
        signed += d;
        n += 1.0;
    }
    (abs / n, worst, signed / n)
}

/// Every slider that reads neighbors, a vignette and a crop off center.
fn every_slider() -> (Adjustments, Reframe) {
    let adjustments = Adjustments {
        exposure: 0.4,
        contrast: 20.0,
        highlights: -40.0,
        shadows: 50.0,
        clarity: 30.0,
        texture: 40.0,
        sharpening: Sharpening { amount: 60.0, ..Sharpening::NONE },
        saturation: 15.0,
        vignette: Vignette { amount: -60.0, midpoint: 40.0 },
        ..Adjustments::NEUTRAL
    };
    let crop = Rect { x: 120, y: 40, width: 500, height: 400 };
    (adjustments, Reframe { crop: Some(crop), ..Reframe::NONE })
}

#[test]
fn a_region_tick_is_that_window_of_the_full_picture() {
    let Some(engine) = crate::test_support::engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (adjustments, reframe) = every_slider();
    let visible = Region { x: 200, y: 100, width: 300, height: 250 };
    for (kind, stack) in stacks(dir.path(), scene(SIZE)) {
        let constants = constants(&stack, &adjustments, reframe);
        let mut copies =
            GpuCopies::make(&engine, &stack, &stack.pixels, Generation::ON_DISK).unwrap();
        let slot = FlatSlot::default();
        let shown = Shown { zoom: 1.0, visible };
        let before = copies.draw_preview(&engine, &constants, shown, Draft::Tick, &slot).unwrap();
        let settled = full(&engine, &mut copies, &constants);
        let after = copies.draw_preview(&engine, &constants, shown, Draft::Settled, &slot).unwrap();
        assert_eq!((before.covers, before.width, before.height), (visible, 300, 250), "{kind}");
        let expected = window(&settled, visible);
        assert!(before.pixels == expected, "{kind}: a tick before the full draw");
        assert!(after.pixels == expected, "{kind}: settled, after it");
    }
}

/// A fit tick on the fine-detail scene keeps the settled picture's level
/// at that zoom: the picture drawn whole, its display values shrunk to the
/// size shown. Texture and Sharpening are left out of the tick.
#[test]
fn a_fit_tick_keeps_the_settled_level() {
    let Some(engine) = crate::test_support::engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (adjustments, _) = every_slider();
    let fine = Adjustments { texture: 0.0, sharpening: Sharpening::NONE, ..adjustments.clone() };
    let shown = Shown { zoom: 0.25, visible: Region::whole(SIZE[0], SIZE[1]) };
    for (kind, stack) in stacks(dir.path(), scene(SIZE)) {
        let mut copies =
            GpuCopies::make(&engine, &stack, &stack.pixels, Generation::ON_DISK).unwrap();
        let slot = FlatSlot::default();
        let with = constants(&stack, &adjustments, Reframe::NONE);
        let without = constants(&stack, &fine, Reframe::NONE);
        let tick = copies.draw_preview(&engine, &with, shown, Draft::Tick, &slot).unwrap();
        let plain = copies.draw_preview(&engine, &without, shown, Draft::Tick, &slot).unwrap();
        assert_eq!([tick.width, tick.height], [225, 150], "{kind}");
        assert!(tick.pixels == plain.pixels, "{kind}: Texture and Sharpening are left out");
        assert!(copies.is_settled(&without, shown), "{kind}: a first tick makes its planes");
        let settled = shrunk(&full(&engine, &mut copies, &without), [225, 150]);
        assert!(copies.is_settled(&without, shown), "{kind}: the full draw made its planes");
        let brighter = Adjustments { exposure: 1.0, ..fine.clone() };
        let brighter = constants(&stack, &brighter, Reframe::NONE);
        assert!(copies.is_settled(&brighter, shown), "{kind}: exposure keeps them");
        let white = match stack.color {
            StackColor::Linear { .. } => WhiteBalance::Kelvin { temperature: 4000.0, tint: 0.0 },
            StackColor::Srgb { .. } => WhiteBalance::Relative { temperature: 40.0, tint: 0.0 },
        };
        let warmer =
            constants(&stack, &Adjustments { white_balance: white, ..fine.clone() }, Reframe::NONE);
        assert!(!copies.is_settled(&warmer, shown), "{kind}: a white makes them anew");
        let (mean, worst, level) = apart(&settled, &tick);
        eprintln!(
            "{kind}: {mean:.2} codes apart on average, {worst:.0} at most, level {level:+.2}"
        );
        assert!(mean <= TICK_MEAN && level.abs() <= TICK_LEVEL, "{kind}: {mean:.2}, {level:+.2}");
    }
}

/// How far a fit tick may be from the settled picture at its zoom on the
/// fine-detail scene (a 4:1 reduction of lines a pixel or two wide), in
/// 8-bit codes: on average, and in mean level. Measured: 3.5 and 3.6 on
/// average, levels -0.5 (standard) and +2.4 (linear).
const TICK_MEAN: f64 = 4.0;
const TICK_LEVEL: f64 = 3.0;

/// The vignette on a plain gray picture: a fit tick puts it where the full
/// draw does, on the crop scaled to the copy's pixels.
#[test]
fn a_fit_tick_keeps_the_vignette_in_place() {
    let Some(engine) = crate::test_support::engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let gray = vec![30_000u16; (SIZE[0] * SIZE[1] * 3) as usize];
    let adjustments = Adjustments {
        vignette: Vignette { amount: -80.0, midpoint: 30.0 },
        ..Adjustments::NEUTRAL
    };
    let crop = Rect { x: 40, y: 200, width: 400, height: 300 };
    let reframe = Reframe { crop: Some(crop), ..Reframe::NONE };
    let shown = Shown { zoom: 0.25, visible: Region::whole(SIZE[0], SIZE[1]) };
    for (kind, stack) in stacks(dir.path(), gray) {
        let constants = constants(&stack, &adjustments, reframe);
        let mut copies =
            GpuCopies::make(&engine, &stack, &stack.pixels, Generation::ON_DISK).unwrap();
        let tick =
            copies.draw_preview(&engine, &constants, shown, Draft::Tick, &FlatSlot::default());
        let settled = shrunk(&full(&engine, &mut copies, &constants), [225, 150]);
        let (mean, worst, _) = apart(&settled, &tick.unwrap());
        eprintln!("{kind}: vignette {mean:.2} codes apart on average, {worst:.0} at most");
        assert!(worst <= VIGNETTE_WORST, "{kind}: {worst} codes apart");
    }
}

/// How far a fit tick's vignette may be from the full draw's, shrunk, in
/// 8-bit codes at any pixel.
const VIGNETTE_WORST: f64 = 2.0;

/// Ticks whose pictures are handed back draw into the same two buffers.
#[test]
fn a_tick_reuses_its_slot() {
    let Some(engine) = crate::test_support::engine() else { return };
    let stack = standard(scene(SIZE));
    let mut copies = GpuCopies::make(&engine, &stack, &stack.pixels, Generation::ON_DISK).unwrap();
    let slot = FlatSlot::default();
    let mut on_screen: Option<FlatPicture> = None;
    for (i, zoom) in [0.25, 1.0].into_iter().cycle().take(12).enumerate() {
        let adjustments = Adjustments { exposure: i as f64 * 0.1, ..Adjustments::NEUTRAL };
        let constants = constants(&stack, &adjustments, Reframe::NONE);
        let visible = Region { x: 0, y: 0, width: 225, height: 150 };
        let shown = Shown { zoom, visible };
        let picture = copies.draw_preview(&engine, &constants, shown, Draft::Tick, &slot).unwrap();
        if let Some(replaced) = on_screen.replace(picture) {
            slot.give_back(replaced);
        }
    }
    assert_eq!(slot.allocations(), 2, "the one on screen and the one being drawn");
    let held = copies.memory().fit;
    assert!(held > 0 && held <= RenderMemory::of(SIZE[0], SIZE[1], 8).fit, "{held}");
}

/// At neutral a linear stack's region tick is what the viewer shows
/// today for that window: its unadjusted render.
#[test]
fn a_neutral_region_tick_draws_as_unadjusted() {
    let Some(engine) = crate::test_support::engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let stack = linear(dir.path(), scene(SIZE));
    let shown_constants = match Look::shown(&stack, &Adjustments::NEUTRAL).resolve() {
        Draw::Rendered(constants) => constants,
        Draw::Stored { .. } => panic!("a linear stack draws on the GPU"),
    };
    let input = engine.upload_for(&shown_constants, &stack.pixels, SIZE[0], SIZE[1]).unwrap();
    let shown_texture = engine.color().render(engine.gpu(), &input, &shown_constants);
    let whole = Region::whole(SIZE[0], SIZE[1]);
    let slot = FlatSlot::default();
    let shown = read_flat(engine.gpu(), &shown_texture, SIZE, whole, OutputSpace::Srgb, &slot);
    let constants = constants(&stack, &Adjustments::NEUTRAL, Reframe::NONE);
    let mut copies = GpuCopies::make(&engine, &stack, &stack.pixels, Generation::ON_DISK).unwrap();
    let visible = Region { x: 50, y: 60, width: 400, height: 300 };
    let tick = copies
        .draw_preview(&engine, &constants, Shown { zoom: 2.0, visible }, Draft::Tick, &slot)
        .unwrap();
    assert!(tick.pixels == window(&shown.unwrap(), visible));
}

/// Ticks and the settle at 45 MP on a synthetic standard stack, median
/// and 95th percentile: `cargo test --release -p deptheos_app --lib
/// preview_times -- --ignored --nocapture`.
#[test]
#[ignore = "a timing at 45 MP"]
fn preview_times_at_45mp() {
    use std::time::Instant;
    let Some(engine) = crate::test_support::engine() else { return };
    let [w, h] = [8256u32, 5504];
    let stack = CompletedStack::srgb(w, h, scene([w, h]), Vec::new());
    let mut copies = GpuCopies::make(&engine, &stack, &stack.pixels, Generation::ON_DISK).unwrap();
    let slot = FlatSlot::default();
    let wait = |engine: &Engine| {
        let _ = engine.gpu().device.poll(wgpu::PollType::wait_indefinitely());
    };
    let stats = |ms: &mut Vec<f64>| {
        ms.sort_by(f64::total_cmp);
        let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
        format!("{:.1} / {:.1} ms", at(0.5), at(0.95))
    };
    let local =
        Adjustments { shadows: 40.0, highlights: -30.0, clarity: 25.0, ..Adjustments::NEUTRAL };
    let fine = Adjustments {
        texture: 30.0,
        sharpening: Sharpening { amount: 50.0, ..Sharpening::NONE },
        ..local.clone()
    };
    let fit = Shown { zoom: 2048.0 / w as f32, visible: Region::whole(w, h) };
    let region =
        Shown { zoom: 1.0, visible: Region { x: 2800, y: 1900, width: 2560, height: 1600 } };
    let cases = [
        ("fit, per pixel", fit, Adjustments::NEUTRAL),
        ("fit, Highlights Shadows Clarity", fit, local.clone()),
        ("100%, per pixel", region, Adjustments::NEUTRAL),
        ("100%, Highlights Shadows Clarity", region, local.clone()),
        ("100%, with Texture and Sharpening", region, fine.clone()),
    ];
    for (name, shown, base) in cases {
        let start = Instant::now();
        let first = constants(&stack, &base, Reframe::NONE);
        copies
            .draw_preview(&engine, &first, shown, Draft::Settled, &slot)
            .map(|p| slot.give_back(p))
            .unwrap();
        wait(&engine);
        let made = start.elapsed().as_secs_f64() * 1e3;
        let mut ms = Vec::new();
        for i in 0..30 {
            let adjustments = Adjustments { exposure: 0.01 * i as f64, ..base.clone() };
            let constants = constants(&stack, &adjustments, Reframe::NONE);
            let start = Instant::now();
            let picture =
                copies.draw_preview(&engine, &constants, shown, Draft::Tick, &slot).unwrap();
            ms.push(start.elapsed().as_secs_f64() * 1e3);
            slot.give_back(picture);
        }
        eprintln!("{name}: tick {} (first, making its planes: {made:.0} ms)", stats(&mut ms));
    }
    for (name, adjustments) in [("per pixel", Adjustments::NEUTRAL), ("local tone", fine)] {
        let constants =
            constants(&stack, &Adjustments { exposure: 0.3, ..adjustments }, Reframe::NONE);
        let mut ms = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            drop(copies.draw_full(&engine, &constants));
            wait(&engine);
            ms.push(start.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!("full redraw, {name}: {}", stats(&mut ms));
    }
}
