//! JPEG output: the quality settings, the banded parallel encode, and the
//! metadata segments written into it.

use image::{ExtendedColorType, ImageEncoder};
use rayon::prelude::*;

use crate::error::{CoreError, Result};

/// JPEG quality for 8-bit outputs: `image`'s default 75 rings around the
/// edges a stacking comparison looks at; 92 keeps that below the stackers'.
pub const JPEG_QUALITY: u8 = 92;

/// The JPEG qualities there are; a save clamps into them.
pub const JPEG_QUALITY_RANGE: std::ops::RangeInclusive<u8> = 1..=100;

/// The APP1 identifier of an XMP packet in a JPEG.
const JPEG_XMP_ID: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// The most payload one JPEG segment holds (its 16-bit length counts
/// itself).
const JPEG_SEGMENT_MAX: usize = 65533;

/// The APP2 identifier of an ICC profile in a JPEG.
const JPEG_ICC_ID: &[u8] = b"ICC_PROFILE\0";

/// `jpeg` with an Exif APP1, an XMP APP1 when it fits one segment, and
/// `icc` across numbered APP2 segments (ICC.1 Annex B), after SOI and any
/// JFIF APP0.
pub(super) fn jpeg_with_metadata(
    jpeg: &[u8],
    exif: &[u8],
    xmp: Option<&str>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let malformed = || CoreError::InvalidState("unexpected JPEG encoder output".into());
    if jpeg.len() < 4 || jpeg[..2] != [0xFF, 0xD8] {
        return Err(malformed());
    }
    let mut at = 2;
    if jpeg[2..4] == [0xFF, 0xE0] {
        at += jpeg_segment_len(jpeg, 2);
    }
    let mut segments = Vec::new();
    let mut push_marker = |marker: u8, id: &[u8], payload: &[u8]| {
        let len = 2 + id.len() + payload.len();
        segments.extend_from_slice(&[0xFF, marker]);
        segments.extend_from_slice(&(len as u16).to_be_bytes());
        segments.extend_from_slice(id);
        segments.extend_from_slice(payload);
    };
    let mut push = |id: &[u8], payload: &[u8]| push_marker(0xE1, id, payload);
    if exif.len() + 6 <= JPEG_SEGMENT_MAX {
        push(b"Exif\0\0", exif);
    } else {
        tracing::warn!("EXIF block of {} bytes is too large for JPEG; left out", exif.len());
    }
    if let Some(xmp) = xmp {
        if xmp.len() + JPEG_XMP_ID.len() <= JPEG_SEGMENT_MAX {
            push(JPEG_XMP_ID, xmp.as_bytes());
        } else {
            tracing::warn!("XMP packet of {} bytes is too large for JPEG; left out", xmp.len());
        }
    }
    if let Some(icc) = icc {
        // The identifier, then this segment's number and the count.
        let chunks: Vec<&[u8]> = icc.chunks(JPEG_SEGMENT_MAX - JPEG_ICC_ID.len() - 2).collect();
        if chunks.len() <= 255 {
            for (i, chunk) in chunks.iter().enumerate() {
                let mut id = JPEG_ICC_ID.to_vec();
                id.extend_from_slice(&[i as u8 + 1, chunks.len() as u8]);
                push_marker(0xE2, &id, chunk);
            }
        } else {
            tracing::warn!("ICC profile of {} bytes is too large for JPEG; left out", icc.len());
        }
    }
    let mut out = Vec::with_capacity(jpeg.len() + segments.len());
    out.extend_from_slice(&jpeg[..at]);
    out.extend_from_slice(&segments);
    out.extend_from_slice(&jpeg[at..]);
    Ok(out)
}

