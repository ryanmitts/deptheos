//! Turning a result into u16/u8 pixels and writing it to disk.

use image::ImageFormat;
use rayon::prelude::*;
use std::path::Path;

use super::jpeg::{JPEG_QUALITY, encode_jpeg, jpeg_with_metadata};
use super::metadata::ExportMetadata;
use super::replace::write_replacing;
use super::tiff::{TiffCompression, write_tiff};
use crate::error::{CoreError, Result};

/// The one float → u16 rounding: clamp to [0, 1], round half up. The GPU's
/// `pack` kernel matches it exactly (its threshold table is built from it).
pub fn quantize_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

/// The one u16 → u8 rounding, `round(v · 255 / 65535)`, for the apps'
/// display. An 8-bit save is dithered instead ([`dither_rgb_to_u8`]).
pub fn u16_to_u8(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32767) / 65535) as u8
}

/// One RGBA result row as interleaved RGB u16 into `rgb` (one row long),
/// alpha dropped; Rgba32Float through [`quantize_u16`].
///
/// # Panics
/// On any other format.
pub fn rgba_row_to_rgb_u16(row: &[u8], format: wgpu::TextureFormat, rgb: &mut [u16]) {
    let out = rgb.as_chunks_mut::<3>().0;
    match format {
        wgpu::TextureFormat::Rgba16Unorm => {
            for (p, o) in row.as_chunks::<8>().0.iter().zip(out) {
                *o = [
                    u16::from_le_bytes([p[0], p[1]]),
                    u16::from_le_bytes([p[2], p[3]]),
                    u16::from_le_bytes([p[4], p[5]]),
                ];
            }
        }
        wgpu::TextureFormat::Rgba32Float => {
            let floats: &[f32] = bytemuck::cast_slice(row);
            for (pixel, o) in floats.as_chunks::<4>().0.iter().zip(out) {
                *o = [quantize_u16(pixel[0]), quantize_u16(pixel[1]), quantize_u16(pixel[2])];
            }
        }
        other => panic!("{other:?} is not an RGBA result format"),
    }
}

/// Save interleaved RGB u16 pixels as the extension says, with `metadata`
/// embedded, through [`write_replacing`]. 16-bit for PNG/TIFF when
/// `sixteen_bit`, else (and always for JPEG) dithered to 8 bits
/// ([`dither_rgb_to_u8`]). No pixels is [`CoreError::EmptyImage`].
pub fn save_rgb16(
    path: &Path,
    pixels: &[u16],
    width: u32,
    height: u32,
    sixteen_bit: bool,
    encoding: Encoding,
    metadata: &ExportMetadata,
) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(CoreError::EmptyImage);
    }
    let expected = width as usize * height as usize * 3;
    if pixels.len() != expected {
        return Err(CoreError::InvalidState(format!(
            "RGB16 buffer has {} values, expected {expected} for {width}x{height}",
            pixels.len()
        )));
    }
    // `image`'s mapping, so every JPEG spelling (`.jfif`) takes 8 bits.
    let format = ImageFormat::from_path(path)?;
    if sixteen_bit && format != ImageFormat::Jpeg {
        let image = Pixels { data: Samples::U16(pixels), width, height };
        return save_tagged(path, format, &image, encoding, metadata);
    }
    let bytes = dither_rgb_to_u8(pixels, width);
    let image = Pixels { data: Samples::U8(&bytes), width, height };
    save_tagged(path, format, &image, encoding, metadata)
}

/// The 8×8 Bayer matrix of ranks 0–63.
const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Interleaved RGB u16 pixels, `width` a row, to 8 bits for an export:
/// `v · 255 / 65535` plus an 8×8 Bayer offset in (−½, ½) LSB, floored, so
/// smooth gradients don't band. The offset depends only on position, the
/// same for all three channels (gray stays gray), so output is
/// deterministic; a value on an 8-bit code stays [`u16_to_u8`]'s.
pub fn dither_rgb_to_u8(pixels: &[u16], width: u32) -> Vec<u8> {
    let row_len = (width as usize * 3).max(1);
    let mut bytes = vec![0u8; pixels.len()];
    bytes.par_chunks_mut(row_len).zip(pixels.par_chunks(row_len)).enumerate().for_each(
        |(y, (out, row))| {
            let bayer = &BAYER8[y % 8];
            for (x, (o, p)) in out.chunks_mut(3).zip(row.chunks(3)).enumerate() {
                // floor(v·255/65535 + (2t + 1)/128), in integers: at most
                // 65535·255·128 + 127·65535, which fits a u32.
                let offset = (2 * u32::from(bayer[x % 8]) + 1) * 65535;
                for (o, &v) in o.iter_mut().zip(p) {
                    *o = ((u32::from(v) * 255 * 128 + offset) / (65535 * 128)) as u8;
                }
            }
        },
    );
    bytes
}

