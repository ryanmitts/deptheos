use std::path::PathBuf;

use thiserror::Error;

pub use crate::converter::ConverterError;

/// The engine's errors. The text is for the log; what a person is told is
/// worded in `deptheos_app::words::explain`. `InvalidState` and `NoFrames`
/// are faults: the code was misused.
#[derive(Error, Debug)]
pub enum CoreError {
    #[error(transparent)]
    Gpu(#[from] deptheos_gpu::GpuError),

    #[error("{0}")]
    InvalidState(String),

    /// The work's cancel token was canceled ([`crate::cancel`]): a codec
    /// stopped mid-decode, a wait gave up. Never a frame left out.
    #[error("canceled")]
    Canceled,

    #[error("no frames accumulated")]
    NoFrames,

    #[error("image: {0}")]
    Image(#[from] image::ImageError),

    #[error(transparent)]
    Raw(deptheos_raw::RawError),

    #[error(transparent)]
    Dng(deptheos_dng::DngError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Converter(#[from] ConverterError),

    /// A frame of another size than the stack's first (`check_frame_size`).
    #[error("frame is {}×{} but the stack's first frame is {}×{}", size.0, size.1, first.0, first.1)]
    FrameSize { size: (u32, u32), first: (u32, u32) },

    /// A method's setting outside its range (`min` to `max`, inclusive).
    #[error("{setting} {value} outside {min} to {max}")]
    OutOfRange { setting: Setting, value: f32, min: f32, max: f32 },

    /// Frames too small for the wavelet, which needs `min_side` pixels a side.
    #[error("{width}×{height} frames: the wavelet needs at least {min_side} px a side")]
    FrameTooSmall { width: u32, height: u32, min_side: u32 },

    /// A decoded image in a pixel layout the upload does not take.
    #[error("pixel layout {0}")]
    PixelLayout(String),

    /// An image with no pixels (decoded, or given to an encoder).
    #[error("empty image")]
    EmptyImage,

    /// A file the decoder could not read as a picture.
    #[error("{}: decode: {message}", path.display())]
    Decode { path: PathBuf, message: String },

    /// A frame that can't be stacked, refused from its header: what it is.
    #[error("{}: refused, {what:?}", path.display())]
    Refused { path: PathBuf, what: crate::io::Refused },

    /// A JPEG that ends before its picture does: a copy or a download that
    /// stopped part-way.
    #[error("{}: the file ends before the picture does", path.display())]
    Truncated { path: PathBuf },

    /// A frame whose header claims more than `max_pixels` pixels, more than
    /// a decoder will allocate for.
    #[error("{}: {width}×{height} is past the decode limit of {max_pixels} pixels", path.display())]
    TooLarge { path: PathBuf, width: u32, height: u32, max_pixels: u64 },

    /// Not enough memory to decode a `width`×`height` frame.
    #[error("{}: no memory to decode {width}×{height}", path.display())]
    NoMemory { path: PathBuf, width: u32, height: u32 },

    /// A DNG that decodes to other than three channels.
    #[error("{}: DNG decodes to {channels} channels, need 3", path.display())]
    DngChannels { path: PathBuf, channels: u32 },

    /// A result too large for a JPEG (65535 pixels a side at most).
    #[error("{width}×{height} is too large for JPEG")]
    JpegTooLarge { width: u32, height: u32 },

    /// A result past the 4 GB a classic TIFF can address.
    #[error("the TIFF would pass 4 GB")]
    TiffTooLarge,

    /// The encoder for an export's `format` failed.
    #[error("{format} encoding: {message}")]
    Encode { format: &'static str, message: String },

    /// A save path whose symbolic links go round or too deep.
    #[error("{}: too many symbolic links", path.display())]
    SymlinkLoop { path: PathBuf },

    /// A save path that names no file.
    #[error("{}: not a file path", path.display())]
    NotAFile { path: PathBuf },
}

/// A codec's cancel is the work's ([`CoreError::Canceled`]), not a decode
/// failure.
impl From<deptheos_raw::RawError> for CoreError {
    fn from(e: deptheos_raw::RawError) -> Self {
        match e {
            deptheos_raw::RawError::Canceled => Self::Canceled,
            e => Self::Raw(e),
        }
    }
}

impl From<deptheos_dng::DngError> for CoreError {
    fn from(e: deptheos_dng::DngError) -> Self {
        match e {
            deptheos_dng::DngError::Canceled => Self::Canceled,
            e => Self::Dng(e),
        }
    }
}

/// A method's setting that [`CoreError::OutOfRange`] can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// Pyramid's.
    Smoothness,
    /// Depth's.
    Radius,
    Smoothing,
    BlendRange,
    /// Wavelet's.
    Denoise,
    Consistency,
}

impl std::fmt::Display for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Smoothness => "pyramid smoothness",
            Self::Radius => "depth radius",
            Self::Smoothing => "depth smoothing",
            Self::BlendRange => "depth blend_range",
            Self::Denoise => "wavelet denoise",
            Self::Consistency => "wavelet consistency",
        })
    }
}

impl Setting {
    /// [`CoreError::OutOfRange`] unless `value` is finite and in
    /// `(min, max)`, inclusive.
    pub fn check(self, value: f32, (min, max): (f32, f32)) -> Result<()> {
        if value.is_finite() && (min..=max).contains(&value) {
            Ok(())
        } else {
            Err(CoreError::OutOfRange { setting: self, value, min, max })
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Every frame in a stack must share the reference frame's pixel dimensions.
/// A mismatch usually means mixed sources: an edited output, another camera
/// or crop mode, or another converter.
pub(crate) fn check_frame_size(frame: (u32, u32), reference: (u32, u32)) -> Result<()> {
    if frame == reference {
        return Ok(());
    }
    Err(CoreError::FrameSize { size: frame, first: reference })
}
