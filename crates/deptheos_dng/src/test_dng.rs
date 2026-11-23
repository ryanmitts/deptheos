//! A small DNG written by hand, for tests: linear RGB or CFA (mosaic), in
//! tiles or strips, either byte order. Uncompressed, so the bridge's fast
//! read paths take it.

use std::path::Path;

/// How the test file stores its pixels.
#[derive(Clone, Copy)]
pub enum Layout {
    Tiles { width: u32, length: u32 },
    Strips { rows: u32 },
}

pub struct TestDng {
    pub width: u32,
    pub height: u32,
    /// A CFA (mosaic, RGGB) file of one sample per pixel, rather than linear
    /// RGB. Its black and white levels are the first of `black` and `white`.
    pub cfa: bool,
    pub big_endian: bool,
    pub layout: Layout,
    pub black: [u16; 3],
    pub white: [u32; 3],
    /// Top, left, bottom, right.
    pub active: [u32; 4],
    /// Origin x, y and size w, h, within the active area.
    pub crop: [u32; 4],
}

impl TestDng {
    /// Samples per pixel: 1 for CFA, 3 for linear RGB.
    pub fn samples(&self) -> u32 {
        if self.cfa { 1 } else { 3 }
    }

    /// Interleaved samples covering the whole range, below black and above
    /// white.
    pub fn pixels(&self) -> Vec<u16> {
        (0..self.width * self.height * self.samples())
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u16)
            .collect()
    }

    pub fn write(&self, path: &Path, pixels: &[u16]) {
        self.write_file(path, pixels, &Extras::default());
    }

    /// [`Self::write`] with the tags that say whose camera it was and where:
    /// the Artist [`OWNER`], the body's and the lens's serial numbers
    /// [`BODY_SERIAL`] and [`LENS_SERIAL`], and a GPS latitude.
    pub fn write_identified(&self, path: &Path, pixels: &[u16]) {
        self.write_file(path, pixels, &Extras { identified: true, ..Extras::default() });
    }

    /// [`Self::write`] with an OpcodeList3 (`list`, as the tag holds it) and
    /// the TIFF orientation `orientation`.
    pub fn write_with_opcode_list3(
        &self,
        path: &Path,
        pixels: &[u16],
        orientation: u16,
        list: &[u8],
    ) {
        let extras = Extras { orientation, opcode_list3: list.to_vec(), ..Extras::default() };
        self.write_file(path, pixels, &extras);
    }

    fn write_file(&self, path: &Path, pixels: &[u16], extras: &Extras) {
        let identified = extras.identified;
        let (w, h, samples) = (self.width, self.height, self.samples());
        let n = samples as usize;
        // Each tile or strip as (x, y, w, h). Edge tiles run past the image;
        // the last strip is cut short.
        let blocks: Vec<(u32, u32, u32, u32)> = match self.layout {
            Layout::Tiles { width, length } => {
                let mut v = Vec::new();
                for ty in 0..h.div_ceil(length) {
                    for tx in 0..w.div_ceil(width) {
                        v.push((tx * width, ty * length, width, length));
                    }
                }
                v
            }
            Layout::Strips { rows } => {
                (0..h.div_ceil(rows)).map(|s| (0, s * rows, w, rows.min(h - s * rows))).collect()
            }
        };
        let byte_counts: Vec<u32> = blocks.iter().map(|b| b.2 * b.3 * 2 * samples).collect();

        let mut e = Encoder { big: self.big_endian, entries: Vec::new() };
        e.long(254, &[0]);
        e.long(256, &[w]);
        e.long(257, &[h]);
        e.short(258, &[16, 16, 16][..n]);
        e.short(259, &[1]);
        e.short(262, &[if self.cfa { 32803 } else { 34892 }]);
        e.ascii(271, "Test");
        let offsets_tag = match self.layout {
            Layout::Strips { rows } => {
                e.long(273, &vec![0; blocks.len()]);
                e.short(274, &[extras.orientation]);
                e.short(277, &[samples as u16]);
                e.long(278, &[rows]);
                e.long(279, &byte_counts);
                273
            }
            Layout::Tiles { width, length } => {
                e.short(274, &[extras.orientation]);
                e.short(277, &[samples as u16]);
                e.long(322, &[width]);
                e.long(323, &[length]);
                e.long(324, &vec![0; blocks.len()]);
                e.long(325, &byte_counts);
                324
            }
        };
        e.short(284, &[1]);
        e.short(339, &[1, 1, 1][..n]);
        if self.cfa {
            e.short(33421, &[2, 2]);
            e.bytes(33422, &[0, 1, 1, 2]);
            e.bytes(50710, &[0, 1, 2]);
            e.short(50711, &[1]);
        }
        e.bytes(50706, &[1, 4, 0, 0]);
        e.ascii(50708, "Test Camera");
        e.short(50714, &self.black[..n]);
        e.long(50717, &self.white[..n]);
        e.long(50719, &self.crop[..2]);
        e.long(50720, &self.crop[2..]);
        e.srational(
            50721,
            &[(1, 1), (0, 1), (0, 1), (0, 1), (1, 1), (0, 1), (0, 1), (0, 1), (1, 1)],
        );
        e.rational(50728, &[(1, 1), (1, 1), (1, 1)]);
        e.short(50778, &[21]);
        e.long(50829, &self.active);
        if !extras.opcode_list3.is_empty() {
            e.undefined(51022, &extras.opcode_list3);
        }
        if identified {
            e.ascii(315, OWNER);
            e.ascii(50735, BODY_SERIAL);
            e.ascii(42037, LENS_SERIAL);
            // GPSInfo: the GPS IFD follows the pixels, its offset set below.
            e.long(34853, &[0]);
        }
        e.entries.sort_by_key(|entry| entry.0);

        // Header, IFD, the values that don't fit in an entry, then pixels.
        let ifd_size = 2 + 12 * e.entries.len() + 4;
        let extra: usize = e
            .entries
            .iter()
            .map(|x| if x.3.len() > 4 { x.3.len().next_multiple_of(2) } else { 0 })
            .sum();
        let mut pixel_offset = (8 + ifd_size + extra) as u32;
        let offsets: Vec<u32> = byte_counts
            .iter()
            .map(|&n| {
                let at = pixel_offset;
                pixel_offset += n;
                at
            })
            .collect();
        let encoded = e.encode_longs(&offsets);
        e.entries.iter_mut().find(|x| x.0 == offsets_tag).unwrap().3 = encoded;
        let gps_at = pixel_offset;
        let gps_offset = e.u32(gps_at).to_vec();
        if let Some(gps) = e.entries.iter_mut().find(|x| x.0 == 34853) {
            gps.3 = gps_offset;
        }

        let mut out = Vec::new();
        out.extend_from_slice(if self.big_endian { b"MM\0\x2a" } else { b"II\x2a\0" });
        out.extend_from_slice(&e.u32(8));
        let mut extra_at = (8 + ifd_size) as u32;
        let mut tail = Vec::new();
        out.extend_from_slice(&e.u16(e.entries.len() as u16));
        for (tag, kind, count, value) in &e.entries {
            out.extend_from_slice(&e.u16(*tag));
            out.extend_from_slice(&e.u16(*kind));
            out.extend_from_slice(&e.u32(*count));
            if value.len() <= 4 {
                let mut v = value.clone();
                v.resize(4, 0);
                out.extend_from_slice(&v);
            } else {
                out.extend_from_slice(&e.u32(extra_at));
                tail.extend_from_slice(value);
                if value.len() % 2 == 1 {
                    tail.push(0);
                }
                extra_at += value.len().next_multiple_of(2) as u32;
            }
        }
        out.extend_from_slice(&e.u32(0));
        out.extend_from_slice(&tail);
        for &(x0, y0, bw, bh) in &blocks {
            for y in y0..y0 + bh {
                for x in x0..x0 + bw {
                    for c in 0..samples {
                        // Padding past the edge holds junk the reader must drop.
                        let v = if x < w && y < h {
                            pixels[((y * w + x) * samples + c) as usize]
                        } else {
                            0xBEEF
                        };
                        out.extend_from_slice(&e.u16(v));
                    }
                }
            }
        }
        if identified {
            // GPSVersionID, GPSLatitudeRef and GPSLatitude, whose three
            // rationals follow the IFD.
            assert_eq!(out.len(), gps_at as usize);
            let latitude_at = gps_at + 2 + 3 * 12 + 4;
            out.extend_from_slice(&e.u16(3));
            for (tag, kind, count, value) in [
                (0u16, 1u16, 4u32, [2, 3, 0, 0]),
                (1, 2, 2, [b'N', 0, 0, 0]),
                (2, 5, 3, e.u32(latitude_at)),
            ] {
                out.extend_from_slice(&e.u16(tag));
                out.extend_from_slice(&e.u16(kind));
                out.extend_from_slice(&e.u32(count));
                out.extend_from_slice(&value);
            }
            out.extend_from_slice(&e.u32(0));
            for (n, d) in [(48, 1), (51, 1), (29_871, 1000)] {
                out.extend_from_slice(&e.u32(n));
                out.extend_from_slice(&e.u32(d));
            }
        }
        std::fs::write(path, out).unwrap();
    }
}

