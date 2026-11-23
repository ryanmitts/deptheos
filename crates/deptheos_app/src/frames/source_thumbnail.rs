//! Small previews of source files for the file list, never written to
//! disk.
//!
//! A RAW or DNG thumbnail is its embedded preview (a few milliseconds; the
//! camera's rendering, so its color may differ slightly from the stack's).
//! Without a usable one, the raw data is rendered: half size with LibRaw,
//! or whole with the DNG SDK for DNGs LibRaw can't decode (lossy JPEG XL).
//! Adobe DNG Converter is never run.
//!
//! A JPEG uses its EXIF thumbnail when big enough and the image's shape
//! (editors leave stale ones after a crop; some cameras pad to 4:3), else
//! it is decoded at the smallest fitting eighth; other formats whole. Each
//! goes through the input stage with its image's color tag and orientation
//! (ADR-132, ADR-146) and is shown in sRGB.

use std::io::Read as _;
use std::path::Path;

use image::RgbImage;
use image::imageops::FilterType;
use thiserror::Error;

mod exif;

use crate::engine::present::CpuImage;
use deptheos_core::io;
use deptheos_raw::PreviewData;

#[derive(Error, Debug)]
pub enum ThumbnailError {
    #[error(transparent)]
    Raw(#[from] deptheos_raw::RawError),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Dng(#[from] deptheos_dng::DngError),
    #[error(transparent)]
    Core(#[from] deptheos_core::CoreError),
    #[error("{0}")]
    Unreadable(&'static str),
}

/// Where a thumbnail's pixels came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailSource {
    /// A preview embedded in a RAW or DNG file, or a JPEG's EXIF
    /// thumbnail.
    Embedded,
    /// The raw data rendered, for want of a usable preview.
    RawRender,
    /// A standard image (JPEG, PNG, TIFF), decoded whole.
    Image,
}

pub struct Thumbnail {
    /// BGRA8 straight alpha, at most the requested long edge.
    pub image: CpuImage,
    pub source: ThumbnailSource,
}

/// A thumbnail of `path`, at most `size` on its long edge, upright.
pub fn thumbnail(path: &Path, size: u32) -> Result<Thumbnail, ThumbnailError> {
    let size = size.max(1);
    let (rgb, source) = if io::is_raw_file(path) {
        let (rgb, source) = raw_thumbnail(path, size)?;
        (fit(rgb, size), source)
    } else {
        let (frame, source) = match exif_thumbnail(path, size) {
            Some(frame) => (frame, ThumbnailSource::Embedded),
            None => match reduced_jpeg(path, size) {
                Some(frame) => (frame, ThumbnailSource::Image),
                None => {
                    let raw = io::CameraRaw::here();
                    (
                        io::load_frame(path, io::FileCache::Use, raw, io::Target::Own)?,
                        ThumbnailSource::Image,
                    )
                }
            },
        };
        (in_srgb(fit(frame.image.into_rgb8(), size), frame.profile.as_ref()), source)
    };
    Ok(Thumbnail { image: CpuImage::from_rgb(&rgb), source })
}

/// Convert from `profile`'s space (`None`: sRGB) to sRGB.
fn in_srgb(rgb: RgbImage, profile: Option<&io::IccProfile>) -> RgbImage {
    let Some(to_srgb) = crate::engine::present::to_srgb(profile) else { return rgb };
    let (w, h) = rgb.dimensions();
    let wide: Vec<u16> = rgb.as_raw().iter().map(|&v| u16::from(v) * 257).collect();
    let mut out = vec![0u16; wide.len()];
    to_srgb.convert(&wide, &mut out);
    RgbImage::from_raw(w, h, out.into_iter().map(io::u16_to_u8).collect()).expect("same size")
}

/// A RAW or DNG file's embedded preview, or its raw data rendered small.
/// A preview under half `size` (some files carry only 160 px) is used only
/// if rendering fails.
fn raw_thumbnail(path: &Path, size: u32) -> Result<(RgbImage, ThumbnailSource), ThumbnailError> {
    let embedded =
        match deptheos_raw::read_preview(path, size).and_then(|p| decode_preview(path, p)) {
            Ok(rgb) if rgb.width().max(rgb.height()) * 2 >= size => {
                return Ok((rgb, ThumbnailSource::Embedded));
            }
            Ok(small) => Some(small),
            Err(e) => {
                tracing::debug!("{}: no usable embedded preview ({e})", path.display());
                None
            }
        };
    // LibRaw at half size first: each 2×2 mosaic block is one pixel, no
    // demosaic, where the SDK renders full size on the CPU. The SDK is for
    // DNGs LibRaw can't decode (lossy JPEG XL).
    let rendered =
        deptheos_core::cancel::codec(|budget| deptheos_raw::read_half_size(path, budget))
            .map(|(pixels, width, height)| {
                RgbImage::from_raw(width, height, pixels)
                    .expect("read_half_size returns width * height * 3 bytes")
            })
            .map_err(ThumbnailError::from)
            .or_else(|e| if io::is_dng(path) { sdk_render(path) } else { Err(e) });
    match (rendered, embedded) {
        (Ok(rgb), _) => Ok((rgb, ThumbnailSource::RawRender)),
        (Err(_), Some(small)) => Ok((small, ThumbnailSource::Embedded)),
        (Err(e), None) => Err(e),
    }
}

/// A DNG rendered whole by the SDK, upright: the last resort.
fn sdk_render(path: &Path) -> Result<RgbImage, ThumbnailError> {
    use deptheos_dng::{DngImage, DngRead, FileCache};
    let read = deptheos_core::cancel::codec(|budget| {
        DngImage::read_with_pixels(path, FileCache::Use, budget)
    })?;
    let DngRead::Linear(image, linear) = read else {
        // A CFA DNG reads as metadata only, and LibRaw already failed.
        return Err(ThumbnailError::Unreadable("DNG SDK returned no image"));
    };
    let srgb =
        image.render_linear_to_srgb(&linear, image.width(), image.height(), image.channels())?;
    let (width, height, channels) = (srgb.width(), srgb.height(), srgb.channels());
    let pixels: Vec<u8> = match channels {
        3 => srgb.pixels().iter().map(|&v| (v >> 8) as u8).collect(),
        1 => srgb.pixels().iter().flat_map(|&v| [(v >> 8) as u8; 3]).collect(),
        _ => return Err(ThumbnailError::Unreadable("DNG SDK returned no image")),
    };
    RgbImage::from_raw(width, height, pixels)
        .ok_or(ThumbnailError::Unreadable("DNG SDK image size mismatch"))
}

/// An embedded preview as upright RGB.
fn decode_preview(
    path: &Path,
    preview: deptheos_raw::Preview,
) -> Result<RgbImage, deptheos_raw::RawError> {
    let failed = |reason: String| deptheos_raw::RawError::DecodeFailed {
        path: path.display().to_string(),
        reason,
    };
    let rgb = match preview.data {
        PreviewData::Jpeg(bytes) => {
            image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
                .map_err(|e| failed(format!("embedded preview: {e}")))?
                .into_rgb8()
        }
        PreviewData::Rgb8(pixels) => RgbImage::from_raw(preview.width, preview.height, pixels)
            .ok_or_else(|| failed("embedded preview size mismatch".into()))?,
    };
    Ok(apply_flip(rgb, preview.flip))
}

/// Turn a preview upright by LibRaw's `flip` code (bit 2 swaps axes, bit 1
/// mirrors vertically, bit 0 horizontally).
fn apply_flip(rgb: RgbImage, flip: i32) -> RgbImage {
    use image::imageops::{flip_horizontal, flip_vertical, rotate90, rotate270};
    match flip {
        3 => image::imageops::rotate180(&rgb),
        5 => rotate270(&rgb),
        6 => rotate90(&rgb),
        1 => flip_horizontal(&rgb),
        2 => flip_vertical(&rgb),
        // Transpose and transverse: no camera writes them; leave as is.
        _ => rgb,
    }
}

/// A JPEG's EXIF thumbnail, if at least `size` and the image's stored shape
/// to within a pixel, given the image's color tag and orientation.
fn exif_thumbnail(path: &Path, size: u32) -> Option<io::Frame> {
    let jpeg = exif::read(path)?;
    let thumb = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok()?;
    let image = io::standard_header(path, io::CameraRaw::here()).ok()??;
    let (tw, th) = (thumb.width(), thumb.height());
    let (w, h) = (u64::from(image.width), u64::from(image.height));
    let skew = (u64::from(tw) * h).abs_diff(u64::from(th) * w);
    if tw.max(th) < size || skew > w.max(h) {
        return None;
    }
    as_image(path, image, thumb.into_rgb8().into_raw(), tw, th)
}

/// A JPEG decoded at the smallest eighth still at least `size`
/// (`deptheos_dng::jpeg`): 45 MP whole is 136 MB, an eighth 2 MB. `None`
/// for other formats or what libjpeg can't read.
fn reduced_jpeg(path: &Path, size: u32) -> Option<io::Frame> {
    // Check the magic first, so another format isn't read twice.
    let mut start = [0u8; 2];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut start)).ok()?;
    if start != [0xFF, 0xD8] {
        return None;
    }
    let image = io::standard_header(path, io::CameraRaw::here()).ok()??;
    let bytes = std::fs::read(path).ok()?;
    let (rgb, w, h) = deptheos_dng::jpeg::decode_scaled(&bytes, size)?;
    as_image(path, image, rgb, w, h)
}

