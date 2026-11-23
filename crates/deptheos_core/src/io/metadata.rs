//! The metadata written into exported results: EXIF tags (in a JPEG's
//! APP1, a PNG's `eXIf` chunk, or a TIFF's own directories) and an XMP
//! packet (JPEG APP1, PNG `iTXt`, TIFF tag 700).
//!
//! Callers fill [`ExportMetadata`] with tags copied from a source frame and
//! the XMP; the export's own tags (orientation, software, date, color
//! space, size) are added here and override copied ones.

/// One EXIF tag's value, in TIFF field types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// Text, written with its terminating NUL.
    Ascii(String),
    Byte(Vec<u8>),
    Short(Vec<u16>),
    Long(Vec<u32>),
    /// Numerator, denominator.
    Rational(Vec<[u32; 2]>),
    SRational(Vec<[i32; 2]>),
    Undefined(Vec<u8>),
}

impl Value {
    /// The TIFF field type number.
    pub fn kind(&self) -> u16 {
        match self {
            Self::Byte(_) => 1,
            Self::Ascii(_) => 2,
            Self::Short(_) => 3,
            Self::Long(_) => 4,
            Self::Rational(_) => 5,
            Self::Undefined(_) => 7,
            Self::SRational(_) => 10,
        }
    }

    /// The number of values, as an IFD entry counts them.
    pub fn count(&self) -> u32 {
        (match self {
            Self::Ascii(s) => s.len() + 1,
            Self::Byte(v) | Self::Undefined(v) => v.len(),
            Self::Short(v) => v.len(),
            Self::Long(v) => v.len(),
            Self::Rational(v) => v.len(),
            Self::SRational(v) => v.len(),
        }) as u32
    }

    /// The values' bytes, little-endian when `little` else big-endian.
    pub fn bytes(&self, little: bool) -> Vec<u8> {
        let u16b = |v: u16| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        match self {
            Self::Ascii(s) => s.bytes().chain([0]).collect(),
            Self::Byte(v) | Self::Undefined(v) => v.clone(),
            Self::Short(v) => v.iter().flat_map(|&x| u16b(x)).collect(),
            Self::Long(v) => v.iter().flat_map(|&x| u32b(x)).collect(),
            Self::Rational(v) => v.iter().flatten().flat_map(|&x| u32b(x)).collect(),
            Self::SRational(v) => v.iter().flatten().flat_map(|&x| u32b(x as u32)).collect(),
        }
    }
}

/// One EXIF tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub tag: u16,
    pub value: Value,
}

impl Field {
    pub fn new(tag: u16, value: Value) -> Self {
        Self { tag, value }
    }
}

/// What an export carries besides its pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportMetadata {
    /// IFD0 tags copied from a source frame (make, model, copyright).
    pub ifd0: Vec<Field>,
    /// Exif IFD tags copied from a source frame (exposure, lens, time).
    pub exif: Vec<Field>,
    /// The `Software` tag: the program that wrote the export.
    pub software: String,
    /// When the export was written, as EXIF `YYYY:MM:DD HH:MM:SS` in UTC
    /// (recorded with an `OffsetTime` of `+00:00`).
    pub modified: String,
    /// A short note for the Exif `UserComment`.
    pub comment: Option<String>,
    /// A complete XMP packet.
    pub xmp: Option<String>,
    /// The pixels' color space when not sRGB (JPEG APP2, PNG `iCCP`, TIFF
    /// tag 34675).
    pub icc: Option<super::profile::IccProfile>,
}

pub const TAG_MAKE: u16 = 0x010F;
pub const TAG_MODEL: u16 = 0x0110;
pub const TAG_ORIENTATION: u16 = 0x0112;
pub(crate) const TAG_SOFTWARE: u16 = 0x0131;
pub const TAG_DATE_TIME: u16 = 0x0132;
pub const TAG_ARTIST: u16 = 0x013B;
pub(crate) const TAG_XMP: u16 = 0x02BC;
/// TIFF only: the ICC profile, in the image directory.
pub(crate) const TAG_ICC_PROFILE: u16 = 0x8773;
pub const TAG_COPYRIGHT: u16 = 0x8298;
pub const TAG_EXIF_IFD: u16 = 0x8769;
pub(crate) const TAG_EXIF_VERSION: u16 = 0x9000;
pub(crate) const TAG_OFFSET_TIME: u16 = 0x9010;
pub(crate) const TAG_USER_COMMENT: u16 = 0x9286;
pub(crate) const TAG_COLOR_SPACE: u16 = 0xA001;
pub(crate) const TAG_PIXEL_X_DIMENSION: u16 = 0xA002;
pub(crate) const TAG_PIXEL_Y_DIMENSION: u16 = 0xA003;

