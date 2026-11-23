//! Capture time, camera and burst position, read from metadata with a few
//! small reads: what auto sort needs, fast enough for a thousand RAWs on a
//! slow SD card.
//!
//! Only the [`BLOCK`]s the metadata points at are read, up to
//! [`MAX_BYTES`] per file. An Adobe DNG's Exif IFD sits after its private
//! data, near 2 MB, which costs one more block.
//!
//! Formats:
//! - TIFF-based files (TIFF, DNG, NEF, ARW, CR2, …), and ORF and RW2, whose
//!   headers differ from TIFF's only in the magic number.
//! - Canon CR3: the `CMT1` (IFD0) and `CMT2` (Exif IFD) TIFF streams in
//!   Canon's `uuid` box inside `moov`.
//! - JPEG: the APP1 Exif block.
//! - Fujifilm RAF: the Exif block of the JPEG preview its header points at.
//! - HEIF/HEIC: the `Exif` item that `meta` lists (`iinf`) and locates
//!   (`iloc`).
//!
//! The burst shot number is OM System / Olympus only: maker note →
//! `CameraSettings` (0x2020) → `DriveMode` (0x0600) = `[mode, shot, …]`;
//! OM DNGs keep the maker note in `DNGPrivateData`.
//!
//! Anything malformed, truncated or past the budget reads as missing; only
//! an I/O failure is an error.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use deptheos_core::io::metadata::{
    Field, TAG_ARTIST, TAG_COPYRIGHT, TAG_EXIF_IFD, TAG_MAKE, TAG_MODEL, Value,
};

/// What [`read_capture_info`] found; files differ in what they record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureInfo {
    /// `DateTimeOriginal` (with its sub-seconds), else `DateTimeDigitized`.
    /// Never `DateTime`, which edits and exports rewrite.
    pub time: Option<CaptureTime>,
    /// The camera model (`Model`), trimmed.
    pub model: Option<String>,
    /// The camera's shot number within a burst or bracket (OM System /
    /// Olympus only; `None` for single shots).
    pub shot: Option<u16>,
}

/// A capture time on the camera's own clock, which records no time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CaptureTime {
    /// Whole seconds since 1970-01-01 00:00:00 on the camera's clock.
    pub seconds: i64,
    /// Microseconds, when recorded. `Some(0)` is a recorded zero, which
    /// some cameras write on every frame.
    pub micros: Option<u32>,
}

impl CaptureTime {
    /// Seconds, with the fraction.
    pub fn as_secs_f64(&self) -> f64 {
        self.seconds as f64 + f64::from(self.micros.unwrap_or(0)) / 1e6
    }
}

/// The capture info of the file at `path`.
pub(crate) fn read_capture_info(path: &Path) -> io::Result<CaptureInfo> {
    read_from(File::open(path)?)
}

/// The capture info of a file read through `reader`.
pub fn read_from<R: Read + Seek>(reader: R) -> io::Result<CaptureInfo> {
    let mut src = Source::new(reader)?;
    let mut found = Found::default();
    for base in tiff_streams(&mut src) {
        read_tiff(&mut src, base, &mut found);
    }
    let Found { mut info, digitized } = found;
    info.time = info.time.or(digitized);
    Ok(info)
}

/// Read so far. `digitized` stands in only when no stream has an original
/// time.
#[derive(Default)]
struct Found {
    info: CaptureInfo,
    digitized: Option<CaptureTime>,
}

/// Where the file's TIFF streams start (the file itself for TIFF-based
/// formats).
fn tiff_streams<R: Read + Seek>(src: &mut Source<R>) -> Vec<u64> {
    let Some(head) = src.bytes(0, 16.min(src.len as usize)) else { return Vec::new() };
    let brand = (head.len() >= 12 && &head[4..8] == b"ftyp").then(|| &head[8..12]);
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        jpeg_exif(src, 0).into_iter().collect()
    } else if head.starts_with(PNG_MAGIC) {
        png_exif(src).into_iter().collect()
    } else if brand == Some(b"crx ") {
        cr3_streams(src)
    } else if brand.is_some_and(|b| HEIF_BRANDS.contains(&b)) {
        heif_exif(src).into_iter().collect()
    } else if head.starts_with(RAF_MAGIC) {
        raf_exif(src).into_iter().collect()
    } else {
        vec![0]
    }
}

