use std::io::Cursor;

use deptheos_core::io::metadata::TAG_ORIENTATION;

use super::*;

/// A little-endian TIFF stream built bottom-up, each IFD followed by its
/// out-of-line values.
struct Builder {
    buf: Vec<u8>,
}

/// An entry to write: tag, type, count and its value bytes.
#[derive(Clone)]
struct Field(u16, u16, u32, Vec<u8>);

fn ascii_field(tag: u16, text: &str) -> Field {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    Field(tag, ASCII, bytes.len() as u32, bytes)
}

fn shorts(tag: u16, values: &[u16]) -> Field {
    Field(tag, SHORT, values.len() as u32, values.iter().flat_map(|v| v.to_le_bytes()).collect())
}

fn pointer_field(tag: u16, offset: u32) -> Field {
    Field(tag, LONG, 1, offset.to_le_bytes().to_vec())
}

fn undefined(tag: u16, bytes: Vec<u8>) -> Field {
    Field(tag, 7, bytes.len() as u32, bytes)
}

impl Builder {
    fn new(prefix: &[u8]) -> Self {
        Self { buf: prefix.to_vec() }
    }

    /// A TIFF header with `magic`, its IFD0 pointer patched by
    /// [`Self::finish`].
    fn tiff(magic: &[u8; 2]) -> Self {
        let mut b = Self::new(b"II");
        b.buf.extend_from_slice(magic);
        b.buf.extend_from_slice(&[0; 4]);
        b
    }

    /// Append an IFD of `fields` (sorted by tag) and its values; its offset.
    fn ifd(&mut self, mut fields: Vec<Field>) -> u32 {
        fields.sort_by_key(|f| f.0);
        if self.buf.len() % 2 == 1 {
            self.buf.push(0);
        }
        let at = self.buf.len();
        let data_start = at + 2 + 12 * fields.len() + 4;
        let mut data = Vec::new();
        self.buf.extend_from_slice(&(fields.len() as u16).to_le_bytes());
        for Field(tag, kind, count, value) in &fields {
            self.buf.extend_from_slice(&tag.to_le_bytes());
            self.buf.extend_from_slice(&kind.to_le_bytes());
            self.buf.extend_from_slice(&count.to_le_bytes());
            if value.len() <= 4 {
                let mut inline = value.clone();
                inline.resize(4, 0);
                self.buf.extend_from_slice(&inline);
            } else {
                let offset = (data_start + data.len()) as u32;
                self.buf.extend_from_slice(&offset.to_le_bytes());
                data.extend_from_slice(value);
                if data.len() % 2 == 1 {
                    data.push(0);
                }
            }
        }
        self.buf.extend_from_slice(&[0; 4]); // no next IFD
        self.buf.extend_from_slice(&data);
        at as u32
    }

    /// The next offset bytes appended at will land on.
    fn here(&self) -> u32 {
        self.buf.len() as u32
    }

    fn finish(mut self, ifd0: u32) -> Vec<u8> {
        self.buf[4..8].copy_from_slice(&ifd0.to_le_bytes());
        self.buf
    }
}

/// An OM SYSTEM maker note recording `DriveMode` `[mode, shot, 0, 0]`.
fn om_note(mode: u16, shot: u16) -> Vec<u8> {
    let mut b = Builder::new(b"OM SYSTEM\0\0\0II\x04\0");
    // The main IFD sits at +16; its pointer to CameraSettings, built after
    // it, is patched in later (first entry's value, 2 + 8 bytes in).
    let ifd_at = b.here();
    assert_eq!(ifd_at, 16);
    let main = b.ifd(vec![pointer_field(TAG_OM_CAMERA_SETTINGS, 0)]);
    let settings = b.ifd(vec![shorts(TAG_OM_DRIVE_MODE, &[mode, shot, 0, 0])]);
    let field = main as usize + 2 + 8;
    b.buf[field..field + 4].copy_from_slice(&settings.to_le_bytes());
    b.buf
}

