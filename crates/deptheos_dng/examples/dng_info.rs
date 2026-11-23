//! Decode a DNG as a stack does, render it to sRGB with the SDK's color
//! science, and report. With `RUST_LOG=deptheos_dng::sdk=debug` it also
//! prints how long each stage of the decode took:
//!   cargo run --release -p deptheos_dng --features examples --example dng_info -- <file.dng>

use std::path::Path;
use std::time::Instant;

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: dng_info <file.dng>");
        std::process::exit(2);
    };
    let path = Path::new(&arg);

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    deptheos_dng::route_logs_to_tracing();

    let start = Instant::now();
    deptheos_dng::initialize();
    println!("sdk init         {:.0?}", start.elapsed());

    let start = Instant::now();
    let (image, pixels) = match deptheos_dng::DngImage::read_with_pixels(
        path,
        deptheos_dng::FileCache::Use,
        deptheos_dng::Budget::unlimited(),
    ) {
        Ok(deptheos_dng::DngRead::Linear(image, pixels)) => (image, pixels),
        Ok(deptheos_dng::DngRead::Mosaic(image)) => (image, Vec::new()),
        Err(e) => {
            eprintln!("read failed: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "read (linear)    {:.1?}  {}x{}x{}  {} samples",
        start.elapsed(),
        image.width(),
        image.height(),
        image.channels(),
        pixels.len()
    );
    summarize("linear", &pixels);

    let start = Instant::now();
    match image.render_linear_to_srgb(&pixels, image.width(), image.height(), image.channels()) {
        Ok(srgb) => {
            println!("render to sRGB   {:.1?}", start.elapsed());
            summarize("sRGB  ", srgb.pixels());
        }
        Err(e) => eprintln!("render failed: {e}"),
    }
}

/// Min/max/mean over every sample: a strided subset aliases against the row
/// stride and skews the mean.
fn summarize(label: &str, pixels: &[u16]) {
    if pixels.is_empty() {
        println!("  {label}        (empty)");
        return;
    }
    let mut min = u16::MAX;
    let mut max = 0u16;
    let mut sum = 0u64;
    for &v in pixels {
        min = min.min(v);
        max = max.max(v);
        sum += v as u64;
    }
    println!("  {label}        min={min} max={max} mean={}", sum / pixels.len() as u64);
}
