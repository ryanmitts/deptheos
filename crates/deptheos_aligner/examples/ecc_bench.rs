//! Time the ECC aligner's two halves in isolation. Loads the first `N`
//! frames (default 6), prepares each (timed), then runs the chain step over
//! them (timed), `REPS` times (default 3), on a pool of `THREADS` threads
//! (default 8, the stacking session's size on a 32-thread machine).
//! Usage: `ecc_bench <frames or dir>`.

use deptheos_aligner::{AlignMode, Aligner};
use image::DynamicImage;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A frame as the aligners read it: RGB8, or RGB16 for deeper sources.
fn load_rgb(path: &Path) -> DynamicImage {
    let image = image::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let color = image.color();
    if color.bits_per_pixel() / color.channel_count() as u16 > 8 {
        DynamicImage::ImageRgb16(image.into_rgb16())
    } else {
        DynamicImage::ImageRgb8(image.into_rgb8())
    }
}

fn main() {
    if std::env::var("RUST_LOG").is_ok_and(|v| !v.is_empty()) {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }
    let n: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    let reps: usize = std::env::var("REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let threads: usize = std::env::var("THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    let mut paths: Vec<PathBuf> = Vec::new();
    for arg in std::env::args().skip(1) {
        let p = PathBuf::from(arg);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .unwrap()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && p.extension().is_some_and(|e| {
                            matches!(
                                e.to_str().unwrap_or("").to_ascii_lowercase().as_str(),
                                "jpg" | "jpeg" | "tif" | "tiff" | "png"
                            )
                        })
                })
                .collect();
            v.sort();
            paths.extend(v);
        } else {
            paths.push(p);
        }
    }
    paths.truncate(n);
    if paths.len() < 2 {
        eprintln!("usage: ecc_bench <frames or dir> (at least two frames)");
        std::process::exit(2);
    }
    let images: Vec<DynamicImage> = paths.iter().map(|p| load_rgb(p)).collect();
    println!(
        "{} frames {}x{}, {threads} threads",
        images.len(),
        images[0].width(),
        images[0].height()
    );

    pool.install(|| bench(&images, reps));
}

fn bench(images: &[DynamicImage], reps: usize) {
    for rep in 1..=reps {
        let aligner = Aligner::new(AlignMode::Ecc);
        let preparer = aligner.preparer();
        let t = Instant::now();
        let prepared: Vec<_> =
            images.iter().map(|i| preparer.prepare(i).expect("prepare")).collect();
        let prep_ms = t.elapsed().as_secs_f64() * 1e3 / images.len() as f64;

        let mut aligner = aligner;
        let t = Instant::now();
        for p in prepared {
            aligner.align_prepared(p).expect("align");
        }
        let step_ms = t.elapsed().as_secs_f64() * 1e3 / (images.len() - 1) as f64;
        println!("rep {rep}: prepare {prep_ms:.2} ms/frame, step {step_ms:.2} ms/frame");
    }
}