/// A TIFF: IFD0 (model, orientation) and an Exif IFD (time, optional
/// sub-seconds and maker note).
fn tiff_file(magic: &[u8; 2], time: &str, subsec: Option<&str>, note: Option<Vec<u8>>) -> Vec<u8> {
    let mut b = Builder::tiff(magic);
    let mut exif = vec![ascii_field(TAG_DATE_TIME_ORIGINAL, time)];
    if let Some(s) = subsec {
        exif.push(ascii_field(TAG_SUB_SEC_TIME_ORIGINAL, s));
    }
    if let Some(note) = note {
        exif.push(undefined(TAG_MAKER_NOTE, note));
    }
    let exif_at = b.ifd(exif);
    let ifd0 = b.ifd(vec![
        ascii_field(TAG_MODEL, "OM-1            "),
        shorts(TAG_ORIENTATION, &[6]),
        pointer_field(TAG_EXIF_IFD, exif_at),
    ]);
    b.finish(ifd0)
}

fn read(bytes: Vec<u8>) -> CaptureInfo {
    read_from(Cursor::new(bytes)).unwrap()
}

/// Seconds since 1970 of a date, for expected values.
fn at(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    days_from_civil(y, mo, d).unwrap() * 86_400 + h * 3600 + mi * 60 + s
}

#[test]
fn tiff_gives_time_subsec_and_model() {
    let info = read(tiff_file(b"*\0", "2025:03:22 15:49:48", Some("70"), None));
    assert_eq!(
        info,
        CaptureInfo {
            time: Some(CaptureTime { seconds: at(2025, 3, 22, 15, 49, 48), micros: Some(700_000) }),
            model: Some("OM-1".into()),
            shot: None,
        }
    );
    assert!(
        (info.time.unwrap().as_secs_f64() - (at(2025, 3, 22, 15, 49, 48) as f64 + 0.7)).abs()
            < 1e-6
    );
}

#[test]
fn orf_header_and_om_shot_number() {
    let info = read(tiff_file(b"RO", "2025:06:14 12:26:07", None, Some(om_note(1, 19))));
    assert_eq!(info.time, Some(CaptureTime { seconds: at(2025, 6, 14, 12, 26, 7), micros: None }));
    assert_eq!(info.shot, Some(19));
}

/// A TIFF whose IFD0 holds the model and `ifd0` and whose Exif IFD holds
/// `exif`.
fn tiff_with(ifd0: Vec<Field>, exif: Vec<Field>) -> Vec<u8> {
    let mut b = Builder::tiff(b"*\0");
    let exif_at = b.ifd(exif);
    let mut fields = vec![ascii_field(TAG_MODEL, "EOS R5"), pointer_field(TAG_EXIF_IFD, exif_at)];
    fields.extend(ifd0);
    let ifd0 = b.ifd(fields);
    b.finish(ifd0)
}

#[test]
fn without_an_original_time_the_digitized_time_is_read() {
    let digitized = || {
        vec![
            ascii_field(TAG_DATE_TIME_DIGITIZED, "2025:03:22 15:49:48"),
            ascii_field(TAG_SUB_SEC_TIME_DIGITIZED, "25"),
        ]
    };
    let want = Some(CaptureTime { seconds: at(2025, 3, 22, 15, 49, 48), micros: Some(250_000) });
    assert_eq!(read(tiff_with(vec![], digitized())).time, want, "no original time");
    let blank = [vec![ascii_field(TAG_DATE_TIME_ORIGINAL, "    :  :     :  :  ")], digitized()];
    assert_eq!(read(tiff_with(vec![], blank.concat())).time, want, "a blank original time");
    // An original time wins, with its own sub-seconds, not the digitized.
    let both = [
        vec![
            ascii_field(TAG_DATE_TIME_ORIGINAL, "2025:03:22 15:49:47"),
            ascii_field(TAG_SUB_SEC_TIME_ORIGINAL, "5"),
        ],
        digitized(),
    ];
    assert_eq!(
        read(tiff_with(vec![], both.concat())).time,
        Some(CaptureTime { seconds: at(2025, 3, 22, 15, 49, 47), micros: Some(500_000) })
    );
    // The digitized time takes only its own sub-seconds.
    let other_subsec = vec![
        ascii_field(TAG_DATE_TIME_DIGITIZED, "2025:03:22 15:49:48"),
        ascii_field(TAG_SUB_SEC_TIME_ORIGINAL, "25"),
    ];
    assert_eq!(
        read(tiff_with(vec![], other_subsec)).time,
        Some(CaptureTime { seconds: at(2025, 3, 22, 15, 49, 48), micros: None })
    );
}

