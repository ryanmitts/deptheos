//! The desktop's decoders of JPEG, PNG and TIFF, each opening a file once
//! for header and pixels, handed over as a [`Decoded`] (ADR-132). What
//! they can't read is refused by name.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use image::metadata::Orientation;

use super::input::{
    Alpha, ColorTag, Decoded, DecodedHeader, Layout, Refused, Samples, StoredDepth,
};
use crate::error::{CoreError, Result};

/// A frame opened by its decoder (the desktop's or `HostDecoder`): header
/// read, pixels not yet.
pub trait OpenFrame: Send {
    fn header(&self) -> &DecodedHeader;
    /// The pixels, exactly as the header said.
    fn decode(self: Box<Self>) -> Result<Decoded>;
}

/// The most bytes one decode may allocate. 4 GiB holds a 16-bit RGB frame
/// of over 700 MP and still refuses absurd headers.
const MAX_DECODE_ALLOC: u64 = 4 << 30;

/// Open the standard image at `path` with the decoder its content calls
/// for.
pub(super) fn open(path: &Path) -> Result<Box<dyn OpenFrame>> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 4];
    let read = file.read(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    match &magic[..read] {
        [0xFF, 0xD8, 0xFF, ..] => Ok(Box::new(JpegFrame::open(path, file)?)),
        [0x89, b'P', b'N', b'G'] => Ok(Box::new(PngFrame::open(path, file)?)),
        b"II*\0" | b"MM\0*" | b"II+\0" | b"MM\0+" => Ok(Box::new(TiffFrame::open(path, file)?)),
        _ => Err(decode_error(path, "not a JPEG, PNG or TIFF file")),
    }
}

fn decode_error(path: &Path, message: impl std::fmt::Display) -> CoreError {
    CoreError::Decode { path: path.to_path_buf(), message: message.to_string() }
}

fn refused(path: &Path, what: Refused) -> CoreError {
    CoreError::Refused { path: path.to_path_buf(), what }
}

/// A frame's EXIF orientation and whether it is in-camera Adobe RGB (`R03`).
fn exif_tags(block: Vec<u8>) -> (Orientation, bool) {
    use exif::{In, Tag, Value};
    let reader = exif::Reader::new();
    let Ok(exif) = reader.read_raw(block).or_else(|e| e.distill_partial_result(|_| {})) else {
        return (Orientation::NoTransforms, false);
    };
    let orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .and_then(|o| u8::try_from(o).ok())
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms);
    let adobe = exif.get_field(Tag::InteroperabilityIndex, In::PRIMARY).is_some_and(
        |f| matches!(&f.value, Value::Ascii(v) if v.first().is_some_and(|s| s.starts_with(b"R03"))),
    );
    (orientation, adobe)
}

// ---------------------------------------------------------------- JPEG

/// A JPEG: its header from a walk of its segments, its pixels from the
/// `image` crate's decoder.
struct JpegFrame {
    path: PathBuf,
    header: DecodedHeader,
    file: File,
    /// Whether the file ends with EOI; one that doesn't is walked whole
    /// before it decodes.
    ends_with_eoi: bool,
}

impl JpegFrame {
    fn open(path: &Path, mut file: File) -> Result<Self> {
        const EOI: [u8; 2] = [0xFF, 0xD9];
        let mut end = [0u8; 2];
        let ends_with_eoi =
            file.seek(SeekFrom::End(-2)).is_ok() && file.read_exact(&mut end).is_ok() && end == EOI;
        file.seek(SeekFrom::Start(0))?;
        let mut segments = Segments::default();
        segments.walk(&mut BufReader::with_capacity(64 * 1024, &mut file), false);
        let Some(frame) = segments.frame else {
            return Err(decode_error(path, "no JPEG frame header before its scan"));
        };
        let layout = match frame.components {
            1 => Layout::Gray,
            3 => Layout::Rgb,
            // CMYK, or YCCK.
            4 => return Err(refused(path, Refused::Cmyk)),
            n => return Err(decode_error(path, format!("{n} components"))),
        };
        let (orientation, adobe) =
            segments.exif.take().map_or((Orientation::NoTransforms, false), exif_tags);
        let color = match segments.icc() {
            Some(icc) => ColorTag::Icc(icc),
            None if adobe => ColorTag::DcfAdobeRgb,
            None => ColorTag::Untagged,
        };
        let header = DecodedHeader {
            width: u32::from(frame.width),
            height: u32::from(frame.height),
            depth: StoredDepth::U8,
            layout,
            color,
            orientation,
        };
        Ok(Self { path: path.to_path_buf(), header, file, ends_with_eoi })
    }
}

