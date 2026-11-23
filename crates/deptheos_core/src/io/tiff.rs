//! TIFF output: the compressions offered, and the encode with its strips
//! compressed in parallel.

use rayon::prelude::*;

use super::metadata::ExportMetadata;
use super::save::{Pixels, Samples};
use crate::error::{CoreError, Result};

/// How TIFF exports store their pixels. The compressed kinds use the
/// horizontal predictor, where most of their gain on photographs is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TiffCompression {
    /// Uncompressed: the largest files, the fastest to write and read.
    None,
    /// LZW, the most widely read compression.
    Lzw,
    /// ZIP (Adobe Deflate): smaller than LZW, above all at 16 bits.
    #[default]
    Zip,
}

impl TiffCompression {
    pub const ALL: [Self; 3] = [Self::Zip, Self::Lzw, Self::None];

    /// What the command line and the preferences call it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Lzw => "lzw",
            Self::None => "none",
        }
    }

    /// The compression `key` names, in any case; "deflate" is ZIP.
    pub fn from_key(key: &str) -> Option<Self> {
        if key.eq_ignore_ascii_case("deflate") {
            return Some(Self::Zip);
        }
        Self::ALL.into_iter().find(|c| c.key().eq_ignore_ascii_case(key))
    }
}

/// About one strip's uncompressed size, also the unit of parallel work.
const TIFF_STRIP_BYTES: usize = 1 << 20;

/// Encode `image` as a TIFF with `metadata`, XMP and `icc` in the image
/// directory and an Exif IFD. Strips compress in parallel; a file past
/// 4 GB (classic TIFF's offsets) is refused.
pub(super) fn write_tiff<W: std::io::Write + std::io::Seek>(
    w: &mut W,
    image: &Pixels,
    metadata: &ExportMetadata,
    icc: Option<&[u8]>,
    compression: TiffCompression,
) -> Result<()> {
    use super::metadata::{Field, TAG_ICC_PROFILE, TAG_XMP, Value};
    use tiff::encoder::{Rational, TiffEncoder, TiffKindStandard};
    use tiff::tags::{Tag, Type};
    let tiff_err =
        |e: tiff::TiffError| CoreError::Encode { format: "TIFF", message: e.to_string() };
    /// Add `fields` to `dir`, in the file's (native) byte order.
    fn write_fields<W: std::io::Write + std::io::Seek>(
        dir: &mut tiff::encoder::DirectoryEncoder<'_, W, TiffKindStandard>,
        fields: &[Field],
    ) -> tiff::TiffResult<()> {
        let little = cfg!(target_endian = "little");
        let mut entries = tiff::Directory::empty();
        for f in fields {
            let ty = Type::from_u16(f.value.kind()).expect("a TIFF field type");
            let entry = dir.write_entry_bytes(ty, &f.value.bytes(little))?;
            entries.extend([(Tag::from_u16_exhaustive(f.tag), entry)]);
        }
        dir.extend_from(&entries);
        Ok(())
    }

    let mut encoder = TiffEncoder::new(w).map_err(tiff_err)?;
    let exif_ifd = {
        let mut dir = encoder.extra_directory().map_err(tiff_err)?;
        write_fields(&mut dir, &metadata.exif_fields(image.width, image.height))
            .map_err(tiff_err)?;
        dir.finish_with_offsets().map_err(tiff_err)?.offset
    };
    let mut ifd0 = metadata.ifd0_fields();
    if let Some(xmp) = &metadata.xmp {
        ifd0.push(Field::new(TAG_XMP, Value::Byte(xmp.as_bytes().to_vec())));
    }
    if let Some(icc) = icc {
        ifd0.push(Field::new(TAG_ICC_PROFILE, Value::Undefined(icc.to_vec())));
    }
    let channels: u16 = 3;
    let bits: u16 = match image.data {
        Samples::U8(_) => 8,
        Samples::U16(_) => 16,
    };
    let row_bytes = image.width as usize * usize::from(channels) * usize::from(bits / 8);
    let rows_per_strip = TIFF_STRIP_BYTES.div_ceil(row_bytes).clamp(1, image.height as usize);
    let too_large = |_| CoreError::TiffTooLarge;
    let mut dir = encoder.image_directory().map_err(tiff_err)?;
    let mut offsets = Vec::new();
    let mut counts = Vec::new();
    if compression == TiffCompression::None {
        let strip_bytes = row_bytes * rows_per_strip;
        let all_bytes: &[u8] = match image.data {
            Samples::U8(b) => b,
            Samples::U16(s) => bytemuck::cast_slice(s),
        };
        for strip in all_bytes.chunks(strip_bytes) {
            let at = dir.write_data(strip).map_err(tiff_err)?;
            offsets.push(u32::try_from(at).map_err(too_large)?);
            counts.push(u32::try_from(strip.len()).map_err(too_large)?);
        }
    } else {
        let strips = match image.data {
            Samples::U8(b) => tiff_strips(b, image.width, channels, rows_per_strip, compression),
            Samples::U16(s) => tiff_strips(s, image.width, channels, rows_per_strip, compression),
        }?;
        offsets.reserve(strips.len());
        counts.reserve(strips.len());
        for strip in &strips {
            let at = dir.write_data(strip.as_slice()).map_err(tiff_err)?;
            offsets.push(u32::try_from(at).map_err(too_large)?);
            counts.push(u32::try_from(strip.len()).map_err(too_large)?);
        }
    }
    let (method, predictor): (u16, u16) = match compression {
        TiffCompression::None => (1, 1),
        TiffCompression::Lzw => (5, 2),
        TiffCompression::Zip => (8, 2),
    };
    let mut tags = || -> tiff::TiffResult<()> {
        dir.write_tag(Tag::ImageWidth, image.width)?;
        dir.write_tag(Tag::ImageLength, image.height)?;
        dir.write_tag(Tag::BitsPerSample, &vec![bits; channels.into()][..])?;
        dir.write_tag(Tag::Compression, method)?;
        dir.write_tag(Tag::PhotometricInterpretation, 2u16)?;
        dir.write_tag(Tag::StripOffsets, &offsets[..])?;
        dir.write_tag(Tag::SamplesPerPixel, channels)?;
        dir.write_tag(Tag::RowsPerStrip, rows_per_strip as u32)?;
        dir.write_tag(Tag::StripByteCounts, &counts[..])?;
        // No physical size: one unit per pixel, as the `image` crate writes.
        dir.write_tag(Tag::XResolution, Rational { n: 1, d: 1 })?;
        dir.write_tag(Tag::YResolution, Rational { n: 1, d: 1 })?;
        dir.write_tag(Tag::PlanarConfiguration, 1u16)?;
        dir.write_tag(Tag::ResolutionUnit, 1u16)?;
        dir.write_tag(Tag::Predictor, predictor)?;
        dir.write_tag(Tag::SampleFormat, &vec![1u16; channels.into()][..])?;
        write_fields(&mut dir, &ifd0)?;
        // LONG, as EXIF readers expect, rather than the IFD type.
        dir.write_tag(Tag::ExifDirectory, exif_ifd)?;
        Ok(())
    };
    tags().map_err(tiff_err)?;
    dir.finish().map_err(tiff_err)
}