#[test]
fn the_modify_time_is_never_a_capture_time() {
    // DateTime (0x0132) changes on every edit or export.
    let ifd0 = vec![ascii_field(0x0132, "2025:03:22 15:49:48")];
    assert_eq!(read(tiff_with(ifd0, vec![])).time, None);
}

#[test]
fn single_shots_have_no_shot_number() {
    let info = read(tiff_file(b"RO", "2025:06:14 12:26:07", None, Some(om_note(0, 1))));
    assert_eq!(info.shot, None);
}

#[test]
fn other_maker_notes_have_no_shot_number() {
    let info = read(tiff_file(
        b"*\0",
        "2025:06:14 12:26:07",
        None,
        Some(b"Canon\0\0\0\0\0\0\0\0\0\0\0".to_vec()),
    ));
    assert_eq!(info.shot, None);
    assert!(info.time.is_some());
}

#[test]
fn adobe_dng_keeps_the_om_maker_note_in_its_private_data() {
    let note = om_note(1, 14);
    let mut private = b"Adobe\0MakN".to_vec();
    private.extend_from_slice(&((note.len() + 6) as u32).to_be_bytes());
    private.extend_from_slice(b"II");
    private.extend_from_slice(&1234u32.to_be_bytes());
    private.extend_from_slice(&note);
    let mut b = Builder::tiff(b"*\0");
    let exif_at = b.ifd(vec![ascii_field(TAG_DATE_TIME_ORIGINAL, "2025:03:30 12:43:49")]);
    let ifd0 = b.ifd(vec![
        ascii_field(TAG_MODEL, "OM-1"),
        pointer_field(TAG_EXIF_IFD, exif_at),
        Field(TAG_DNG_PRIVATE_DATA, 1, private.len() as u32, private),
    ]);
    let info = read(b.finish(ifd0));
    assert_eq!(info.shot, Some(14));
    assert_eq!(info.model.as_deref(), Some("OM-1"));
}

#[test]
fn big_endian_tiff() {
    // IFD0 at 8: one ASCII Model entry, value out of line at 26.
    let mut f = b"MM\0*\0\0\0\x08".to_vec();
    f.extend_from_slice(&1u16.to_be_bytes());
    f.extend_from_slice(&TAG_MODEL.to_be_bytes());
    f.extend_from_slice(&ASCII.to_be_bytes());
    f.extend_from_slice(&6u32.to_be_bytes());
    f.extend_from_slice(&26u32.to_be_bytes());
    f.extend_from_slice(&[0; 4]);
    f.extend_from_slice(b"D850\0\0");
    assert_eq!(read(f).model.as_deref(), Some("D850"));
}

#[test]
fn jpeg_reads_its_app1_exif() {
    let tiff = tiff_file(b"*\0", "2025:03:22 15:49:52", Some("45"), None);
    let mut f = vec![0xFF, 0xD8];
    // An APP0 first, then APP1 with the Exif block.
    f.extend_from_slice(&[0xFF, 0xE0, 0, 4, 0, 0]);
    f.extend_from_slice(&[0xFF, 0xE1]);
    f.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    f.extend_from_slice(b"Exif\0\0");
    f.extend_from_slice(&tiff);
    f.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
    let info = read(f);
    assert_eq!(info.time.unwrap().micros, Some(450_000));
}

#[test]
fn jpeg_without_exif_gives_nothing() {
    let f = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xDA, 0, 2, 0xFF, 0xD9];
    assert_eq!(read(f), CaptureInfo::default());
}

/// An ISO BMFF box.
fn bmff(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(payload);
    b
}

