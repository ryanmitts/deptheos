//! Times brush dabs on the CPU canvas by radius, on a 44 MP (8172 × 5448)
//! result, and prints the cost per dab and per touched pixel.
//!
//! Run with:
//! cargo run --release -p deptheos_app --features examples --example dab_bench -- [--flow F] [--tolerance T]

use std::sync::Arc;
use std::time::Instant;

use deptheos_app::CompletedStack;
use deptheos_app::paint::{Brush, Canvas, CoveragePool, Dab, Spacer, Stroke, StrokeSource};

const WIDTH: u32 = 8172;
const HEIGHT: u32 = 5448;
const RADII: [f32; 7] = [2.0, 8.0, 30.0, 100.0, 300.0, 1000.0, 2048.0];

fn main() {
    let mut brush = Brush::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().and_then(|v| v.parse().ok()).expect("a number");
        match arg.as_str() {
            "--flow" => brush.flow = value(),
            "--tolerance" => brush.tolerance = Some(value()),
            other => panic!("unknown argument {other}"),
        }
    }

    let len = (3 * WIDTH * HEIGHT) as usize;
    let base: Vec<u16> = (0..len).map(|i| (i * 2654435761 % 65536) as u16).collect();
    let source = Arc::new(base.iter().map(|v| v ^ 0x5555).collect::<Vec<u16>>());
    let stack = Arc::new(CompletedStack::srgb(WIDTH, HEIGHT, base, Vec::new()));

    println!("flow {} tolerance {:?}", brush.flow, brush.tolerance);
    println!("{:>7} {:>6} {:>12} {:>12} {:>10}", "radius", "dabs", "first µs", "dab µs", "ns/px");
    let mut pool = CoveragePool::default();
    for radius in RADII {
        let brush = Brush { radius, ..brush };
        let mut canvas = Canvas::new(stack.clone());
        let source = StrokeSource::Image { id: 1, pixels: source.clone() };
        // A horizontal stroke across the middle, up to 40 dabs.
        let (y, x0) = (HEIGHT as f32 / 2.0, radius.min(WIDTH as f32 / 2.0));
        let x1 = (x0 + 40.0 * brush.spacing()).min(WIDTH as f32 - x0).max(x0 + 1.0);
        let mut spacer = Spacer::new(&brush);
        let mut dabs = Vec::new();
        for x in [x0, x1] {
            spacer.to(Dab { x, y, pressure: 1.0 }, &mut dabs);
        }

        let mut stroke = Stroke::new(&canvas, brush, source, pool);
        // The first dab also snapshots and copies every tile it touches.
        let start = Instant::now();
        stroke.dab(&mut canvas, dabs[0]);
        let first = start.elapsed();
        let start = Instant::now();
        for &dab in &dabs[1..] {
            stroke.dab(&mut canvas, dab);
        }
        let rest = start.elapsed();
        pool = stroke.finish(&mut canvas).pool;

        let per_dab = rest.as_secs_f64() / (dabs.len() - 1).max(1) as f64;
        let side = 2.0 * f64::from(radius);
        println!(
            "{radius:>7} {:>6} {:>12.1} {:>12.1} {:>10.2}",
            dabs.len(),
            first.as_secs_f64() * 1e6,
            per_dab * 1e6,
            per_dab * 1e9 / (side * side),
        );
    }
}