/// A linear RGB DNG of one `width` × `height` tile (multiples of 16)
/// compressed as lossy JPEG, whose tile is `jpeg`: 8 bits a sample, as
/// DNG's lossy JPEG is.
pub fn write_lossy_jpeg(path: &Path, width: u32, height: u32, jpeg: &[u8]) {
    let mut e = Encoder { big: false, entries: Vec::new() };
    e.long(254, &[0]);
    e.long(256, &[width]);
    e.long(257, &[height]);
    e.short(258, &[8, 8, 8]);
    e.short(259, &[34892]);
    e.short(262, &[34892]);
    e.ascii(271, "Test");
    e.short(274, &[1]);
    e.short(277, &[3]);
    e.short(284, &[1]);
    e.long(322, &[width]);
    e.long(323, &[height]);
    e.long(324, &[0]);
    e.long(325, &[jpeg.len() as u32]);
    e.bytes(50706, &[1, 4, 0, 0]);
    e.ascii(50708, "Test Camera");
    e.srational(50721, &[(1, 1), (0, 1), (0, 1), (0, 1), (1, 1), (0, 1), (0, 1), (0, 1), (1, 1)]);
    e.rational(50728, &[(1, 1), (1, 1), (1, 1)]);
    e.short(50778, &[21]);
    e.entries.sort_by_key(|entry| entry.0);
    let ifd_size = 2 + 12 * e.entries.len() + 4;
    let extra: usize = e
        .entries
        .iter()
        .map(|x| if x.3.len() > 4 { x.3.len().next_multiple_of(2) } else { 0 })
        .sum();
    let tile_at = (8 + ifd_size + extra) as u32;
    e.entries.iter_mut().find(|x| x.0 == 324).unwrap().3 = tile_at.to_le_bytes().to_vec();
    let mut out = b"II\x2a\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    let mut extra_at = (8 + ifd_size) as u32;
    let mut tail = Vec::new();
    out.extend_from_slice(&(e.entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in &e.entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if value.len() <= 4 {
            let mut v = value.clone();
            v.resize(4, 0);
            out.extend_from_slice(&v);
        } else {
            out.extend_from_slice(&extra_at.to_le_bytes());
            tail.extend_from_slice(value);
            if value.len() % 2 == 1 {
                tail.push(0);
            }
            extra_at += value.len().next_multiple_of(2) as u32;
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&tail);
    assert_eq!(out.len(), tile_at as usize);
    out.extend_from_slice(jpeg);
    std::fs::write(path, out).unwrap();
}

/// What a test file has besides its pixels.
struct Extras {
    /// Artist, serial numbers and GPS ([`TestDng::write_identified`]).
    identified: bool,
    /// The TIFF orientation, 1 for upright as stored.
    orientation: u16,
    /// OpcodeList3's bytes; none when empty.
    opcode_list3: Vec<u8>,
}

impl Default for Extras {
    fn default() -> Self {
        Self { identified: false, orientation: 1, opcode_list3: Vec::new() }
    }
}

/// An OpcodeList3 of one optional WarpRectilinear for one plane, centered
/// at (`cx`, `cy`) of the image, as the OpcodeList3 tag holds it (big
/// endian, whatever the file's byte order).
pub fn warp_rectilinear_list(cx: f64, cy: f64) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&1u32.to_be_bytes()); // opcodes
    v.extend_from_slice(&1u32.to_be_bytes()); // WarpRectilinear
    v.extend_from_slice(&[1, 3, 0, 0]); // DNG 1.3
    v.extend_from_slice(&1u32.to_be_bytes()); // optional
    let params: Vec<u8> = std::iter::once(1u32.to_be_bytes().to_vec())
        .chain(
            [1.0, 0.01, 0.0, 0.0, 0.0, 0.0, cx, cy].iter().map(|x: &f64| x.to_be_bytes().to_vec()),
        )
        .flatten()
        .collect();
    v.extend_from_slice(&(params.len() as u32).to_be_bytes());
    v.extend_from_slice(&params);
    v
}

