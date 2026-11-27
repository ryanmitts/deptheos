//! A render's white point and exposure, the SDK's temperature, and a DNG
//! written with a chosen white balance.

use std::path::Path;
use std::time::Instant;

use deptheos_raw::{RawDngColor, TestMetadata};

use crate::{DngImage, DngTags, RenderParams, Temperature, WhiteXy};

/// A camera with a dual-illuminant profile (Standard A and D65, so a white
/// moves its matrix) shot under a light whose camera multipliers are
/// `cam_mul`.
fn camera(cam_mul: [f32; 4]) -> TestMetadata {
    let matrix = |m: [f32; 9]| {
        let mut out = [0.0; 12];
        out[..9].copy_from_slice(&m);
        Some(out)
    };
    TestMetadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_mul,
        raw_colors: 3,
        dng_color: [
            RawDngColor {
                illuminant: 17,
                color_matrix: matrix([
                    0.6722, -0.0635, -0.0963, -0.4287, 1.2460, 0.2028, -0.0908, 0.2162, 0.5668,
                ]),
                ..Default::default()
            },
            RawDngColor {
                illuminant: 21,
                color_matrix: matrix([
                    0.6875, -0.0970, -0.0932, -0.4691, 1.2459, 0.2501, -0.0874, 0.1953, 0.7064,
                ]),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// Daylight-like multipliers.
const DAYLIGHT: [f32; 4] = [2.1, 1.0, 1.5, 1.0];

fn image(meta: TestMetadata) -> DngImage {
    DngImage::from_raw_metadata(&meta.into()).unwrap()
}

fn close(a: &RenderParams, b: &RenderParams, tolerance: f32) -> bool {
    let near = |x: &[f32], y: &[f32]| {
        x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (p - q).abs() <= tolerance)
    };
    let maps = |x: &Option<crate::HueSatMap>, y: &Option<crate::HueSatMap>| match (x, y) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            (x.hue_divisions, x.sat_divisions, x.val_divisions, x.srgb_encoded)
                == (y.hue_divisions, y.sat_divisions, y.val_divisions, y.srgb_encoded)
                && near(&x.data, &y.data)
        }
        _ => false,
    };
    near(&a.camera_to_prophoto, &b.camera_to_prophoto)
        && near(&a.camera_white, &b.camera_white)
        && a.black_level == b.black_level
        && a.baseline_exposure == b.baseline_exposure
        && a.shadow_scale == b.shadow_scale
        && maps(&a.hue_sat_map, &b.hue_sat_map)
        && maps(&a.look_table, &b.look_table)
        && a.tone_curve == b.tone_curve
}

/// A 16x16 patch of camera RGB `rgb`, as 16-bit samples.
fn patch(rgb: [f64; 3]) -> Vec<u16> {
    let px = rgb.map(|v| (v * 65535.0).round().clamp(0.0, 65535.0) as u16);
    px.iter().copied().cycle().take(16 * 16 * 3).collect()
}

/// The mean sRGB of a rendered patch, 0 to 1.
fn mean(image: &DngImage, pixels: &[u16], white: Option<WhiteXy>, exposure: f64) -> [f64; 3] {
    let srgb = image.render_linear_to_srgb_at(pixels, 16, 16, 3, white, exposure).unwrap();
    let mut sum = [0.0; 3];
    for px in srgb.pixels().as_chunks::<3>().0 {
        for c in 0..3 {
            sum[c] += f64::from(px[c]) / 65535.0;
        }
    }
    sum.map(|s| s / (srgb.pixels().len() / 3) as f64)
}

/// A neutral gray under the shot's light: the camera's as-shot neutral,
/// scaled to `level`.
fn gray(image: &DngImage, level: f64) -> Vec<u16> {
    let white = image.as_shot_white().unwrap().unwrap();
    let params = image.render_params_at(Some(white)).unwrap();
    patch(params.camera_white.map(|c| f64::from(c) * level))
}

