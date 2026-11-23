//! `io::decode_linear` decodes every kind of DNG to linear camera RGB with
//! the file's own color science.

use deptheos_core::io::{self, LinearColor};
use deptheos_dng::test_dng::{Layout, TestDng};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

/// The whole frame as the active area and the crop.
const WHOLE: ([u32; 4], [u32; 4]) = ([0, 0, HEIGHT, WIDTH], [0, 0, WIDTH, HEIGHT]);

/// An active area inside masked borders (top, left, bottom, right) and a
/// DefaultCrop inside it (x, y, w, h), as cameras write them. Offsets are
/// even, so the mosaic's phase is the same in each.
const CROPPED: ([u32; 4], [u32; 4]) = ([2, 4, HEIGHT - 2, WIDTH - 2], [6, 4, 40, 30]);

fn write(
    cfa: bool,
    (active, crop): ([u32; 4], [u32; 4]),
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dng = TestDng {
        width: WIDTH,
        height: HEIGHT,
        cfa,
        big_endian: false,
        layout: Layout::Strips { rows: 16 },
        black: [0; 3],
        white: [65535; 3],
        active,
        crop,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.dng");
    dng.write(&path, &dng.pixels());
    (dir, path)
}

/// A DNG decodes to its DefaultCrop's size, the size its header says,
/// linear or mosaic (LibRaw demosaics a mosaic, cropped as the SDK would).
fn check(cfa: bool, area: ([u32; 4], [u32; 4])) {
    let (_dir, path) = write(cfa, area);
    // The SDK reads a mosaic as metadata only, no channels.
    let sdk = deptheos_dng::DngImage::read_with_pixels(
        &path,
        io::FileCache::Use,
        deptheos_dng::Budget::unlimited(),
    )
    .unwrap();
    match sdk {
        deptheos_dng::DngRead::Mosaic(dng) => assert!(cfa && dng.channels() == 0),
        deptheos_dng::DngRead::Linear(dng, _) => assert!(!cfa && dng.channels() == 3),
    }
    let linear = io::decode_linear(&path).unwrap();
    let image = linear.image.as_rgb16().expect("16-bit RGB");
    let [_, _, w, h] = area.1;
    assert_eq!(image.dimensions(), (w, h), "the DefaultCrop");
    assert!(matches!(linear.color, LinearColor::Dng(_)), "the SDK's color science");
    // Its header says the size and depth it decodes to, without decoding.
    let header = io::Header {
        width: image.width(),
        height: image.height(),
        depth: deptheos_core::FrameDepth::Sixteen,
    };
    assert_eq!(io::header_info(&path, io::CameraRaw::LibRaw), Some(header));
    // load_frame takes the same path.
    let frame =
        io::load_frame(&path, io::FileCache::Use, io::CameraRaw::LibRaw, io::Target::Own).unwrap();
    assert_eq!(frame.image.as_rgb16().map(|i| i.dimensions()), Some(image.dimensions()));
}

#[test]
fn linear_dng() {
    check(false, WHOLE);
}

#[test]
fn linear_dng_cropped() {
    check(false, CROPPED);
}

#[test]
fn cfa_dng() {
    check(true, WHOLE);
}

/// LibRaw decodes the active area; the DNG's DefaultCrop inside it is
/// applied too, so a mosaic comes out at the size its header says.
#[test]
fn cfa_dng_cropped() {
    check(true, CROPPED);
}

/// A camera RAW file's header, read without unpacking, says the size it
/// decodes to, so a stack is sized before any decode; other makes are
/// checked by `real_raws_decode_to_their_header`.
#[test]
fn raw_header_matches_its_decode() {
    for name in ["mosaic.dng", "converter.dng"] {
        let path = crate::known::fixture(name);
        let header = io::header_info(&path, io::CameraRaw::LibRaw);
        let frame =
            io::load_frame(&path, io::FileCache::Use, io::CameraRaw::LibRaw, io::Target::Own)
                .unwrap();
        let image = frame.image.as_rgb16().expect("16-bit RGB");
        let decoded = io::Header {
            width: image.width(),
            height: image.height(),
            depth: deptheos_core::FrameDepth::Sixteen,
        };
        assert_eq!(header, Some(decoded), "{name}");
    }
}

/// Real camera RAW and DNG files, `;`-separated in `DEPTHEOS_RAW_DECODE`
/// (too large for the repository): each decodes to the size its header
/// says (a mosaic DNG through LibRaw, cropped to its DefaultCrop, ADR-131).
#[test]
#[ignore = "real files: give them in DEPTHEOS_RAW_DECODE"]
fn real_raws_decode_to_their_header() {
    let paths = std::env::var("DEPTHEOS_RAW_DECODE").expect("DEPTHEOS_RAW_DECODE");
    for path in paths.split(';').filter(|p| !p.is_empty()) {
        let path = std::path::Path::new(path);
        let header = io::header_info(path, io::CameraRaw::LibRaw).expect("header");
        let image = io::decode_linear(path).unwrap().image;
        eprintln!(
            "{}: header {header:?}, decoded {:?}",
            path.display(),
            (image.width(), image.height())
        );
        assert_eq!((image.width(), image.height()), (header.width, header.height));
    }
}

/// A DNG the SDK can't read is an error with the SDK's verdict, not read
/// again by LibRaw: the frame is left out with that reason.
#[test]
fn a_dng_the_sdk_cannot_read_is_not_read_by_libraw() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.dng");
    // A TIFF header and nothing after it.
    std::fs::write(&path, b"II*\0\x08\0\0\0\0\0").unwrap();
    match io::decode_linear(&path) {
        Err(deptheos_core::CoreError::Dng(deptheos_dng::DngError::ReadFailed {
            cause: deptheos_dng::Cause::Damaged,
            ..
        })) => {}
        Err(e) => panic!("expected the SDK's error, got {e:?}"),
        Ok(_) => panic!("a broken DNG decoded"),
    }
}

