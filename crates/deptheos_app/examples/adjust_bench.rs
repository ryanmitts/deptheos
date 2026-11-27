//! Times Adjust at 45 MP against its budgets (ADR-138, ADR-114): a preview
//! tick at each tier, the histogram, and the full redraw a drag settles
//! on, median and 95th percentile, and the open render's GPU memory.
//!
//! `cargo run --release -p deptheos_app --features examples --example adjust_bench -- [RAW]`
//!
//! Without RAW, a synthetic 8256 × 5504 standard stack of fine detail;
//! with one, that RAW decoded as a linear stack at its own size.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use deptheos_app::adjust::GpuCopies;
use deptheos_app::adjust::copies::{GPU_BUDGET_45MP, VIEWPORT};
use deptheos_app::adjust::histogram::clipping;
use deptheos_app::adjust::preview::{Draft, FlatSlot, SETTLE_BUDGET, Shown, Tier};
use deptheos_app::engine::generation::Generation;
use deptheos_app::{CompletedStack, Engine, Look, Platform, StackColor, Words};
use deptheos_render::dng_render::{Constants, Region};
use deptheos_render::{Adjustments, Reframe, Sharpening};

const SYNTHETIC: [u32; 2] = [8256, 5504];
const TICKS: usize = 30;
const REDRAWS: usize = 5;

/// A fine-detail scene: dark lines every 7 pixels on a bright gradient.
fn scene([w, h]: [u32; 2]) -> Vec<u16> {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let line = x % 7 == 0 || y % 11 == 0;
            let base = 0.25 + 0.6 * x as f32 / w as f32;
            for c in 0..3 {
                let v = if line { 0.04 } else { base * (0.9 + 0.1 * c as f32) };
                pixels.push((v * 65535.0) as u16);
            }
        }
    }
    pixels
}

fn stack(raw: Option<PathBuf>) -> (String, CompletedStack) {
    let Some(raw) = raw else {
        let [w, h] = SYNTHETIC;
        let name = format!("synthetic standard stack, {w} × {h}");
        return (name, CompletedStack::srgb(w, h, scene(SYNTHETIC), Vec::new()));
    };
    let (image, metadata) =
        deptheos_app::decode_linear_with_metadata(&raw, Default::default()).expect("a RAW");
    let render_params = metadata.render_params().expect("render params");
    let rgb = image.to_rgb16();
    let (w, h) = rgb.dimensions();
    let name = format!("{} (linear), {w} × {h}", raw.display());
    let stack = CompletedStack {
        color: StackColor::Linear { metadata, render_params },
        ..CompletedStack::srgb(w, h, rgb.into_raw(), vec![raw])
    };
    (name, stack)
}

fn constants(stack: &CompletedStack, adjustments: &Adjustments) -> Constants {
    Look::adjusting(stack, adjustments, Reframe::NONE).constants().expect("drawn on the GPU")
}

fn wait(engine: &Engine) {
    let _ = engine.gpu().device.poll(wgpu::PollType::wait_indefinitely());
}

/// Median and 95th percentile, in ms.
fn stats(ms: &mut [f64]) -> (f64, f64) {
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.95))
}

fn row(name: &str, ms: &mut [f64], budget: Option<Duration>) {
    let (median, p95) = stats(ms);
    let verdict = match budget {
        Some(b) => {
            let b = b.as_secs_f64() * 1e3;
            format!("{b:>6.0} {}", if p95 <= b { "PASS" } else { "MISS" })
        }
        None => format!("{:>6} {}", "—", ""),
    };
    println!("{name:<40} {median:>8.1} {p95:>8.1} {verdict}");
}

/// Each tick a little more exposure over `base`, so no draw is a repeat.
fn nudged(base: &Adjustments, i: usize) -> Adjustments {
    Adjustments { exposure: base.exposure + 0.01 * i as f64, ..base.clone() }
}

