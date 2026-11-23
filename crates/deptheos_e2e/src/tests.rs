//! The checker's own checks: what it reads, and what it says.

use super::*;
use crate::metadata::FrameTags;
use crate::space::{ADOBE_RGB, SRGB, Space, icc_primaries, same_primaries};
use crate::tags::tag_differences;

/// A matrix profile's primaries are read from its colorants: the engine's
/// Adobe RGB profile is Adobe RGB, not sRGB.
#[test]
fn a_profile_says_its_primaries() {
    let adobe = deptheos_core::io::IccProfile::adobe_rgb_compatible();
    let primaries = icc_primaries(adobe.bytes()).expect("a matrix profile");
    assert!(same_primaries(&primaries, &ADOBE_RGB), "{primaries:?}");
    assert!(!same_primaries(&primaries, &SRGB));
    assert_eq!(icc_primaries(b"not a profile"), None);
    // A gray profile's frames stack in sRGB's primaries.
    let mut gray = vec![0u8; 132];
    gray[16..20].copy_from_slice(b"GRAY");
    assert_eq!(icc_primaries(&gray), Some(SRGB));
}

fn frames_in(space: Space) -> Frames {
    Frames {
        width: 4,
        height: 4,
        k: 1,
        mean: Small { w: 1, h: 1, px: vec![[0.5; 3]] },
        tiles: Vec::new(),
        cols: 0,
        rows: 0,
        tags: FrameTags { kept: Vec::new(), private: Vec::new() },
        colors: Vec::new(),
        space,
    }
}

/// The export of frames in Adobe RGB passes only with their profile; an
/// untagged one is read as sRGB, another space.
#[test]
fn an_export_is_in_its_frames_space() {
    use image::ImageEncoder as _;
    let dir = tempfile::tempdir().unwrap();
    let adobe = deptheos_core::io::IccProfile::adobe_rgb_compatible();
    let write = |name: &str, icc: Option<&[u8]>| {
        let path = dir.path().join(name);
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        if let Some(icc) = icc {
            encoder.set_icc_profile(icc.to_vec()).unwrap();
        }
        encoder.write_image(&[128; 3 * 4 * 4], 4, 4, image::ExtendedColorType::Rgb8).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let tagged = write("tagged.png", Some(adobe.bytes()));
    let untagged = write("untagged.png", None);
    let check = |file: &Path, frames: &Frames| {
        let mut report = Report::new("space");
        space_check(&mut report, file, ExportFormat::Png, frames);
        report.passed()
    };
    let in_adobe = frames_in(Space::of_frame(&tagged).unwrap());
    let in_srgb = frames_in(Space::of_frame(&untagged).unwrap());
    assert!(check(&tagged, &in_adobe));
    assert!(!check(&untagged, &in_adobe));
    assert!(check(&untagged, &in_srgb));
    assert!(!check(&tagged, &in_srgb));
}

/// A DNG is read through the engine: 16-bit sRGB from the SDK, with the
/// color science the color tags check compares.
#[test]
fn a_dng_opens_through_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let dng = deptheos_dng::test_dng::TestDng {
        width: 64,
        height: 48,
        cfa: false,
        big_endian: false,
        layout: deptheos_dng::test_dng::Layout::Strips { rows: 16 },
        black: [0; 3],
        white: [65535; 3],
        active: [0, 0, 48, 64],
        crop: [0, 0, 64, 48],
    };
    let path = dir.path().join("frame.dng");
    dng.write(&path, &dng.pixels());
    let read = read(&path, 1).unwrap();
    assert_eq!((read.width, read.height, read.bits), (64, 48, 16));
    let color = read.color.expect("its color science");
    assert!(tag_differences(&color, &color).is_empty());
    let mut brighter = color.clone();
    brighter.baseline_exposure += 0.5;
    let said = tag_differences(&brighter, &color);
    assert!(said.len() == 1 && said[0].starts_with("BaselineExposure"), "{said:?}");
    // A set of DNG frames is a set.
    let set = Set::open(dir.path()).unwrap();
    assert_eq!(set.frames, vec![path]);
}

/// With no reference the check fails and writes none; after `bless` it
/// passes.
#[test]
fn no_reference_yet_fails_and_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let reference = dir.path().join("case/desktop-jpeg-8.png");
    let small = Small { w: 2, h: 2, px: vec![[0.25; 3]; 4] };
    let mut report = Report::new("reference");
    compare_reference(&mut report, &small, &reference, false);
    assert!(!report.passed());
    assert!(report.checks[0].detail.starts_with("not signed off yet"), "{report}");
    assert!(!reference.exists());
    let mut report = Report::new("reference");
    compare_reference(&mut report, &small, &reference, true);
    assert!(reference.exists());
    let mut report = Report::new("reference");
    compare_reference(&mut report, &small, &reference, false);
    assert!(report.passed(), "{report}");
}

/// A picture that failed a check isn't signed off, however `bless` asks.
#[test]
fn a_failed_picture_is_not_signed_off() {
    let dir = tempfile::tempdir().unwrap();
    let reference = dir.path().join("case/desktop-jpeg-8.png");
    let small = Small { w: 2, h: 2, px: vec![[0.25; 3]; 4] };
    let mut report = Report::new("reference");
    report.check("bits", false, "8 per channel, 16 asked for");
    compare_reference(&mut report, &small, &reference, true);
    assert!(!reference.exists());
    let last = report.checks.last().unwrap();
    assert!(!last.ok && last.name == "reference", "{report}");
}