impl OpenFrame for JpegFrame {
    fn header(&self) -> &DecodedHeader {
        &self.header
    }

    fn decode(self: Box<Self>) -> Result<Decoded> {
        use image::ImageDecoder;
        let Self { path, header, mut file, ends_with_eoi } = *self;
        // The decoder silently fills a truncated file with gray. One not
        // ending in EOI may still be whole with a trailer, so walk it.
        if !ends_with_eoi {
            file.seek(SeekFrom::Start(0))?;
            let mut walk = Segments::default();
            if !walk.walk(&mut BufReader::with_capacity(64 * 1024, &mut file), true) {
                return Err(CoreError::Truncated { path });
            }
        }
        file.seek(SeekFrom::Start(0))?;
        let mut decoder = image::codecs::jpeg::JpegDecoder::new(BufReader::new(file))?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODE_ALLOC);
        limits.reserve(decoder.total_bytes())?;
        decoder.set_limits(limits)?;
        let mut pixels = vec![0u8; decoder.total_bytes() as usize];
        decoder.read_image(&mut pixels)?;
        Ok(Decoded { header, samples: Samples::U8(pixels) })
    }
}

/// A JPEG's frame header: its size and component count.
#[derive(Clone, Copy)]
struct JpegFrameHeader {
    width: u16,
    height: u16,
    components: u8,
}

/// What a walk of a JPEG's segments finds before its scan.
#[derive(Default)]
struct Segments {
    frame: Option<JpegFrameHeader>,
    /// APP1's EXIF block, as a TIFF stream.
    exif: Option<Vec<u8>>,
    /// APP2's ICC profile chunks: sequence number, then bytes.
    icc: Vec<(u8, Vec<u8>)>,
}

impl Segments {
    /// The ICC profile its chunks make, in order; `None` without them, or
    /// with one missing.
    fn icc(&mut self) -> Option<Vec<u8>> {
        self.icc.sort_by_key(|(n, _)| *n);
        let whole = self.icc.iter().enumerate().all(|(i, (n, _))| usize::from(*n) == i + 1);
        (whole && !self.icc.is_empty())
            .then(|| self.icc.iter().flat_map(|(_, b)| b.clone()).collect())
    }