fn main() {
    let raw = std::env::args().nth(1).map(PathBuf::from);
    let engine = Engine::new(Words::english(Platform::Desktop)).expect("a GPU engine");
    let (name, stack) = stack(raw);
    let [w, h] = [stack.width, stack.height];
    println!(
        "{name} on {} ({:?})",
        engine.gpu().adapter_info.name,
        engine.gpu().adapter_info.backend
    );

    let start = Instant::now();
    let mut copies =
        GpuCopies::make(&engine, &stack, &stack.pixels, Generation::ON_DISK).expect("copies");
    wait(&engine);
    println!("GPU copies made in {:.0} ms", start.elapsed().as_secs_f64() * 1e3);

    let per_pixel = Adjustments { contrast: 20.0, saturation: 10.0, ..Adjustments::NEUTRAL };
    let local =
        Adjustments { highlights: -30.0, shadows: 40.0, clarity: 25.0, ..per_pixel.clone() };
    let fine = Adjustments {
        texture: 30.0,
        sharpening: Sharpening { amount: 50.0, ..Sharpening::NONE },
        ..local.clone()
    };
    let long = w.max(h) as f32;
    let fit = Shown { zoom: 2048.0 / long, visible: Region::whole(w, h) };
    let [vw, vh] = [VIEWPORT[0].min(w), VIEWPORT[1].min(h)];
    let visible = Region { x: (w - vw) / 2, y: (h - vh) / 2, width: vw, height: vh };
    let region = Shown { zoom: 1.0, visible };
    let slot = FlatSlot::default();

    println!("{:<40} {:>8} {:>8} {:>6}", "(ms)", "median", "p95", "budget");
    let ticks = [
        ("tick at fit, per pixel", fit, &per_pixel, false),
        ("tick at fit, Highlights Shadows Clarity", fit, &local, false),
        ("tick at 100%, per pixel", region, &per_pixel, false),
        ("tick at 100%, Highlights Shadows Clarity", region, &local, false),
        ("tick at 100%, Texture and Sharpening", region, &fine, true),
    ];
    for (name, shown, base, fine_detail) in ticks {
        // The first draw makes the planes a drag then reads.
        let first = constants(&stack, base);
        let picture = copies.draw_preview(&engine, &first, shown, Draft::Settled, &slot);
        slot.give_back(picture.expect("a draw"));
        let mut ms = Vec::new();
        for i in 0..TICKS {
            let c = constants(&stack, &nudged(base, i + 1));
            let start = Instant::now();
            let picture =
                copies.draw_preview(&engine, &c, shown, Draft::Tick, &slot).expect("a tick");
            ms.push(start.elapsed().as_secs_f64() * 1e3);
            slot.give_back(picture);
        }
        row(name, &mut ms, Some(Tier::of(shown.zoom).budget(fine_detail)));
    }

    for (name, base) in [("histogram, per pixel", &per_pixel), ("histogram, local tone", &local)] {
        let mut ms = Vec::new();
        for i in 0..TICKS {
            let c = constants(&stack, &nudged(base, i));
            let start = Instant::now();
            copies.histogram(&engine, &c, None, Draft::Tick).expect("a histogram");
            ms.push(start.elapsed().as_secs_f64() * 1e3);
        }
        row(name, &mut ms, None);
    }

    // A settle hands the view on screen over first, then draws the whole.
    let settles = [
        ("settle on screen at fit, every slider", fit),
        ("settle on screen at 100%, every slider", region),
    ];
    for (name, shown) in settles {
        drop(copies.draw_full(&engine, &constants(&stack, &fine)));
        let mut ms = Vec::new();
        for i in 0..REDRAWS {
            let c = constants(&stack, &nudged(&fine, i + 1));
            let start = Instant::now();
            let picture =
                copies.draw_preview(&engine, &c, shown, Draft::Settled, &slot).expect("a draw");
            copies.histogram(&engine, &c, None, Draft::Settled).expect("a histogram");
            ms.push(start.elapsed().as_secs_f64() * 1e3);
            slot.give_back(picture);
        }
        row(name, &mut ms, Some(SETTLE_BUDGET));
    }

    let redraws = [("full redraw, per pixel", &per_pixel), ("full redraw, every slider", &fine)];
    for (name, base) in redraws {
        // Planes made once, as opening Adjust does; a drag then keeps them.
        drop(copies.draw_full(&engine, &constants(&stack, base)));
        wait(&engine);
        let mut ms = Vec::new();
        for i in 0..REDRAWS {
            let c = constants(&stack, &nudged(base, i + 1));
            let start = Instant::now();
            let texture = copies.draw_full(&engine, &c);
            clipping(&engine, &texture, None).expect("clipping counted");
            wait(&engine);
            ms.push(start.elapsed().as_secs_f64() * 1e3);
        }
        row(name, &mut ms, Some(SETTLE_BUDGET));
    }

    let memory = copies.memory();
    let (gpu, budget) = (memory.gpu() >> 20, GPU_BUDGET_45MP >> 20);
    let verdict = if memory.gpu() <= GPU_BUDGET_45MP { "PASS" } else { "MISS" };
    println!("memory: {memory}; budget {budget} MB: {verdict} ({gpu} MB)");
    println!(
        "ticks and redraws are wall clock, readback included; a full redraw includes its clipping count"
    );
}