/// Encode 8-bit RGB as a baseline JPEG at `quality`, in parallel: bands of
/// whole 8×8 block rows encoded alone, joined into one scan with restart
/// markers. The `image` encoder is single-threaded (≈ 0.5 s at 36 MP) with
/// fixed tables, and a restart resets DC prediction, so the result decodes
/// to the same pixels as a single-threaded encode.
pub(super) fn encode_jpeg(bytes: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>> {
    // A JPEG frame header holds the height in 16 bits.
    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return Err(CoreError::JpegTooLarge { width, height });
    }
    let quality = quality.clamp(*JPEG_QUALITY_RANGE.start(), *JPEG_QUALITY_RANGE.end());
    let color = ExtendedColorType::Rgb8;
    let channels = color.channel_count() as usize;
    let encode = |rows: &[u8], band_height: u32| -> Result<Vec<u8>> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).write_image(
            rows,
            width,
            band_height,
            color,
        )?;
        Ok(out)
    };
    // A restart interval counts MCUs in 16 bits.
    let blocks_per_row = width.div_ceil(8);
    let block_rows = height.div_ceil(8);
    let max_rows_per_band = (u32::from(u16::MAX) / blocks_per_row.max(1)).max(1);
    let target_bands = (rayon::current_num_threads() as u32 * 2).max(1);
    let rows_per_band = block_rows.div_ceil(target_bands).clamp(1, max_rows_per_band);
    if blocks_per_row > u32::from(u16::MAX) || rows_per_band >= block_rows {
        return encode(bytes, height);
    }
    let band_height = rows_per_band * 8;
    let row_bytes = width as usize * channels;
    let bands: Vec<Vec<u8>> = bytes
        .par_chunks(row_bytes * band_height as usize)
        .map(|rows| encode(rows, (rows.len() / row_bytes) as u32))
        .collect::<Result<_>>()?;

    let malformed = || CoreError::InvalidState("unexpected JPEG encoder output".into());
    let (header, _) = split_jpeg(&bands[0]).ok_or_else(malformed)?;
    let mut out = Vec::with_capacity(bands.iter().map(Vec::len).sum::<usize>() + 64);
    // Patch the frame height to the full image; DRI before SOS.
    let sos = find_marker(header, 0xDA).ok_or_else(malformed)?;
    out.extend_from_slice(&header[..sos]);
    let sof = find_marker(&out, 0xC0).ok_or_else(malformed)?;
    out[sof + 5..sof + 7].copy_from_slice(&(height as u16).to_be_bytes());
    let interval = (rows_per_band * blocks_per_row) as u16;
    out.extend_from_slice(&[0xFF, 0xDD, 0x00, 0x04]);
    out.extend_from_slice(&interval.to_be_bytes());
    out.extend_from_slice(&header[sos..]);
    for (i, band) in bands.iter().enumerate() {
        let (_, scan) = split_jpeg(band).ok_or_else(malformed)?;
        if i > 0 {
            out.extend_from_slice(&[0xFF, 0xD0 + ((i - 1) % 8) as u8]);
        }
        out.extend_from_slice(scan);
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

/// A single-scan JPEG split into everything up to and including the start
/// of scan segment, and the entropy-coded scan (without the end marker).
fn split_jpeg(jpeg: &[u8]) -> Option<(&[u8], &[u8])> {
    let sos = find_marker(jpeg, 0xDA)?;
    let scan_start = sos + jpeg_segment_len(jpeg, sos);
    let end =
        jpeg.len().checked_sub(2).filter(|&e| jpeg[e..] == [0xFF, 0xD9] && e >= scan_start)?;
    Some((&jpeg[..scan_start], &jpeg[scan_start..end]))
}

/// The offset of the first `0xFF marker` segment among the segments before
/// the scan, walking segment lengths from the start-of-image marker.
fn find_marker(jpeg: &[u8], marker: u8) -> Option<usize> {
    let mut at = 2; // after SOI
    while at + 4 <= jpeg.len() && jpeg[at] == 0xFF {
        if jpeg[at + 1] == marker {
            return Some(at);
        }
        if jpeg[at + 1] == 0xDA {
            return None;
        }
        at += jpeg_segment_len(jpeg, at);
    }
    None
}

/// The length of the segment at `at`, its marker included.
fn jpeg_segment_len(jpeg: &[u8], at: usize) -> usize {
    2 + u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The banded JPEG decodes to the single-threaded encode's pixels, with
    /// a short last band.
    #[test]
    fn banded_jpeg_matches_single_threaded_encode() {
        let (w, h) = (1203u32, 997u32);
        let rgb: Vec<u8> = (0..w * h * 3)
            .map(|i| {
                let (p, c) = (i / 3, i % 3);
                let (x, y) = (p % w, p / w);
                ((x * (c + 1) + y * 3 + (x * y) % 17) % 256) as u8
            })
            .collect();
        let banded = encode_jpeg(&rgb, w, h, JPEG_QUALITY).unwrap();
        let mut whole = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut whole, JPEG_QUALITY)
            .write_image(&rgb, w, h, ExtendedColorType::Rgb8)
            .unwrap();
        let banded = image::load_from_memory(&banded).unwrap();
        let whole = image::load_from_memory(&whole).unwrap();
        assert_eq!((banded.width(), banded.height()), (w, h));
        assert!(banded.as_bytes() == whole.as_bytes(), "pixels differ");
    }
}