impl ExportMetadata {
    /// IFD0's tags for an image (without the Exif IFD pointer), sorted.
    pub(crate) fn ifd0_fields(&self) -> Vec<Field> {
        let own = [
            Field::new(TAG_ORIENTATION, Value::Short(vec![1])),
            Field::new(TAG_SOFTWARE, Value::Ascii(self.software.clone())),
            Field::new(TAG_DATE_TIME, Value::Ascii(self.modified.clone())),
        ];
        merge(&self.ifd0, own)
    }

    /// The Exif IFD's tags for an image of `width` × `height`, sorted.
    pub(crate) fn exif_fields(&self, width: u32, height: u32) -> Vec<Field> {
        let mut own = vec![
            Field::new(TAG_EXIF_VERSION, Value::Undefined(b"0232".to_vec())),
            Field::new(TAG_OFFSET_TIME, Value::Ascii("+00:00".into())),
            // sRGB, or "uncalibrated": the embedded profile says what.
            Field::new(TAG_COLOR_SPACE, Value::Short(vec![self.color_space()])),
            Field::new(TAG_PIXEL_X_DIMENSION, Value::Long(vec![width])),
            Field::new(TAG_PIXEL_Y_DIMENSION, Value::Long(vec![height])),
        ];
        if let Some(comment) = &self.comment {
            own.push(Field::new(TAG_USER_COMMENT, user_comment(comment)));
        }
        merge(&self.exif, own)
    }

    /// EXIF `ColorSpace`: 1 for sRGB, 0xFFFF (uncalibrated) when the
    /// pixels are in another space, which the ICC profile names.
    fn color_space(&self) -> u16 {
        if self.icc.is_some() { 0xFFFF } else { 1 }
    }

    /// The EXIF block as a little-endian TIFF stream (what follows
    /// `Exif\0\0` in a JPEG's APP1, and a PNG's `eXIf` chunk).
    pub(crate) fn exif_tiff(&self, width: u32, height: u32) -> Vec<u8> {
        let ifd0 = self.ifd0_fields();
        let exif = self.exif_fields(width, height);
        // IFD0 at 8, with one more entry for the Exif IFD pointer; the Exif
        // IFD right after IFD0 and its values.
        let ifd0_size = ifd_size(ifd0.len() + 1, &ifd0);
        let exif_at = 8 + ifd0_size;
        let mut ifd0 = ifd0;
        ifd0.push(Field::new(TAG_EXIF_IFD, Value::Long(vec![exif_at])));
        ifd0.sort_by_key(|f| f.tag);

        let mut out = b"II*\0\x08\0\0\0".to_vec();
        write_ifd(&mut out, &ifd0);
        debug_assert_eq!(out.len() as u32, exif_at);
        write_ifd(&mut out, &exif);
        out
    }
}

/// `own` over `copied`: one field per tag, sorted by tag.
fn merge(copied: &[Field], own: impl IntoIterator<Item = Field>) -> Vec<Field> {
    let mut fields: std::collections::BTreeMap<u16, Field> =
        copied.iter().map(|f| (f.tag, f.clone())).collect();
    fields.extend(own.into_iter().map(|f| (f.tag, f)));
    fields.into_values().collect()
}

/// A `UserComment`: an 8-byte character code, then the text. ASCII when it
/// is, else UTF-16 in the stream's (little-endian) byte order.
fn user_comment(text: &str) -> Value {
    let mut bytes = Vec::new();
    if text.is_ascii() {
        bytes.extend_from_slice(b"ASCII\0\0\0");
        bytes.extend_from_slice(text.as_bytes());
    } else {
        bytes.extend_from_slice(b"UNICODE\0");
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    }
    Value::Undefined(bytes)
}

/// The bytes an IFD of `entries` entries takes with `fields`' values
/// (counting values past 4 bytes, each padded to an even length).
fn ifd_size(entries: usize, fields: &[Field]) -> u32 {
    let values: usize = fields
        .iter()
        .map(|f| f.value.bytes(true).len())
        .filter(|&n| n > 4)
        .map(|n| n + n % 2)
        .sum();
    (2 + 12 * entries + 4 + values) as u32
}