/// The tags an export copies from its reference frame
/// ([`deptheos_core::io::metadata`]): camera, lens, exposure and time. Not
/// GPS, maker notes, owner names, serial numbers, or what varies across a
/// focus bracket (the focus distance).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceTags {
    /// For IFD0: make, model, artist, copyright.
    pub ifd0: Vec<Field>,
    /// For the Exif IFD.
    pub exif: Vec<Field>,
}

/// The [`SourceTags`] of the file at `path`.
pub(crate) fn read_source_tags(path: &Path) -> io::Result<SourceTags> {
    let mut src = Source::new(File::open(path)?)?;
    let mut tags = SourceTags::default();
    for base in tiff_streams(&mut src) {
        if let Some((s, ifd0)) = tiff_header(&mut src, base) {
            collect_tags(&mut src, s, ifd0, &mut tags, 0);
        }
    }
    Ok(tags)
}

/// The file's metadata TIFF streams, each from its header on (up to
/// [`MAX_BYTES`]), for the end-to-end checker on formats it can't open.
pub fn metadata_streams(path: &Path) -> io::Result<Vec<Vec<u8>>> {
    let mut file = File::open(path)?;
    let bases = tiff_streams(&mut Source::new(&mut file)?);
    let len = file.seek(SeekFrom::End(0))?;
    bases
        .into_iter()
        .map(|base| {
            let mut out = vec![0; (len.saturating_sub(base)).min(MAX_BYTES) as usize];
            file.seek(SeekFrom::Start(base))?;
            file.read_exact(&mut out)?;
            Ok(out)
        })
        .collect()
}

/// IFD0 tags an export copies.
const COPIED_IFD0: &[u16] = &[TAG_MAKE, TAG_MODEL, TAG_ARTIST, TAG_COPYRIGHT];

/// Exif IFD tags an export copies.
const COPIED_EXIF: &[u16] = &[
    0x829A, // ExposureTime
    0x829D, // FNumber
    0x8822, // ExposureProgram
    0x8827, // ISO
    0x8830, // SensitivityType
    0x8832, // RecommendedExposureIndex
    0x9003, // DateTimeOriginal
    0x9004, // DateTimeDigitized
    0x9011, // OffsetTimeOriginal
    0x9012, // OffsetTimeDigitized
    0x9201, // ShutterSpeedValue
    0x9202, // ApertureValue
    0x9204, // ExposureBiasValue
    0x9205, // MaxApertureValue
    0x9207, // MeteringMode
    0x9208, // LightSource
    0x9209, // Flash
    0x920A, // FocalLength
    0x9291, // SubSecTimeOriginal
    0x9292, // SubSecTimeDigitized
    0xA402, // ExposureMode
    0xA403, // WhiteBalance
    0xA405, // FocalLengthIn35mmFilm
    0xA406, // SceneCaptureType
    0xA432, // LensSpecification
    0xA433, // LensMake
    0xA434, // LensModel
];