/// A sample a TIFF strip is made of.
trait StripSample: Copy + Send + Sync + bytemuck::Pod {
    fn wrapping_sub(self, other: Self) -> Self;
}

impl StripSample for u8 {
    fn wrapping_sub(self, other: Self) -> Self {
        u8::wrapping_sub(self, other)
    }
}

impl StripSample for u16 {
    fn wrapping_sub(self, other: Self) -> Self {
        u16::wrapping_sub(self, other)
    }
}

/// `samples` cut into strips of `rows_per_strip` rows, in native byte
/// order, each through the horizontal predictor (TIFF 6.0 §14) and
/// compressed, in parallel.
fn tiff_strips<T: StripSample>(
    samples: &[T],
    width: u32,
    channels: u16,
    rows_per_strip: usize,
    compression: TiffCompression,
) -> Result<Vec<Vec<u8>>> {
    use tiff::encoder::compression::{CompressionAlgorithm, Deflate, DeflateLevel, Lzw};
    let row = width as usize * usize::from(channels);
    let step = usize::from(channels);
    samples
        .par_chunks(row * rows_per_strip)
        .map(|strip| {
            let mut predicted = strip.to_vec();
            for row in predicted.chunks_exact_mut(row) {
                for i in (step..row.len()).rev() {
                    row[i] = row[i].wrapping_sub(row[i - step]);
                }
            }
            let bytes: &[u8] = bytemuck::cast_slice(&predicted);
            let mut out = Vec::with_capacity(bytes.len() / 2);
            match compression {
                TiffCompression::Lzw => Lzw.write_to(&mut out, bytes)?,
                TiffCompression::Zip => {
                    Deflate::with_level(DeflateLevel::Balanced).write_to(&mut out, bytes)?
                }
                TiffCompression::None => unreachable!("uncompressed strips are written whole"),
            };
            Ok(out)
        })
        .collect()
}