    /// Walk the segments from the start of `r`, keeping what [`Segments`]
    /// holds. With `whole`, walk through the scan and return whether EOI
    /// was reached before the file ended; without, stop at the scan (the
    /// result means nothing).
    fn walk(&mut self, r: &mut (impl BufRead + Seek), whole: bool) -> bool {
        let mut soi = [0u8; 2];
        if r.read_exact(&mut soi).is_err() {
            return false;
        }
        if soi != [0xFF, 0xD8] {
            return true;
        }
        let mut byte = [0u8; 1];
        loop {
            if r.read_exact(&mut byte).is_err() {
                return false;
            }
            if byte[0] != 0xFF {
                return true;
            }
            let code = loop {
                if r.read_exact(&mut byte).is_err() {
                    return false;
                }
                if byte[0] != 0xFF {
                    break byte[0];
                }
            };
            match code {
                0xD9 => return true,
                // Markers that stand alone: restarts, TEM, and a stray SOI.
                0x01 | 0xD0..=0xD8 => continue,
                _ => {}
            }
            let mut length = [0u8; 2];
            if r.read_exact(&mut length).is_err() {
                return false;
            }
            let Some(len) = u16::from_be_bytes(length).checked_sub(2) else { return false };
            let sof = matches!(code, 0xC0..=0xCF) && !matches!(code, 0xC4 | 0xC8 | 0xCC);
            let app = matches!(code, 0xE1 | 0xE2) && self.frame.is_none();
            if (sof && len >= 6) || app {
                let mut body = vec![0u8; usize::from(len)];
                if r.read_exact(&mut body).is_err() {
                    return false;
                }
                self.keep(code, body);
            } else if code == 0xDA && !whole {
                return true;
            } else if r.seek(SeekFrom::Current(i64::from(len))).is_err() {
                // A seek past the end succeeds; the next read finds it.
                return false;
            }
            if code != 0xDA {
                continue;
            }
            // A scan's data runs to the next marker that isn't a stuffed
            // zero or a restart.
            loop {
                let Ok(buf) = r.fill_buf() else { return false };
                if buf.is_empty() {
                    return false;
                }
                if let Some(pos) = buf.iter().position(|&b| b == 0xFF) {
                    r.consume(pos + 1);
                    if r.read_exact(&mut byte).is_err() {
                        return false;
                    }
                    match byte[0] {
                        0x00 | 0xD0..=0xD7 => continue,
                        0xFF => {
                            // Back 1 byte so the outer loop sees 0xFF.
                            let _ = r.seek(SeekFrom::Current(-1));
                            break;
                        }
                        _ => {
                            // Back 2 bytes so the outer loop sees 0xFF, code.
                            let _ = r.seek(SeekFrom::Current(-2));
                            break;
                        }
                    }
                } else {
                    let len = buf.len();
                    r.consume(len);
                }
            }
        }
    }