/// Opaque standard frames that need no color work skip every input-stage
/// step but orientation (ADR-132): their pixels, and so their alignment
/// (ADR-112), are pinned by a hash of what `load_frame` hands over, for an
/// 8-bit sRGB JPEG, PNG and TIFF, one turned by EXIF, a 16-bit one and an
/// untagged gray one.
#[test]
fn opaque_srgb_frames_load_bit_identically() {
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Luma, Rgb};
    let dir = tempfile::tempdir().unwrap();
    let (w, h) = (97u32, 61u32);
    let rgb8 = ImageBuffer::<Rgb<u8>, _>::from_fn(w, h, |x, y| {
        Rgb([(x * 7 + y * 3) as u8, (x ^ y) as u8, (y * 5 + x) as u8])
    });
    let rgb16 = ImageBuffer::<Rgb<u16>, _>::from_fn(w, h, |x, y| {
        Rgb([(x * 677 + y) as u16, (y * 1031) as u16, ((x * y * 37) % 65536) as u16])
    });
    let gray8 = ImageBuffer::<Luma<u8>, _>::from_fn(w, h, |x, y| Luma([(x * 3 + y * 5) as u8]));
    // Little-endian TIFF: IFD0 with one SHORT entry, Orientation = 6.
    let mut turned = b"II*\0\x08\0\0\0\x01\0".to_vec();
    turned.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0]);

    let write = |name: &str, bytes: Vec<u8>| {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .write_image(rgb8.as_raw(), w, h, ExtendedColorType::Rgb8)
        .unwrap();
    let mut jpeg_turned = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_turned, 90);
    encoder.set_exif_metadata(turned).unwrap();
    encoder.write_image(rgb8.as_raw(), w, h, ExtendedColorType::Rgb8).unwrap();
    let png = |bytes: &[u8], color| {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out).write_image(bytes, w, h, color).unwrap();
        out
    };
    let mut tiff = std::io::Cursor::new(Vec::new());
    image::codecs::tiff::TiffEncoder::new(&mut tiff)
        .write_image(rgb8.as_raw(), w, h, ExtendedColorType::Rgb8)
        .unwrap();
    let rgb16_bytes: Vec<u8> = rgb16.as_raw().iter().flat_map(|v| v.to_ne_bytes()).collect();

    let fixtures = [
        (write("rgb8.jpg", jpeg), 0x6c59_3651_d9c7_74e7u64),
        (write("turned.jpg", jpeg_turned), 0x3a63_8e67_70ef_040f),
        (write("rgb8.png", png(rgb8.as_raw(), ExtendedColorType::Rgb8)), 0x2942_f034_4677_c62a),
        (write("rgb8.tif", tiff.into_inner()), 0x2942_f034_4677_c62a),
        (write("rgb16.png", png(&rgb16_bytes, ExtendedColorType::Rgb16)), 0x2d0d_b537_cfcf_9f34),
        (write("gray8.png", png(gray8.as_raw(), ExtendedColorType::L8)), 0x3f3c_52d7_126c_e302),
    ];
    let mut got = Vec::new();
    for (path, _) in &fixtures {
        let frame =
            io::load_frame(path, io::FileCache::Use, io::CameraRaw::LibRaw, io::Target::Own)
                .unwrap();
        assert_eq!(frame.profile, None, "{}: sRGB", path.display());
        assert!(frame.notes.is_empty(), "{}: {:?}", path.display(), frame.notes);
        let image = &frame.image;
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let header = [image.width(), image.height(), u32::from(image.color().bytes_per_pixel())];
        for byte in
            header.iter().flat_map(|v| v.to_le_bytes()).chain(image.as_bytes().iter().copied())
        {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        got.push(hash);
    }
    let pinned: Vec<u64> = fixtures.iter().map(|(_, hash)| *hash).collect();
    assert_eq!(got, pinned, "{got:#x?}");
}