/// Interleaved RGB pixels to save with metadata.
pub(super) struct Pixels<'a> {
    pub(super) data: Samples<'a>,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Copy)]
pub(super) enum Samples<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}

/// Save `image` as JPEG (8-bit only), PNG or TIFF with `metadata`
/// embedded, through a file renamed into place.
fn save_tagged(
    path: &Path,
    format: ImageFormat,
    image: &Pixels,
    encoding: Encoding,
    metadata: &ExportMetadata,
) -> Result<()> {
    let exif = metadata.exif_tiff(image.width, image.height);
    let icc = metadata.icc.as_ref().map(|p| p.bytes());
    match format {
        ImageFormat::Jpeg => {
            let Samples::U8(bytes) = image.data else {
                return Err(CoreError::InvalidState("JPEG takes 8-bit pixels".into()));
            };
            let encoded = encode_jpeg(bytes, image.width, image.height, encoding.jpeg_quality)?;
            let encoded = jpeg_with_metadata(&encoded, &exif, metadata.xmp.as_deref(), icc)?;
            write_replacing(path, |tmp| Ok(std::fs::write(tmp, &encoded)?))
        }
        ImageFormat::Png => write_replacing(path, |tmp| {
            let file = std::io::BufWriter::new(std::fs::File::create(tmp)?);
            write_png(file, image, exif, metadata.xmp.as_deref(), icc)
        }),
        ImageFormat::Tiff => write_replacing(path, |tmp| {
            let mut file = std::io::BufWriter::new(std::fs::File::create(tmp)?);
            write_tiff(&mut file, image, metadata, icc, encoding.tiff_compression)?;
            use std::io::Write;
            Ok(file.flush()?)
        }),
        other => Err(CoreError::InvalidState(format!("{other:?} cannot carry metadata"))),
    }
}

/// Encode `image` as PNG to `w` with `eXIf`, XMP in `iTXt`, and `icc` in
/// `iCCP` or, with none, an `sRGB` chunk.
fn write_png(
    w: impl std::io::Write,
    image: &Pixels,
    exif: Vec<u8>,
    xmp: Option<&str>,
    icc: Option<&[u8]>,
) -> Result<()> {
    let png_err =
        |e: png::EncodingError| CoreError::Encode { format: "PNG", message: e.to_string() };
    let mut info = png::Info::with_size(image.width, image.height);
    info.exif_metadata = Some(exif.into());
    info.icc_profile = icc.map(Into::into);
    let mut encoder = png::Encoder::with_info(w, info).map_err(png_err)?;
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::Filter::Adaptive);
    // The encoder drops `iCCP` when `sRGB` is set.
    if icc.is_none() {
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    }
    if let Some(xmp) = xmp {
        encoder.add_itxt_chunk("XML:com.adobe.xmp".into(), xmp.into()).map_err(png_err)?;
    }
    let writer = match image.data {
        Samples::U8(bytes) => {
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(png_err)?;
            writer.write_image_data(bytes).map_err(png_err)?;
            writer
        }
        Samples::U16(samples) => {
            use std::io::Write as _;
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().map_err(png_err)?;
            {
                let mut stream = writer.stream_writer().map_err(png_err)?;
                let mut chunk_buf = vec![0u8; 1 << 16];
                for chunk in samples.chunks(1 << 15) {
                    let out = &mut chunk_buf[..chunk.len() * 2];
                    for (o, &v) in out.as_chunks_mut::<2>().0.iter_mut().zip(chunk) {
                        *o = v.to_be_bytes();
                    }
                    stream.write_all(out).map_err(CoreError::Io)?;
                }
                stream.finish().map_err(png_err)?;
            }
            writer
        }
    };
    writer.finish().map_err(png_err)
}

