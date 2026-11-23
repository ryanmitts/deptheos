//! A JPEG's IFD1 thumbnail, read by `kamadak-exif` without touching the
//! compressed image data.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use exif::{In, Tag};

/// How far to look for the EXIF block (normally in the first tens of KB),
/// so a JPEG without one isn't read to its end.
const SEARCH_LIMIT: u64 = 1 << 20;

/// The JPEG at `path`'s EXIF thumbnail, as JPEG bytes, if any.
pub(super) fn read(path: &Path) -> Option<Vec<u8>> {
    let mut reader = BufReader::new(File::open(path).ok()?.take(SEARCH_LIMIT));
    // Errors on anything but a JPEG, before reading past its first bytes.
    let tiff = exif::get_exif_attr_from_jpeg(&mut reader).ok()?;
    let exif =
        exif::Reader::new().read_raw(tiff).or_else(|e| e.distill_partial_result(|_| {})).ok()?;
    let uint = |tag| exif.get_field(tag, In::THUMBNAIL)?.value.get_uint(0);
    if uint(Tag::Compression).is_some_and(|c| c != 6) {
        return None; // an uncompressed (TIFF-style) thumbnail
    }
    let offset = uint(Tag::JPEGInterchangeFormat)? as usize;
    let length = uint(Tag::JPEGInterchangeFormatLength)? as usize;
    let jpeg = exif.buf().get(offset..offset.checked_add(length)?)?;
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    Some(jpeg.to_vec())
}