/// With no white, both calls give what they gave before there was one.
#[test]
fn no_white_is_the_as_shot_render() {
    for (name, meta) in [("dual", camera(DAYLIGHT)), ("tungsten", camera([1.2, 1.0, 2.6, 1.0]))] {
        let image = image(meta);
        assert_eq!(image.render_params_at(None).unwrap(), image.render_params().unwrap(), "{name}");
        let pixels = gray(&image, 0.2);
        let plain = image.render_linear_to_srgb(&pixels, 16, 16, 3).unwrap();
        let at = image.render_linear_to_srgb_at(&pixels, 16, 16, 3, None, 0.0).unwrap();
        assert_eq!(plain.pixels(), at.pixels(), "{name}");
    }
}

/// The as-shot white, given back, renders as the shot does.
#[test]
fn the_as_shot_white_given_is_the_as_shot_render() {
    for meta in [camera(DAYLIGHT), camera([1.2, 1.0, 2.6, 1.0])] {
        let image = image(meta);
        let white = image.as_shot_white().unwrap().expect("a color profile");
        assert!(white.is_valid(), "{white:?}");
        let at = image.render_params_at(Some(white)).unwrap();
        assert!(close(&at, &image.render_params().unwrap(), 1e-5), "{white:?}");
    }
}

/// A monochrome file has no white to change: none as shot, and the same
/// parameters at any white.
#[test]
fn a_monochrome_file_has_no_white() {
    let meta = TestMetadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        cam_mul: [1.0; 4],
        raw_colors: 1,
        ..Default::default()
    };
    let image = image(meta);
    assert_eq!(image.as_shot_white().unwrap(), None);
    let at = image.render_params_at(Some(WhiteXy { x: 0.4, y: 0.4 })).unwrap();
    assert_eq!(at, image.render_params().unwrap());
}