/// How a save encodes its pixels, by format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoding {
    /// JPEG quality, 1–100.
    pub jpeg_quality: u8,
    pub tiff_compression: TiffCompression,
}

impl Default for Encoding {
    fn default() -> Self {
        Self { jpeg_quality: JPEG_QUALITY, tiff_compression: TiffCompression::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb};

    #[test]
    fn rgba_f32_row_drops_alpha_and_clamps() {
        let floats = [0.0f32, 0.5, 1.0, 0.25, -1.0, 2.0, 1.0 / 65535.0, 1.0];
        let mut rgb = [0; 6];
        rgba_row_to_rgb_u16(
            bytemuck::cast_slice(&floats),
            wgpu::TextureFormat::Rgba32Float,
            &mut rgb,
        );
        assert_eq!(rgb, [0, 32768, 65535, 0, 65535, 1]);
    }

    /// A slow gray ramp, a little over four 8-bit codes across 4096
    /// pixels and 16 rows.
    fn slow_ramp() -> (Vec<u16>, u32) {
        let w = 4096u32;
        let row: Vec<u16> =
            (0..w).flat_map(|x| [20_000 + (x * 1100 / w) as u16; 3]).collect::<Vec<_>>();
        (row.repeat(16), w)
    }

    /// The gray of each 8×8 block along the top 8 rows, summed (64 times
    /// its mean), as the eye sees the ramp from a little way off.
    fn block_sums(bytes: &[u8], w: u32) -> Vec<u32> {
        let w = w as usize;
        (0..w / 8)
            .map(|b| {
                let px = (0..8).flat_map(|y| (0..8).map(move |x| (y * w + b * 8 + x) * 3));
                px.map(|i| u32::from(bytes[i])).sum()
            })
            .collect()
    }

    /// The longest run of equal values.
    fn longest_run(values: &[u32]) -> usize {
        values.chunk_by(|a, b| a == b).map(<[u32]>::len).max().unwrap()
    }

    /// Rounded, a slow ramp bands; dithered, it steps a 64th of a code at a
    /// time and each block averages to the 16-bit value.
    #[test]
    fn dither_breaks_up_bands_on_a_slow_ramp() {
        let (rgb, w) = slow_ramp();
        let rounded: Vec<u8> = rgb.iter().map(|&v| u16_to_u8(v)).collect();
        let bands = block_sums(&rounded, w);
        let distinct = |v: &[u32]| v.chunk_by(|a, b| a == b).count();
        assert!(distinct(&bands) < 10 && longest_run(&bands) > 100, "{bands:?}");
        let dithered = dither_rgb_to_u8(&rgb, w);
        let steps = block_sums(&dithered, w);
        assert!(distinct(&steps) > 250, "{}", distinct(&steps));
        assert!(longest_run(&steps) <= 3, "{}", longest_run(&steps));
        for (b, &got) in steps.iter().enumerate() {
            let want: f64 = (0..8)
                .flat_map(|y| (0..8).map(move |x| (y * w as usize + b * 8 + x) * 3))
                .map(|i| f64::from(rgb[i]) * 255.0 / 65535.0)
                .sum();
            assert!((f64::from(got) - want).abs() / 64.0 < 0.1, "block {b}: {got} for {want}");
        }
        // A pixel's three channels share their offset: a gray stays gray.
        assert!(dithered.chunks(3).all(|p| p[0] == p[1] && p[1] == p[2]));
    }

    /// Dither is deterministic, in bytes and saved files, and keeps exact
    /// 8-bit codes.
    #[test]
    fn dither_is_deterministic_and_keeps_exact_codes() {
        let (rgb, w) = slow_ramp();
        assert_eq!(dither_rgb_to_u8(&rgb, w), dither_rgb_to_u8(&rgb, w));
        let exact: Vec<u16> = (0..=255u16).flat_map(|k| [k * 257; 3]).collect::<Vec<_>>().repeat(8);
        let want: Vec<u8> = exact.iter().map(|&v| u16_to_u8(v)).collect();
        assert_eq!(dither_rgb_to_u8(&exact, 256), want);
        let mut ranks: Vec<u8> = BAYER8.iter().flatten().copied().collect();
        ranks.sort_unstable();
        assert_eq!(ranks, (0..64).collect::<Vec<u8>>(), "each rank once");

        let dir = tempfile::tempdir().unwrap();
        let save = |name: &str| {
            let path = dir.path().join(name);
            let meta = ExportMetadata::default();
            save_rgb16(&path, &rgb, w, 16, false, Encoding::default(), &meta).unwrap();
            std::fs::read(path).unwrap()
        };
        for ext in ["png", "jpg", "tif"] {
            assert_eq!(save(&format!("a.{ext}")), save(&format!("b.{ext}")), "{ext}");
        }
        let png = image::open(dir.path().join("a.png")).unwrap();
        assert_eq!(png.as_rgb8().unwrap().as_raw(), &dither_rgb_to_u8(&rgb, w));
    }

    #[test]
    fn rgba16_row_drops_alpha() {
        let codes = [1u16, 2, 65535, 9, 0, 32768, 7, 0];
        let mut rgb = [0; 6];
        rgba_row_to_rgb_u16(
            bytemuck::cast_slice(&codes),
            wgpu::TextureFormat::Rgba16Unorm,
            &mut rgb,
        );
        assert_eq!(rgb, [1, 2, 65535, 0, 32768, 7]);
    }

    /// Saves carry EXIF and XMP in every format and depth, with the pixels
    /// of `image`'s own save.
    #[test]
    fn saves_carry_exif_and_xmp() {
        use super::super::metadata::{Field, TAG_MODEL, Value};
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (21u32, 9u32);
        let rgb: Vec<u16> =
            (0..w * h * 3).map(|i| (i.wrapping_mul(2654435761) >> 16) as u16).collect();
        let rgb8 = ImageBuffer::<Rgb<u8>, _>::from_raw(w, h, dither_rgb_to_u8(&rgb, w)).unwrap();
        let xmp = "<?xpacket begin=\"\u{FEFF}\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>\
                   <?xpacket end=\"w\"?>";
        let metadata = ExportMetadata {
            ifd0: vec![Field::new(TAG_MODEL, Value::Ascii("OM-1".into()))],
            software: "Deptheos test".into(),
            modified: "2026:10:31 09:00:00".into(),
            comment: Some("Focus stack of 3 frames".into()),
            xmp: Some(xmp.into()),
            ..Default::default()
        };
        let check_exif = |path: &Path| {
            let file = std::fs::File::open(path).unwrap();
            let exif = exif::Reader::new()
                .read_from_container(&mut std::io::BufReader::new(file))
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let get = |tag| {
                let field = exif.get_field(tag, exif::In::PRIMARY);
                field
                    .unwrap_or_else(|| panic!("{}: no {tag}", path.display()))
                    .display_value()
                    .to_string()
            };
            assert_eq!(get(exif::Tag::Model), "\"OM-1\"");
            assert_eq!(get(exif::Tag::Software), "\"Deptheos test\"");
            assert_eq!(get(exif::Tag::PixelXDimension), w.to_string());
            let bytes = std::fs::read(path).unwrap();
            assert!(bytes.windows(xmp.len()).any(|win| win == xmp.as_bytes()), "no XMP");
        };
        // The `image` crate's save of `image`, at our JPEG quality.
        let save_owned = |path: &Path, image: DynamicImage| {
            if path.extension().is_some_and(|e| e == "jpg") {
                let writer = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
                let encoder =
                    image::codecs::jpeg::JpegEncoder::new_with_quality(writer, JPEG_QUALITY);
                image.write_with_encoder(encoder).unwrap();
            } else {
                image.save(path).unwrap();
            }
        };
        let same_pixels = |a: &Path, b: &Path, what: &str| {
            let (a, b) = (image::open(a).unwrap(), image::open(b).unwrap());
            assert_eq!(a.color(), b.color(), "{what}");
            assert!(a.as_bytes() == b.as_bytes(), "{what}: pixels differ");
        };
        for ext in ["png", "tif", "jpg"] {
            let (tagged, owned) =
                (dir.path().join(format!("t.{ext}")), dir.path().join(format!("o.{ext}")));
            for sixteen in [true, false] {
                save_rgb16(&tagged, &rgb, w, h, sixteen, Encoding::default(), &metadata).unwrap();
                check_exif(&tagged);
                let image = if sixteen && ext != "jpg" {
                    DynamicImage::ImageRgb16(ImageBuffer::from_raw(w, h, rgb.clone()).unwrap())
                } else {
                    DynamicImage::ImageRgb8(rgb8.clone())
                };
                save_owned(&owned, image);
                same_pixels(&tagged, &owned, &format!(".{ext} 16-bit {sixteen}"));
            }
        }
    }

    /// A non-sRGB result carries its profile in every format, EXIF
    /// `ColorSpace` uncalibrated, no PNG `sRGB` chunk; a large profile
    /// spans several JPEG segments.
    #[test]
    fn saves_carry_the_color_profile() {
        use super::super::profile::IccProfile;
        use image::ImageDecoder;
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (24u32, 16u32);
        let rgb: Vec<u16> = (0..w * h * 3).map(|i| (i * 97 % 65536) as u16).collect();
        let mut large = IccProfile::adobe_rgb_compatible().bytes().to_vec();
        large.resize(150_000, 7);
        let profiles = [IccProfile::adobe_rgb_compatible(), IccProfile::from_bytes(large).unwrap()];
        // TIFF through `tiff`: `image` caps a tag at the pixel buffer's size.
        let read_icc = |path: &Path| {
            if path.extension().is_some_and(|e| e == "tif") {
                let file = std::fs::File::open(path).unwrap();
                let mut tiff = tiff::decoder::Decoder::new(file).unwrap();
                return tiff.get_tag_u8_vec(tiff::tags::Tag::IccProfile).ok();
            }
            let reader = image::ImageReader::open(path).unwrap().with_guessed_format().unwrap();
            reader.into_decoder().unwrap().icc_profile().unwrap()
        };
        let color_space = |path: &Path| {
            let file = std::fs::File::open(path).unwrap();
            let exif = exif::Reader::new()
                .read_from_container(&mut std::io::BufReader::new(file))
                .unwrap();
            let field = exif.get_field(exif::Tag::ColorSpace, exif::In::PRIMARY).unwrap();
            field.value.get_uint(0).unwrap()
        };
        for icc in profiles {
            let metadata = ExportMetadata { icc: Some(icc.clone()), ..Default::default() };
            for ext in ["png", "tif", "jpg"] {
                let path = dir.path().join(format!("p.{ext}"));
                save_rgb16(&path, &rgb, w, h, true, Encoding::default(), &metadata).unwrap();
                let what = format!(".{ext}, a {}-byte profile", icc.bytes().len());
                assert_eq!(read_icc(&path).as_deref(), Some(icc.bytes()), "{what}");
                assert_eq!(color_space(&path), 0xFFFF, "{what}");
                if ext == "png" {
                    let bytes = std::fs::read(&path).unwrap();
                    assert!(!bytes.windows(4).any(|c| c == b"sRGB"), "{what}: an sRGB chunk");
                }
            }
        }
    }

    /// Every TIFF compression round-trips at both depths (with a short last
    /// strip), the compressed ones smaller on smooth data.
    #[test]
    fn tiff_compressions_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (301u32, 1747u32);
        let rgb: Vec<u16> = (0..w * h * 3)
            .map(|i| {
                let (p, c) = (i / 3, i % 3);
                ((p % w) * 97 + (p / w) * 31 + c * 5000 + (i % 7)) as u16
            })
            .collect();
        let mut sizes = Vec::new();
        for compression in [TiffCompression::None, TiffCompression::Lzw, TiffCompression::Zip] {
            let encoding = Encoding { tiff_compression: compression, ..Default::default() };
            let path = dir.path().join(format!("{compression:?}.tif"));
            for sixteen in [true, false] {
                save_rgb16(&path, &rgb, w, h, sixteen, encoding, &ExportMetadata::default())
                    .unwrap();
                let back = image::open(&path).unwrap();
                if sixteen {
                    assert!(back.as_rgb16().unwrap().as_raw() == &rgb, "{compression:?} 16-bit");
                    sizes.push(std::fs::metadata(&path).unwrap().len());
                } else {
                    let want = dither_rgb_to_u8(&rgb, w);
                    assert!(back.as_rgb8().unwrap().as_raw() == &want, "{compression:?} 8-bit");
                }
            }
        }
        assert!(sizes[1] < sizes[0] && sizes[2] < sizes[1], "sizes {sizes:?}");
    }