/// The copied tags of one IFD and its Exif IFD, sorted by tag number, not
/// where found (a CR3 keeps each set in its own stream); the first wins.
fn collect_tags<R: Read + Seek>(
    src: &mut Source<R>,
    s: Stream,
    at: u64,
    tags: &mut SourceTags,
    depth: u8,
) {
    let Some(entries) = ifd_entries(src, s, at) else { return };
    for e in &entries {
        if e.tag == TAG_EXIF_IFD {
            if depth == 0
                && let Some(offset) = pointer(src, s, e)
            {
                collect_tags(src, s, s.base + u64::from(offset), tags, depth + 1);
            }
            continue;
        }
        let list = if COPIED_IFD0.contains(&e.tag) {
            &mut tags.ifd0
        } else if COPIED_EXIF.contains(&e.tag) {
            &mut tags.exif
        } else {
            continue;
        };
        if list.iter().any(|f| f.tag == e.tag) {
            continue;
        }
        if let Some(value) = field_value(src, s, e) {
            list.push(Field::new(e.tag, value));
        }
    }
}

/// An entry's value: text up to 256 bytes, other types up to 16 values.
fn field_value<R: Read + Seek>(src: &mut Source<R>, s: Stream, e: &Entry) -> Option<Value> {
    if e.kind == ASCII {
        return ascii(src, s, e).map(Value::Ascii);
    }
    if e.count == 0 || e.count > 16 {
        return None;
    }
    let (pos, size) = value_pos(src, s, e)?;
    let b = src.bytes(pos, size as usize)?;
    let u16s = || b.as_chunks::<2>().0.iter().map(|&c| s.order.u16(c)).collect();
    let u32s = || b.as_chunks::<4>().0.iter().map(|&c| s.order.u32(c)).collect::<Vec<_>>();
    let pairs = |v: Vec<u32>| v.as_chunks::<2>().0.to_vec();
    Some(match e.kind {
        1 => Value::Byte(b),
        SHORT => Value::Short(u16s()),
        LONG => Value::Long(u32s()),
        5 => Value::Rational(pairs(u32s())),
        7 => Value::Undefined(b),
        10 => {
            Value::SRational(pairs(u32s()).into_iter().map(|[n, d]| [n as i32, d as i32]).collect())
        }
        _ => return None,
    })
}

/// The block size reads are made in.
pub(crate) const BLOCK: u64 = 4096;
/// The most read from one file.
pub(crate) const MAX_BYTES: u64 = 512 * 1024;
/// The most entries one IFD may claim.
const MAX_ENTRIES: u16 = 1000;
/// The most boxes or segments walked at one level.
const MAX_ITEMS: usize = 256;

/// A file read in blocks, each at most once, up to [`MAX_BYTES`].
struct Source<R> {
    inner: R,
    len: u64,
    blocks: HashMap<u64, Box<[u8]>>,
    loaded: u64,
}

impl<R: Read + Seek> Source<R> {
    fn new(mut inner: R) -> io::Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        Ok(Self { inner, len, blocks: HashMap::new(), loaded: 0 })
    }

    /// `n` bytes at `offset`, or `None` past the end or the budget.
    fn bytes(&mut self, offset: u64, n: usize) -> Option<Vec<u8>> {
        let end = offset.checked_add(n as u64).filter(|&e| e <= self.len)?;
        let mut out = Vec::with_capacity(n);
        let mut pos = offset;
        while pos < end {
            let index = pos / BLOCK;
            let block = self.block(index)?;
            let start = (pos - index * BLOCK) as usize;
            let take = ((end - pos) as usize).min(block.len() - start);
            out.extend_from_slice(&block[start..start + take]);
            pos += take as u64;
        }
        Some(out)
    }

    fn block(&mut self, index: u64) -> Option<&[u8]> {
        if !self.blocks.contains_key(&index) {
            let start = index * BLOCK;
            let n = BLOCK.min(self.len - start);
            if self.loaded + n > MAX_BYTES {
                return None;
            }
            let mut buf = vec![0; n as usize];
            self.inner.seek(SeekFrom::Start(start)).ok()?;
            self.inner.read_exact(&mut buf).ok()?;
            self.loaded += n;
            self.blocks.insert(index, buf.into());
        }
        self.blocks.get(&index).map(|b| &b[..])
    }

    fn u16_at(&mut self, offset: u64, order: Order) -> Option<u16> {
        let b = self.bytes(offset, 2)?;
        Some(order.u16([b[0], b[1]]))
    }

    fn u32_at(&mut self, offset: u64, order: Order) -> Option<u32> {
        let b = self.bytes(offset, 4)?;
        Some(order.u32([b[0], b[1], b[2], b[3]]))
    }
}