/// A CR3 with Canon's two metadata streams (model and time).
fn cr3_file() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut cmt1 = Builder::tiff(b"*\0");
    let ifd =
        cmt1.ifd(vec![ascii_field(TAG_MODEL, "Canon EOS R5m2"), shorts(TAG_ORIENTATION, &[1])]);
    let cmt1 = cmt1.finish(ifd);
    let mut cmt2 = Builder::tiff(b"*\0");
    let ifd = cmt2.ifd(vec![
        ascii_field(TAG_DATE_TIME_ORIGINAL, "2025:04:13 14:03:42"),
        ascii_field(TAG_SUB_SEC_TIME_ORIGINAL, "91"),
    ]);
    let cmt2 = cmt2.finish(ifd);
    let mut uuid = CANON_UUID.to_vec();
    uuid.extend(bmff(b"CNCV", b"CanonCR3_001/01.09.00/00.00.00"));
    uuid.extend(bmff(b"CMT1", &cmt1));
    uuid.extend(bmff(b"CMT2", &cmt2));
    let moov = bmff(b"moov", &bmff(b"uuid", &uuid));
    let mut f = bmff(b"ftyp", b"crx \0\0\0\x01crx isom");
    f.extend(moov);
    f.extend(bmff(b"mdat", &[0; 64]));
    (f, cmt1, cmt2)
}

#[test]
fn cr3_reads_its_cmt_boxes() {
    let info = read(cr3_file().0);
    assert_eq!(info.model.as_deref(), Some("Canon EOS R5m2"));
    assert_eq!(
        info.time,
        Some(CaptureTime { seconds: at(2025, 4, 13, 14, 3, 42), micros: Some(910_000) })
    );
}

/// A CR3's metadata streams are its CMT boxes from their TIFF headers on.
#[test]
fn a_cr3s_metadata_streams_are_its_cmt_boxes() {
    let (file, cmt1, cmt2) = cr3_file();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.CR3");
    std::fs::write(&path, &file).unwrap();
    let streams = metadata_streams(&path).unwrap();
    assert_eq!(streams.len(), 2);
    assert!(streams[0].starts_with(&cmt1));
    assert!(streams[1].starts_with(&cmt2));
}

/// A JPEG whose APP1 holds `tiff` as its Exif block.
fn jpeg_file(tiff: &[u8]) -> Vec<u8> {
    let mut f = vec![0xFF, 0xD8, 0xFF, 0xE1];
    f.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    f.extend_from_slice(b"Exif\0\0");
    f.extend_from_slice(tiff);
    f.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
    f
}

#[test]
fn raf_reads_its_preview_exif() {
    let jpeg = jpeg_file(&tiff_file(b"*\0", "2025:05:02 09:12:30", Some("25"), None));
    let mut f = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
    f.resize(84, 0);
    let offset = 148u32;
    f.extend_from_slice(&offset.to_be_bytes());
    f.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
    f.resize(offset as usize, 0);
    f.extend(jpeg);
    f.extend_from_slice(&[0; 64]);
    let info = read(f);
    assert_eq!(info.model.as_deref(), Some("OM-1"));
    assert_eq!(
        info.time,
        Some(CaptureTime { seconds: at(2025, 5, 2, 9, 12, 30), micros: Some(250_000) })
    );
}

