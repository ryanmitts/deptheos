//! The pixels every draw site hands over, on screen and in exports, written
//! to a folder and checked against it bit for bit, so a change to the render
//! path can be shown to change no picture. GPU results differ between
//! adapters, so a capture is checked only on the machine that wrote it.
//!
//! `just handoff-capture DIR` writes; `just handoff-check DIR` compares.
//! The stacks are synthetic, plus real frames from the end-to-end sets when
//! they are on disk (`DEPTHEOS_E2E_SETS`, default `~/DeptheosE2E/sets`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_core::{FrameAlignment, io};
use image::math::Rect;

use crate::engine::present::PresentedImage;
use crate::engine::present::set_color_managed;
use crate::paint::render::{RectPixels, RectRenderer};
use crate::runs::session::{CompletedStack, SaveOptions, StackColor, export_pixels};
use crate::test_support::engine;
use crate::{Adjustments, Engine, Look, Overlay, RawBackend, SourceInput};

/// A synthetic stack's size: several display tiles and levels.
const SIZE: (u32, u32) = (1100, 700);
const NEUTRAL: Adjustments = Adjustments::NEUTRAL;
/// Where a real frame is cut, from its top left.
const CUT: (u32, u32) = (1800, 1200);

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Write,
    Check,
}

/// Past this many bytes a capture keeps only a hash of its pixels.
const KEPT_WHOLE: usize = 16 << 20;

/// FNV-1a, stable across builds.
fn hash(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// Each capture's size line and pixels (or their hash), by name.
struct Captures {
    dir: PathBuf,
    mode: Mode,
    seen: BTreeMap<String, ()>,
    failed: Vec<String>,
}

impl Captures {
    fn record(&mut self, name: &str, size: String, bytes: &[u8]) {
        assert!(self.seen.insert(name.to_string(), ()).is_none(), "{name} recorded twice");
        let file = self.dir.join(name.replace('/', "__"));
        let hashed;
        let bytes = if bytes.len() > KEPT_WHOLE {
            hashed = format!("hash {:016x}", hash(bytes)).into_bytes();
            &hashed[..]
        } else {
            bytes
        };
        let header = format!("{size}\n");
        match self.mode {
            Mode::Write => {
                let mut out = header.into_bytes();
                out.extend_from_slice(bytes);
                std::fs::write(&file, out).unwrap();
            }
            Mode::Check => {
                let Ok(old) = std::fs::read(&file) else {
                    return self.failed.push(format!("{name}: not in the capture"));
                };
                let split = old.iter().position(|&b| b == b'\n').unwrap() + 1;
                let (old_header, old_bytes) = old.split_at(split);
                if old_header != header.as_bytes() {
                    let was = String::from_utf8_lossy(old_header);
                    return self.failed.push(format!("{name}: {} now, was {}", size, was.trim()));
                }
                if old_bytes != bytes {
                    let differ = old_bytes.iter().zip(bytes).filter(|(a, b)| a != b).count();
                    let most = old_bytes.iter().zip(bytes).map(|(a, b)| a.abs_diff(*b)).max();
                    self.failed.push(format!(
                        "{name}: {differ} bytes differ, by up to {}",
                        most.unwrap_or(0)
                    ));
                }
            }
        }
    }

    /// Every level of a display image, assembled.
    fn image(&mut self, name: &str, image: &PresentedImage) {
        let mut bytes = Vec::new();
        let mut sizes = format!("{}x{} from {}:", image.width(), image.height(), image.first_level);
        for level in &image.levels {
            sizes.push_str(&format!(" {}x{}", level.width, level.height));
            bytes.extend_from_slice(&level.assemble().pixels);
        }
        self.record(name, sizes, &bytes);
    }

    fn rgb16(&mut self, name: &str, (w, h): (u32, u32), pixels: &[u16]) {
        let bytes: Vec<u8> = pixels.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.record(name, format!("{w}x{h} rgb16"), &bytes);
    }

    /// An export's decoded pixels and its embedded profile.
    fn export(&mut self, name: &str, path: &Path) {
        use image::ImageDecoder;
        let reader = image::ImageReader::open(path).unwrap().with_guessed_format().unwrap();
        let mut decoder = reader.into_decoder().unwrap();
        let mut icc = decoder.icc_profile().unwrap().unwrap_or_default();
        // The header's creation time, which a made profile sets.
        if let Some(date) = icc.get_mut(24..36) {
            date.fill(0);
        }
        let decoded = image::DynamicImage::from_decoder(decoder).unwrap();
        let (w, h) = (decoded.width(), decoded.height());
        let kind = format!("{:?}", decoded.color());
        let mut bytes = decoded.as_bytes().to_vec();
        bytes.extend_from_slice(&icc);
        self.record(name, format!("{w}x{h} {kind} icc {}", icc.len()), &bytes);
    }
}

/// A stack to draw, with a second picture of it (painted, or another
/// render) and a source file when it has one.
struct Case {
    name: &'static str,
    stack: CompletedStack,
    other: Vec<u16>,
    file: Option<PathBuf>,
    /// The stack's color again, as a `StackColor` can't be cloned.
    color: Box<dyn Fn() -> StackColor>,
}

impl Case {
    /// The stack with `pixels` in place of its own.
    fn with(&self, pixels: &[u16]) -> CompletedStack {
        let s = &self.stack;
        CompletedStack {
            pixels: pixels.to_vec(),
            color: (self.color)(),
            geometry: s.geometry.clone(),
            ..CompletedStack::srgb(s.width, s.height, pixels.to_vec(), s.paths.clone())
        }
    }
}

/// `stack`'s pixels with every value moved, as paint or another render
/// would give.
fn other_pixels(pixels: &[u16]) -> Vec<u16> {
    pixels.iter().enumerate().map(|(i, &v)| v.wrapping_add((i % 4099) as u16 * 7)).collect()
}

fn gradient((w, h): (u32, u32)) -> Vec<u16> {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32);
            for c in [fx, fy, (fx + fy) * 0.5 * (1.0 + (x as f32 * 0.05).sin()) * 0.5 + 0.25] {
                pixels.push((c.clamp(0.0, 1.0) * 65535.0).round() as u16);
            }
        }
    }
    pixels
}

