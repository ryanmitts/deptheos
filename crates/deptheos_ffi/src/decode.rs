//! Decoding frames through the app's [`FrameDecoder`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_core::CoreError;
use deptheos_core::io::{self, HostDecoder, OpenFrame};

use crate::EngineError;

/// What a frame's samples are stored as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SampleDepth {
    Eight,
    /// Native-endian.
    Sixteen,
    /// 32-bit float, native-endian.
    Float,
}

/// How a pixel's samples are laid out. Alpha is premultiplied, as a bitmap
/// context draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FrameLayout {
    Gray,
    GrayAlpha,
    /// Red, green, blue and a fourth sample the engine skips.
    Rgbx,
    Rgba,
}

impl FrameLayout {
    fn channels(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::GrayAlpha => 2,
            Self::Rgbx | Self::Rgba => 4,
        }
    }
}

/// A frame's colors.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FrameColor {
    /// In the space this ICC profile describes.
    Icc {
        profile: Vec<u8>,
    },
    /// No profile: taken as sRGB.
    Untagged,
    /// Refused by name before anything is drawn.
    Cmyk,
    Lab,
    /// A space with an ITU-R BT.2100 transfer.
    Hdr,
}

/// A frame as the app's decoder opened it.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FrameHeader {
    /// As stored, not turned.
    pub width: u32,
    pub height: u32,
    pub depth: SampleDepth,
    pub layout: FrameLayout,
    pub color: FrameColor,
    /// The EXIF orientation, 1 to 8, which the engine applies.
    pub orientation: u8,
}

/// The app's decoder for every standard image, and camera RAW and mosaic
/// DNGs when a stack asks (`CameraRaw::Host`, ADR-131). Called from the
/// prefetch threads, several frames at once: it must be thread-safe.
#[uniffi::export(foreign)]
pub trait FrameDecoder: Send + Sync {
    /// Opens `path` once, for its header then its pixels (ADR-132). Fails
    /// when it can't be read as an image. Not `open`: a Swift keyword the
    /// C header carries unescaped.
    fn open_frame(&self, path: PathBuf) -> std::result::Result<Arc<dyn OpenedFrame>, EngineError>;
}

/// A frame the app's decoder opened.
#[uniffi::export(foreign)]
pub trait OpenedFrame: Send + Sync {
    /// What the frame is, without its pixels.
    fn header(&self) -> FrameHeader;

    /// Draws the frame as stored into `destination`: `width × height` pixels
    /// in the header's layout, depth and color, rows tightly packed.
    /// `destination` addresses a buffer of `length` bytes live for the call;
    /// the decoder must write exactly that size.
    fn decode_into(&self, destination: u64, length: u64) -> std::result::Result<(), EngineError>;
}

/// [`FrameDecoder`] as the engine's [`HostDecoder`].
pub(crate) struct HostAdapter(pub(crate) Arc<dyn FrameDecoder>);

/// Most pixels a decoded frame may have, as the JXL loader allows
/// (`deptheos_dng::jxl`): the size comes from the header, so a broken or
/// hostile one must not ask for any amount of memory.
pub(crate) const MAX_DECODE_PIXELS: u64 = 1 << 29;

/// Why a frame's header size can't be decoded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unfit {
    /// No pixels.
    Empty,
    /// More than [`MAX_DECODE_PIXELS`], or more than memory can address.
    TooLarge,
}

/// The samples of a `w × h` frame of `channels` a pixel, if within
/// [`MAX_DECODE_PIXELS`] and not empty.
pub(crate) fn frame_samples(w: u32, h: u32, channels: usize) -> std::result::Result<usize, Unfit> {
    let pixels = u64::from(w) * u64::from(h);
    if pixels == 0 {
        return Err(Unfit::Empty);
    }
    if pixels > MAX_DECODE_PIXELS {
        return Err(Unfit::TooLarge);
    }
    usize::try_from(pixels).ok().and_then(|p| p.checked_mul(channels)).ok_or(Unfit::TooLarge)
}

/// `n` zeroes, or `None` when they can't be allocated.
fn zeroed<T: Clone + Default>(n: usize) -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, T::default());
    Some(v)
}