// --------------------------------------------------------------- TIFF

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    Little,
    Big,
}

impl Order {
    fn from_mark(mark: &[u8]) -> Option<Self> {
        match mark {
            b"II" => Some(Self::Little),
            b"MM" => Some(Self::Big),
            _ => None,
        }
    }

    fn u16(self, b: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(b),
            Self::Big => u16::from_be_bytes(b),
        }
    }

    fn u32(self, b: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(b),
            Self::Big => u32::from_be_bytes(b),
        }
    }
}

/// A TIFF stream inside the file: offsets in it count from `base`.
#[derive(Clone, Copy, Debug)]
struct Stream {
    base: u64,
    order: Order,
}

/// One IFD entry: where its 4-byte value-or-offset field sits in the file.
#[derive(Clone, Copy, Debug)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    field: u64,
}

const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const IFD: u16 = 13;

pub(crate) const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;
const TAG_DATE_TIME_DIGITIZED: u16 = 0x9004;
pub(crate) const TAG_OFFSET_TIME_ORIGINAL: u16 = 0x9011;
pub(crate) const TAG_SUB_SEC_TIME_ORIGINAL: u16 = 0x9291;
const TAG_SUB_SEC_TIME_DIGITIZED: u16 = 0x9292;
const TAG_MAKER_NOTE: u16 = 0x927C;
const TAG_DNG_PRIVATE_DATA: u16 = 0xC634;
const TAG_OM_CAMERA_SETTINGS: u16 = 0x2020;
const TAG_OM_DRIVE_MODE: u16 = 0x0600;

