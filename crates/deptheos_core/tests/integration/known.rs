//! Known images: frames whose pixels are known, made outside the engine,
//! each loaded as the app loads it and compared, with a tolerance, with
//! what it holds. The fixtures are in `tests/fixtures`:
//!
//! - `chart-16.png`: a chart of twelve flat patches ([`PATCHES`], sRGB,
//!   16 bits, written by hand with an `sRGB` chunk), and from it, by macOS
//!   `sips`: `chart-16.tif` (16-bit LZW TIFF), `chart-p3.jpg` and
//!   `chart-adobe-rgb.jpg` (matched to Display P3 and Adobe RGB (1998),
//!   their profiles embedded), and `chart-orientation-6.jpg` (stored turned
//!   a quarter left, with EXIF Orientation 6 to turn it back).
//! - `chart-gama-chrm.png`: the chart in Display P3's primaries with a 1.8
//!   gamma, said by `gAMA` and `cHRM` chunks only.
//! - `basn2c16.png`: the 16-bit RGB image of Willem van Schaik's PngSuite
//!   (free to use, copy and distribute), linear by its `gAMA`.
//! - `converter.dng`: Adobe DNG Converter's linear DNG (lossy, 384 px on
//!   its long side) of a Canon EOS R5 Mark II CR3, its owner's and serial
//!   numbers blanked.
//! - `mosaic.dng`: a mosaic DNG of smooth ramps, and `mosaic.dcraw_emu.txt`
//!   its patch means as LibRaw's `dcraw_emu` decodes it with the engine's
//!   settings (`-4 -o 0 -r 1 1 1 1 -q 2 -c 0`: linear 16-bit camera RGB, no
//!   white balance, PPG, the camera's white level).
//! - `truncated.cr3`: the first 256 KB of that CR3, for the damaged-file
//!   tests (`corrupt.rs`), its owner's and serial numbers blanked.

use std::path::{Path, PathBuf};

use deptheos_core::io::{self, CameraRaw, FileCache, LinearColor, StackSpace, Target};
use image::DynamicImage;

