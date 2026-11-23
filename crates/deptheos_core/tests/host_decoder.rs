//! Which decoder a frame goes to once the host installed one (the iOS app,
//! ADR-131): its own test binary, since the host decoder is the process's
//! and would take the other tests' standard images.

use std::path::Path;
use std::sync::Arc;

use deptheos_core::FrameDepth;
use deptheos_core::io::{
    self, CameraRaw, ColorTag, Decoded, DecodedHeader, Header, HostDecoder, OpenFrame, Samples,
    StoredDepth,
};
use deptheos_dng::test_dng::{Layout, TestDng};

/// The host's header for any file: a size no other decoder gives.
const HOST: Header = Header { width: 7, height: 5, depth: FrameDepth::Eight };

struct Host;

/// Any file, as the host opens it: black, HOST's size.
struct Opened(DecodedHeader);

impl OpenFrame for Opened {
    fn header(&self) -> &DecodedHeader {
        &self.0
    }

    fn decode(self: Box<Self>) -> deptheos_core::Result<Decoded> {
        let samples = Samples::U8(vec![0; (HOST.width * HOST.height * 3) as usize]);
        Ok(Decoded { header: self.0, samples })
    }
}

impl HostDecoder for Host {
    fn open(&self, _path: &Path) -> deptheos_core::Result<Box<dyn OpenFrame>> {
        Ok(Box::new(Opened(DecodedHeader {
            width: HOST.width,
            height: HOST.height,
            depth: StoredDepth::U8,
            layout: io::Layout::Rgb,
            color: ColorTag::Untagged,
            orientation: image::metadata::Orientation::NoTransforms,
        })))
    }
}

#[test]
fn camera_raw_goes_where_the_stack_says() {
    io::set_host_decoder(Arc::new(Host));
    assert_eq!(CameraRaw::here(), CameraRaw::Host);
    let raw = Path::new("/nowhere/frame.CR3");
    let jpeg = Path::new("/nowhere/frame.jpg");
    let dng = Path::new("/nowhere/frame.dng");
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

    // Apple's decoder renders camera RAW: a standard image.
    assert!(!io::is_raw_source(raw, CameraRaw::Host));
    assert_eq!(io::header_info(raw, CameraRaw::Host), Some(HOST));
    // LibRaw decodes it linear, and its header is LibRaw's (none here,
    // the file being absent), never the host's.
    assert!(io::is_raw_source(raw, CameraRaw::LibRaw));
    assert_ne!(io::header_info(raw, CameraRaw::LibRaw), Some(HOST));

    // Standard images go to the host whichever decodes camera RAW.
    for decoder in [CameraRaw::Host, CameraRaw::LibRaw] {
        assert!(!io::is_raw_source(jpeg, decoder));
        assert_eq!(io::header_info(jpeg, decoder), Some(HOST));
        let image = io::load_image(jpeg, decoder).unwrap();
        assert_eq!((image.width(), image.height()), (HOST.width, HOST.height));
        // A linear DNG never goes to the host: nothing reads it better
        // than the SDK. Nor does a DNG that can't be read.
        for dng in [&linear, dng] {
            assert!(io::is_raw_source(dng, decoder));
            assert_ne!(io::header_info(dng, decoder), Some(HOST));
        }
    }

    // A mosaic DNG goes where camera RAW goes.
    assert!(io::is_mosaic_dng(&mosaic) && !io::is_mosaic_dng(&linear));
    assert!(!io::is_raw_source(&mosaic, CameraRaw::Host));
    assert_eq!(io::header_info(&mosaic, CameraRaw::Host), Some(HOST));
    assert!(io::is_raw_source(&mosaic, CameraRaw::LibRaw));
    assert_ne!(io::header_info(&mosaic, CameraRaw::LibRaw), Some(HOST));
}
