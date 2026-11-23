//! How camera RAW frames are decoded: the backend a stack chooses.

use std::path::Path;

use deptheos_core::{converter, io};

/// How camera RAW (CR3/NEF/RAF/…) decodes, recorded in each render so its
/// frames always decode as stacked. DNG and standard images ignore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawBackend {
    /// LibRaw decode (default).
    #[default]
    #[serde(rename = "libraw")]
    LibRaw,
    /// Adobe DNG Converter to linear DNG, then the DNG SDK. Frames don't
    /// line up with LibRaw's (different crop and demosaic); lens warp stays
    /// in OpcodeList3.
    AdobeDng,
    /// The host's decoder, Apple's on iOS (ADR-131): RAW and mosaic DNGs
    /// render to 16-bit Display P3 (ADR-132), so the stack can't export as
    /// DNG.
    Apple,
}

impl RawBackend {
    pub const ALL: [Self; 3] = [Self::LibRaw, Self::AdobeDng, Self::Apple];

    /// Its serde spelling, as preferences and render files store it.
    pub fn key(self) -> &'static str {
        match self {
            Self::LibRaw => "libraw",
            Self::AdobeDng => "adobe_dng",
            Self::Apple => "apple",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LibRaw => "LibRaw",
            Self::AdobeDng => "Adobe DNG Converter",
            Self::Apple => "Apple",
        }
    }

    /// Whether it runs here: Adobe needs the converter at its default path;
    /// Apple's is iOS only.
    pub fn is_available(self) -> bool {
        match self {
            Self::LibRaw => true,
            Self::AdobeDng => converter::is_available(),
            Self::Apple => io::CameraRaw::here() == io::CameraRaw::Host,
        }
    }

    /// Who decodes camera RAW (after any conversion to DNG).
    pub(crate) fn camera_raw(self) -> io::CameraRaw {
        match self {
            Self::LibRaw | Self::AdobeDng => io::CameraRaw::LibRaw,
            Self::Apple => io::CameraRaw::Host,
        }
    }

    /// Whether `path` decodes to linear camera RGB under this backend
    /// ([`io::is_raw_source`]): a stack of such frames is a RAW stack.
    pub fn decodes_linear(self, path: &Path) -> bool {
        io::is_raw_source(path, self.camera_raw())
    }

    /// Whether the backend decides how `path` decodes on iOS
    /// ([`io::host_may_decode`]).
    pub fn chooses(path: &Path) -> bool {
        io::host_may_decode(path)
    }

    /// Whether `path` is converted first: non-DNG RAW under Adobe.
    pub fn converts(self, path: &Path) -> bool {
        self == Self::AdobeDng && io::is_raw_file(path) && !io::is_dng(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deptheos_dng::test_dng::{Layout, TestDng};

    /// The backend governs camera RAW and mosaic DNGs, never linear or
    /// unreadable DNGs or standard images.
    #[test]
    fn the_backend_chooses_for_camera_raw_and_mosaic_dngs() {
        let dir = tempfile::tempdir().unwrap();
        let write = |cfa: bool| {
            let path = dir.path().join(if cfa { "mosaic.dng" } else { "linear.dng" });
            let dng = TestDng {
                width: 32,
                height: 24,
                cfa,
                big_endian: false,
                layout: Layout::Strips { rows: 8 },
                black: [0; 3],
                white: [65535; 3],
                active: [0, 0, 24, 32],
                crop: [0, 0, 32, 24],
            };
            dng.write(&path, &dng.pixels());
            path
        };
        let (mosaic, linear) = (write(true), write(false));
        assert!(RawBackend::chooses(Path::new("/nowhere/frame.CR3")));
        assert!(RawBackend::chooses(&mosaic));
        assert!(!RawBackend::chooses(&linear));
        assert!(!RawBackend::chooses(Path::new("/nowhere/frame.dng")));
        assert!(!RawBackend::chooses(Path::new("/nowhere/frame.jpg")));
    }
}