/// A little-endian TIFF stream of one IFD, values over four bytes after it.
fn tiff(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"II*\0\x08\0\0\0".to_vec();
    let mut data: Vec<u8> = Vec::new();
    let data_at = 8 + 2 + 12 * entries.len() + 4;
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in entries {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if value.len() <= 4 {
            let mut v = value.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            out.extend(((data_at + data.len()) as u32).to_le_bytes());
            data.extend(value);
        }
    }
    out.extend([0; 4]);
    out.extend(data);
    out
}

fn ascii(tag: u16, text: &str) -> (u16, u16, u32, Vec<u8>) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    (tag, 2, bytes.len() as u32, bytes)
}

/// An ISO BMFF box.
fn bmff(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(payload);
    b
}

/// A CR3's tags are read: IFD0 from `CMT1`, the Exif IFD from `CMT2`.
#[test]
fn a_cr3s_tags_are_read_through_the_engine() {
    let cmt1 = tiff(&[ascii(0x0110, "Canon EOS R5m2")]);
    let exposure = [1u32.to_le_bytes(), 125u32.to_le_bytes()].concat();
    let cmt2 = tiff(&[(0x829A, 5, 1, exposure), ascii(0xA431, "BODY0001")]);
    let mut uuid = vec![
        0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a,
        0x48,
    ];
    uuid.extend(bmff(b"CMT1", &cmt1));
    uuid.extend(bmff(b"CMT2", &cmt2));
    let mut file = bmff(b"ftyp", b"crx \0\0\0\x01crx isom");
    file.extend(bmff(b"moov", &bmff(b"uuid", &uuid)));
    file.extend(bmff(b"mdat", &[0; 64]));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.CR3");
    std::fs::write(&path, file).unwrap();
    let tags = FrameTags::read(&[path]);
    let kept: Vec<_> = tags.kept.iter().map(|(t, v)| (*t, v.join(""))).collect();
    assert_eq!(
        kept,
        [
            (exif::Tag::Model, "\"Canon EOS R5m2\"".into()),
            (exif::Tag::ExposureTime, "1/125".into())
        ]
    );
    assert_eq!(tags.private, ["BodySerialNumber"]);
}

/// Frames whose tags read as none, blank ones included, fail the metadata
/// check.
#[test]
fn frames_without_tags_fail_the_metadata_check() {
    let dir = tempfile::tempdir().unwrap();
    let export = dir.path().join("export.tif");
    std::fs::write(&export, tiff(&[ascii(0x0131, "Deptheos 0.2.0")])).unwrap();
    let frame = dir.path().join("frame.tif");
    std::fs::write(&frame, tiff(&[ascii(0x8298, "")])).unwrap();
    let mut frames = frames_in(Space::Untagged);
    frames.tags = FrameTags::read(&[frame]);
    assert!(frames.tags.kept.is_empty());
    let mut report = Report::new("metadata");
    metadata_checks(&mut report, &export, &frames);
    assert!(!report.passed(), "{report}");
}

/// A set is its pictures and RAW frames, any extension case; nothing else.
#[test]
fn a_set_is_its_frames() {
    let dir = tempfile::tempdir().unwrap();
    let names =
        ["a.TIF", "b.tiff", "c.JPEG", "d.jpg", "e.PNG", "f.dng", "g.nef", "h.txt", "i.webp"];
    for name in names {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    let set = Set::open(dir.path()).unwrap();
    let found: Vec<String> =
        set.frames.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(found, names[..7]);
}

/// Frames are measured at about 2048 pixels a side.
#[test]
fn frames_are_measured_at_about_2048_a_side() {
    use crate::frames::scale_for;
    assert_eq!(scale_for(8192, 5464), 4);
    assert_eq!(scale_for(5184, 3888), 3);
    assert_eq!(scale_for(4032, 3024), 2);
    assert_eq!(scale_for(2048, 1366), 1);
    assert_eq!(scale_for(1024, 683), 1);
}

/// Any frame changed, added or removed after the manifest fails the set.
#[test]
fn a_set_is_held_to_its_manifest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.jpg"), b"one").unwrap();
    std::fs::write(dir.path().join("b.jpg"), b"two").unwrap();
    let error = Set::open_checked(dir.path()).err().unwrap();
    assert!(error.contains("e2e-check manifest"), "{error}");
    Set::open(dir.path()).unwrap().write_manifest().unwrap();
    assert!(Set::open(dir.path()).unwrap().write_manifest().is_err(), "never replaced");
    Set::open_checked(dir.path()).unwrap();
    std::fs::write(dir.path().join("b.jpg"), b"TWO").unwrap();
    let error = Set::open_checked(dir.path()).err().unwrap();
    assert!(error.contains("b.jpg"), "{error}");
    std::fs::write(dir.path().join("b.jpg"), b"two").unwrap();
    std::fs::write(dir.path().join("c.jpg"), b"three").unwrap();
    let error = Set::open_checked(dir.path()).err().unwrap();
    assert!(error.contains("c.jpg"), "{error}");
}
