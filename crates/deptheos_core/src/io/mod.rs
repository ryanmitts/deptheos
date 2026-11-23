//! Image loading, frame prefetch and result saving.
//!
//! # Where to load what
//!
//! | To get | Call | It returns |
//! |---|---|---|
//! | A frame's pixels, to stack or show | [`load_frame`] | a [`Frame`]: upright pixels (RAW and DNG as 16-bit linear camera RGB, every other format through [`input_frame`] in the space asked for), its color space, the area with data, and the notes the load made |
//! | A frame's size and depth, without decoding it | [`header_info`] | a [`Header`] (upright width and height, 8 or 16 bits), `None` when it can't be read or stacked |
//! | A RAW or DNG frame with its color science | [`decode_linear`] | a [`LinearImage`]: 16-bit linear camera RGB and its [`LinearColor`] (the DNG's own, or LibRaw's) |
//! | When, with which camera, which shot of a burst | `deptheos_app::frames::capture::read_from` | a `CaptureInfo` (capture time, model, burst shot), read from small blocks of the file |
//! | The tags an export copies from its reference frame | `deptheos_app::frames::capture::read_source_tags` | `SourceTags`: the allowed IFD0 and Exif fields |
//! | A small picture for a list | `thumbnail` in deptheos_app's `frames/source_thumbnail.rs` | a `Thumbnail`: the file's embedded preview where it has one, else a render or decode, upright, BGRA8 |
//!
//! DNG goes through the Adobe DNG SDK (`deptheos_dng`), other camera RAW
//! through LibRaw (`deptheos_raw`), both to linear camera RGB. Standard
//! images go through the desktop's decoders, or the host's where one is
//! installed ([`set_host_decoder`], iOS; camera RAW too under
//! [`CameraRaw::Host`]), then the one input stage ([`input_frame`],
//! ADR-132). A stack is one kind or the other, never both.

mod decoders;
mod input;
mod jpeg;
mod load;
pub mod metadata;
mod prefetch;
mod profile;
mod replace;
mod save;
mod tiff;

pub use decoders::OpenFrame;
pub use deptheos_dng::FileCache;
pub use input::{
    Alpha, ColorTag, Decoded, DecodedHeader, Layout, LoadNote, Refused, Samples, StackSpace,
    StoredDepth, Target, input_frame, input_header,
};
pub use jpeg::{JPEG_QUALITY, JPEG_QUALITY_RANGE};
pub use load::{
    CameraRaw, Frame, Header, HostDecoder, LinearColor, LinearImage, decode_linear, header_info,
    host_may_decode, is_dng, is_mosaic_dng, is_raw_file, is_raw_source, load_frame, load_image,
    set_host_decoder, standard_header, supported_extensions,
};
pub use prefetch::{FramePrefetcher, Loader};
pub use profile::{IccProfile, ToSrgb};
pub use replace::{is_temporary, rename_retrying, sync_file, sync_folder, write_replacing};
pub use save::{
    Encoding, dither_rgb_to_u8, quantize_u16, rgba_row_to_rgb_u16, save_rgb16, u16_to_u8,
};
pub use tiff::TiffCompression;