/// A white that is not valid is refused, and an exposure not finite.
#[test]
fn a_white_the_sdk_does_not_take_is_refused() {
    let image = image(camera(DAYLIGHT));
    let pixels = gray(&image, 0.2);
    for bad in [
        WhiteXy { x: f64::NAN, y: 0.33 },
        WhiteXy { x: 0.33, y: f64::INFINITY },
        WhiteXy { x: 0.0, y: 0.33 },
        WhiteXy { x: 0.33, y: -0.1 },
    ] {
        assert!(image.render_params_at(Some(bad)).is_err(), "{bad:?}");
        assert!(image.render_linear_to_srgb_at(&pixels, 16, 16, 3, Some(bad), 0.0).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY] {
        assert!(image.render_linear_to_srgb_at(&pixels, 16, 16, 3, None, bad).is_err());
    }
}

/// The SDK's temperature and its white undo each other across the range a
/// white balance offers, to a tenth of a percent of the kelvin and a
/// hundredth of a tint step (its isotemperature lines are interpolated, so
/// a strong tint comes back a little off).
#[test]
fn temperature_round_trips() {
    let (mut kelvin_error, mut tint_error) = (0.0f64, 0.0f64);
    let mut kelvin = 2000.0;
    while kelvin <= 50_000.0 {
        for tint in (-150..=150).step_by(10).map(f64::from) {
            let white = Temperature { kelvin, tint }.white();
            assert!(white.is_valid(), "{kelvin} K, {tint}: {white:?}");
            let back = Temperature::of(white);
            kelvin_error = kelvin_error.max((back.kelvin - kelvin).abs() / kelvin);
            tint_error = tint_error.max((back.tint - tint).abs());
        }
        kelvin *= 1.1;
    }
    assert!(kelvin_error < 1e-3 && tint_error < 0.01, "{kelvin_error} of the kelvin, {tint_error}");
}

/// Input that is no white or temperature reads as D50, about 5000 K.
#[test]
fn non_finite_temperature_reads_as_d50() {
    let d50 = WhiteXy { x: 0.3457, y: 0.3585 };
    for bad in [WhiteXy { x: f64::NAN, y: 0.3 }, WhiteXy { x: 0.0, y: 0.0 }] {
        assert_eq!(Temperature::of(bad), Temperature::of(d50), "{bad:?}");
    }
    let t = Temperature::of(d50);
    assert!((t.kelvin - 5000.0).abs() < 10.0, "{t:?}");
    for bad in [
        Temperature { kelvin: f64::NAN, tint: 0.0 },
        Temperature { kelvin: 0.0, tint: 0.0 },
        Temperature { kelvin: -3000.0, tint: 0.0 },
        Temperature { kelvin: 5000.0, tint: f64::INFINITY },
    ] {
        let white = bad.white();
        assert!((white.x - d50.x).abs() < 1e-4 && (white.y - d50.y).abs() < 1e-4, "{bad:?}");
    }
}

/// The SDK's conventions, on a gray patch shot under the as-shot light:
/// rendered at a higher kelvin than the light's, it comes out warmer (red
/// up, blue down); at a lower one, cooler; at a positive tint, magenta
/// (green down); at a negative one, green. The as-shot render is gray, and
/// a stop of exposure brightens it.
#[test]
fn a_render_at_a_white_moves_as_the_sdk_says() {
    let image = image(camera(DAYLIGHT));
    let pixels = gray(&image, 0.1);
    let shot = Temperature::of(image.as_shot_white().unwrap().unwrap());
    let at = |kelvin: f64, tint: f64| {
        mean(&image, &pixels, Some(Temperature { kelvin, tint }.white()), 0.0)
    };
    let [r, g, b] = mean(&image, &pixels, None, 0.0);
    assert!((r - g).abs() < 0.01 && (b - g).abs() < 0.01, "as shot: {r} {g} {b}");

    let [wr, wg, wb] = at(shot.kelvin + 1500.0, shot.tint);
    assert!(wr > r + 0.02 && wb < b - 0.02, "{} K: {wr} {wg} {wb}", shot.kelvin + 1500.0);
    let [cr, _, cb] = at(shot.kelvin - 1500.0, shot.tint);
    assert!(cr < r - 0.02 && cb > b + 0.02, "{} K: {cr} {cb}", shot.kelvin - 1500.0);

    let [mr, mg, mb] = at(shot.kelvin, shot.tint + 30.0);
    assert!(mg < g - 0.01 && mg < mr && mg < mb, "tint +30: {mr} {mg} {mb}");
    let [gr, gg, gb] = at(shot.kelvin, shot.tint - 30.0);
    assert!(gg > g + 0.01 && gg > gr && gg > gb, "tint -30: {gr} {gg} {gb}");

    let [er, eg, eb] = mean(&image, &pixels, None, 1.0);
    assert!(er > r + 0.05 && eg > g + 0.05 && eb > b + 0.05, "+1 stop: {er} {eg} {eb}");
}

const AS_SHOT_NEUTRAL: u16 = 0xC628;

fn as_shot_neutral(file: &Path) -> Option<Vec<(u32, u32)>> {
    let bytes = std::fs::read(file).unwrap();
    let exif = exif::Reader::new().read_raw(bytes).unwrap();
    let field = exif
        .fields()
        .find(|f| f.tag.number() == AS_SHOT_NEUTRAL && f.ifd_num == exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Rational(v) => Some(v.iter().map(|r| (r.num, r.denom)).collect()),
        other => panic!("AsShotNeutral is {other:?}"),
    }
}

/// An export with a white writes it as AsShotNeutral, which reads back as
/// that white; without one, the carried neutral stays.
#[test]
fn an_export_carries_its_white_balance() {
    let dir = tempfile::tempdir().unwrap();
    let source = image(camera(DAYLIGHT));
    let (w, h) = (16, 16);
    let pixels = vec![1000u16; (w * h * 3) as usize];
    let write = |name: &str, white: Option<WhiteXy>| {
        let path = dir.path().join(format!("{name}.dng"));
        let tags = DngTags {
            software: "Test",
            comment: "",
            xmp: "",
            capture_time: "",
            fields: &[],
            white,
            user_crop: None,
            orientation: 1,
        };
        source.write_tagged(&path, &pixels, w, h, 3, &tags).unwrap();
        path
    };

    let plain = write("plain", None);
    let reference = dir.path().join("reference.dng");
    source.write(&reference, &pixels, w, h, 3).unwrap();
    assert_eq!(as_shot_neutral(&plain), as_shot_neutral(&reference));
    assert!(as_shot_neutral(&plain).is_some());
    let shot = source.as_shot_white().unwrap().unwrap();
    let back = DngImage::read_metadata(&plain).unwrap().as_shot_white().unwrap().unwrap();
    assert!((back.x - shot.x).abs() < 1e-4 && (back.y - shot.y).abs() < 1e-4, "{back:?}");

    for (kelvin, tint) in [(3200.0, 0.0), (4300.0, 12.0), (7500.0, -20.0)] {
        let white = Temperature { kelvin, tint }.white();
        let path = write(&format!("{kelvin}"), Some(white));
        assert_ne!(as_shot_neutral(&path), as_shot_neutral(&plain), "{kelvin} K");
        let read = DngImage::read_metadata(&path).unwrap();
        let back = read.as_shot_white().unwrap().unwrap();
        assert!(
            (back.x - white.x).abs() < 1e-4 && (back.y - white.y).abs() < 1e-4,
            "{kelvin} K, {tint}: wrote {white:?}, read {back:?}"
        );
    }

    let bad = DngTags {
        software: "Test",
        comment: "",
        xmp: "",
        capture_time: "",
        fields: &[],
        white: Some(WhiteXy { x: f64::NAN, y: 0.3 }),
        user_crop: None,
        orientation: 1,
    };
    assert!(source.write_tagged(&dir.path().join("bad.dng"), &pixels, w, h, 3, &bad).is_err());
}

/// Real DNGs, `;`-separated in `DEPTHEOS_DNG_WHITE`: the checks above on
/// each, and the time `render_params_at` takes at a white. In release:
///   cargo test -p deptheos_dng --release -- --ignored --nocapture white_real_files
#[test]
#[ignore]
fn white_real_files() {
    let paths = std::env::var("DEPTHEOS_DNG_WHITE").expect("DEPTHEOS_DNG_WHITE");
    for path in paths.split(';').filter(|p| !p.is_empty()) {
        let image = DngImage::read_metadata(Path::new(path)).unwrap();
        let params = image.render_params().unwrap();
        assert_eq!(image.render_params_at(None).unwrap(), params, "{path}");
        let Some(shot) = image.as_shot_white().unwrap() else {
            println!("{path}: no white");
            continue;
        };
        assert!(close(&image.render_params_at(Some(shot)).unwrap(), &params, 1e-5), "{path}");
        let t = Temperature::of(shot);
        let warmer = Temperature { kelvin: t.kelvin + 1000.0, tint: t.tint + 10.0 }.white();
        let calls = 200;
        let start = Instant::now();
        for _ in 0..calls {
            std::hint::black_box(image.render_params_at(Some(warmer)).unwrap());
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(calls);
        println!(
            "{path}: as shot {shot:?} = {:.0} K, tint {:.1}; HueSatMap {}; \
             render_params_at {ms:.3} ms a call",
            t.kelvin,
            t.tint,
            params.hue_sat_map.as_ref().map_or(0, |m| m.data.len() / 3),
        );

        let pixels = patch(params.camera_white.map(|c| f64::from(c) * 0.1));
        let [r, g, b] = mean(&image, &pixels, None, 0.0);
        let [wr, wg, wb] = mean(&image, &pixels, Some(warmer), 0.0);
        println!("{path}: gray {r:.4} {g:.4} {b:.4}; +1000 K, +10: {wr:.4} {wg:.4} {wb:.4}");
    }
}

/// The time `render_params_at` takes at a white on the dual-illuminant
/// test camera. In release:
///   cargo test -p deptheos_dng --release -- --ignored --nocapture white_timing
#[test]
#[ignore]
fn white_timing() {
    let image = image(camera(DAYLIGHT));
    let white = Temperature { kelvin: 4500.0, tint: 5.0 }.white();
    let calls = 200;
    let start = Instant::now();
    for _ in 0..calls {
        std::hint::black_box(image.render_params_at(Some(white)).unwrap());
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(calls);
    println!("render_params_at: {ms:.3} ms a call");
}