impl HostDecoder for HostAdapter {
    fn open(&self, path: &Path) -> deptheos_core::Result<Box<dyn OpenFrame>> {
        let undecodable = |message: String| CoreError::Decode { path: path.to_path_buf(), message };
        let opened =
            self.0.open_frame(path.to_path_buf()).map_err(|e| undecodable(e.to_string()))?;
        let header = opened.header();
        let refused = |what| CoreError::Refused { path: path.to_path_buf(), what };
        let color = match &header.color {
            FrameColor::Icc { profile } => io::ColorTag::Icc(profile.clone()),
            FrameColor::Untagged => io::ColorTag::Untagged,
            FrameColor::Cmyk => return Err(refused(io::Refused::Cmyk)),
            FrameColor::Lab => return Err(refused(io::Refused::Lab)),
            FrameColor::Hdr => return Err(refused(io::Refused::Hdr)),
        };
        let premultiplied = io::Alpha::Premultiplied;
        let decoded = io::DecodedHeader {
            width: header.width,
            height: header.height,
            depth: match header.depth {
                SampleDepth::Eight => io::StoredDepth::U8,
                SampleDepth::Sixteen => io::StoredDepth::U16,
                SampleDepth::Float => io::StoredDepth::F32,
            },
            layout: match header.layout {
                FrameLayout::Gray => io::Layout::Gray,
                FrameLayout::GrayAlpha => io::Layout::GrayAlpha(premultiplied),
                FrameLayout::Rgbx => io::Layout::Rgb,
                FrameLayout::Rgba => io::Layout::RgbAlpha(premultiplied),
            },
            color,
            orientation: image::metadata::Orientation::from_exif(header.orientation)
                .unwrap_or(image::metadata::Orientation::NoTransforms),
        };
        Ok(Box::new(HostFrame { opened, path: path.to_path_buf(), header, decoded }))
    }
}

/// A frame the app's decoder opened, with its header as the engine reads it.
struct HostFrame {
    opened: Arc<dyn OpenedFrame>,
    path: PathBuf,
    header: FrameHeader,
    decoded: io::DecodedHeader,
}

impl OpenFrame for HostFrame {
    fn header(&self) -> &io::DecodedHeader {
        &self.decoded
    }

    fn decode(self: Box<Self>) -> deptheos_core::Result<io::Decoded> {
        let Self { opened, path, header, decoded } = *self;
        let undecodable = |message: String| CoreError::Decode { path: path.clone(), message };
        let (w, h) = (header.width, header.height);
        let samples =
            frame_samples(w, h, header.layout.channels()).map_err(|unfit| match unfit {
                Unfit::Empty => undecodable(format!("the header claims {w} × {h}")),
                Unfit::TooLarge => CoreError::TooLarge {
                    path: path.clone(),
                    width: w,
                    height: h,
                    max_pixels: MAX_DECODE_PIXELS,
                },
            })?;
        let no_memory = || CoreError::NoMemory { path: path.clone(), width: w, height: h };
        // The decoder gets a raw address. Sound because the buffer outlives
        // the synchronous call, Rust doesn't touch it until it returns, and
        // the length is exactly its size in bytes.
        fn fill<T: Clone + Default>(
            opened: &dyn OpenedFrame,
            samples: usize,
        ) -> Option<std::result::Result<Vec<T>, EngineError>> {
            let mut pixels = zeroed::<T>(samples)?;
            let bytes = samples.checked_mul(std::mem::size_of::<T>())? as u64;
            Some(opened.decode_into(pixels.as_mut_ptr() as u64, bytes).map(|()| pixels))
        }
        let failed = |e: EngineError| undecodable(e.to_string());
        let skip = header.layout == FrameLayout::Rgbx;
        let samples = match header.depth {
            SampleDepth::Eight => {
                let pixels =
                    fill::<u8>(&*opened, samples).ok_or_else(no_memory)?.map_err(failed)?;
                io::Samples::U8(if skip { skip_fourth(pixels) } else { pixels })
            }
            SampleDepth::Sixteen => {
                let pixels =
                    fill::<u16>(&*opened, samples).ok_or_else(no_memory)?.map_err(failed)?;
                io::Samples::U16(if skip { skip_fourth(pixels) } else { pixels })
            }
            SampleDepth::Float => {
                let pixels =
                    fill::<f32>(&*opened, samples).ok_or_else(no_memory)?.map_err(failed)?;
                io::Samples::F32(if skip { skip_fourth(pixels) } else { pixels })
            }
        };
        Ok(io::Decoded { header: decoded, samples })
    }
}

/// RGBX samples as RGB, in the same allocation.
fn skip_fourth<T: Copy>(mut samples: Vec<T>) -> Vec<T> {
    let pixels = samples.len() / 4;
    for i in 0..pixels {
        for c in 0..3 {
            samples[i * 3 + c] = samples[i * 4 + c];
        }
    }
    samples.truncate(pixels * 3);
    samples.shrink_to_fit();
    samples
}
