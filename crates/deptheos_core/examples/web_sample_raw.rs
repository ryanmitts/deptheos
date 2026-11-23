//! A website sample stack from camera RAW files: each frame decoded at half
//! size (camera white balance, sRGB; JPEG, PNG or TIFF exports are halved),
//! optionally aligned (ECC) and cropped to the area every frame covers, then
//! downscaled to `max_side` and written as `<prefix>-NN.jpg` plus `stack.json`.
//!
//! cargo run --release -p deptheos_core --features examples --example web_sample_raw -- \
//!     <out_dir> <max_side> <jpeg_quality> <prefix> \
//!     [--align] [--every N] [--crop x,y,w,h] [--full-size] [--format jpg|tif] <raw>...
//!
//! `--every N` keeps frames 1, 1+N, …, aligned only to each other.
//! `--crop` keeps a region, as fractions of the frame, before the downscale.
//! `--full-size` keeps exports at full size (RAW is always halved).
//! `--format tif` writes uncompressed 8-bit TIFFs, for frames cut up or
//! stacked again (`web/scripts/make-lab-assets.ts`).
//!
//! Frames are decoded twice (align, then warp and write), so a stack never
//! has to fit in memory. `stack.json` lists each frame's source and, when
//! aligned, its transform (reference → frame pixel centers, row-major 2×3,
//! before the crop).

use std::path::{Path, PathBuf};

use deptheos_aligner::{AlignMode, Aligner};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};
use rayon::prelude::*;

/// Reference → frame pixel centers, as the aligner reports it.
type Transform = deptheos_aligner::Affine2<f32>;

fn main() {
    let usage = "usage: web_sample_raw <out_dir> <max_side> <jpeg_quality> <prefix> [--align] [--every N] [--crop x,y,w,h] [--full-size] [--format jpg|tif] <raw>...";
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |args: &mut Vec<String>, name: &str| {
        let i = args.iter().position(|a| a == name)?;
        args.remove(i);
        Some(())
    };
    let value = |args: &mut Vec<String>, name: &str| {
        let i = args.iter().position(|a| a == name)?;
        args.remove(i);
        Some(args.remove(i))
    };
    let align = flag(&mut args, "--align").is_some();
    let full_size = flag(&mut args, "--full-size").is_some();
    let tiff = match value(&mut args, "--format").as_deref() {
        None | Some("jpg") => false,
        Some("tif") => true,
        Some(other) => panic!("--format {other}: jpg or tif\n{usage}"),
    };
    let every = value(&mut args, "--every").map_or(1, |n| n.parse::<usize>().expect(usage));
    let crop_frac: Option<[f32; 4]> = value(&mut args, "--crop").map(|v| {
        let v: Vec<f32> = v.split(',').map(|x| x.parse().expect(usage)).collect();
        [v[0], v[1], v[2], v[3]]
    });
    let region = |r: image::math::Rect| match crop_frac {
        None => r,
        Some([fx, fy, fw, fh]) => image::math::Rect {
            x: r.x + (fx * r.width as f32) as u32,
            y: r.y + (fy * r.height as f32) as u32,
            width: (fw * r.width as f32) as u32,
            height: (fh * r.height as f32) as u32,
        },
    };
    let mut args = args.into_iter();
    let out = PathBuf::from(args.next().expect(usage));
    let max_side: u32 = args.next().and_then(|s| s.parse().ok()).expect(usage);
    let quality: u8 = args.next().and_then(|s| s.parse().ok()).expect(usage);
    let prefix = args.next().expect(usage);
    let mut paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
    paths.sort();
    let paths: Vec<PathBuf> = paths.into_iter().step_by(every.max(1)).collect();
    assert!(paths.len() >= 2, "{usage}");
    std::fs::create_dir_all(&out).expect("create out dir");
    let load = |path: &Path| decode(path, full_size);

    // Pass 1: align, one frame at a time.
    let (kept, crop): (Vec<(&PathBuf, Option<Transform>)>, _) = if align {
        let mut aligner = Aligner::new(AlignMode::Ecc);
        let mut kept = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            let image = DynamicImage::ImageRgb8(load(path));
            match aligner.preparer().prepare(&image).and_then(|f| aligner.align_prepared(f)) {
                Ok(a) => kept.push((path, Some(a.transform))),
                Err(e) => println!("frame {} left out: {e}", i + 1),
            }
        }
        let covered = aligner.crop().expect("valid area");
        let crop = region(covered);
        println!("{} frames aligned; every frame covers {covered:?}; keeping {crop:?}", kept.len());
        (kept, crop)
    } else {
        let (w, h) = load(&paths[0]).dimensions();
        (
            paths.iter().map(|p| (p, None)).collect(),
            region(image::math::Rect { x: 0, y: 0, width: w, height: h }),
        )
    };

    // Pass 2: warp (or crop), downscale and write, a few frames at a time.
    let ext = if tiff { "tif" } else { "jpg" };
    let entries: Vec<String> = kept
        .par_iter()
        .enumerate()
        .map(|(i, (path, transform))| {
            let frame = load(path);
            let rgb = match transform {
                Some(t) => warp(&frame, t, crop),
                None => image::imageops::crop_imm(&frame, crop.x, crop.y, crop.width, crop.height)
                    .to_image(),
            };
            drop(frame);
            let (w, h) = rgb.dimensions();
            let s = max_side as f64 / w.max(h) as f64;
            let small = if s < 1.0 {
                let (nw, nh) = (
                    ((w as f64 * s).round() as u32).max(1),
                    ((h as f64 * s).round() as u32).max(1),
                );
                image::imageops::resize(&rgb, nw, nh, FilterType::Lanczos3)
            } else {
                rgb
            };
            let name = format!("{prefix}-{:02}.{ext}", i + 1);
            let file = std::fs::File::create(out.join(&name)).expect("create frame");
            let mut writer = std::io::BufWriter::new(file);
            if tiff {
                let mut encoder = tiff::encoder::TiffEncoder::new(&mut writer).expect("tiff");
                encoder
                    .write_image::<tiff::encoder::colortype::RGB8>(
                        small.width(),
                        small.height(),
                        small.as_raw(),
                    )
                    .expect("encode tiff");
            } else {
                JpegEncoder::new_with_quality(&mut writer, quality)
                    .encode_image(&small)
                    .expect("encode");
            }
            let source = path.file_name().unwrap().to_string_lossy();
            let transform = match transform {
                Some(t) => {
                    let m = t.matrix();
                    format!(
                        ", \"transform\": [{}, {}, {}, {}, {}, {}]",
                        m[(0, 0)],
                        m[(0, 1)],
                        m[(0, 2)],
                        m[(1, 0)],
                        m[(1, 1)],
                        m[(1, 2)]
                    )
                }
                None => String::new(),
            };
            format!("    {{ \"file\": {name:?}, \"source\": {source:?}{transform} }}")
        })
        .collect();

    let names = entries.join(",\n");
    let crop_json = format!("[{}, {}, {}, {}]", crop.x, crop.y, crop.width, crop.height);
    std::fs::write(
        out.join("stack.json"),
        format!("{{\n  \"crop\": {crop_json},\n  \"frames\": [\n{names}\n  ]\n}}\n"),
    )
    .expect("write stack.json");
    println!("{} frames -> {}", entries.len(), out.display());
}

