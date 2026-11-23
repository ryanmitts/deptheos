//! Pixel work for the website's lab figures, run by
//! `web/scripts/make-lab-assets.ts`.
//!
//! cargo run --release -p deptheos_core --features examples --example web_lab -- <command> …
//!
//! - `crop <image> <quality> <x,y,w,h[@width]>=<out.jpg> …`: cuts
//!   rectangles (resized to `width` if given) out of one image, writing each
//!   as an sRGB JPEG, or a PNG when the name ends in `.png`.
//! - `pick <out_dir> <quality> <x,y,w,h> <side,side,…> <frame>…`: the
//!   per-pixel pick no app method does: each pixel from the frame whose
//!   pyramid finest-level score (`focus_metric` in `laplacian.wgsl`),
//!   averaged over a square of each side, is best. Writes `w<side>.jpg` and
//!   `w<side>-map.png` (each pixel's frame as a gray level, 0 the first).
//! - `depthmap [--radius <r>] <x,y,w,h>=<out.png>… -- <frame>…`: stacks
//!   with the depth method (default settings but Radius) and cuts
//!   rectangles of its frame map, as `pick` writes its maps.
//! - `merged <quality> <x,y,w,h>=<out.jpg>… -- <frame>…`: runs the wavelet
//!   method's first pass and cuts rectangles of the merged gray target, as
//!   gray JPEGs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_core::StackInfo;
use deptheos_core::depth::{DepthConfig, DepthStacker};
use deptheos_core::wavelet::{WaveletConfig, WaveletStacker};
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;
use image::DynamicImage;
use image::codecs::jpeg::JpegEncoder;

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: web_lab crop <image> <quality> <x,y,w,h>=<out.jpg>… | pick <out_dir> <quality> <x,y,w,h> <side,…> <frame>… | depthmap [--radius <r>] <x,y,w,h>=<out.png>… -- <frame>… | merged <quality> <x,y,w,h>=<out.jpg>… -- <frame>…";
    match args.next().as_deref() {
        Some("crop") => {
            let image = PathBuf::from(args.next().expect(usage));
            let quality: u8 = args.next().and_then(|q| q.parse().ok()).expect(usage);
            let cuts: Vec<(Rect, Option<u32>, PathBuf)> = args
                .map(|a| {
                    let (rect, out) = a.split_once('=').expect(usage);
                    let (rect, width) = match rect.split_once('@') {
                        Some((r, w)) => (r, Some(w.parse::<u32>().expect(usage))),
                        None => (rect, None),
                    };
                    (parse_rect(rect), width, PathBuf::from(out))
                })
                .collect();
            crop(&image, quality, &cuts);
        }
        Some("pick") => {
            let out = PathBuf::from(args.next().expect(usage));
            let quality: u8 = args.next().and_then(|q| q.parse().ok()).expect(usage);
            let rect = parse_rect(&args.next().expect(usage));
            let sides: Vec<u32> =
                args.next().expect(usage).split(',').map(|s| s.parse().expect(usage)).collect();
            let frames: Vec<PathBuf> = args.map(PathBuf::from).collect();
            assert!((2..=256).contains(&frames.len()), "{usage}");
            pick(&out, quality, rect, &sides, &frames);
        }
        Some("depthmap") => {
            let mut args = args.peekable();
            let mut config = DepthConfig::default();
            if args.next_if_eq("--radius").is_some() {
                config.radius = args.next().and_then(|r| r.parse().ok()).expect(usage);
            }
            let (cuts, frames) = cuts_and_frames(args, usage);
            depth_map(config, &cuts, &frames);
        }
        Some("merged") => {
            let quality: u8 = args.next().and_then(|q| q.parse().ok()).expect(usage);
            let (cuts, frames) = cuts_and_frames(args, usage);
            merged(quality, &cuts, &frames);
        }
        _ => panic!("{usage}"),
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

fn parse_rect(s: &str) -> Rect {
    let v: Vec<u32> = s.split(',').map(|x| x.trim().parse().expect("rect: x,y,w,h")).collect();
    assert_eq!(v.len(), 4, "rect: x,y,w,h");
    Rect { x: v[0], y: v[1], w: v[2], h: v[3] }
}

/// The `<x,y,w,h>=<out>` cuts before `--`, and the frames after it.
fn cuts_and_frames(
    mut args: impl Iterator<Item = String>,
    usage: &str,
) -> (Vec<(Rect, PathBuf)>, Vec<PathBuf>) {
    let cuts = args
        .by_ref()
        .take_while(|a| a != "--")
        .map(|a| {
            let (rect, out) = a.split_once('=').expect(usage);
            (parse_rect(rect), PathBuf::from(out))
        })
        .collect();
    let frames: Vec<PathBuf> = args.map(PathBuf::from).collect();
    assert!((2..=256).contains(&frames.len()), "{usage}");
    (cuts, frames)
}

fn gpu() -> Arc<GpuContext> {
    Arc::new(GpuContext::new().expect("a GPU"))
}

/// The rectangle `r` of a full-size r32float texture, row-major.
fn download(ctx: &GpuContext, texture: &wgpu::Texture, r: Rect) -> Vec<f32> {
    let (w, h) = (texture.width(), texture.height());
    assert!(r.x + r.w <= w && r.y + r.h <= h, "{r:?} is outside the {w}x{h} stack");
    let mut values = Vec::with_capacity((r.w * r.h) as usize);
    download_texture_with(ctx, texture, [r.x, r.y], [r.w, r.h], |row| {
        values.extend_from_slice(bytemuck::cast_slice(row))
    })
    .expect("download");
    values
}

fn depth_map(config: DepthConfig, cuts: &[(Rect, PathBuf)], frames: &[PathBuf]) {
    let ctx = gpu();
    let total = frames.len() as u32;
    let mut stacker =
        DepthStacker::new(ctx.clone(), config, StackInfo { total_frames: total, is_linear: false });
    for (k, path) in frames.iter().enumerate() {
        stacker.accumulate(&open(path), None).expect("accumulate");
        println!("depth: scored frame {} of {total}", k + 1);
    }
    stacker.end_pass_one().expect("end pass one");
    for (k, path) in frames.iter().enumerate() {
        stacker.blend_frame(&open(path), k as u32, None).expect("blend");
        println!("depth: blended frame {} of {total}", k + 1);
    }
    stacker.finalize().expect("finalize");
    let map = stacker.frame_map().expect("a finished depth stack has its frame map");
    let last = (total - 1) as f32;
    for (r, out) in cuts {
        let planes = download(&ctx, map, *r);
        let index = image::GrayImage::from_fn(r.w, r.h, |x, y| {
            image::Luma([(planes[(y * r.w + x) as usize] * last).round().clamp(0.0, last) as u8])
        });
        index.save(out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    }
}

fn merged(quality: u8, cuts: &[(Rect, PathBuf)], frames: &[PathBuf]) {
    let ctx = gpu();
    let total = frames.len() as u32;
    let mut stacker = WaveletStacker::new(
        ctx.clone(),
        WaveletConfig::default(),
        StackInfo { total_frames: total, is_linear: false },
    );
    for (k, path) in frames.iter().enumerate() {
        stacker.accumulate(&open(path), None).expect("accumulate");
        println!("wavelet: took frame {} of {total}", k + 1);
    }
    stacker.end_pass_one().expect("end pass one");
    let target = stacker.merged_target().expect("the merged target after pass one");
    for (r, out) in cuts {
        let luma = download(&ctx, target, *r);
        let gray = image::RgbImage::from_fn(r.w, r.h, |x, y| {
            let v = (luma[(y * r.w + x) as usize].clamp(0.0, 1.0) * 255.0).round() as u8;
            image::Rgb([v, v, v])
        });
        write_jpeg(&gray, quality, out);
    }
}

fn open(path: &Path) -> DynamicImage {
    image::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn crop(image: &Path, quality: u8, cuts: &[(Rect, Option<u32>, PathBuf)]) {
    let rgb = open(image).to_rgb8();
    let (w, h) = rgb.dimensions();
    for (r, width, out) in cuts {
        assert!(
            r.x + r.w <= w && r.y + r.h <= h,
            "{}: {r:?} is outside the {w}x{h} image",
            image.display()
        );
        let mut part = image::imageops::crop_imm(&rgb, r.x, r.y, r.w, r.h).to_image();
        if let Some(w) = *width {
            let h = ((r.h as f64 * w as f64 / r.w as f64).round() as u32).max(1);
            part = image::imageops::resize(&part, w, h, image::imageops::FilterType::Lanczos3);
        }
        if out.extension().is_some_and(|e| e == "png") {
            part.save(out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
        } else {
            write_jpeg(&part, quality, out);
        }
    }
}

fn write_jpeg(image: &image::RgbImage, quality: u8, out: &Path) {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("create out dir");
    }
    let file = std::fs::File::create(out).expect("create jpeg");
    JpegEncoder::new_with_quality(std::io::BufWriter::new(file), quality)
        .encode_image(image)
        .expect("encode");
}

/// The pyramid's 5-tap binomial Gaussian, [1 4 6 4 1] / 16.
const GAUSSIAN: [f32; 5] = [0.0625, 0.25, 0.375, 0.25, 0.0625];
/// The Weber floor (`DARK_FLOOR` in `metric.wgsl`).
const DARK_FLOOR: f32 = 0.1;

/// RGB planes of `w`×`h` floats in 0..1.
struct Planes {
    w: usize,
    h: usize,
    c: [Vec<f32>; 3],
}

impl Planes {
    fn at(&self, k: usize, x: i64, y: i64) -> f32 {
        let x = x.clamp(0, self.w as i64 - 1) as usize;
        let y = y.clamp(0, self.h as i64 - 1) as usize;
        self.c[k][y * self.w + x]
    }
}

/// The next coarser Gaussian level: blur and take every other pixel, edges clamped.
fn downsample(p: &Planes) -> Planes {
    let (w, h) = (p.w.div_ceil(2), p.h.div_ceil(2));
    let c = std::array::from_fn(|k| {
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for (j, wy) in GAUSSIAN.iter().enumerate() {
                    for (i, wx) in GAUSSIAN.iter().enumerate() {
                        sum += p.at(k, 2 * x as i64 + i as i64 - 2, 2 * y as i64 + j as i64 - 2)
                            * wx
                            * wy;
                    }
                }
                out[y * w + x] = sum;
            }
        }
        out
    });
    Planes { w, h, c }
}

/// The parity upsample of `gaussian_upsample` (laplacian.wgsl), at fine pixel (x, y).
fn upsample_at(coarse: &Planes, k: usize, x: usize, y: usize) -> f32 {
    let weights = |odd: bool| {
        if odd { [0.0, GAUSSIAN[1], GAUSSIAN[3]] } else { [GAUSSIAN[0], GAUSSIAN[2], GAUSSIAN[4]] }
    };
    let (wx, wy) = (weights(x & 1 == 1), weights(y & 1 == 1));
    let (cx, cy) = ((x / 2) as i64, (y / 2) as i64);
    let mut sum = 0.0;
    for (j, wy) in wy.iter().enumerate() {
        for (i, wx) in wx.iter().enumerate() {
            sum += coarse.at(k, cx - 1 + i as i64, cy - 1 + j as i64) * wx * wy;
        }
    }
    sum * 4.0
}

fn pick(out: &Path, quality: u8, r: Rect, sides: &[u32], frames: &[PathBuf]) {
    // Score a margin around the rectangle too, so the squares at its edge
    // average real scores, as they would inside a whole frame.
    let margin = sides.iter().max().copied().unwrap_or(1) / 2 + 8;
    let n = frames.len();
    let (rw, rh) = (r.w as usize, r.h as usize);
    struct Best {
        side: usize,
        score: Vec<f32>,
        frame: Vec<usize>,
        rgb: Vec<[u8; 3]>,
    }
    let mut best: Vec<Best> = sides
        .iter()
        .map(|&s| Best {
            side: s as usize,
            score: vec![f32::MIN; rw * rh],
            frame: vec![0; rw * rh],
            rgb: vec![[0; 3]; rw * rh],
        })
        .collect();
    for (k, path) in frames.iter().enumerate() {
        let rgb = open(path).to_rgb8();
        let (fw, fh) = rgb.dimensions();
        let x0 = r.x.saturating_sub(margin);
        let y0 = r.y.saturating_sub(margin);
        let x1 = (r.x + r.w + margin).min(fw);
        let y1 = (r.y + r.h + margin).min(fh);
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let planes = Planes {
            w,
            h,
            c: std::array::from_fn(|k| {
                let mut v = vec![0.0; w * h];
                for y in 0..h {
                    for x in 0..w {
                        v[y * w + x] =
                            rgb.get_pixel(x0 + x as u32, y0 + y as u32)[k] as f32 / 255.0;
                    }
                }
                v
            }),
        };
        let coarse = downsample(&planes);
        // The score, and its integral image for the square means.
        let mut integral = vec![0.0f64; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0.0f64;
            for x in 0..w {
                let mut rel = 0.0;
                for c in 0..3 {
                    let u = upsample_at(&coarse, c, x, y);
                    rel += (planes.c[c][y * w + x] - u).abs() / (u.max(0.0) + DARK_FLOOR);
                }
                row += (rel / 3.0) as f64;
                integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
            }
        }
        let (ox, oy) = ((r.x - x0) as usize, (r.y - y0) as usize);
        for b in &mut best {
            let lo = (b.side - 1) / 2;
            for y in 0..rh {
                let ya = (oy + y).saturating_sub(lo);
                let yb = (ya + b.side).min(h);
                for x in 0..rw {
                    let xa = (ox + x).saturating_sub(lo);
                    let xb = (xa + b.side).min(w);
                    let sum = integral[yb * (w + 1) + xb]
                        - integral[ya * (w + 1) + xb]
                        - integral[yb * (w + 1) + xa]
                        + integral[ya * (w + 1) + xa];
                    let mean = (sum / ((xb - xa) * (yb - ya)) as f64) as f32;
                    let i = y * rw + x;
                    if mean > b.score[i] {
                        b.score[i] = mean;
                        b.frame[i] = k;
                        b.rgb[i] = rgb.get_pixel(r.x + x as u32, r.y + y as u32).0;
                    }
                }
            }
        }
        println!("scored frame {} of {n}", k + 1);
    }
    for b in &best {
        let picture = image::RgbImage::from_fn(r.w, r.h, |x, y| {
            image::Rgb(b.rgb[y as usize * rw + x as usize])
        });
        write_jpeg(&picture, quality, &out.join(format!("w{}.jpg", b.side)));
        // Frame numbers as gray levels, colored by the page: far smaller
        // than a JPEG of the colors, which can't compress the pick's noise.
        let map = image::GrayImage::from_fn(r.w, r.h, |x, y| {
            image::Luma([b.frame[y as usize * rw + x as usize] as u8])
        });
        map.save(out.join(format!("w{}-map.png", b.side))).expect("write map");
    }
}
