//! Decoding a frame from disk.

use image::math::Rect;
use image::{DynamicImage, ImageBuffer, Rgb};
use std::path::Path;
use std::sync::{Arc, RwLock};

use super::FileCache;
use super::decoders::{self, OpenFrame};
use super::input::{self, DecodedHeader, LoadNote, Target};
use super::profile::IccProfile;
use crate::error::{CoreError, Result};
use crate::upload::FrameDepth;

/// A decoder the host application supplies (iOS: Apple's frameworks), for
/// standard images, and for camera RAW and mosaic DNGs under
/// [`CameraRaw::Host`], rendered to Display P3. A linear DNG always goes
/// through the DNG SDK. Called from several prefetch threads at once.
pub trait HostDecoder: Send + Sync {
    /// The frame at `path` opened, as stored, for the input stage
    /// ([`input::input_frame`]).
    fn open(&self, path: &Path) -> Result<Box<dyn OpenFrame>>;
}

static HOST_DECODER: RwLock<Option<Arc<dyn HostDecoder>>> = RwLock::new(None);

/// Install the process's [`HostDecoder`], replacing any before.
pub fn set_host_decoder(decoder: Arc<dyn HostDecoder>) {
    *HOST_DECODER.write().unwrap_or_else(|e| e.into_inner()) = Some(decoder);
}

fn host_decoder() -> Option<Arc<dyn HostDecoder>> {
    HOST_DECODER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Who decodes a stack's camera RAW and mosaic DNGs, which decides whether
/// they come out linear or rendered (ADR-131).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraRaw {
    /// LibRaw: linear camera RGB.
    LibRaw,
    /// The [`HostDecoder`]: 16-bit Display P3, a standard image. LibRaw
    /// where none is installed.
    Host,
}

impl CameraRaw {
    /// The decoder for files outside any stack (a thumbnail, a file
    /// dropped on the CLI): the host's where it installed one, else LibRaw.
    pub fn here() -> Self {
        if host_decoder().is_some() { Self::Host } else { Self::LibRaw }
    }
}

/// Extensions of the standard formats the desktop's decoders read
/// (lower-case).
const STANDARD_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff"];

/// Every extension the loader accepts, lower-case; the single source for
/// file dialogs and argv filters. HEIC too with a [`HostDecoder`].
pub fn supported_extensions() -> Vec<&'static str> {
    if host_decoder().is_some() {
        let host: &[&str] = &["heic", "heif"];
        return STANDARD_EXTENSIONS
            .iter()
            .chain(host)
            .chain(deptheos_raw::RAW_EXTENSIONS)
            .copied()
            .collect();
    }
    STANDARD_EXTENSIONS.iter().chain(deptheos_raw::RAW_EXTENSIONS).copied().collect()
}

/// Which decoder reads a frame: the one place that decides, so loading a
/// frame, reading its header and telling its kind agree.
enum Decoder {
    /// Linear camera RGB, with the file's own color science.
    DngSdk,
    /// Standard images, and camera RAW under [`CameraRaw::Host`], rendered.
    Host(Arc<dyn HostDecoder>),
    /// Linear camera RGB.
    LibRaw,
    /// The desktop's decoders ([`decoders::open`]).
    Image,
}

fn decoder_for(path: &Path, raw: CameraRaw) -> Decoder {
    if raw == CameraRaw::Host
        && let Some(host) = host_decoder()
        && host_may_decode(path)
    {
        return Decoder::Host(host);
    }
    // A mosaic DNG the host didn't take above goes through the SDK too.
    if is_dng(path) {
        Decoder::DngSdk
    } else if is_raw_file(path) {
        Decoder::LibRaw
    } else {
        host_decoder().map_or(Decoder::Image, Decoder::Host)
    }
}

/// Whether [`CameraRaw`] decides how `path` decodes: camera RAW and mosaic
/// DNGs (ADR-131). Reads a DNG's header, once.
pub fn host_may_decode(path: &Path) -> bool {
    if is_dng(path) { is_mosaic_dng(path) } else { is_raw_file(path) }
}

/// Whether this path decodes to linear camera RGB (a RAW stack) under
/// `raw`, rather than as a standard image.
pub fn is_raw_source(path: &Path, raw: CameraRaw) -> bool {
    matches!(decoder_for(path, raw), Decoder::DngSdk | Decoder::LibRaw)
}