/// Append one IFD (no next IFD) with its out-of-line values after it.
/// Offsets count from the start of `out`, the TIFF stream.
fn write_ifd(out: &mut Vec<u8>, fields: &[Field]) {
    let start = out.len();
    let mut values_at = start + 2 + 12 * fields.len() + 4;
    let mut values = Vec::new();
    out.extend_from_slice(&(fields.len() as u16).to_le_bytes());
    for f in fields {
        let bytes = f.value.bytes(true);
        out.extend_from_slice(&f.tag.to_le_bytes());
        out.extend_from_slice(&f.value.kind().to_le_bytes());
        out.extend_from_slice(&f.value.count().to_le_bytes());
        if bytes.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..bytes.len()].copy_from_slice(&bytes);
            out.extend_from_slice(&inline);
        } else {
            out.extend_from_slice(&(values_at as u32).to_le_bytes());
            values.extend_from_slice(&bytes);
            if bytes.len() % 2 == 1 {
                values.push(0);
            }
            values_at += bytes.len() + bytes.len() % 2;
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&values);
}

/// The XML-escaped `text`, for an element's content or an attribute.
pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters other than tab and newlines are not
            // allowed in XML 1.0 at all.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// The current UTC time as EXIF `YYYY:MM:DD HH:MM:SS`.
pub fn exif_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    exif_date_time(secs)
}

/// `secs` since 1970 (UTC) as EXIF `YYYY:MM:DD HH:MM:SS`.
pub(crate) fn exif_date_time(secs: i64) -> String {
    let time = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default();
    time.format("%Y:%m:%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ExportMetadata {
        ExportMetadata {
            ifd0: vec![
                Field::new(TAG_MAKE, Value::Ascii("OM Digital Solutions".into())),
                Field::new(TAG_MODEL, Value::Ascii("OM-1".into())),
                // A copied orientation loses to the export's own.
                Field::new(TAG_ORIENTATION, Value::Short(vec![6])),
            ],
            exif: vec![
                Field::new(0x829A, Value::Rational(vec![[1, 200]])),
                Field::new(0x9003, Value::Ascii("2025:03:30 12:43:49".into())),
            ],
            software: "Deptheos 1.2.3".into(),
            modified: "2026:10:31 09:00:00".into(),
            comment: Some("Focus stack of 3 frames".into()),
            xmp: None,
            icc: None,
        }
    }

    #[test]
    fn exif_block_reads_back() {
        let tiff = sample().exif_tiff(640, 480);
        let exif = exif::Reader::new().read_raw(tiff).unwrap();
        let get = |tag: exif::Tag| {
            exif.get_field(tag, exif::In::PRIMARY).unwrap().display_value().to_string()
        };
        assert_eq!(get(exif::Tag::Make), "\"OM Digital Solutions\"");
        assert_eq!(get(exif::Tag::Model), "\"OM-1\"");
        assert_eq!(get(exif::Tag::Orientation), "row 0 at top and column 0 at left");
        assert_eq!(get(exif::Tag::Software), "\"Deptheos 1.2.3\"");
        assert_eq!(get(exif::Tag::DateTime), "2026-10-31 09:00:00");
        assert_eq!(get(exif::Tag::ExposureTime), "1/200");
        assert_eq!(get(exif::Tag::DateTimeOriginal), "2025-03-30 12:43:49");
        assert_eq!(get(exif::Tag::PixelXDimension), "640");
        assert_eq!(get(exif::Tag::PixelYDimension), "480");
        assert_eq!(get(exif::Tag::ColorSpace), "sRGB");
        let comment = exif.get_field(exif::Tag::UserComment, exif::In::PRIMARY).unwrap();
        let exif::Value::Undefined(comment, _) = &comment.value else { panic!() };
        assert_eq!(comment.as_slice(), b"ASCII\0\0\0Focus stack of 3 frames");
    }

    #[test]
    fn non_ascii_comment_is_utf16() {
        let Value::Undefined(bytes) = user_comment("Mücke") else { panic!() };
        assert_eq!(&bytes[..8], b"UNICODE\0");
        let units: Vec<u16> =
            bytes[8..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "Mücke");
    }

    #[test]
    fn dates_format() {
        assert_eq!(exif_date_time(0), "1970:01:01 00:00:00");
        assert_eq!(exif_date_time(1_743_338_629), "2025:03:30 12:43:49");
        assert_eq!(exif_date_time(951_782_400), "2000:02:29 00:00:00");
        assert_eq!(exif_date_time(-1), "1969:12:31 23:59:59");
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(xml_escape("a&b<c>\"d'\u{1}"), "a&amp;b&lt;c&gt;&quot;d&apos;");
    }
}