    /// Keep what segment `code` says, from its `body`.
    fn keep(&mut self, code: u8, body: Vec<u8>) {
        const EXIF: &[u8] = b"Exif\0\0";
        const ICC: &[u8] = b"ICC_PROFILE\0";
        match code {
            0xE1 if body.starts_with(EXIF) && self.exif.is_none() => {
                self.exif = Some(body[EXIF.len()..].to_vec());
            }
            0xE2 if body.starts_with(ICC) && body.len() > ICC.len() + 2 => {
                self.icc.push((body[ICC.len()], body[ICC.len() + 2..].to_vec()));
            }
            // A frame header: precision, height, width, then components.
            0xC0..=0xCF if self.frame.is_none() => {
                self.frame = Some(JpegFrameHeader {
                    height: u16::from_be_bytes([body[1], body[2]]),
                    width: u16::from_be_bytes([body[3], body[4]]),
                    components: body[5],
                });
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- PNG

/// A PNG through the `png` crate, its header from the chunks before its
/// pixels.
struct PngFrame {
    path: PathBuf,
    header: DecodedHeader,
    reader: png::Reader<BufReader<File>>,
}

impl PngFrame {
    fn open(path: &Path, file: File) -> Result<Self> {
        let limits = png::Limits { bytes: MAX_DECODE_ALLOC as usize };
        let mut decoder = png::Decoder::new_with_limits(BufReader::new(file), limits);
        // Palettes to RGB, low-bit gray to 8, tRNS to alpha; 16 bits kept.
        decoder.set_transformations(png::Transformations::EXPAND);
        let reader = decoder.read_info().map_err(|e| decode_error(path, e))?;
        // The crate's limit doesn't cover the output buffer: check it here.
        if reader.output_buffer_size().is_none_or(|bytes| bytes as u64 > MAX_DECODE_ALLOC) {
            return Err(decode_error(path, "PNG: too large"));
        }
        let (color, depth) = reader.output_color_type();
        let layout = match color {
            png::ColorType::Grayscale => Layout::Gray,
            png::ColorType::GrayscaleAlpha => Layout::GrayAlpha(Alpha::Straight),
            png::ColorType::Rgb | png::ColorType::Indexed => Layout::Rgb,
            png::ColorType::Rgba => Layout::RgbAlpha(Alpha::Straight),
        };
        let depth = match depth {
            png::BitDepth::Sixteen => StoredDepth::U16,
            _ => StoredDepth::U8,
        };
        let info = reader.info();
        let (orientation, adobe) = info
            .exif_metadata
            .as_ref()
            .map_or((Orientation::NoTransforms, false), |e| exif_tags(e.to_vec()));
        // The specification's order: cICP, iCCP, sRGB, then cHRM and gAMA.
        let color = if let Some(c) = info.coding_independent_code_points {
            ColorTag::Cicp {
                primaries: c.color_primaries,
                transfer: c.transfer_function,
                full_range: c.is_video_full_range_image,
            }
        } else if let Some(icc) = &info.icc_profile {
            ColorTag::Icc(icc.to_vec())
        } else if info.srgb.is_some() || info.gama_chunk.is_some() || info.chrm_chunk.is_some() {
            let chromaticities = info.chrm_chunk.map(|c| {
                let p =
                    |(x, y): (png::ScaledFloat, png::ScaledFloat)| (x.into_value(), y.into_value());
                [p(c.white), p(c.red), p(c.green), p(c.blue)]
            });
            ColorTag::PngChunks {
                srgb: info.srgb.is_some(),
                gamma: info.gama_chunk.map(png::ScaledFloat::into_value),
                chromaticities,
            }
        } else if adobe {
            ColorTag::DcfAdobeRgb
        } else {
            ColorTag::Untagged
        };
        let header = DecodedHeader {
            width: info.width,
            height: info.height,
            depth,
            layout,
            color,
            orientation,
        };
        Ok(Self { path: path.to_path_buf(), header, reader })
    }
}

impl OpenFrame for PngFrame {
    fn header(&self) -> &DecodedHeader {
        &self.header
    }

    fn decode(self: Box<Self>) -> Result<Decoded> {
        let Self { path, header, mut reader } = *self;
        let failed = |e: png::DecodingError| decode_error(&path, format!("PNG: {e}"));
        let bytes = reader
            .output_buffer_size()
            .filter(|bytes| *bytes as u64 <= MAX_DECODE_ALLOC)
            .ok_or_else(|| decode_error(&path, "PNG: too large"))?;
        let samples = match header.depth {
            StoredDepth::U16 => {
                // Read into the u16s' own bytes, then from big-endian.
                let mut pixels = vec![0u16; bytes / 2];
                reader.next_frame(bytemuck::cast_slice_mut(&mut pixels)).map_err(failed)?;
                pixels.iter_mut().for_each(|v| *v = u16::from_be(*v));
                Samples::U16(pixels)
            }
            _ => {
                let mut pixels = vec![0u8; bytes];
                reader.next_frame(&mut pixels).map_err(failed)?;
                Samples::U8(pixels)
            }
        };
        Ok(Decoded { header, samples })
    }
}

// ---------------------------------------------------------------- TIFF

/// A TIFF through the `tiff` crate.
struct TiffFrame {
    path: PathBuf,
    header: DecodedHeader,
    decoder: tiff::decoder::Decoder<BufReader<File>>,
    /// How its samples become the header's.
    kind: TiffKind,
}

#[derive(Clone, Copy)]
enum TiffKind {
    /// As decoded.
    Plain,
    /// Planar: one plane a sample, interleaved after.
    Planar,
    /// 32-bit unsigned gray, scaled to 16 bits.
    Gray32,
    /// YCbCr, converted to RGB through its coefficients (red, green, blue)
    /// and reference black and white.
    YCbCr { luma: [f32; 3], reference: [f32; 6] },
}

impl TiffFrame {
    fn open(path: &Path, file: File) -> Result<Self> {
        use tiff::ColorType as C;
        use tiff::tags::Tag;
        let tiff_error = |e: tiff::TiffError| decode_error(path, format!("TIFF: {e}"));
        let mut limits = tiff::decoder::Limits::default();
        limits.decoding_buffer_size = usize::try_from(MAX_DECODE_ALLOC).unwrap_or(usize::MAX);
        limits.intermediate_buffer_size = limits.decoding_buffer_size;
        let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
            .map_err(tiff_error)?
            .with_limits(limits);
        let (width, height) = decoder.dimensions().map_err(tiff_error)?;
        let photometric = decoder.find_tag_unsigned::<u16>(Tag::PhotometricInterpretation);
        let refuse = |what| Err(refused(path, what));
        // The crate refuses palettes, and these before their color type.
        match photometric.ok().flatten() {
            Some(3) => {
                tracing::debug!("{}: a palette TIFF", path.display());
                return refuse(Refused::TiffLayout);
            }
            Some(5) => return refuse(Refused::Cmyk),
            Some(8..=10) => return refuse(Refused::Lab),
            _ => {}
        }
        let color = decoder.colortype().map_err(|e| {
            tracing::debug!("{}: {e}", path.display());
            refused(path, Refused::TiffLayout)
        })?;
        let extra = decoder.find_tag_unsigned_vec::<u16>(Tag::ExtraSamples).ok().flatten();
        // 1 associated (premultiplied), 2 unassociated, 0 unspecified:
        // taken as straight, as writers mean it.
        let alpha = match extra.as_deref() {
            Some([1, ..]) => Alpha::Premultiplied,
            _ => Alpha::Straight,
        };
        // SampleFormat: 1 unsigned, 2 signed (refused), 3 float.
        let format = decoder
            .find_tag_unsigned_vec::<u16>(Tag::SampleFormat)
            .ok()
            .flatten()
            .unwrap_or_default();
        if format.first() == Some(&2) {
            tracing::debug!("{}: a TIFF of signed samples", path.display());
            return refuse(Refused::TiffLayout);
        }
        let float = format.first() == Some(&3);
        let planar =
            decoder.find_tag_unsigned::<u16>(Tag::PlanarConfiguration).ok().flatten() == Some(2);
        let (layout, bits) = match color {
            C::Gray(b) => (Layout::Gray, b),
            C::GrayA(b) => (Layout::GrayAlpha(alpha), b),
            C::Multiband { bit_depth, num_samples: 2 }
                if extra.as_deref().is_some_and(|e| e.len() == 1) =>
            {
                (Layout::GrayAlpha(alpha), bit_depth)
            }
            C::RGB(b) | C::YCbCr(b) => (Layout::Rgb, b),
            C::RGBA(b) => (Layout::RgbAlpha(alpha), b),
            C::CMYK(_) | C::CMYKA(_) => return refuse(Refused::Cmyk),
            C::Lab(_) => return refuse(Refused::Lab),
            other => {
                tracing::debug!("{}: a TIFF of {other:?}", path.display());
                return refuse(Refused::TiffLayout);
            }
        };
        let rationals = |decoder: &mut tiff::decoder::Decoder<_>, tag: u16| -> Option<Vec<f32>> {
            let value = decoder.find_tag(Tag::Unknown(tag)).ok().flatten()?;
            let list = match value {
                tiff::decoder::ifd::Value::List(list) => list,
                single => vec![single],
            };
            list.into_iter()
                .map(|v| match v {
                    tiff::decoder::ifd::Value::Rational(n, d) if d != 0 => {
                        Some(n as f32 / d as f32)
                    }
                    _ => None,
                })
                .collect()
        };
        // Subsampled YCbCr is read only from JPEG compression (7), which
        // the crate's JPEG decoder upsamples.
        if let C::YCbCr(_) = color {
            let sub = decoder.find_tag_unsigned_vec::<u16>(Tag::Unknown(530)).ok().flatten();
            let jpeg = decoder.find_tag_unsigned::<u16>(Tag::Compression).ok().flatten() == Some(7);
            if sub.as_deref().unwrap_or(&[2, 2]) != [1, 1] && !jpeg {
                tracing::debug!("{}: subsampled YCbCr", path.display());
                return refuse(Refused::TiffLayout);
            }
        }
        let (depth, kind) = match (bits, float, color) {
            (8, false, C::YCbCr(_)) => {
                let luma = rationals(&mut decoder, 529)
                    .and_then(|v| <[f32; 3]>::try_from(v).ok())
                    .unwrap_or([0.299, 0.587, 0.114]);
                let reference = rationals(&mut decoder, 532)
                    .and_then(|v| <[f32; 6]>::try_from(v).ok())
                    .unwrap_or([0.0, 255.0, 128.0, 255.0, 128.0, 255.0]);
                (StoredDepth::U8, TiffKind::YCbCr { luma, reference })
            }
            (_, _, C::YCbCr(_)) => return refuse(Refused::TiffLayout),
            (8, false, _) => (StoredDepth::U8, TiffKind::Plain),
            (16, false, _) => (StoredDepth::U16, TiffKind::Plain),
            (32, true, _) => (StoredDepth::F32, TiffKind::Plain),
            (32, false, C::Gray(_)) => (StoredDepth::U16, TiffKind::Gray32),
            _ => {
                tracing::debug!("{}: a TIFF of {bits}-bit samples", path.display());
                return refuse(Refused::TiffLayout);
            }
        };
        let kind = if planar { TiffKind::Planar } else { kind };
        if planar && !matches!(color, C::RGB(_) | C::RGBA(_) | C::GrayA(_) | C::Multiband { .. }) {
            return refuse(Refused::TiffLayout);
        }
        let orientation = decoder
            .find_tag_unsigned::<u8>(Tag::Orientation)
            .ok()
            .flatten()
            .and_then(Orientation::from_exif)
            .unwrap_or(Orientation::NoTransforms);
        let color = match decoder.find_tag(Tag::IccProfile).ok().flatten().map(|v| v.into_u8_vec())
        {
            Some(Ok(icc)) => ColorTag::Icc(icc),
            _ => ColorTag::Untagged,
        };
        let header = DecodedHeader { width, height, depth, layout, color, orientation };
        Ok(Self { path: path.to_path_buf(), header, decoder, kind })
    }
}

impl OpenFrame for TiffFrame {
    fn header(&self) -> &DecodedHeader {
        &self.header
    }

    fn decode(self: Box<Self>) -> Result<Decoded> {
        use tiff::decoder::DecodingResult as D;
        let Self { path, header, mut decoder, kind } = *self;
        let tiff_error = |e: tiff::TiffError| decode_error(&path, format!("TIFF: {e}"));
        let mut result = D::U8(Vec::new());
        decoder.read_image_to_buffer(&mut result).map_err(tiff_error)?;
        let channels = header.layout.channels();
        // The crate reads only the first plane of one past its limit.
        let pixels = header.width as usize * header.height as usize;
        let every = || decode_error(&path, "a planar TIFF without every plane");
        let every = (pixels, every);
        let samples = match (result, kind) {
            (D::U8(v), TiffKind::YCbCr { luma, reference }) => {
                Samples::U8(ycbcr_to_rgb(&v, luma, reference))
            }
            (D::U8(v), TiffKind::Planar) => Samples::U8(interleave(v, channels, every)?),
            (D::U16(v), TiffKind::Planar) => Samples::U16(interleave(v, channels, every)?),
            (D::F32(v), TiffKind::Planar) => Samples::F32(interleave(v, channels, every)?),
            (D::U8(v), TiffKind::Plain) => Samples::U8(v),
            (D::U16(v), TiffKind::Plain) => Samples::U16(v),
            (D::F32(v), TiffKind::Plain) => Samples::F32(v),
            (D::U32(v), TiffKind::Gray32) => Samples::U16(
                v.iter()
                    .map(|&s| ((u64::from(s) * 65535 + (1 << 31)) / u64::from(u32::MAX)) as u16)
                    .collect(),
            ),
            _ => return Err(decode_error(&path, "TIFF samples of another kind than its header's")),
        };
        Ok(Decoded { header, samples })
    }
}

/// `channels` planes of `pixels` samples as interleaved pixels, or
/// `short`'s error.
fn interleave<T: Copy + Default>(
    planes: Vec<T>,
    channels: usize,
    (pixels, short): (usize, impl FnOnce() -> CoreError),
) -> Result<Vec<T>> {
    if planes.len() != pixels * channels {
        return Err(short());
    }
    let mut out = vec![T::default(); pixels * channels];
    for (c, plane) in planes.chunks(pixels).enumerate().take(channels) {
        for (i, &v) in plane.iter().enumerate() {
            out[i * channels + c] = v;
        }
    }
    Ok(out)
}

/// 8-bit YCbCr as RGB (TIFF 6.0, section 21): the reference black and
/// white scale each component, and the luma coefficients undo the mix.
fn ycbcr_to_rgb(samples: &[u8], [lr, lg, lb]: [f32; 3], reference: [f32; 6]) -> Vec<u8> {
    let scale = |v: u8, i: usize, range: f32| {
        let (black, white) = (reference[i * 2], reference[i * 2 + 1]);
        (f32::from(v) - black) * range / (white - black).max(1.0)
    };
    let byte = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    samples
        .chunks(3)
        .flat_map(|p| {
            let y = scale(p[0], 0, 255.0);
            let (cb, cr) = (scale(p[1], 1, 127.0), scale(p[2], 2, 127.0));
            let r = y + (2.0 - 2.0 * lr) * cr;
            let b = y + (2.0 - 2.0 * lb) * cb;
            let g = (y - lb * b - lr * r) / lg;
            [byte(r), byte(g), byte(b)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Planes interleave; a buffer missing some is refused.
    #[test]
    fn a_planar_tiff_needs_every_plane() {
        let short = || CoreError::InvalidState("short".into());
        let planes = vec![1u8, 2, 10, 20, 100, 200];
        assert_eq!(interleave(planes, 3, (2, short)).unwrap(), [1, 10, 100, 2, 20, 200]);
        assert!(interleave(vec![1u8, 2], 3, (2, short)).is_err(), "one plane of three");
    }

    /// A TIFF of signed integers is refused as it opens, not failed as it
    /// decodes.
    #[test]
    fn a_signed_tiff_is_refused_as_it_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("signed.tif");
        let mut tiff =
            tiff::encoder::TiffEncoder::new(std::fs::File::create(&path).unwrap()).unwrap();
        tiff.write_image::<tiff::encoder::colortype::GrayI16>(2, 2, &[-1, 0, 1, 2]).unwrap();
        drop(tiff);
        let opened = open(&path);
        assert!(
            matches!(opened, Err(CoreError::Refused { what: Refused::TiffLayout, .. })),
            "{:?}",
            opened.err()
        );
    }

    /// A PNG claiming more than a decode may allocate is refused as it opens.
    #[test]
    fn a_png_claiming_a_huge_size_is_refused_as_it_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.png");
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Sixteen);
        encoder.write_header().unwrap().write_image_data(&[0; 8]).unwrap();
        // The header's width and height, and its checksum again.
        bytes[16..24].copy_from_slice(&[0, 0x2D, 0xC6, 0xC0, 0, 0x2D, 0xC6, 0xC0]);
        let crc = bytes[12..29].iter().fold(!0u32, |crc, byte| {
            (0..8).fold(crc ^ u32::from(*byte), |c, _| {
                (c >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(c & 1))
            })
        });
        bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        let opened = open(&path);
        let said = opened.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(said.contains("too large"), "{said}");
    }
}