/// Stored RGB8 `rgb` through the input stage with `image`'s color tag and
/// orientation.
fn as_image(
    path: &Path,
    image: io::DecodedHeader,
    rgb: Vec<u8>,
    w: u32,
    h: u32,
) -> Option<io::Frame> {
    let header = io::DecodedHeader {
        width: w,
        height: h,
        depth: io::StoredDepth::U8,
        layout: io::Layout::Rgb,
        ..image
    };
    let decoded = io::Decoded { header, samples: io::Samples::U8(rgb) };
    io::input_frame(path, decoded, io::Target::Own).ok()
}

/// Scale down to `size` on the long edge; never up.
fn fit(rgb: RgbImage, size: u32) -> RgbImage {
    let (w, h) = rgb.dimensions();
    let (tw, th) = crate::engine::present::fit_long_edge(w, h, size);
    if (tw, th) == (w, h) {
        return rgb;
    }
    // Box prefilter to twice the target, then a proper filter: Lanczos
    // sharpness at a fraction of the cost.
    let rgb = if w.min(h) > th.min(tw) * 4 {
        image::imageops::thumbnail(&rgb, tw * 2, th * 2)
    } else {
        rgb
    };
    image::imageops::resize(&rgb, tw, th, FilterType::CatmullRom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_images_fit_the_long_edge() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.png");
        RgbImage::from_pixel(600, 200, image::Rgb([10, 20, 30])).save(&path).unwrap();
        let thumb = thumbnail(&path, 120).unwrap();
        assert_eq!(thumb.source, ThumbnailSource::Image);
        assert_eq!((thumb.image.width, thumb.image.height), (120, 40));
        // BGRA, opaque.
        assert_eq!(&thumb.image.pixels[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn small_images_are_not_enlarged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.png");
        RgbImage::new(50, 80).save(&path).unwrap();
        let thumb = thumbnail(&path, 120).unwrap();
        assert_eq!((thumb.image.width, thumb.image.height), (50, 80));
    }

    #[test]
    fn flips_turn_the_preview_upright() {
        // 2×1: red then green.
        let mut rgb = RgbImage::new(2, 1);
        rgb.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        rgb.put_pixel(1, 0, image::Rgb([0, 255, 0]));
        // 90° clockwise: red ends on top.
        let cw = apply_flip(rgb.clone(), 6);
        assert_eq!(cw.dimensions(), (1, 2));
        assert_eq!(cw.get_pixel(0, 0)[0], 255);
        // 90° counter-clockwise: green on top.
        let ccw = apply_flip(rgb.clone(), 5);
        assert_eq!(ccw.get_pixel(0, 0)[1], 255);
        assert_eq!(apply_flip(rgb.clone(), 3).get_pixel(0, 0)[1], 255);
        assert_eq!(apply_flip(rgb, 0).get_pixel(0, 0)[0], 255);
    }

    /// A red JPEG with a blue EXIF thumbnail and `orientation`, behind an
    /// XMP APP1.
    fn jpeg_with_exif(
        dir: &Path,
        size: (u32, u32),
        thumb: (u32, u32),
        orientation: u16,
    ) -> std::path::PathBuf {
        let encode = |(w, h): (u32, u32), rgb: [u8; 3]| {
            let mut out = Vec::new();
            RgbImage::from_pixel(w, h, image::Rgb(rgb))
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
                .unwrap();
            out
        };
        let main = encode(size, [255, 0, 0]);
        let small = encode(thumb, [0, 0, 255]);
        // IFD0 (orientation), IFD1 (thumbnail offset, length), thumbnail.
        let entry = |tiff: &mut Vec<u8>, tag: u16, kind: u16, value: u32| {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&kind.to_le_bytes());
            tiff.extend_from_slice(&1u32.to_le_bytes());
            tiff.extend_from_slice(&value.to_le_bytes());
        };
        let ifd1 = 8 + 2 + 12 + 4;
        let thumb_at = ifd1 + 2 + 2 * 12 + 4;
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&1u16.to_le_bytes());
        entry(&mut tiff, 0x0112, 3, orientation.into());
        tiff.extend_from_slice(&(ifd1 as u32).to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut tiff, 0x0201, 4, thumb_at as u32);
        entry(&mut tiff, 0x0202, 4, small.len() as u32);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(tiff.len(), thumb_at);
        tiff.extend_from_slice(&small);
        let segment = |payload: &[u8]| {
            let mut seg = vec![0xFF, 0xE1];
            seg.extend_from_slice(&(payload.len() as u16 + 2).to_be_bytes());
            seg.extend_from_slice(payload);
            seg
        };
        let mut file = main[..2].to_vec();
        file.extend(segment(b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>"));
        file.extend(segment(&[b"Exif\0\0".as_slice(), &tiff].concat()));
        file.extend_from_slice(&main[2..]);
        let path = dir.join("photo.jpg");
        std::fs::write(&path, file).unwrap();
        path
    }

    #[test]
    fn jpegs_use_their_exif_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let path = jpeg_with_exif(dir.path(), (600, 400), (256, 171), 1);
        let thumb = thumbnail(&path, 128).unwrap();
        assert_eq!(thumb.source, ThumbnailSource::Embedded);
        assert_eq!((thumb.image.width, thumb.image.height), (128, 86));
        // Blue, BGRA.
        let p = &thumb.image.pixels[..3];
        assert!(p[0] > 200 && p[2] < 50, "{p:?}");
    }

    #[test]
    fn small_or_misshapen_exif_thumbnails_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // Too small for the size asked.
        let path = jpeg_with_exif(dir.path(), (600, 400), (96, 64), 1);
        assert_eq!(thumbnail(&path, 128).unwrap().source, ThumbnailSource::Image);
        // Padded to 4:3, or stale after a crop.
        let path = jpeg_with_exif(dir.path(), (600, 400), (160, 120), 1);
        let thumb = thumbnail(&path, 128).unwrap();
        assert_eq!(thumb.source, ThumbnailSource::Image);
        // Red: the image itself.
        assert!(thumb.image.pixels[2] > 200);
    }

    #[test]
    fn rotated_jpegs_are_upright() {
        let dir = tempfile::tempdir().unwrap();
        // Stored landscape, EXIF orientation 6 (turn 90 degrees clockwise).
        let path = jpeg_with_exif(dir.path(), (600, 400), (256, 171), 6);
        let thumb = thumbnail(&path, 128).unwrap();
        assert_eq!(thumb.source, ThumbnailSource::Embedded);
        assert_eq!((thumb.image.width, thumb.image.height), (86, 128));
        // No usable EXIF thumbnail: the whole image, turned the same way.
        let path = jpeg_with_exif(dir.path(), (600, 400), (96, 64), 6);
        let thumb = thumbnail(&path, 128).unwrap();
        assert_eq!(thumb.source, ThumbnailSource::Image);
        assert_eq!((thumb.image.width, thumb.image.height), (85, 128));
    }

    /// Without a usable EXIF thumbnail a JPEG decodes reduced and upright.
    #[test]
    fn jpegs_without_a_thumbnail_decode_reduced() {
        let dir = tempfile::tempdir().unwrap();
        let path = jpeg_with_exif(dir.path(), (1600, 1200), (96, 72), 6);
        if deptheos_dng::jpeg::decode_scaled(&std::fs::read(&path).unwrap(), 1).is_some() {
            // 1/8 is 200 × 150, under 256; 2/8 fits.
            let frame = reduced_jpeg(&path, 256).unwrap();
            assert_eq!((frame.image.width(), frame.image.height()), (300, 400));
            assert!(frame.image.into_rgb8().get_pixel(10, 10)[0] > 200, "red");
        }
        let thumb = thumbnail(&path, 256).unwrap();
        assert_eq!(thumb.source, ThumbnailSource::Image);
        assert_eq!((thumb.image.width, thumb.image.height), (192, 256));
        // A PNG is no JPEG: decoded whole.
        let png = dir.path().join("frame.png");
        RgbImage::new(64, 64).save(&png).unwrap();
        assert!(reduced_jpeg(&png, 32).is_none());
    }

    #[test]
    fn missing_raw_file_errors() {
        assert!(thumbnail(Path::new("/nonexistent/frame.cr3"), 120).is_err());
    }
}