/// A decode in canceled work stops in its codec and fails as canceled
/// (ADR-143): the DNG SDK through its abort sniffer, LibRaw through its
/// progress handler. Uncanceled, the same files decode, and a LibRaw
/// interrupt is harmless once its decode has ended.
#[test]
fn a_canceled_decode_stops_in_its_codec() {
    use deptheos_core::CoreError;
    use deptheos_core::cancel::{Cancel, within};
    let canceled = Cancel::root();
    canceled.cancel();
    for cfa in [false, true] {
        let (_dir, path) = write(cfa, WHOLE);
        let read = within(&canceled, 2, || io::decode_linear(&path));
        assert!(matches!(read, Err(CoreError::Canceled)), "cfa {cfa}: {:?}", read.err());
        within(&Cancel::root(), 2, || io::decode_linear(&path)).unwrap();
    }
    // LibRaw alone, its flag set: its own check stops it.
    let (_dir, path) = write(true, WHOLE);
    let flag = std::sync::atomic::AtomicBool::new(true);
    let budget = deptheos_raw::Budget { cancel: &flag, threads: 1, watch: None };
    let read = deptheos_raw::read(&path, budget);
    assert!(matches!(read, Err(deptheos_raw::RawError::Canceled)), "{:?}", read.err());
    let kept = std::sync::Mutex::new(None);
    let keep = |interrupt: deptheos_raw::Interrupt| -> Box<dyn std::any::Any> {
        *kept.lock().unwrap() = Some(interrupt);
        Box::new(())
    };
    flag.store(false, std::sync::atomic::Ordering::Relaxed);
    let budget = deptheos_raw::Budget { cancel: &flag, threads: 1, watch: Some(&keep) };
    deptheos_raw::read(&path, budget).unwrap();
    kept.lock().unwrap().take().expect("an interrupt was handed out").interrupt();
}

/// Alpha is flattened on decode: a PNG or TIFF with partial alpha, 8 or
/// 16 bits, loads as RGB composited over white and noted; an opaque one
/// loads as its color alone.
#[test]
fn alpha_is_flattened_on_decode() {
    use image::{ImageBuffer, Rgba};
    let dir = tempfile::tempdir().unwrap();
    // (0, 100, 255) at alpha 128 of 255, over white.
    let over_white = [127.0, 177.2, 255.0];
    let check = |name: &str, want: [f32; 3], noted: bool| {
        let path = dir.path().join(name);
        let frame =
            io::load_frame(&path, io::FileCache::Use, io::CameraRaw::LibRaw, io::Target::Own)
                .unwrap();
        assert!(!frame.image.color().has_alpha(), "{name}: an alpha channel is left");
        let pixel = frame.image.to_rgb16().get_pixel(1, 1).0;
        for (got, want) in pixel.iter().zip(want) {
            let got = f32::from(*got) / 257.0;
            assert!((got - want).abs() <= 1.0, "{name}: {pixel:?}, want {want}");
        }
        assert_eq!(frame.notes.contains(&io::LoadNote::OverWhite), noted, "{name}");
    };
    let partial = ImageBuffer::from_pixel(4, 3, Rgba([0u8, 100, 255, 128]));
    let opaque = ImageBuffer::from_pixel(4, 3, Rgba([0u8, 100, 255, 255]));
    let sixteen = ImageBuffer::from_pixel(4, 3, Rgba([0u16, 100 * 257, 65535, 128 * 257]));
    for name in ["eight.png", "eight.tif"] {
        partial.save(dir.path().join(name)).unwrap();
        check(name, over_white, true);
    }
    for name in ["sixteen.png", "sixteen.tif"] {
        sixteen.save(dir.path().join(name)).unwrap();
        check(name, over_white, true);
    }
    opaque.save(dir.path().join("opaque.png")).unwrap();
    check("opaque.png", [0.0, 100.0, 255.0], false);
}
