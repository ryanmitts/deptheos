//! A batch's stacks keep one frame kind as decoded under the host's decoder
//! (iOS, ADR-131); its own binary, since the decoder is process-wide.

use std::path::Path;
use std::sync::Arc;

use deptheos_app::RawBackend;
use deptheos_app::frames::capture::CaptureInfo;
use deptheos_app::frames::folder::Source;
use deptheos_app::runs::batch::{self, Input, Split};
use deptheos_app::runs::session::check_same_kind;
use deptheos_app::words::Words;
use deptheos_core::io::{self, Decoded, DecodedHeader, HostDecoder, OpenFrame, Samples};

struct Host;

/// Any file, as the host opens it: 4×4 and black.
struct Opened(DecodedHeader);

impl OpenFrame for Opened {
    fn header(&self) -> &DecodedHeader {
        &self.0
    }

    fn decode(self: Box<Self>) -> deptheos_core::Result<Decoded> {
        Ok(Decoded { header: self.0, samples: Samples::U8(vec![0; 4 * 4 * 3]) })
    }
}

impl HostDecoder for Host {
    fn open(&self, _path: &Path) -> deptheos_core::Result<Box<dyn OpenFrame>> {
        Ok(Box::new(Opened(DecodedHeader {
            width: 4,
            height: 4,
            depth: io::StoredDepth::U8,
            layout: io::Layout::Rgb,
            color: io::ColorTag::Untagged,
            orientation: image::metadata::Orientation::NoTransforms,
        })))
    }
}

/// Under Apple's decoder camera RAW is a standard image and a linear DNG
/// stays RAW, so the DNG is the odd one out.
#[test]
fn a_batch_keeps_the_kind_the_decoder_makes() {
    io::set_host_decoder(Arc::new(Host));
    let dir = tempfile::tempdir().unwrap();
    // An empty `.dng` reads as no mosaic: a linear DNG.
    for name in ["a1.ORF", "a2.ORF", "a3.ORF", "a4.dng"] {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    let words = Words::english(deptheos_app::Platform::Ios);
    let plan = |backend| {
        batch::plan(
            &[Input::Folder(dir.path().to_path_buf())],
            Split::PerFolder,
            Source::Raw,
            deptheos_app::FrameOrder::Name,
            backend,
            |_: &Path| CaptureInfo::default(),
            &words,
        )
    };

    let apple = plan(RawBackend::Apple);
    let names: Vec<_> =
        apple.jobs[0].frames.iter().map(|f| f.file_name().unwrap().to_owned()).collect();
    assert_eq!(names, ["a1.ORF", "a2.ORF", "a3.ORF"]);
    assert_eq!(apple.skipped, 1);
    assert!(check_same_kind(&apple.jobs[0].frames, RawBackend::Apple).is_ok());

    // Under LibRaw all four decode to linear camera RGB.
    let libraw = plan(RawBackend::LibRaw);
    assert_eq!(libraw.jobs[0].frames.len(), 4);
    assert_eq!(libraw.skipped, 0);
}