/// A HEIC with an image item and an `Exif` item holding `tiff`, both in
/// `mdat`. `iloc_version` 0, 1 or 2.
fn heic_file(tiff: &[u8], iloc_version: u8) -> Vec<u8> {
    let infe = |id: u16, kind: &[u8; 4]| {
        let mut p = vec![2, 0, 0, 0];
        p.extend_from_slice(&id.to_be_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(kind);
        p.push(0);
        bmff(b"infe", &p)
    };
    let mut iinf = vec![0, 0, 0, 0, 0, 2];
    iinf.extend(infe(1, b"hvc1"));
    iinf.extend(infe(2, b"Exif"));
    let mut exif = 6u32.to_be_bytes().to_vec();
    exif.extend_from_slice(b"Exif\0\0");
    exif.extend_from_slice(tiff);
    let image = [0u8; 32];

    // `iloc` with every item's offset counted from the file's start.
    let iloc = |image_at: u32, exif_at: u32| {
        let wide = iloc_version == 2;
        let mut p = vec![iloc_version, 0, 0, 0, 0x44, 0];
        let id = |p: &mut Vec<u8>, v: u32| {
            if wide {
                p.extend_from_slice(&v.to_be_bytes());
            } else {
                p.extend_from_slice(&(v as u16).to_be_bytes());
            }
        };
        id(&mut p, 2);
        for (item, at, len) in [(1, image_at, image.len()), (2, exif_at, exif.len())] {
            id(&mut p, item);
            if iloc_version > 0 {
                p.extend_from_slice(&[0, 0]); // construction method 0
            }
            p.extend_from_slice(&[0, 0, 0, 1]); // data reference, one extent
            p.extend_from_slice(&at.to_be_bytes());
            p.extend_from_slice(&(len as u32).to_be_bytes());
        }
        bmff(b"iloc", &p)
    };
    let meta = |iloc: Vec<u8>| {
        let mut p = vec![0, 0, 0, 0];
        p.extend(bmff(b"hdlr", &[0; 24]));
        p.extend(bmff(b"iinf", &iinf));
        p.extend(iloc);
        bmff(b"meta", &p)
    };
    let ftyp = bmff(b"ftyp", b"heic\0\0\0\0mif1heic");
    // The layout fixes the offsets: lay out once to measure, then for real.
    let mdat_at = (ftyp.len() + meta(iloc(0, 0)).len() + 8) as u32;
    let mut f = ftyp;
    f.extend(meta(iloc(mdat_at, mdat_at + image.len() as u32)));
    let mut mdat = image.to_vec();
    mdat.extend(exif);
    f.extend(bmff(b"mdat", &mdat));
    f
}

#[test]
fn heic_reads_its_exif_item() {
    let tiff = tiff_file(b"*\0", "2025:06:14 18:20:05", Some("123"), None);
    for version in 0..=2 {
        let info = read(heic_file(&tiff, version));
        assert_eq!(info.model.as_deref(), Some("OM-1"), "iloc version {version}");
        assert_eq!(
            info.time,
            Some(CaptureTime { seconds: at(2025, 6, 14, 18, 20, 5), micros: Some(123_000) }),
            "iloc version {version}"
        );
    }
}

#[test]
fn unknown_and_truncated_files_give_nothing() {
    assert_eq!(read(b"not an image at all".to_vec()), CaptureInfo::default());
    assert_eq!(read(Vec::new()), CaptureInfo::default());
    let mut f = tiff_file(b"*\0", "2025:03:22 15:49:48", None, None);
    f.truncate(20);
    assert_eq!(read(f).time, None);
}

#[test]
fn hostile_offsets_stay_within_the_budget() {
    // IFD0 claims 999 entries and an Exif IFD pointing back at IFD0.
    let mut b = Builder::tiff(b"*\0");
    let ifd0 = b.here();
    let ifd = b.ifd(vec![
        pointer_field(TAG_EXIF_IFD, ifd0),
        Field(TAG_MODEL, ASCII, 200, vec![b'x'; 200]),
    ]);
    let mut f = b.finish(ifd);
    f[ifd as usize..ifd as usize + 2].copy_from_slice(&999u16.to_le_bytes());
    let info = read(f);
    assert_eq!(info.time, None);
}

#[test]
fn only_the_blocks_holding_metadata_are_read() {
    // Exif IFD far into a big file, as in an Adobe DNG.
    let mut b = Builder::tiff(b"*\0");
    b.buf.resize(3 << 20, 0);
    let exif_at = b.ifd(vec![ascii_field(TAG_DATE_TIME_ORIGINAL, "2025:03:22 15:49:48")]);
    b.buf.resize(b.buf.len() + (5 << 20), 0);
    let ifd0 = b.ifd(vec![ascii_field(TAG_MODEL, "X"), pointer_field(TAG_EXIF_IFD, exif_at)]);
    let bytes = b.finish(ifd0);
    let mut src = Source::new(Cursor::new(bytes)).unwrap();
    let mut found = Found::default();
    read_tiff(&mut src, 0, &mut found);
    assert!(found.info.time.is_some());
    assert!(src.loaded <= 6 * BLOCK, "read {} bytes", src.loaded);
}

/// An export copies camera, lens, exposure, artist and copyright, but no
/// owner name or serial number.
#[test]
fn source_tags_leave_out_the_owner_and_serial_numbers() {
    let mut b = Builder::tiff(b"*\0");
    let exif_at = b.ifd(vec![
        ascii_field(TAG_DATE_TIME_ORIGINAL, "2025:03:22 15:49:48"),
        ascii_field(0xA430, "A. Photographer"),
        ascii_field(0xA431, "BODY0001"),
        ascii_field(0xA434, "60mm F2.8 Macro"),
        ascii_field(0xA435, "LENS0002"),
    ]);
    let ifd0 = b.ifd(vec![
        ascii_field(TAG_MODEL, "OM-1"),
        ascii_field(TAG_ARTIST, "A. Photographer"),
        ascii_field(TAG_COPYRIGHT, "(c) A. Photographer"),
        pointer_field(TAG_EXIF_IFD, exif_at),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.tif");
    std::fs::write(&path, b.finish(ifd0)).unwrap();
    let tags = read_source_tags(&path).unwrap();
    let ifd0: Vec<u16> = tags.ifd0.iter().map(|f| f.tag).collect();
    let exif: Vec<u16> = tags.exif.iter().map(|f| f.tag).collect();
    assert_eq!(ifd0, [TAG_MODEL, TAG_ARTIST, TAG_COPYRIGHT]);
    assert_eq!(exif, [TAG_DATE_TIME_ORIGINAL, 0xA434]);
}

/// A PNG's `eXIf` chunk gives its capture time and export tags.
#[test]
fn a_png_exif_chunk_is_read() {
    let tiff = tiff_file(b"*\0", "2025:03:22 15:49:48", None, None);
    let chunk = |kind: &[u8], data: &[u8]| {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0; 4]); // the CRC, unread
        c
    };
    let mut png = PNG_MAGIC.to_vec();
    png.extend(chunk(b"IHDR", &[0; 13]));
    png.extend(chunk(b"eXIf", &tiff));
    png.extend(chunk(b"IDAT", &[]));
    png.extend(chunk(b"IEND", &[]));
    let info = read(png.clone());
    assert_eq!(info.time, Some(CaptureTime { seconds: at(2025, 3, 22, 15, 49, 48), micros: None }));
    assert_eq!(info.model.as_deref(), Some("OM-1"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.png");
    std::fs::write(&path, png).unwrap();
    let tags = read_source_tags(&path).unwrap();
    assert!(tags.ifd0.iter().any(|f| f.tag == TAG_MODEL));
    assert!(tags.exif.iter().any(|f| f.tag == TAG_DATE_TIME_ORIGINAL));
}

#[test]
fn date_time_parsing() {
    assert_eq!(parse_date_time("1970:01:01 00:00:00"), Some(0));
    assert_eq!(parse_date_time("2000:03:01 00:00:00"), Some(951_868_800));
    assert_eq!(parse_date_time("2024-02-29T23:59:59"), Some(at(2024, 2, 29, 23, 59, 59)));
    assert_eq!(parse_date_time("2024-02-29T23:59:59"), Some(1_709_251_199));
    assert_eq!(parse_date_time("1900:01:01 00:00:00"), Some(-2_208_988_800));
    // A day past the month's end rolls into the next month.
    assert_eq!(parse_date_time("2025:02:31 00:00:00"), parse_date_time("2025:03:03 00:00:00"));
    assert_eq!(parse_date_time("2025:03:22 12:00:60"), parse_date_time("2025:03:22 12:01:00"));
    assert_eq!(parse_date_time("0000:00:00 00:00:00"), None);
    assert_eq!(parse_date_time("    :  :     :  :  "), None);
    assert_eq!(parse_date_time("2025:13:01 00:00:00"), None);
    assert_eq!(parse_date_time("2025:03:22"), None);
}

#[test]
fn subsec_parsing() {
    assert_eq!(parse_subsec("7"), Some(700_000));
    assert_eq!(parse_subsec("070"), Some(70_000));
    assert_eq!(parse_subsec("000"), Some(0));
    assert_eq!(parse_subsec(" 45 "), Some(450_000));
    assert_eq!(parse_subsec("1234567"), Some(123_456));
    assert_eq!(parse_subsec(""), None);
    assert_eq!(parse_subsec("  "), None);
}
