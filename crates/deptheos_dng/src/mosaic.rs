//! Whether a DNG holds a mosaic (CFA) or linear image, from its TIFF
//! structure alone: a few hundred bytes read, no SDK, so the kind of every
//! DNG in a list can be told while the app draws (ADR-131).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const NEW_SUBFILE_TYPE: u16 = 254;
const PHOTOMETRIC: u16 = 262;
const SUB_IFDS: u16 = 330;
/// PhotometricInterpretation of a mosaic: Color Filter Array.
const CFA: u32 = 32803;
/// IFDs looked at, at most: IFD 0 and its SubIFDs (a DNG's main image is
/// one of them, often beside a preview or two).
const MOST_IFDS: usize = 16;
/// Entries in one IFD, at most: a DNG's IFD 0 has a hundred or so.
const MOST_ENTRIES: u16 = 1024;

/// Whether the DNG at `path`'s main image (its IFD whose NewSubfileType is
/// 0) is a mosaic. Linear DNGs (Adobe DNG Converter's `-l`, Apple ProRAW)
/// are not. An error when the file isn't a TIFF or has no main image.
pub fn is_mosaic(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header)?;
    let big = match &header[..4] {
        b"II*\0" => false,
        b"MM\0*" => true,
        _ => return Err(invalid("not a TIFF file")),
    };
    let tiff = Tiff { file, big };
    let mut pending = vec![tiff.u32_at(&header[4..8])];
    let mut seen = 0;
    while let Some(offset) = pending.pop() {
        seen += 1;
        if seen > MOST_IFDS {
            break;
        }
        let ifd = tiff.read_ifd(offset)?;
        if ifd.subfile_type == 0
            && let Some(photometric) = ifd.photometric
        {
            return Ok(photometric == CFA);
        }
        pending.extend(ifd.sub_ifds);
    }
    Err(invalid("no main image"))
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

struct Tiff {
    file: File,
    big: bool,
}

/// What [`is_mosaic`] needs of one IFD.
struct Ifd {
    subfile_type: u32,
    photometric: Option<u32>,
    sub_ifds: Vec<u32>,
}

impl Tiff {
    fn u16_at(&self, b: &[u8]) -> u16 {
        let b = [b[0], b[1]];
        if self.big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
    }

    fn u32_at(&self, b: &[u8]) -> u32 {
        let b = [b[0], b[1], b[2], b[3]];
        if self.big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buf)
    }

    /// An entry's single SHORT or LONG value, held in the entry.
    fn value(&self, entry: &[u8]) -> Option<u32> {
        match self.u16_at(&entry[2..4]) {
            3 => Some(u32::from(self.u16_at(&entry[8..10]))),
            4 | 13 => Some(self.u32_at(&entry[8..12])),
            _ => None,
        }
    }

    fn read_ifd(&self, offset: u32) -> io::Result<Ifd> {
        let mut count = [0u8; 2];
        self.read_at(u64::from(offset), &mut count)?;
        let count = self.u16_at(&count).min(MOST_ENTRIES);
        let mut entries = vec![0u8; usize::from(count) * 12];
        self.read_at(u64::from(offset) + 2, &mut entries)?;
        let mut ifd = Ifd { subfile_type: 0, photometric: None, sub_ifds: Vec::new() };
        for entry in entries.as_chunks::<12>().0 {
            match self.u16_at(&entry[..2]) {
                NEW_SUBFILE_TYPE => ifd.subfile_type = self.value(entry).unwrap_or(0),
                PHOTOMETRIC => ifd.photometric = self.value(entry),
                SUB_IFDS => ifd.sub_ifds = self.offsets(entry)?,
                _ => {}
            }
        }
        Ok(ifd)
    }

    /// The SubIFDs entry's offsets: in the entry for one, else at the
    /// offset it holds.
    fn offsets(&self, entry: &[u8]) -> io::Result<Vec<u32>> {
        let n = self.u32_at(&entry[4..8]).min(MOST_IFDS as u32) as usize;
        match n {
            // None: the entry's value is not an offset.
            0 => return Ok(Vec::new()),
            1 => return Ok(self.value(entry).into_iter().collect()),
            _ => {}
        }
        let mut raw = vec![0u8; n * 4];
        self.read_at(u64::from(self.u32_at(&entry[8..12])), &mut raw)?;
        Ok(raw.as_chunks::<4>().0.iter().map(|b| self.u32_at(b)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dng::{Layout, TestDng};

    fn dng(cfa: bool, big_endian: bool) -> TestDng {
        TestDng {
            width: 32,
            height: 24,
            cfa,
            big_endian,
            layout: Layout::Strips { rows: 8 },
            black: [0; 3],
            white: [65535; 3],
            active: [0, 0, 24, 32],
            crop: [0, 0, 32, 24],
        }
    }

    #[test]
    fn tells_a_mosaic_from_a_linear_dng() {
        let dir = tempfile::tempdir().unwrap();
        for big in [false, true] {
            for cfa in [false, true] {
                let path = dir.path().join(format!("{cfa}-{big}.dng"));
                let dng = dng(cfa, big);
                dng.write(&path, &dng.pixels());
                assert_eq!(is_mosaic(&path).unwrap(), cfa, "cfa {cfa}, big-endian {big}");
            }
        }
        let text = dir.path().join("not.dng");
        std::fs::write(&text, b"hello, world").unwrap();
        assert!(is_mosaic(&text).is_err());
        assert!(is_mosaic(&dir.path().join("absent.dng")).is_err());
    }

    /// Real DNGs, `;`-separated in `DEPTHEOS_DNG_MOSAIC`, each said
    /// mosaic or linear, and checked against the SDK's read where it is
    /// built in.
    #[test]
    #[ignore]
    fn real_files() {
        let paths = std::env::var("DEPTHEOS_DNG_MOSAIC").expect("DEPTHEOS_DNG_MOSAIC");
        for path in paths.split(';').filter(|p| !p.is_empty()) {
            let path = Path::new(path);
            let mosaic = is_mosaic(path).unwrap();
            println!("{}: {}", path.display(), if mosaic { "mosaic" } else { "linear" });
            let read = crate::DngImage::read_with_pixels(
                path,
                crate::FileCache::Use,
                crate::Budget::unlimited(),
            )
            .unwrap();
            assert_eq!(mosaic, matches!(read, crate::DngRead::Mosaic(_)), "{}", path.display());
        }
    }
}