/// A linear stack of one synthetic DNG frame, with its color science.
fn synthetic_linear(dir: &Path) -> Case {
    use deptheos_dng::test_dng::{Layout, TestDng};
    let (w, h) = SIZE;
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
    let path = dir.join("synthetic.dng");
    dng.write(&path, &gradient(SIZE));
    linear_case("linear-synthetic", &path, None)
}

/// A linear stack of `path` (cut to `cut` when given), rendered through its
/// own color science.
fn linear_case(name: &'static str, path: &Path, cut: Option<Rect>) -> Case {
    let decode = |path: &Path| crate::decode_linear_with_metadata(path, Default::default());
    let (image, _) = decode(path).unwrap();
    let (pixels, (w, h)) = cut_pixels(&image.to_rgb16(), cut);
    let owned = path.to_path_buf();
    let color = Box::new(move || {
        let (_, metadata) = decode(&owned).unwrap();
        let render_params = metadata.render_params().unwrap();
        StackColor::Linear { metadata, render_params }
    });
    case(name, path, cut, (w, h), pixels, color)
}

/// A standard stack of `path` as stored, in its own profile.
fn standard_case(name: &'static str, path: &Path, cut: Option<Rect>, sixteen: bool) -> Case {
    let frame =
        io::load_frame(path, io::FileCache::Use, io::CameraRaw::LibRaw, io::Target::Own).unwrap();
    let (pixels, (w, h)) = cut_pixels(&frame.image.to_rgb16(), cut);
    let profile = frame.profile;
    let color =
        Box::new(move || StackColor::Srgb { sixteen_bit: sixteen, profile: profile.clone() });
    case(name, path, cut, (w, h), pixels, color)
}

