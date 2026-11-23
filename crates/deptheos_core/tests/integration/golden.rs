//! The stackers' golden outputs, each compared with a 16-bit reference
//! picture in `tests/goldens/` within a tolerance a GPU's rounding stays
//! inside (ADR-113). `DEPTHEOS_BLESS_GOLDENS=1` (`just bless-goldens`)
//! writes the references from this machine's outputs instead.

use std::path::PathBuf;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

/// A channel off by more than this changes its pixel: one 8-bit step.
const CHANGED: f32 = 1.0 / 255.0;

/// The largest share of pixels that may change: winners a near-tie flips.
const MAX_CHANGED: f64 = 0.001;

/// The lowest PSNR (dB, peak 1) of the pixels that did not change. A
/// rerun here gives about 110 (the reference's 16-bit rounding); one
/// frame's exposure off by 0.1%, or its warp by 0.002 px, gives 70 to 74.
const MIN_PSNR: f64 = 75.0;

/// One output of a golden stack: RGBA (`channels` 4) or a frame map (1),
/// row by row.
pub struct Output {
    name: String,
    width: u32,
    height: u32,
    channels: usize,
    samples: Vec<f32>,
}

/// A golden run: its name, and its result and frame map as rgba32float and
/// r32float bytes.
pub type Run<'a> = (&'a str, (Vec<u8>, Vec<u8>));

/// A stacker's golden runs as `w`×`h` outputs, named
/// `<stacker>-<run>-result` and `<stacker>-<run>-map`.
pub fn outputs(stacker: &str, w: u32, h: u32, runs: &[Run]) -> Vec<Output> {
    let output = |run: &str, what: &str, channels, bytes: &[u8]| Output {
        name: format!("{stacker}-{run}-{what}"),
        width: w,
        height: h,
        channels,
        samples: bytemuck::cast_slice(bytes).to_vec(),
    };
    runs.iter()
        .flat_map(|(run, (result, map))| {
            [output(run, "result", 4, result), output(run, "map", 1, map)]
        })
        .collect()
}

/// How far an output is from its reference: the share of pixels changed,
/// the PSNR of the rest, and the largest channel difference.
struct Distance {
    changed: f64,
    psnr: f64,
    max: f32,
}

impl std::fmt::Display for Distance {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let Self { changed, psnr, max } = self;
        let percent = changed * 100.0;
        write!(f, "{percent:.3}% of pixels changed, the rest {psnr:.1} dB, largest {max:.6}")
    }
}

/// Check each output against its reference, or write the references when
/// blessing. Every distance is printed; the test fails after all of them.
pub fn check(outputs: &[Output]) {
    let blessing = std::env::var_os("DEPTHEOS_BLESS_GOLDENS").is_some_and(|v| v != "0");
    let mut failed = Vec::new();
    for output in outputs {
        let pixels = (output.width * output.height) as usize;
        assert_eq!(output.samples.len(), pixels * output.channels, "{}: size", output.name);
        if blessing {
            bless(output);
            continue;
        }
        let distance = distance(output, &read(output));
        println!("{}: {distance}", output.name);
        if distance.changed > MAX_CHANGED || distance.psnr < MIN_PSNR {
            failed.push(format!("{}: {distance}", output.name));
        }
    }
    assert!(
        failed.is_empty(),
        "outputs moved from their references (at most {}% of pixels changed, the rest at \
         least {MIN_PSNR} dB): {failed:#?}; `just bless-goldens` if intended",
        MAX_CHANGED * 100.0
    );
}

fn reference_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens").join(format!("{name}.png"))
}

fn bless(output: &Output) {
    let (lo, hi) =
        output.samples.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    assert!(lo >= 0.0 && hi <= 1.0, "{}: values {lo}..{hi} outside [0, 1]", output.name);
    let samples: Vec<u16> = output.samples.iter().map(|&v| (v * 65535.0).round() as u16).collect();
    let color = match output.channels {
        4 => ExtendedColorType::Rgba16,
        1 => ExtendedColorType::L16,
        n => panic!("{}: {n} channels", output.name),
    };
    let path = reference_path(&output.name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    PngEncoder::new_with_quality(file, CompressionType::Best, FilterType::Adaptive)
        .write_image(bytemuck::cast_slice(&samples), output.width, output.height, color)
        .unwrap();
    println!("{}: blessed {}", output.name, path.display());
}

fn read(output: &Output) -> Vec<f32> {
    let path = reference_path(&output.name);
    let image = image::open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; `just bless-goldens` writes it", path.display()));
    assert_eq!(
        (image.width(), image.height()),
        (output.width, output.height),
        "{}: reference size",
        output.name
    );
    let samples = match output.channels {
        4 => image.into_rgba16().into_raw(),
        _ => image.into_luma16().into_raw(),
    };
    samples.iter().map(|&v| f32::from(v) / 65535.0).collect()
}

fn distance(output: &Output, reference: &[f32]) -> Distance {
    let channels = output.channels;
    let (mut changed, mut squares, mut kept, mut max) = (0usize, 0.0f64, 0usize, 0.0f32);
    for (got, want) in output.samples.chunks_exact(channels).zip(reference.chunks_exact(channels)) {
        let diffs = got.iter().zip(want).map(|(&g, &r)| (g.clamp(0.0, 1.0) - r).abs());
        let largest = diffs.clone().fold(0.0f32, f32::max);
        max = max.max(largest);
        if largest > CHANGED {
            changed += 1;
        } else {
            squares += diffs.map(|d| f64::from(d) * f64::from(d)).sum::<f64>();
            kept += channels;
        }
    }
    let mse = if kept == 0 { 0.0 } else { squares / kept as f64 };
    let psnr = if mse == 0.0 { f64::INFINITY } else { -10.0 * mse.log10() };
    let pixels = output.samples.len() / channels;
    Distance { changed: changed as f64 / pixels as f64, psnr, max }
}