/// Whether this path is a camera RAW or DNG file by its name, whoever
/// decodes it (a heavy decode).
pub fn is_raw_file(path: &Path) -> bool {
    deptheos_raw::is_raw_path(path)
}

/// The pixels of a frame on its own ([`load_frame`] into its own space),
/// read through the OS file cache.
pub fn load_image(path: &Path, raw: CameraRaw) -> Result<DynamicImage> {
    Ok(load_frame(path, FileCache::Use, raw, Target::Own)?.image)
}

/// A frame as loaded: the pixels (8- or 16-bit RGB), the color space they
/// are in, and what the load had to assume or could not keep.
pub struct Frame {
    pub image: DynamicImage,
    /// A standard image's color space ([`Target`]); `None` for sRGB and
    /// for linear camera RGB.
    pub profile: Option<IccProfile>,
    /// Where the frame has data, upright, when its alpha marks an empty
    /// border (filled from its edge); `None` for the whole frame.
    pub data_area: Option<Rect>,
    /// Logged as the frame loads; the caller may show them too.
    pub notes: Vec<LoadNote>,
}

/// Load a frame: RAW and DNG as 16-bit linear camera RGB (unless `raw`
/// gives them to the host), every other through the input stage into
/// `into`'s space ([`input::input_frame`]). `cache` applies to DNG only.
pub fn load_frame(
    path: &Path,
    cache: FileCache,
    raw: CameraRaw,
    into: Target<'_>,
) -> Result<Frame> {
    let _t = deptheos_gpu::profile::cpu_scope("decode (worker)");
    let Some(open) = open_frame(path, raw)? else {
        let image = decode_linear_with(path, cache)?.image;
        return Ok(Frame { image, profile: None, data_area: None, notes: Vec::new() });
    };
    let frame = input::input_frame(path, open.decode()?, into)?;
    for note in &frame.notes {
        tracing::warn!("{}: {note:?}", path.display());
    }
    Ok(frame)
}

/// A standard image opened by its decoder (the host's, else the desktop's
/// own); `None` for a frame that decodes to linear camera RGB.
fn open_frame(path: &Path, raw: CameraRaw) -> Result<Option<Box<dyn OpenFrame>>> {
    match decoder_for(path, raw) {
        Decoder::Host(host) => host.open(path).map(Some),
        Decoder::Image => decoders::open(path).map(Some),
        Decoder::DngSdk | Decoder::LibRaw => Ok(None),
    }
}

/// A standard image's header as stored, for planning a stack
/// ([`input::StackSpace::of`], [`input::input_header`]); `None` for linear
/// camera RGB. [`CoreError::Refused`] when it can't be stacked.
pub fn standard_header(path: &Path, raw: CameraRaw) -> Result<Option<DecodedHeader>> {
    Ok(open_frame(path, raw)?.map(|open| open.header().clone()))
}

/// A camera RAW or DNG frame as linear camera RGB, with the metadata that
/// renders it.
pub struct LinearImage {
    /// 16-bit linear camera RGB.
    pub image: DynamicImage,
    pub color: LinearColor,
}

/// Where a [`LinearImage`]'s color science comes from.
pub enum LinearColor {
    /// The DNG's own, parsed by the SDK. A metadata-only handle.
    Dng(deptheos_dng::DngImage),
    /// LibRaw's, for camera RAW.
    Raw(deptheos_raw::RawMetadata),
}

/// Decode a camera RAW or DNG file to linear camera RGB:
///
/// - A DNG through the DNG SDK, with the file's own color science.
/// - A CFA (mosaic) DNG demosaiced by LibRaw, cropped to its DefaultCrop,
///   with the SDK's color science (ADR-131).
/// - A DNG the SDK can't read is an error, not a LibRaw fallback, which
///   would differ in color and crop from the stack's other frames.
/// - All other camera RAW through LibRaw.
pub fn decode_linear(path: &Path) -> Result<LinearImage> {
    decode_linear_with(path, FileCache::Use)
}