fn case(
    name: &'static str,
    path: &Path,
    cut: Option<Rect>,
    (w, h): (u32, u32),
    pixels: Vec<u16>,
    color: Box<dyn Fn() -> StackColor>,
) -> Case {
    let mut stack = CompletedStack::srgb(w, h, pixels, vec![path.to_path_buf()]);
    stack.color = color();
    stack.geometry.crop = cut;
    Case { name, other: other_pixels(&stack.pixels), stack, file: Some(path.to_path_buf()), color }
}

fn cut_pixels(
    image: &image::ImageBuffer<image::Rgb<u16>, Vec<u16>>,
    cut: Option<Rect>,
) -> (Vec<u16>, (u32, u32)) {
    match cut {
        None => (image.as_raw().clone(), image.dimensions()),
        Some(r) => {
            let view = image::imageops::crop_imm(image, r.x, r.y, r.width, r.height).to_image();
            (view.into_raw(), (r.width, r.height))
        }
    }
}

/// The first file of `set` in the end-to-end sets, if they are on disk.
fn set_file(set: &str) -> Option<PathBuf> {
    let root = std::env::var_os("DEPTHEOS_E2E_SETS")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join("DeptheosE2E/sets")))?;
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(root.join(set)).ok()?.filter_map(|e| Some(e.ok()?.path())).collect();
    files.retain(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')));
    files.sort();
    files.into_iter().find(|p| p.extension().is_some_and(|e| e != "json" && e != "txt"))
}

fn cut() -> Option<Rect> {
    Some(Rect { x: CUT.0, y: CUT.1, width: SIZE.0, height: SIZE.1 })
}

fn cases(dir: &Path) -> Vec<Case> {
    let srgb16 = dir.join("srgb16.png");
    let pixels = gradient(SIZE);
    image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(SIZE.0, SIZE.1, pixels)
        .unwrap()
        .save(&srgb16)
        .unwrap();
    let mut cases = vec![
        synthetic_linear(dir),
        standard_case("standard-synthetic-16", &srgb16, None, true),
        standard_case("standard-synthetic-8", &srgb16, None, false),
    ];
    let real = [
        ("linear-dng", "dng-linear/canon1"),
        ("linear-cr3", "cr3/canon1"),
        ("standard-p3", "p3/hornet"),
        ("standard-png16", "png16/hornet"),
        ("standard-tiff16", "tiff16/canon1"),
        ("standard-jpeg", "jpeg/canon1"),
    ];
    for (name, set) in real {
        let Some(file) = set_file(set) else {
            eprintln!("{set} isn't on disk: {name} left out");
            continue;
        };
        let linear = RawBackend::default().decodes_linear(&file);
        cases.push(match linear {
            true => linear_case(name, &file, cut()),
            false => standard_case(name, &file, cut(), name != "standard-jpeg"),
        });
    }
    cases
}