/// Bytes per value of a TIFF field type; 0 for types this reader skips.
fn type_size(kind: u16) -> u64 {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

/// The TIFF stream at `base` and its first IFD. Accepts the magic of TIFF
/// (42), ORF (`IIRO`, `IIRS`) and RW2 (`IIU`).
fn tiff_header<R: Read + Seek>(src: &mut Source<R>, base: u64) -> Option<(Stream, u64)> {
    let head = src.bytes(base, 8)?;
    let order = Order::from_mark(&head[..2])?;
    let magic = order.u16([head[2], head[3]]);
    if !matches!(magic, 42 | 0x4F52 | 0x5352 | 0x55) {
        return None;
    }
    let ifd = order.u32([head[4], head[5], head[6], head[7]]);
    Some((Stream { base, order }, base + u64::from(ifd)))
}

/// The entries of the IFD at absolute position `at`.
fn ifd_entries<R: Read + Seek>(src: &mut Source<R>, s: Stream, at: u64) -> Option<Vec<Entry>> {
    let n = src.u16_at(at, s.order)?;
    if n > MAX_ENTRIES {
        return None;
    }
    let raw = src.bytes(at + 2, 12 * usize::from(n))?;
    Some(
        raw.as_chunks::<12>()
            .0
            .iter()
            .enumerate()
            .map(|(i, e)| Entry {
                tag: s.order.u16([e[0], e[1]]),
                kind: s.order.u16([e[2], e[3]]),
                count: s.order.u32([e[4], e[5], e[6], e[7]]),
                field: at + 2 + 12 * i as u64 + 8,
            })
            .collect(),
    )
}

/// Where an entry's values are: inline when they fit in 4 bytes, else at
/// the field's offset.
fn value_pos<R: Read + Seek>(src: &mut Source<R>, s: Stream, e: &Entry) -> Option<(u64, u64)> {
    let size = type_size(e.kind).checked_mul(u64::from(e.count)).filter(|&n| n > 0)?;
    let pos = if size <= 4 { e.field } else { s.base + u64::from(src.u32_at(e.field, s.order)?) };
    Some((pos, size))
}

fn ascii<R: Read + Seek>(src: &mut Source<R>, s: Stream, e: &Entry) -> Option<String> {
    if e.kind != ASCII || e.count > 256 {
        return None;
    }
    let (pos, size) = value_pos(src, s, e)?;
    let bytes = src.bytes(pos, size as usize)?;
    let text = bytes.split(|&b| b == 0).next().unwrap_or(&[]);
    let text = String::from_utf8_lossy(text).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The `index`th SHORT of an entry.
fn short<R: Read + Seek>(src: &mut Source<R>, s: Stream, e: &Entry, index: u32) -> Option<u16> {
    if e.kind != SHORT || index >= e.count {
        return None;
    }
    let (pos, _) = value_pos(src, s, e)?;
    src.u16_at(pos + 2 * u64::from(index), s.order)
}

/// The offset a pointer entry (`LONG` or `IFD`) holds.
fn pointer<R: Read + Seek>(src: &mut Source<R>, s: Stream, e: &Entry) -> Option<u32> {
    match e.kind {
        LONG | IFD => src.u32_at(e.field, s.order),
        _ => None,
    }
}

/// Everything the TIFF stream at `base` records.
fn read_tiff<R: Read + Seek>(src: &mut Source<R>, base: u64, found: &mut Found) {
    if let Some((s, ifd0)) = tiff_header(src, base) {
        read_ifd(src, s, ifd0, found, 0);
    }
}

/// The fields of interest in one IFD and its Exif IFD (their tags don't
/// overlap). Each time takes its sub-seconds from the same IFD.
fn read_ifd<R: Read + Seek>(src: &mut Source<R>, s: Stream, at: u64, found: &mut Found, depth: u8) {
    let Some(entries) = ifd_entries(src, s, at) else { return };
    let date_time = |src: &mut Source<R>, e: &Entry| {
        ascii(src, s, e)
            .and_then(|t| parse_date_time(&t))
            .map(|seconds| CaptureTime { seconds, micros: None })
    };
    let (mut subsec, mut subsec_digitized) = (None, None);
    for e in &entries {
        match e.tag {
            TAG_MODEL if found.info.model.is_none() => found.info.model = ascii(src, s, e),
            TAG_DATE_TIME_ORIGINAL if found.info.time.is_none() => {
                found.info.time = date_time(src, e);
            }
            TAG_DATE_TIME_DIGITIZED if found.digitized.is_none() => {
                found.digitized = date_time(src, e);
            }
            TAG_SUB_SEC_TIME_ORIGINAL => subsec = ascii(src, s, e).and_then(|t| parse_subsec(&t)),
            TAG_SUB_SEC_TIME_DIGITIZED => {
                subsec_digitized = ascii(src, s, e).and_then(|t| parse_subsec(&t));
            }
            TAG_MAKER_NOTE if found.info.shot.is_none() => {
                if let Some((pos, _)) = value_pos(src, s, e) {
                    found.info.shot = om_shot(src, pos);
                }
            }
            TAG_DNG_PRIVATE_DATA if found.info.shot.is_none() => {
                if let Some((pos, size)) = value_pos(src, s, e) {
                    found.info.shot =
                        adobe_maker_note(src, pos, size).and_then(|note| om_shot(src, note));
                }
            }
            TAG_EXIF_IFD if depth == 0 => {
                if let Some(offset) = pointer(src, s, e) {
                    read_ifd(src, s, s.base + u64::from(offset), found, depth + 1);
                }
            }
            _ => {}
        }
    }
    if let (Some(time), Some(micros)) = (found.info.time.as_mut(), subsec) {
        time.micros.get_or_insert(micros);
    }
    if let (Some(time), Some(micros)) = (found.digitized.as_mut(), subsec_digitized) {
        time.micros.get_or_insert(micros);
    }
}

/// The original maker note in Adobe's `DNGPrivateData`: `Adobe\0`, then
/// `MakN` (big-endian size, original byte order and offset), then the note.
fn adobe_maker_note<R: Read + Seek>(src: &mut Source<R>, pos: u64, size: u64) -> Option<u64> {
    if size < 20 {
        return None;
    }
    let head = src.bytes(pos, 20)?;
    (head.starts_with(b"Adobe\0") && &head[6..10] == b"MakN").then_some(pos + 20)
}

/// The shot number in an OM System / Olympus maker note, whose offsets
/// count from its start.
fn om_shot<R: Read + Seek>(src: &mut Source<R>, note: u64) -> Option<u16> {
    let head = src.bytes(note, 16)?;
    // Signature, byte order mark, 2-byte version, IFD.
    let (order_at, ifd_at) = if head.starts_with(b"OM SYSTEM\0") {
        (12, 16)
    } else if head.starts_with(b"OLYMPUS\0") {
        (8, 12)
    } else {
        return None;
    };
    let s = Stream { base: note, order: Order::from_mark(&head[order_at..order_at + 2])? };
    let entries = ifd_entries(src, s, note + ifd_at)?;
    let settings = entries.iter().find(|e| e.tag == TAG_OM_CAMERA_SETTINGS)?;
    let at = note + u64::from(pointer(src, s, settings)?);
    let drive = *ifd_entries(src, s, at)?.iter().find(|e| e.tag == TAG_OM_DRIVE_MODE)?;
    let mode = short(src, s, &drive, 0)?;
    let shot = short(src, s, &drive, 1)?;
    // Mode 0 is a single shot, whose number means nothing.
    (mode != 0).then_some(shot)
}

// --------------------------------------------------------------- JPEG

/// How a PNG starts.
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Where a PNG's `eXIf` TIFF stream starts; it precedes `IDAT`, so the
/// walk stops there.
fn png_exif<R: Read + Seek>(src: &mut Source<R>) -> Option<u64> {
    let mut pos = PNG_MAGIC.len() as u64;
    for _ in 0..MAX_ITEMS {
        let head = src.bytes(pos, 8)?;
        let len = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        match &head[4..8] {
            b"eXIf" => return Some(pos + 8),
            b"IDAT" | b"IEND" => return None,
            _ => pos += 12 + len,
        }
    }
    None
}

/// Where the Exif (APP1) TIFF stream of the JPEG at `start` starts.
fn jpeg_exif<R: Read + Seek>(src: &mut Source<R>, start: u64) -> Option<u64> {
    if src.bytes(start, 2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = start + 2;
    for _ in 0..MAX_ITEMS {
        let marker = src.bytes(pos, 4)?;
        if marker[0] != 0xFF {
            return None;
        }
        match marker[1] {
            // Fill byte: the marker follows.
            0xFF => {
                pos += 1;
                continue;
            }
            // Start of scan or end of image: no metadata after.
            0xDA | 0xD9 => return None,
            _ => {}
        }
        let len = u64::from(u16::from_be_bytes([marker[2], marker[3]]));
        if len < 2 {
            return None;
        }
        if marker[1] == 0xE1 && src.bytes(pos + 4, 6).as_deref() == Some(b"Exif\0\0") {
            return Some(pos + 10);
        }
        pos += 2 + len;
    }
    None
}

// ---------------------------------------------------------------- RAF

/// How a Fujifilm RAF starts.
const RAF_MAGIC: &[u8] = b"FUJIFILMCCD-RAW";

/// A RAF's Exif TIFF stream, in the JPEG preview whose offset is at byte 84
/// (big-endian).
fn raf_exif<R: Read + Seek>(src: &mut Source<R>) -> Option<u64> {
    let jpeg = src.u32_at(84, Order::Big)?;
    jpeg_exif(src, u64::from(jpeg))
}

// --------------------------------------------------------------- HEIF

/// `ftyp` major brands of HEIF stills (HEIC, AVIF and the generic ones).
const HEIF_BRANDS: [&[u8]; 6] = [b"heic", b"heix", b"heim", b"heis", b"mif1", b"avif"];

/// A HEIF's `Exif` item's TIFF stream (ISO/IEC 23008-12 A.2.1: a 4-byte
/// offset to the TIFF header, then the data).
fn heif_exif<R: Read + Seek>(src: &mut Source<R>) -> Option<u64> {
    let len = src.len;
    let meta = boxes(src, 0, len).into_iter().find(|b| &b.kind == b"meta")?;
    // `meta` is a full box: version and flags before its children.
    let children = boxes(src, meta.start + 4, meta.end);
    let iinf = children.iter().find(|b| &b.kind == b"iinf")?;
    let iloc = children.iter().find(|b| &b.kind == b"iloc")?;
    let id = heif_exif_item(src, iinf)?;
    let at = heif_item_offset(src, iloc, id)?;
    let skip = src.u32_at(at, Order::Big)?;
    Some(at + 4 + u64::from(skip))
}

/// The item ID of the `Exif` entry in `iinf`.
fn heif_exif_item<R: Read + Seek>(src: &mut Source<R>, iinf: &BoxSpan) -> Option<u32> {
    let version = src.bytes(iinf.start, 1)?[0];
    let first = iinf.start + 4 + if version == 0 { 2 } else { 4 };
    for infe in boxes(src, first, iinf.end).into_iter().filter(|b| &b.kind == b"infe") {
        let version = src.bytes(infe.start, 1)?[0];
        // Versions 0 and 1 name no item type.
        let (id, kind) = match version {
            2 => (u32::from(src.u16_at(infe.start + 4, Order::Big)?), infe.start + 8),
            3 => (src.u32_at(infe.start + 4, Order::Big)?, infe.start + 10),
            _ => continue,
        };
        if src.bytes(kind, 4).as_deref() == Some(b"Exif") {
            return Some(id);
        }
    }
    None
}

/// Where item `id`'s data starts: base plus first extent offset. Items not
/// built from file offsets (construction method 1 or 2) read as missing.
fn heif_item_offset<R: Read + Seek>(src: &mut Source<R>, iloc: &BoxSpan, id: u32) -> Option<u64> {
    let version = src.bytes(iloc.start, 1)?[0];
    if version > 2 {
        return None;
    }
    let sizes = src.bytes(iloc.start + 4, 2)?;
    let (offset_size, length_size) = (sizes[0] >> 4, sizes[0] & 0xF);
    let (base_size, index_size) = (sizes[1] >> 4, if version == 0 { 0 } else { sizes[1] & 0xF });
    let mut pos = iloc.start + 6;
    let uint = |src: &mut Source<R>, pos: &mut u64, size: u8| -> Option<u64> {
        let value = match size {
            0 => 0,
            2 => u64::from(src.u16_at(*pos, Order::Big)?),
            4 => u64::from(src.u32_at(*pos, Order::Big)?),
            8 => u64::from_be_bytes(src.bytes(*pos, 8)?.try_into().ok()?),
            _ => return None,
        };
        *pos += u64::from(size);
        Some(value)
    };
    let count = uint(src, &mut pos, if version < 2 { 2 } else { 4 })?;
    for _ in 0..count.min(MAX_ITEMS as u64) {
        let item = uint(src, &mut pos, if version < 2 { 2 } else { 4 })?;
        let method = if version == 0 { 0 } else { uint(src, &mut pos, 2)? & 0xF };
        let _data_reference = uint(src, &mut pos, 2)?;
        let base = uint(src, &mut pos, base_size)?;
        let extents = uint(src, &mut pos, 2)?;
        let mut first = None;
        for _ in 0..extents.min(MAX_ITEMS as u64) {
            uint(src, &mut pos, index_size)?;
            let offset = uint(src, &mut pos, offset_size)?;
            uint(src, &mut pos, length_size)?;
            first.get_or_insert(offset);
        }
        if item == u64::from(id) {
            return (method == 0).then(|| base.checked_add(first?)).flatten();
        }
    }
    None
}

// ---------------------------------------------------------------- CR3

/// Canon's `uuid` box in a CR3's `moov`, which holds the `CMT` boxes.
const CANON_UUID: [u8; 16] = [
    0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0, 0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A, 0x48,
];

/// A box's type and its payload's `[start, end)`.
struct BoxSpan {
    kind: [u8; 4],
    start: u64,
    end: u64,
}

/// The boxes in `[start, end)` (ISO/IEC 14496-12: 32-bit sizes, 64-bit
/// when the size is 1, to the end when it is 0).
fn boxes<R: Read + Seek>(src: &mut Source<R>, start: u64, end: u64) -> Vec<BoxSpan> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end && out.len() < MAX_ITEMS {
        let Some(head) = src.bytes(pos, 8) else { break };
        let kind = [head[4], head[5], head[6], head[7]];
        let (size, header) = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) {
            0 => (end - pos, 8),
            1 => {
                let Some(large) = src.bytes(pos + 8, 8) else { break };
                (u64::from_be_bytes(large.try_into().expect("8 bytes")), 16)
            }
            n => (u64::from(n), 8),
        };
        let Some(box_end) = pos.checked_add(size).filter(|&e| size >= header && e <= end) else {
            break;
        };
        out.push(BoxSpan { kind, start: pos + header, end: box_end });
        pos = box_end;
    }
    out
}

/// Where a CR3's TIFF streams start: IFD0 in `CMT1`, the Exif IFD in
/// `CMT2`.
fn cr3_streams<R: Read + Seek>(src: &mut Source<R>) -> Vec<u64> {
    let len = src.len;
    let Some(moov) = boxes(src, 0, len).into_iter().find(|b| &b.kind == b"moov") else {
        return Vec::new();
    };
    let canon = boxes(src, moov.start, moov.end)
        .into_iter()
        .find(|b| &b.kind == b"uuid" && src.bytes(b.start, 16).as_deref() == Some(&CANON_UUID[..]));
    let Some(canon) = canon else { return Vec::new() };
    boxes(src, canon.start + 16, canon.end)
        .into_iter()
        .filter(|b| &b.kind == b"CMT1" || &b.kind == b"CMT2")
        .map(|b| b.start)
        .collect()
}

// --------------------------------------------------------------- Time

/// Seconds since 1970 of `YYYY:MM:DD HH:MM:SS`; `None` for the blank or
/// zero values of an unset clock.
fn parse_date_time(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes().all(|c| c.is_ascii_digit()).then(|| part.parse().ok()).flatten()
    };
    let date_sep = |i: usize| b[i] == b':' || b[i] == b'-';
    if !date_sep(4)
        || !date_sep(7)
        || !(b[10] == b' ' || b[10] == b'T')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let valid = year >= 1900
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60
        && second < 61;
    if !valid {
        return None;
    }
    Some(days_from_civil(year, month, day)? * 86_400 + hour * 3600 + minute * 60 + second)
}

/// A `SubSecTimeOriginal` value as microseconds: its digits are a decimal
/// fraction ("7" is 0.7 s, "070" is 0.07 s).
fn parse_subsec(text: &str) -> Option<u32> {
    let digits: String = text.trim().chars().take_while(char::is_ascii_digit).take(6).collect();
    if digits.is_empty() {
        return None;
    }
    format!("{digits:0<6}").parse().ok()
}

/// Days since 1970-01-01, counted from the month's first so an invalid 31
/// February is 3 March.
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let first = chrono::NaiveDate::from_ymd_opt(year.try_into().ok()?, month.try_into().ok()?, 1)?;
    Some((first - chrono::NaiveDate::default()).num_days() + day - 1)
}

#[cfg(test)]
mod tests;
