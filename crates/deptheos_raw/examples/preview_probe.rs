//! Which embedded preview LibRaw picks for a thumbnail, and how long it
//! takes, next to the half-size raw render it replaces.
//!
//! `cargo run --release -p deptheos_raw --features examples --example preview_probe -- FILE...`

use std::path::Path;
use std::time::Instant;

use deptheos_raw::{PreviewData, read_half_size, read_preview};

fn main() {
    // `--rotated FILE...`: only list the files whose preview needs turning.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--rotated") {
        for arg in &args[1..] {
            if let Ok(p) = read_preview(Path::new(arg), 256)
                && p.flip != 0
            {
                println!("{arg}: flip {}", p.flip);
            }
        }
        return;
    }
    for arg in args.drain(..) {
        let path = Path::new(&arg);
        println!("{arg}");
        for min_size in [256, 100_000] {
            let start = Instant::now();
            match read_preview(path, min_size) {
                Ok(p) => {
                    let (kind, bytes) = match &p.data {
                        PreviewData::Jpeg(b) => ("jpeg", b.len()),
                        PreviewData::Rgb8(b) => ("rgb8", b.len()),
                    };
                    println!(
                        "  preview >= {min_size:>6}: {kind} {}x{} flip {} {} KB in {:.1} ms",
                        p.width,
                        p.height,
                        p.flip,
                        bytes / 1024,
                        start.elapsed().as_secs_f64() * 1e3
                    );
                }
                Err(e) => println!("  preview >= {min_size:>6}: {e}"),
            }
        }
        let start = Instant::now();
        match read_half_size(path, deptheos_raw::Budget::unlimited()) {
            Ok((_, w, h)) => {
                println!("  half-size: {w}x{h} in {:.0} ms", start.elapsed().as_secs_f64() * 1e3)
            }
            Err(e) => println!("  half-size: {e}"),
        }
    }
}