/// [`decode_linear`], reading a DNG as `cache` says.
fn decode_linear_with(path: &Path, cache: FileCache) -> Result<LinearImage> {
    if is_dng(path) {
        return decode_dng(path, cache)
            .inspect_err(|e| tracing::warn!("DNG SDK could not read {}: {e}", path.display()));
    }
    let raw = crate::cancel::codec(|budget| deptheos_raw::read(path, budget))?;
    let image = rgb16_image(path, raw.width, raw.height, raw.pixels)?;
    Ok(LinearImage { image, color: LinearColor::Raw(raw.metadata) })
}

/// A DNG through the SDK, with a CFA one demosaiced by LibRaw (ADR-131).
fn decode_dng(path: &Path, cache: FileCache) -> Result<LinearImage> {
    use deptheos_dng::{DngImage, DngRead};
    let read = crate::cancel::codec(|budget| DngImage::read_with_pixels(path, cache, budget))?;
    let (dng, image) = match read {
        DngRead::Linear(dng, pixels) => {
            if dng.channels() != 3 {
                let channels = dng.channels();
                return Err(CoreError::DngChannels { path: path.to_path_buf(), channels });
            }
            let image = rgb16_image(path, dng.width(), dng.height(), pixels)?;
            (dng, image)
        }
        DngRead::Mosaic(dng) => {
            tracing::debug!("{}: CFA DNG, demosaicing with LibRaw", path.display());
            let raw = crate::cancel::codec(|budget| deptheos_raw::read(path, budget))?;
            (dng, rgb16_image(path, raw.width, raw.height, raw.pixels)?)
        }
    };
    Ok(LinearImage { image, color: LinearColor::Dng(dng) })
}

/// What a frame's header says of the frame [`load_frame`] decodes from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Upright, as it decodes.
    pub width: u32,
    pub height: u32,
    /// RGB at 8 or 16 bits, as [`load_frame`] lays it out.
    pub depth: FrameDepth,
}

/// A frame's [`Header`] ([`Target::Own`]) without decoding it; `None` when
/// it can't be read or stacked.
pub fn header_info(path: &Path, raw: CameraRaw) -> Option<Header> {
    match decoder_for(path, raw) {
        Decoder::DngSdk => dng_header(path),
        Decoder::LibRaw => {
            let (width, height) = deptheos_raw::read_size(path).ok()?;
            Some(Header { width, height, depth: FrameDepth::Sixteen })
        }
        Decoder::Host(_) | Decoder::Image => {
            let header = standard_header(path, raw).ok()??;
            input::input_header(path, &header, Target::Own).ok()
        }
    }
}

/// A DNG's upright default-crop size from its metadata alone; `None` when
/// it can't be parsed (its decode says why).
fn dng_header(path: &Path) -> Option<Header> {
    let dng = deptheos_dng::DngImage::read_metadata(path).ok()?;
    let (width, height) = (dng.width(), dng.height());
    (width > 0 && height > 0).then_some(Header { width, height, depth: FrameDepth::Sixteen })
}

/// Whether this path names a DNG.
pub fn is_dng(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dng"))
}

/// Whether `path` is a mosaic (CFA) DNG ([`deptheos_dng::is_mosaic`]),
/// cached by length and modification time since the apps ask while they
/// draw. An unreadable file counts as not and isn't cached. The lock is
/// never held during the read, so a slow share holds up no other file.
pub fn is_mosaic_dng(path: &Path) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard};
    use std::time::SystemTime;
    type Known = HashMap<std::path::PathBuf, (u64, Option<SystemTime>, bool)>;
    static KNOWN: Mutex<Option<Known>> = Mutex::new(None);
    fn known() -> MutexGuard<'static, Option<Known>> {
        KNOWN.lock().unwrap_or_else(|e| e.into_inner())
    }
    if !is_dng(path) {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else { return false };
    let stamp = (meta.len(), meta.modified().ok());
    if let Some(&(len, modified, mosaic)) = known().get_or_insert_with(HashMap::new).get(path)
        && (len, modified) == stamp
    {
        return mosaic;
    }
    match deptheos_dng::is_mosaic(path) {
        Ok(mosaic) => {
            let mut known = known();
            known
                .get_or_insert_with(HashMap::new)
                .insert(path.to_path_buf(), (stamp.0, stamp.1, mosaic));
            mosaic
        }
        Err(e) => {
            tracing::debug!("{}: can't tell mosaic from linear: {e}", path.display());
            false
        }
    }
}