/// The Artist [`TestDng::write_identified`] writes.
pub const OWNER: &str = "Jane Q. Owner";
/// The body's serial number [`TestDng::write_identified`] writes.
pub const BODY_SERIAL: &str = "BODYSERIAL0001";
/// The lens's serial number [`TestDng::write_identified`] writes.
pub const LENS_SERIAL: &str = "LENSSERIAL0002";

/// TIFF entries as (tag, type, count, value bytes).
struct Encoder {
    big: bool,
    entries: Vec<(u16, u16, u32, Vec<u8>)>,
}

impl Encoder {
    fn u16(&self, v: u16) -> [u8; 2] {
        if self.big { v.to_be_bytes() } else { v.to_le_bytes() }
    }
    fn u32(&self, v: u32) -> [u8; 4] {
        if self.big { v.to_be_bytes() } else { v.to_le_bytes() }
    }
    fn encode_longs(&self, v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|&x| self.u32(x)).collect()
    }
    fn bytes(&mut self, tag: u16, v: &[u8]) {
        self.entries.push((tag, 1, v.len() as u32, v.to_vec()));
    }
    fn undefined(&mut self, tag: u16, v: &[u8]) {
        self.entries.push((tag, 7, v.len() as u32, v.to_vec()));
    }
    fn ascii(&mut self, tag: u16, s: &str) {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        self.entries.push((tag, 2, v.len() as u32, v));
    }
    fn short(&mut self, tag: u16, v: &[u16]) {
        let b = v.iter().flat_map(|&x| self.u16(x)).collect();
        self.entries.push((tag, 3, v.len() as u32, b));
    }
    fn long(&mut self, tag: u16, v: &[u32]) {
        let b = self.encode_longs(v);
        self.entries.push((tag, 4, v.len() as u32, b));
    }
    fn rational(&mut self, tag: u16, v: &[(u32, u32)]) {
        let b = v.iter().flat_map(|&(n, d)| [self.u32(n), self.u32(d)]).flatten().collect();
        self.entries.push((tag, 5, v.len() as u32, b));
    }
    fn srational(&mut self, tag: u16, v: &[(i32, i32)]) {
        let b = v
            .iter()
            .flat_map(|&(n, d)| [self.u32(n as u32), self.u32(d as u32)])
            .flatten()
            .collect();
        self.entries.push((tag, 10, v.len() as u32, b));
    }
}