/// The chart's patches, sRGB, in rows of four of 64 × 64 pixels.
const PATCHES: [[f32; 3]; 12] = [
    [0.80, 0.20, 0.20],
    [0.20, 0.70, 0.30],
    [0.25, 0.30, 0.80],
    [0.90, 0.85, 0.20],
    [0.50, 0.50, 0.50],
    [0.10, 0.10, 0.10],
    [0.95, 0.95, 0.95],
    [0.70, 0.40, 0.60],
    [0.30, 0.60, 0.70],
    [0.60, 0.45, 0.30],
    [0.40, 0.20, 0.50],
    [0.75, 0.75, 0.40],
];
const CHART: (u32, u32) = (256, 192);
const PATCH: u32 = 64;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// A frame as a stack of sRGB frames loads it.
fn load_srgb(path: &Path) -> io::Frame {
    let srgb = StackSpace::new(None);
    io::load_frame(path, FileCache::Use, CameraRaw::LibRaw, Target::Stack(&srgb))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Each channel of `image` at (x, y), 0 to 1.
fn at(image: &DynamicImage, x: u32, y: u32) -> [f32; 3] {
    image.to_rgb32f().get_pixel(x, y).0
}

/// The mean of the inner half of each `cols` × `rows` patch, 0 to 1.
fn patch_means(image: &DynamicImage, cols: u32, rows: u32) -> Vec<[f32; 3]> {
    let rgb = image.to_rgb32f();
    let (pw, ph) = (rgb.width() / cols, rgb.height() / rows);
    (0..rows * cols)
        .map(|i| {
            let (x0, y0) = ((i % cols) * pw, (i / cols) * ph);
            let mut sum = [0f64; 3];
            let mut n = 0.0;
            for y in y0 + ph / 4..y0 + 3 * ph / 4 {
                for x in x0 + pw / 4..x0 + 3 * pw / 4 {
                    for (s, v) in sum.iter_mut().zip(rgb.get_pixel(x, y).0) {
                        *s += f64::from(v);
                    }
                    n += 1.0;
                }
            }
            sum.map(|s| (s / n) as f32)
        })
        .collect()
}

/// Every chart frame loads into sRGB upright, at its depth, as the chart:
/// each patch within `tolerance` of what it is.
#[test]
fn chart_frames_load_as_their_patches() {
    let cases = [
        // Lossless: only rounding.
        ("chart-16.png", 16, 0.001),
        ("chart-16.tif", 16, 0.001),
        // 8-bit JPEG.
        ("chart-orientation-6.jpg", 8, 0.01),
        // 8-bit JPEG in another space, converted to sRGB at 16 bits.
        ("chart-p3.jpg", 16, 0.015),
        ("chart-adobe-rgb.jpg", 16, 0.015),
        // 16 bits in P3 primaries and a 1.8 gamma, said by chunks.
        ("chart-gama-chrm.png", 16, 0.004),
    ];
    for (name, bits, tolerance) in cases {
        let frame = load_srgb(&fixture(name));
        let image = &frame.image;
        assert_eq!((image.width(), image.height()), CHART, "{name}: upright");
        let depth = if image.as_rgb16().is_some() { 16 } else { 8 };
        assert_eq!(depth, bits, "{name}: its depth");
        assert_eq!(frame.profile, None, "{name}: loaded into sRGB");
        let means = patch_means(image, CHART.0 / PATCH, CHART.1 / PATCH);
        for (i, (got, want)) in means.iter().zip(PATCHES).enumerate() {
            let off = (0..3).map(|c| (got[c] - want[c]).abs()).fold(0f32, f32::max);
            assert!(off <= tolerance, "{name}: patch {i} is {got:?}, the chart's {want:?}");
        }
    }
}

/// A frame in its own space keeps it: a profile it embeds, or one made
/// from its chunks; an sRGB frame has none.
#[test]
fn chart_frames_say_their_space() {
    let own = |name: &str| {
        io::load_frame(&fixture(name), FileCache::Use, CameraRaw::LibRaw, Target::Own)
            .unwrap()
            .profile
            .map(|p| p.name().unwrap_or_default())
    };
    assert_eq!(own("chart-16.png"), None);
    assert_eq!(own("chart-16.tif"), None);
    assert_eq!(own("chart-orientation-6.jpg"), None);
    assert_eq!(own("chart-p3.jpg").as_deref(), Some("Display P3"));
    assert_eq!(own("chart-adobe-rgb.jpg").as_deref(), Some("Adobe RGB (1998)"));
    assert!(own("chart-gama-chrm.png").is_some(), "a profile made from gAMA and cHRM");
}

/// PngSuite's 16-bit RGB PNG, linear by its gAMA chunk, loads at 16 bits
/// encoded to sRGB's curve.
#[test]
fn a_pngsuite_16_bit_png_loads() {
    let frame = load_srgb(&fixture("basn2c16.png"));
    let image = &frame.image;
    assert!(image.as_rgb16().is_some(), "16 bits");
    assert_eq!((image.width(), image.height()), (32, 32));
    let encode = |v: u16| {
        let x = f32::from(v) / 65535.0;
        if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
    };
    // As the file stores them, read by hand from its IDAT.
    let stored: [((u32, u32), [u16; 3]); 6] = [
        ((0, 0), [65535, 65535, 0]),
        ((31, 0), [0, 65535, 0]),
        ((0, 31), [65535, 0, 0]),
        ((31, 31), [0, 0, 65535]),
        ((16, 8), [31710, 48623, 0]),
        ((8, 24), [48623, 14798, 2114]),
    ];
    for ((x, y), raw) in stored {
        let got = at(image, x, y);
        let want = raw.map(encode);
        let off = (0..3).map(|c| (got[c] - want[c]).abs()).fold(0f32, f32::max);
        assert!(off <= 0.002, "({x}, {y}): {got:?}, sRGB-encoded {want:?}");
    }
}

/// Adobe DNG Converter's linear DNG decodes through the SDK, with its own
/// color science, at the size its header says.
#[test]
fn a_converter_dng_decodes() {
    let path = fixture("converter.dng");
    let linear = io::decode_linear(&path).unwrap();
    assert!(matches!(linear.color, LinearColor::Dng(_)), "the SDK's color science");
    let image = linear.image.as_rgb16().expect("16-bit RGB");
    let (w, h) = image.dimensions();
    assert_eq!(w.max(h), 384, "its long side");
    let header = io::header_info(&path, CameraRaw::LibRaw).expect("a header");
    assert_eq!((header.width, header.height), (w, h));
    // A photograph: not black, not clipped.
    let mean = image.as_raw().iter().map(|&v| f64::from(v)).sum::<f64>()
        / image.as_raw().len() as f64
        / 65535.0;
    assert!((0.005..0.9).contains(&mean), "mean {mean}");
}

/// A mosaic DNG decodes as LibRaw's own `dcraw_emu` decodes it with the
/// engine's settings: each patch's mean within 0.5%.
#[test]
fn a_mosaic_dng_decodes_as_dcraw_emu_does() {
    let path = fixture("mosaic.dng");
    let frame = io::load_frame(&path, FileCache::Use, CameraRaw::LibRaw, Target::Own).unwrap();
    let image = &frame.image;
    assert!(image.as_rgb16().is_some(), "16-bit linear camera RGB");
    assert_eq!((image.width(), image.height()), (256, 192));
    let reference = std::fs::read_to_string(fixture("mosaic.dcraw_emu.txt")).unwrap();
    let want: Vec<[f32; 3]> = reference
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let v: Vec<f32> = l.split(' ').map(|v| v.parse::<f32>().unwrap() / 65535.0).collect();
            [v[0], v[1], v[2]]
        })
        .collect();
    let got = patch_means(image, 8, 6);
    assert_eq!(got.len(), want.len());
    for (i, (got, want)) in got.iter().zip(&want).enumerate() {
        for c in 0..3 {
            let off = (got[c] - want[c]).abs() / want[c].max(0.01);
            assert!(off <= 0.005, "patch {i}: {got:?}, dcraw_emu's {want:?}");
        }
    }
}