fn draw(engine: &Arc<Engine>, case: &Case, dir: &Path, out: &mut Captures) {
    let Case { name, stack, other, file, .. } = case;
    let at = |site: &str| format!("{name}/{site}");
    let size = (stack.width, stack.height);

    // The result as opened, painted, and as another render shows it.
    let presented = engine.present_stack(case.with(&stack.pixels), &NEUTRAL).unwrap();
    out.image(&at("present_stack"), presented.display.as_ref().unwrap());
    out.image(
        &at("present_pixels"),
        &engine.present_pixels(Look::shown(stack, &NEUTRAL), other).unwrap(),
    );
    let other_stack = case.with(other);
    out.image(
        &at("render_compare"),
        &engine.load_render_compare(stack, Look::shown(&other_stack, &NEUTRAL)).unwrap(),
    );
    let input = SourceInput::Render {
        other: &other_stack,
        pixels: other,
        alignment: FrameAlignment::identity(),
    };
    let (pixels, display) = engine.paint_source(Look::stored(stack), input, true).unwrap();
    out.rgb16(&at("paint_source/pixels"), size, &pixels.unwrap());
    out.image(&at("paint_source"), &display);

    // A frame shown and compared.
    if let Some(file) = file {
        let backend = stack.options.raw_backend;
        out.image(&at("load_preview"), &engine.load_preview(file, backend).unwrap());
        let shown = engine.load_compare(file, Some(0), Look::stored(stack), Overlay::None).unwrap();
        out.image(&at("load_compare"), &shown);
        let input = SourceInput::Frame { path: file, alignment: FrameAlignment::identity() };
        let (_, display) = engine.paint_source(Look::stored(stack), input, false).unwrap();
        out.image(&at("paint_source/frame"), &display);
    }

    // Paint's display of a rect, as a dab redraws it.
    let mut renderer = RectRenderer::new(engine.gpu());
    let rect = Rect { x: 130, y: 70, width: 300, height: 200 };
    let rect_pixels: Vec<u16> = (rect.y..rect.y + rect.height)
        .flat_map(|y| {
            let row = (y * stack.width + rect.x) as usize * 3;
            other[row..row + rect.width as usize * 3].iter().copied()
        })
        .collect();
    let input = RectPixels { pixels: &rect_pixels, width: rect.width, height: rect.height };
    let tile = renderer.render(engine.gpu(), engine.color(), input, Look::stored(stack)).unwrap();
    out.record(&at("paint_rect"), format!("{}x{}", tile.width, tile.height), &tile.pixels);

    // Exports, and the color step sequences share.
    let exported = Look::exported(stack, Some(&NEUTRAL));
    if stack.is_linear() {
        let srgb = export_pixels(engine, exported, other, size).unwrap();
        out.rgb16(&at("render_linear"), size, &srgb);
    }
    for (ext, sixteen) in [("png", true), ("png", false), ("tif", true), ("jpg", false)] {
        let path = dir.join(format!("{name}-{sixteen}.{ext}"));
        let options = SaveOptions { sixteen_bit: sixteen, ..SaveOptions::default() };
        engine.save(exported, &path, options).unwrap();
        out.export(&at(&format!("save/{ext}-{}", if sixteen { 16 } else { 8 })), &path);
        let painted = dir.join(format!("{name}-{sixteen}-painted.{ext}"));
        engine.save_pixels(exported, other, &painted, options).unwrap();
        out.export(&at(&format!("save_pixels/{ext}-{}", if sixteen { 16 } else { 8 })), &painted);
    }

    // With color management off, a profile's values show as they are.
    if stack.profile().is_some() {
        set_color_managed(false);
        let shown = engine.present_pixels(Look::shown(stack, &NEUTRAL), &stack.pixels);
        set_color_managed(true);
        out.image(&at("present_pixels/unmanaged"), &shown.unwrap());
    }
}

/// Writes or checks the capture in `DEPTHEOS_HANDOFF_WRITE` or
/// `DEPTHEOS_HANDOFF_CHECK`.
#[test]
#[ignore = "run through `just handoff-capture` and `just handoff-check`"]
fn handoff_pixels() {
    let (mode, dir) = match (
        std::env::var_os("DEPTHEOS_HANDOFF_WRITE"),
        std::env::var_os("DEPTHEOS_HANDOFF_CHECK"),
    ) {
        (Some(dir), None) => (Mode::Write, PathBuf::from(dir)),
        (None, Some(dir)) => (Mode::Check, PathBuf::from(dir)),
        _ => panic!("set one of DEPTHEOS_HANDOFF_WRITE or DEPTHEOS_HANDOFF_CHECK"),
    };
    let Some(engine) = engine() else { return };
    if mode == Mode::Write {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
    }
    let scratch = tempfile::tempdir().unwrap();
    let mut out = Captures { dir: dir.clone(), mode, seen: BTreeMap::new(), failed: Vec::new() };
    for case in cases(scratch.path()) {
        draw(&engine, &case, scratch.path(), &mut out);
    }
    if mode == Mode::Check {
        let written = std::fs::read_dir(&dir).unwrap().count();
        if written != out.seen.len() {
            out.failed.push(format!("{written} captured, {} drawn now", out.seen.len()));
        }
    }
    println!(
        "{} hand-offs {}",
        out.seen.len(),
        if mode == Mode::Write { "written" } else { "checked" }
    );
    assert!(out.failed.is_empty(), "changed:\n{}", out.failed.join("\n"));
}