/// One frame as 8-bit sRGB: an export (halved unless `full_size`, to match
/// a RAW decode), or a RAW file decoded at half size.
fn decode(path: &Path, full_size: bool) -> RgbImage {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    if matches!(ext.as_deref(), Some("jpg" | "jpeg" | "png" | "tif" | "tiff")) {
        let rgb = image::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).to_rgb8();
        if full_size {
            return rgb;
        }
        let (w, h) = rgb.dimensions();
        return image::imageops::resize(&rgb, w / 2, h / 2, FilterType::Lanczos3);
    }
    assert!(!full_size, "{}: --full-size reads JPEG, PNG and TIFF exports only", path.display());
    let (pixels, w, h) = deptheos_raw::read_half_size(path, deptheos_raw::Budget::unlimited())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    RgbImage::from_raw(w, h, pixels).expect("pixel count")
}

/// The frame resampled bilinearly onto the reference's grid, edges
/// clamped, cropped to `crop` in reference coordinates.
fn warp(frame: &RgbImage, t: &deptheos_aligner::Affine2<f32>, crop: image::math::Rect) -> RgbImage {
    let m = t.matrix();
    let (fw, fh) = frame.dimensions();
    let px = |x: i64, y: i64| {
        frame.get_pixel(x.clamp(0, fw as i64 - 1) as u32, y.clamp(0, fh as i64 - 1) as u32).0
    };
    RgbImage::from_fn(crop.width, crop.height, |cx, cy| {
        let (rx, ry) = ((cx + crop.x) as f32, (cy + crop.y) as f32);
        let sx = (m[(0, 0)] * rx + m[(0, 1)] * ry + m[(0, 2)]).clamp(0.0, fw as f32 - 1.0);
        let sy = (m[(1, 0)] * rx + m[(1, 1)] * ry + m[(1, 2)]).clamp(0.0, fh as f32 - 1.0);
        let (x0, y0) = (sx.floor() as i64, sy.floor() as i64);
        let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
        let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
        image::Rgb(std::array::from_fn(|k| {
            let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
            let bottom = c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
            (top * (1.0 - fy) + bottom * fy).round() as u8
        }))
    })
}