    /// `.jfif` is JPEG: a 16-bit result saves to it at 8 bits.
    #[test]
    fn a_16_bit_result_saves_as_jfif() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.JFIF");
        let rgb = vec![30000u16; 16 * 16 * 3];
        save_rgb16(&path, &rgb, 16, 16, true, Encoding::default(), &ExportMetadata::default())
            .unwrap();
        let back = image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.color(), image::ColorType::Rgb8);
    }

    /// A save writes through a link, and keeps permissions and (Apple)
    /// extended attributes, but not a stale resource fork.
    #[cfg(unix)]
    #[test]
    fn a_save_writes_through_links_and_keeps_attributes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.png");
        let link = dir.path().join("link.png");
        let save = |path: &Path, v: u16| {
            let rgb = vec![v; 4 * 4 * 3];
            save_rgb16(path, &rgb, 4, 4, true, Encoding::default(), &ExportMetadata::default())
                .unwrap();
        };
        save(&real, 1000);
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        #[cfg(target_vendor = "apple")]
        let set = |name: &str, value: &[u8]| {
            let path = std::ffi::CString::new(real.to_str().unwrap()).unwrap();
            let name = std::ffi::CString::new(name).unwrap();
            let ok = unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                    0,
                )
            };
            assert_eq!(ok, 0, "{}", std::io::Error::last_os_error());
        };
        #[cfg(target_vendor = "apple")]
        {
            set("com.example.tag", b"Red");
            set("com.apple.ResourceFork", b"old preview");
        }
        std::os::unix::fs::symlink("real.png", &link).unwrap();

        save(&link, 2000);
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "link kept");
        let back = image::open(&real).unwrap().into_rgb16();
        assert_eq!(back.get_pixel(0, 0).0, [2000; 3], "the link's target was replaced");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "permissions kept");
        #[cfg(target_vendor = "apple")]
        {
            let get = |name: &str| {
                let path = std::ffi::CString::new(real.to_str().unwrap()).unwrap();
                let name = std::ffi::CString::new(name).unwrap();
                let mut buf = [0u8; 64];
                let n = unsafe {
                    libc::getxattr(
                        path.as_ptr(),
                        name.as_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        0,
                        0,
                    )
                };
                (n >= 0).then(|| buf[..n as usize].to_vec())
            };
            assert_eq!(get("com.example.tag").as_deref(), Some(&b"Red"[..]), "attribute kept");
            assert_eq!(get("com.apple.ResourceFork"), None, "stale resource fork dropped");
        }
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 2, "no temporary file left behind");
    }

    /// A failed save leaves the existing file and no temporary.
    #[test]
    fn a_failed_save_keeps_the_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.png");
        let rgb = vec![1000u16; 4 * 4 * 3];
        save_rgb16(&path, &rgb, 4, 4, true, Encoding::default(), &ExportMetadata::default())
            .unwrap();
        let before = std::fs::read(&path).unwrap();

        // Too few samples: the PNG encoder fails after writing the file.
        // Below save_rgb16, which checks the count.
        let short = Pixels { data: Samples::U16(&rgb[..rgb.len() / 2]), width: 4, height: 4 };
        let failed = save_tagged(
            &path,
            ImageFormat::Png,
            &short,
            Encoding::default(),
            &ExportMetadata::default(),
        );
        assert!(failed.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1, "a temporary file was left behind");
    }

    /// An image with no width or height is refused in every format, and
    /// nothing is written.
    #[test]
    fn a_zero_dimension_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        for ext in ["png", "tif", "jpg"] {
            let path = dir.path().join(format!("empty.{ext}"));
            for (w, h) in [(0u32, 64u32), (0, 9), (0, 0), (64, 0)] {
                let rgb = vec![0u16; (w * h * 3) as usize];
                let saved =
                    save_rgb16(&path, &rgb, w, h, false, Encoding::default(), &Default::default());
                assert!(matches!(saved, Err(CoreError::EmptyImage)), ".{ext} {w}x{h}");
            }
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "a file was written");
    }
}