/// A decoder's interleaved RGB u16 pixels as an image.
fn rgb16_image(path: &Path, width: u32, height: u32, pixels: Vec<u16>) -> Result<DynamicImage> {
    ImageBuffer::<Rgb<u16>, Vec<u16>>::from_raw(width, height, pixels)
        .map(DynamicImage::ImageRgb16)
        .ok_or_else(|| {
            CoreError::InvalidState(format!("{}: pixel buffer size mismatch", path.display()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame's embedded profile comes back with it from every standard
    /// format; an sRGB one comes back as sRGB (`None`), and the pixels are
    /// as stored either way.
    #[test]
    fn standard_frames_load_with_their_profile() {
        use image::ImageEncoder;
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (128, 96);
        let pixels: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
        let encode = |path: &Path, icc: Vec<u8>| {
            let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
            let rgb = image::ExtendedColorType::Rgb8;
            match path.extension().and_then(|e| e.to_str()).unwrap() {
                "png" => {
                    let mut e = image::codecs::png::PngEncoder::new(file);
                    e.set_icc_profile(icc).unwrap();
                    e.write_image(&pixels, w, h, rgb).unwrap();
                }
                "tif" => {
                    let mut e = image::codecs::tiff::TiffEncoder::new(file);
                    e.set_icc_profile(icc).unwrap();
                    e.write_image(&pixels, w, h, rgb).unwrap();
                }
                _ => {
                    let mut e = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 100);
                    e.set_icc_profile(icc).unwrap();
                    e.write_image(&pixels, w, h, rgb).unwrap();
                }
            }
        };
        let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let srgb = moxcms::ColorProfile::new_srgb().encode().unwrap();
        for ext in ["png", "tif", "jpg"] {
            let path = dir.path().join(format!("frame.{ext}"));
            encode(&path, p3.clone());
            let Frame { image, profile, .. } =
                load_frame(&path, FileCache::Use, CameraRaw::LibRaw, Target::Own).unwrap();
            assert_eq!(profile.map(|p| p.bytes().to_vec()), Some(p3.clone()), ".{ext}");
            if ext != "jpg" {
                assert_eq!(image.as_bytes(), pixels.as_slice(), ".{ext}: values as stored");
            }
            encode(&path, srgb.clone());
            let profile =
                load_frame(&path, FileCache::Use, CameraRaw::LibRaw, Target::Own).unwrap().profile;
            assert_eq!(profile, None, ".{ext}: sRGB");
        }
    }

    /// A 16-bit 8192×5464 TIFF (268.6 MB) loads, which `image::open`'s
    /// default 512 MiB cap refuses (TIFF counts the frame twice).
    #[test]
    fn large_16bit_tiff_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.tif");
        let (w, h) = (8192u32, 5464u32);
        let frame =
            ImageBuffer::<Rgb<u16>, _>::from_fn(w, h, |x, y| Rgb([x as u16, y as u16, 65535]));
        frame.save(&path).unwrap();
        drop(frame);

        let loaded = load_image(&path, crate::io::CameraRaw::LibRaw).unwrap();
        let rgb = loaded.as_rgb16().expect("16-bit RGB stays 16-bit RGB");
        assert_eq!(rgb.dimensions(), (w, h));
        assert_eq!(rgb.get_pixel(w - 1, h - 1).0, [(w - 1) as u16, (h - 1) as u16, 65535]);
    }

    #[test]
    fn supported_extensions_cover_raw_and_standard() {
        let all = supported_extensions();
        for ext in ["jpg", "png", "tif", "dng", "cr3", "x3f", "iiq", "3fr"] {
            assert!(all.contains(&ext), "{ext} missing");
        }
        assert!(
            all.iter().all(|e| e.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
        );
    }

    /// A JPEG of `w`×`h` with detail in every row.
    fn detailed_jpeg(w: u32, h: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let pixels = ImageBuffer::<Rgb<u8>, _>::from_fn(w, h, |x, y| {
            Rgb([(x * 7 + y * 3) as u8, (x ^ y) as u8, (y * 5) as u8])
        });
        let mut file = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut file)
            .write_image(pixels.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        file
    }

    /// A truncated JPEG is refused, not filled with gray.
    #[test]
    fn a_truncated_jpeg_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let whole = detailed_jpeg(256, 256);
        let path = dir.path().join("cut.jpg");
        std::fs::write(&path, &whole[..whole.len() * 6 / 10]).unwrap();
        let error = load_image(&path, CameraRaw::LibRaw).expect_err("a cut-off JPEG loads");
        assert!(matches!(error, CoreError::Truncated { .. }), "{error}");
    }

    /// A JPEG with data after its end (a motion photo) loads.
    #[test]
    fn a_jpeg_with_a_trailer_loads() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = detailed_jpeg(256, 256);
        file.extend(std::iter::repeat_n(0x5A, 200_000));
        let path = dir.path().join("trailer.jpg");
        std::fs::write(&path, file).unwrap();
        assert_eq!(
            load_image(&path, CameraRaw::LibRaw).unwrap().into_rgb8().dimensions(),
            (256, 256)
        );
    }

    /// A JPEG's header comes from its segments: its size and layout, and a
    /// four-component frame (CMYK or YCCK) is refused before it decodes.
    #[test]
    fn a_jpeg_header_tells_its_layout_and_refuses_cmyk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.jpg");
        let whole = detailed_jpeg(64, 48);
        std::fs::write(&path, &whole).unwrap();
        let open = decoders::open(&path).unwrap();
        let header = open.header();
        assert_eq!((header.width, header.height, header.layout), (64, 48, input::Layout::Rgb));
        let sof = whole.windows(2).position(|m| m == [0xFF, 0xC0]).unwrap();
        let mut cmyk = whole.clone();
        cmyk[sof + 9] = 4;
        std::fs::write(&path, &cmyk).unwrap();
        let error = decoders::open(&path).err().expect("CMYK opens");
        assert!(matches!(error, CoreError::Refused { what: input::Refused::Cmyk, .. }), "{error}");
    }

    /// A landscape JPEG with EXIF orientation 6 loads, and reports its
    /// header, portrait.
    #[test]
    fn rotated_jpegs_load_upright() {
        use image::ImageEncoder;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("portrait.jpg");
        // Red left half, blue right half.
        let (w, h) = (64u32, 32u32);
        let pixels = ImageBuffer::<Rgb<u8>, _>::from_fn(w, h, |x, _| {
            if x < w / 2 { Rgb([255, 0, 0]) } else { Rgb([0, 0, 255]) }
        });
        // Little-endian TIFF: IFD0 with one SHORT entry, Orientation = 6.
        let mut tiff = b"II*\0\x08\0\0\0\x01\0".to_vec();
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0]);
        let mut file = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut file);
        encoder.set_exif_metadata(tiff).unwrap();
        encoder.write_image(pixels.as_raw(), w, h, image::ExtendedColorType::Rgb8).unwrap();
        std::fs::write(&path, file).unwrap();

        let upright = Header { width: h, height: w, depth: FrameDepth::Eight };
        assert_eq!(header_info(&path, crate::io::CameraRaw::LibRaw), Some(upright));
        let loaded = load_image(&path, crate::io::CameraRaw::LibRaw).unwrap().into_rgb8();
        assert_eq!(loaded.dimensions(), (h, w));
        // Turned clockwise, the left (red) half is now on top.
        assert!(loaded.get_pixel(h / 2, 4)[0] > 200);
        assert!(loaded.get_pixel(h / 2, w - 4)[2] > 200);
    }

    /// A DNG whose read blocks (a FIFO standing in for a slow share) holds
    /// up no other DNG's answer.
    #[cfg(unix)]
    #[test]
    fn a_slow_dng_read_holds_up_no_other() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let (slow, other) = (dir.path().join("slow.dng"), dir.path().join("other.dng"));
        let made = std::process::Command::new("mkfifo").arg(&slow).status().unwrap();
        assert!(made.success());
        std::fs::write(&other, b"not a tiff").unwrap();
        let held = std::thread::spawn({
            let slow = slow.clone();
            move || is_mosaic_dng(&slow)
        });
        // Long enough for the first to be waiting in its read.
        std::thread::sleep(Duration::from_millis(100));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || tx.send(is_mosaic_dng(&other)).unwrap());
        let answered = rx.recv_timeout(Duration::from_secs(5));
        // Let the first read go: open the pipe's other end and close it.
        drop(std::fs::OpenOptions::new().write(true).open(&slow).unwrap());
        assert!(!held.join().unwrap());
        assert_eq!(answered, Ok(false), "the other DNG waited for the slow one");
    }
}
