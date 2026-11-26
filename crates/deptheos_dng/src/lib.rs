//! Adobe DNG SDK access: read DNGs to linear camera RGB, render with the
//! SDK's own color science, and write results back as DNG.
//!
//! `build.rs` compiles the C++ side; `native/dng_c_api.h` is its plain-C
//! ABI, with DNG metadata behind an opaque handle that Rust only carries
//! from a source file to a written result.

use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::path::Path;
use std::ptr::NonNull;

pub use deptheos_raw::Budget;
use deptheos_raw::ffi::DtRawMetadata;
use thiserror::Error;

pub mod jpeg;
pub mod jxl;
mod mosaic;
pub use mosaic::is_mosaic;

/// Test DNGs, for this crate's tests and, through the `test-support`
/// feature, other crates' tests.
#[cfg(any(test, feature = "test-support"))]
pub mod test_dng;

#[cfg(all(test, dng_test_hooks))]
mod fast_read_tests;

#[cfg(test)]
mod white_tests;

/// What failed is the variant; why, when the SDK or libjxl said, is its
/// [`Cause`].
#[derive(Error, Debug)]
pub enum DngError {
    #[error("not valid for the C API: {0}")]
    InvalidPath(String),

    #[error("failed to read DNG {path}: {cause}")]
    ReadFailed { path: String, cause: Cause },

    #[error("failed to render DNG to sRGB: {0}")]
    RenderFailed(Cause),

    #[error("failed to write DNG {path}: {cause}")]
    WriteFailed { path: String, cause: Cause },

    #[error("failed to build DNG metadata from RAW metadata: {0}")]
    MetadataFailed(Cause),

    #[error("failed to compute DNG render parameters: {0}")]
    ParamsFailed(Cause),

    #[error("pixel buffer of {got} does not match {width}x{height}x{channels}")]
    PixelCountMismatch { got: usize, width: u32, height: u32, channels: u32 },

    #[error("JPEG XL {what} failed: {cause}")]
    JxlFailed { what: &'static str, cause: Cause },

    /// The file a JPEG XL encode wrote to failed.
    #[error("writing JPEG XL: {0}")]
    JxlWrite(std::io::Error),

    /// The read's cancel flag was set ([`Budget::cancel`]).
    #[error("canceled")]
    Canceled,
}

impl DngError {
    /// Why the SDK or libjxl failed, for the variants that call it.
    pub fn cause(&self) -> Option<&Cause> {
        match self {
            Self::ReadFailed { cause, .. }
            | Self::WriteFailed { cause, .. }
            | Self::JxlFailed { cause, .. }
            | Self::RenderFailed(cause)
            | Self::MetadataFailed(cause)
            | Self::ParamsFailed(cause) => Some(cause),
            _ => None,
        }
    }
}

/// Why a call into the DNG SDK or libjxl failed: the causes a user can act
/// on (`native/dt_status.h`), and the rest as they came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The file is damaged, or isn't the kind of file it should be.
    Damaged,
    /// A kind of file the SDK doesn't read: a newer DNG version, or one
    /// whose pixels the bridge doesn't decode (floating point).
    Unsupported,
    /// Memory ran out.
    OutOfMemory,
    /// The file claims more pixels than a read takes ([`MAX_READ_PIXELS`]):
    /// an image that large, or a damaged header.
    TooLarge,
    /// Anything else: the SDK's error code (0 when the call failed without
    /// throwing) and a standard exception's message, for the log.
    Other { code: i32, message: String },
}

impl Cause {
    /// The cause the SDK's `dng_error_code` names (`dng_errors.h`, whose
    /// values `native/guard.h` pins).
    fn from_code(code: i32, message: String) -> Self {
        match code {
            // bad format, end of file, file is damaged, JPEG XL decoder.
            100_006 | 100_011 | 100_012 | 100_018 => Self::Damaged,
            // not yet implemented, unsupported DNG.
            100_001 | 100_015 => Self::Unsupported,
            100_005 => Self::OutOfMemory,
            // The bridge's own (`native/dt_status.h`).
            200_001 => Self::TooLarge,
            _ => Self::Other { code, message },
        }
    }
}

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Damaged => f.write_str("damaged, or not a file of its kind"),
            Self::Unsupported => f.write_str("a kind the SDK doesn't read"),
            Self::OutOfMemory => f.write_str("out of memory"),
            Self::TooLarge => write!(f, "more than {MAX_READ_PIXELS} pixels"),
            Self::Other { code: 0, message } if message.is_empty() => {
                f.write_str("no reason given")
            }
            Self::Other { code: 0, message } => f.write_str(message),
            Self::Other { code, message } if message.is_empty() => write!(f, "SDK error {code}"),
            Self::Other { code, message } => write!(f, "{message} (SDK error {code})"),
        }
    }
}

/// C mirror of `native/dt_status.h`'s DtStatus: what the C side caught, as
/// a call that can fail fills it in.
#[repr(C)]
pub(crate) struct DtStatus {
    code: i32,
    message: [c_char; 120],
}

impl DtStatus {
    pub(crate) fn new() -> Self {
        Self { code: 0, message: [0; 120] }
    }

    pub(crate) fn cause(&self) -> Cause {
        // SAFETY: the C side always NUL-terminates the message
        // (`snprintf`), and `new` zeroes it.
        let message = unsafe { std::ffi::CStr::from_ptr(self.message.as_ptr()) };
        Cause::from_code(self.code, message.to_string_lossy().into_owned())
    }
}

/// A C call's result, 1 for success, as a `Result`: for anything else,
/// `err` with the cause `status` gives.
pub(crate) fn check(ok: i32, status: &DtStatus, err: impl FnOnce(Cause) -> DngError) -> Result<()> {
    if ok == 1 { Ok(()) } else { Err(err(status.cause())) }
}

pub type Result<T> = std::result::Result<T, DngError>;

#[repr(C)]
struct DtDngImage {
    _opaque: [u8; 0],
}

// The C API (`native/dng_c_api.h`). Every entry point is exception-guarded
// on the C++ side, so none unwinds into Rust. Handles are only ever ones it
// returned and not yet freed, paths and strings are NUL-terminated
// `CString`s that outlive the call, and pixel slices are passed with their
// exact length, which the C side checks against the geometry.
unsafe extern "C" {
    #[cfg(feature = "examples")]
    fn dt_dng_initialize();
    fn dt_dng_read_metadata(path: *const c_char, status: *mut DtStatus) -> *mut DtDngImage;
    #[allow(clippy::too_many_arguments)]
    fn dt_dng_read_into(
        path: *const c_char,
        bypass_cache: i32,
        max_pixels: u64,
        alloc: PixelAlloc,
        ctx: *mut c_void,
        threads: u32,
        cancel: *const u8,
        status: *mut DtStatus,
    ) -> *mut DtDngImage;
    fn dt_dng_width(img: *const DtDngImage) -> i32;
    fn dt_dng_height(img: *const DtDngImage) -> i32;
    fn dt_dng_channels(img: *const DtDngImage) -> i32;
    fn dt_dng_pixel_count(img: *const DtDngImage) -> i64;
    fn dt_dng_pixels(img: *const DtDngImage) -> *const u16;
    #[allow(clippy::too_many_arguments)]
    fn dt_dng_render_linear_to_srgb(
        meta_source: *const DtDngImage,
        pixels: *const u16,
        pixel_count: usize,
        width: i32,
        height: i32,
        channels: i32,
        white: *const WhiteXy,
        exposure: f64,
        status: *mut DtStatus,
    ) -> *mut DtDngImage;
    #[allow(clippy::too_many_arguments)]
    fn dt_dng_write(
        meta_source: *const DtDngImage,
        out_path: *const c_char,
        pixels: *const u16,
        pixel_count: usize,
        width: i32,
        height: i32,
        channels: i32,
        tags: *const DtDngTags,
        status: *mut DtStatus,
    ) -> i32;
    fn dt_dng_free(img: *mut DtDngImage);
    // The test hooks: only called from tests, which compare the fast read
    // paths with the SDK's, and built only where build.rs says.
    #[cfg(dng_test_hooks)]
    #[cfg_attr(not(test), allow(dead_code))]
    fn dt_dng_set_reference_reads(on: i32);
    #[cfg(dng_test_hooks)]
    #[cfg_attr(not(test), allow(dead_code))]
    fn dt_dng_last_read_path(stage1: *mut i32, deferred_black_level: *mut i32);
    #[cfg(dng_test_hooks)]
    #[cfg_attr(not(test), allow(dead_code))]
    fn dt_dng_encode_lossless_jpeg(
        samples: *const u16,
        rows: u32,
        cols: u32,
        channels: u32,
        bits: u32,
        out: *mut u8,
        capacity: usize,
    ) -> usize;
    #[cfg(dng_test_hooks)]
    #[cfg_attr(not(test), allow(dead_code))]
    fn dt_dng_compare_lossless_jpeg(data: *const u8, size: usize, samples: usize) -> i32;
    #[cfg(dng_test_hooks)]
    fn dt_dng_render_file_srgb(
        path: *const c_char,
        dcp_path: *const c_char,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        out: *mut u16,
        capacity: usize,
    ) -> i32;
    fn dt_dng_set_log_callback(
        callback: Option<unsafe extern "C" fn(level: i32, message: *const c_char)>,
    );
    fn dt_dng_from_raw_metadata(
        meta: *const DtRawMetadata,
        status: *mut DtStatus,
    ) -> *mut DtDngImage;
    fn dt_dng_apply_dcp(
        img: *mut DtDngImage,
        dcp_path: *const c_char,
        status: *mut DtStatus,
    ) -> i32;
    fn dt_dng_compute_render_params(
        meta_source: *const DtDngImage,
        white: *const WhiteXy,
        status: *mut DtStatus,
    ) -> *mut DtDngRenderParams;
    fn dt_dng_render_params_free(params: *mut DtDngRenderParams);
    fn dt_dng_as_shot_white(
        meta_source: *const DtDngImage,
        out: *mut WhiteXy,
        has: *mut i32,
        status: *mut DtStatus,
    ) -> i32;
    fn dt_dng_temperature_of(white: WhiteXy, kelvin: *mut f64, tint: *mut f64) -> i32;
    fn dt_dng_temperature_white(kelvin: f64, tint: f64, out: *mut WhiteXy) -> i32;
}

/// A white point in CIE 1931 xy. Laid out as `native/dng_c_api.h`'s
/// DtDngWhite, which the C API takes it as.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WhiteXy {
    pub x: f64,
    pub y: f64,
}

impl WhiteXy {
    /// Whether the SDK takes this white: finite, both parts positive
    /// (`dng_xy_coord::IsValid`). Its sum may pass 1, as its whites at a low
    /// kelvin and a magenta tint do. A call given a white it does not take
    /// fails without reaching the SDK.
    pub fn is_valid(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.x > 0.0 && self.y > 0.0
    }

    /// `white` as the C API takes it: null for none, else a pointer into
    /// `white`, which outlives the call.
    fn c_ptr(white: &Option<Self>) -> *const Self {
        white.as_ref().map_or(std::ptr::null(), std::ptr::from_ref)
    }

    /// Why a call refuses `white`, if it does.
    fn refused(white: Option<Self>) -> Option<Cause> {
        let white = white.filter(|w| !w.is_valid())?;
        Some(Cause::Other { code: 0, message: format!("{white:?} is not a white point") })
    }
}

/// A white point as the SDK's temperature (kelvin) and tint
/// (`dng_temperature`). A render at a higher kelvin than the light's comes
/// out warmer, and at a higher tint more magenta
/// (`a_render_at_a_white_moves_as_the_sdk_says`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Temperature {
    pub kelvin: f64,
    pub tint: f64,
}

impl Temperature {
    /// The SDK's temperature of `white`. A white that is not valid
    /// ([`WhiteXy::is_valid`]) reads as D50 (about 5000 K).
    pub fn of(white: WhiteXy) -> Self {
        let (mut kelvin, mut tint) = (0.0, 0.0);
        // SAFETY: plain values in, two f64s out that live for the call.
        let ok = unsafe { dt_dng_temperature_of(white, &mut kelvin, &mut tint) };
        debug_assert_eq!(ok, 1, "dng_temperature's math does not throw");
        Self { kelvin, tint }
    }

    /// The SDK's white at this temperature and tint. A kelvin that is not
    /// finite and positive, or a tint not finite, reads as D50.
    pub fn white(self) -> WhiteXy {
        let mut out = WhiteXy { x: 0.0, y: 0.0 };
        // SAFETY: plain values in, a WhiteXy out that lives for the call.
        let ok = unsafe { dt_dng_temperature_white(self.kelvin, self.tint, &mut out) };
        debug_assert_eq!(ok, 1, "dng_temperature's math does not throw");
        out
    }
}

/// Whether a read goes through the OS file cache.
///
/// [`Use`](Self::Use) lets a stack that fits in memory reread from it (a
/// depth map's second pass, a rerun). [`Bypass`](Self::Bypass) is for a
/// stack too big to stay cached: on Windows it fills the cache and every
/// read then waits on evictions (about 2 GB/s from a 6.7 GB/s SSD). Only
/// uncompressed DNGs on Windows read differently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileCache {
    #[default]
    Use,
    Bypass,
}

/// The most pixels a DNG read takes, in the file's image or its default
/// crop, and a JPEG XL decode ([`jxl`]): 2^29, past any camera's or
/// stack's, so a damaged or hostile header is refused before gigabytes are
/// allocated for it.
pub const MAX_READ_PIXELS: u64 = 1 << 29;

/// `DtDngPixelAlloc`: hands the C side the buffer a read decodes into.
type PixelAlloc = unsafe extern "C" fn(ctx: *mut c_void, count: usize) -> *mut u16;

/// The buffer [`DngImage::read_with_pixels`] lends the decoder, and the
/// sample count the decoder asked for.
struct PixelSink {
    pixels: Vec<u16>,
    requested: Option<usize>,
}

/// The `PixelAlloc` for a [`PixelSink`]: room for `count` samples, left
/// uninitialized for the decoder to fill, or null when it can't be had. It
/// must not panic, as it is called from C++.
unsafe extern "C" fn alloc_pixels(ctx: *mut c_void, count: usize) -> *mut u16 {
    // SAFETY: `ctx` is the `PixelSink` that `read_capped` lent the C call,
    // which this runs inside.
    let sink = unsafe { &mut *ctx.cast::<PixelSink>() };
    if sink.requested.is_some() || sink.pixels.try_reserve_exact(count).is_err() {
        return std::ptr::null_mut();
    }
    sink.requested = Some(count);
    sink.pixels.as_mut_ptr()
}

/// Initialize the DNG and XMP SDKs. Idempotent, and every SDK call does it
/// first, so this is only for an example that times it.
#[cfg(feature = "examples")]
pub fn initialize() {
    // SAFETY: no arguments; the C side initializes once behind a once_flag.
    unsafe { dt_dng_initialize() }
}

unsafe extern "C" fn log_to_tracing(level: i32, message: *const c_char) {
    if message.is_null() {
        return;
    }
    // SAFETY: the bridge passes a NUL-terminated buffer that lives for the
    // call (`dng_log`'s stack buffer).
    let text = unsafe { std::ffi::CStr::from_ptr(message) }.to_string_lossy();
    match level {
        0 | 1 => tracing::debug!(target: "deptheos_dng::sdk", "{text}"),
        2 => tracing::info!(target: "deptheos_dng::sdk", "{text}"),
        3 => tracing::warn!(target: "deptheos_dng::sdk", "{text}"),
        _ => tracing::error!(target: "deptheos_dng::sdk", "{text}"),
    }
}

/// For render parity tests: `[x, y, w, h]` of `path`'s default crop rendered
/// to 16-bit sRGB (`w × h × 3` samples) by the SDK alone from the file's own
/// negative, with `dcp`, if given, in place of the file's profiles. Leaves
/// out opcode list 3, as [`DngImage::read_with_pixels`] does. Fails for a
/// mosaic file or one not upright as stored.
#[doc(hidden)]
#[cfg(dng_test_hooks)]
pub fn sdk_render_file_srgb(path: &Path, dcp: Option<&Path>, rect: [u32; 4]) -> Result<Vec<u16>> {
    let [x, y, w, h] = rect;
    let c = c_path(path)?;
    let dcp = dcp.map(c_path).transpose()?;
    let len = (w as usize) * (h as usize) * 3;
    let mut out = vec![0u16; len];
    // SAFETY: NUL-terminated paths that outlive the call; `out` holds `len`
    // samples, which the C side writes only when its render has exactly
    // that many.
    let ok = unsafe {
        dt_dng_render_file_srgb(
            c.as_ptr(),
            dcp.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            x,
            y,
            w,
            h,
            out.as_mut_ptr(),
            len,
        )
    };
    // The hook takes no status: its failure has no cause.
    check(ok, &DtStatus::new(), DngError::RenderFailed)?;
    Ok(out)
}

/// Route the bridge's internal diagnostics into `tracing` (the errors behind
/// a `RenderFailed`/`ReadFailed` are otherwise silent).
pub fn route_logs_to_tracing() {
    // SAFETY: `log_to_tracing` is a `'static` fn that is safe to call from
    // any thread, as the bridge's worker threads will.
    unsafe { dt_dng_set_log_callback(Some(log_to_tracing)) }
}

/// A decoded DNG: 16-bit interleaved pixels plus the source file's metadata,
/// held opaquely for use when rendering or writing.
pub struct DngImage {
    // A live handle from the C API, owned by this value and freed once in
    // `Drop`. Every `unsafe` call on it below relies on that.
    ptr: NonNull<DtDngImage>,
}

// The handle owns a plain C++ object (value types only, no pointers into SDK
// state), so moving it between threads is safe. It is Sync because every
// `&self` method reaches C through a `const DtDngImage *` and only reads;
// mutation (`apply_dcp`) takes `&mut self`. Keep it that way:
// a `&self` method that mutates the native side must become `&mut self`.
unsafe impl Send for DngImage {}
unsafe impl Sync for DngImage {}

/// A DNG as [`DngImage::read_with_pixels`] reads it. Either way the handle
/// is metadata only but still renders and writes.
pub enum DngRead {
    /// Linear camera RGB, 16 bits per channel, interleaved
    /// (`width × height × channels`).
    Linear(DngImage, Vec<u16>),
    /// A CFA (mosaic) DNG, read as metadata only (`channels() == 0`): its
    /// pixels need a demosaic.
    Mosaic(DngImage),
}

impl DngImage {
    /// Decode a DNG to linear camera RGB, or a CFA (mosaic) one to its
    /// metadata alone, within `budget`'s threads and stopped by its flag.
    /// More than [`MAX_READ_PIXELS`] is refused with [`Cause::TooLarge`]
    /// before anything is allocated.
    pub fn read_with_pixels(path: &Path, cache: FileCache, budget: Budget) -> Result<DngRead> {
        Self::read_capped(path, cache, budget, MAX_READ_PIXELS)
    }

    /// [`Self::read_with_pixels`], refusing more than `max_pixels`.
    fn read_capped(
        path: &Path,
        cache: FileCache,
        budget: Budget,
        max_pixels: u64,
    ) -> Result<DngRead> {
        let canceled = || budget.cancel.load(std::sync::atomic::Ordering::Relaxed);
        let c = c_path(path)?;
        let mut sink = PixelSink { pixels: Vec::new(), requested: None };
        let mut status = DtStatus::new();
        // SAFETY: `alloc_pixels` matches `DtDngPixelAlloc`, and `sink` and
        // `status` outlive the call, the only time the C side uses them.
        let ptr = unsafe {
            dt_dng_read_into(
                c.as_ptr(),
                i32::from(cache == FileCache::Bypass),
                max_pixels,
                alloc_pixels,
                (&raw mut sink).cast::<c_void>(),
                budget.threads.min(u32::MAX as usize) as u32,
                budget.cancel.as_ptr().cast_const().cast(),
                &mut status,
            )
        };
        if ptr.is_null() && canceled() {
            return Err(DngError::Canceled);
        }
        let image = Self::from_ptr(ptr, &status, |cause| read_failed(path, cause))?;
        let (width, height, channels) = (image.width(), image.height(), image.channels());
        if channels == 0 {
            return Ok(DngRead::Mosaic(image));
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(channels as usize));
        match sink.requested {
            Some(count) if Some(count) == expected => {
                // SAFETY: a non-null handle with channels means the decoder
                // filled the `count` samples it asked for, within the
                // capacity `alloc_pixels` reserved.
                unsafe { sink.pixels.set_len(count) };
                Ok(DngRead::Linear(image, sink.pixels))
            }
            got => {
                Err(DngError::PixelCountMismatch { got: got.unwrap_or(0), width, height, channels })
            }
        }
    }

    /// Parse a DNG's metadata without its image (`channels() == 0`), as the
    /// color source for [`DngImage::render_params`],
    /// [`DngImage::render_linear_to_srgb`] and [`DngImage::write`].
    /// Milliseconds, where [`DngImage::read_with_pixels`] is a full decode.
    pub fn read_metadata(path: &Path) -> Result<Self> {
        let c = c_path(path)?;
        let mut status = DtStatus::new();
        let ptr = unsafe { dt_dng_read_metadata(c.as_ptr(), &mut status) };
        Self::from_ptr(ptr, &status, |cause| read_failed(path, cause))
    }

    /// DNG metadata synthesized from a LibRaw frame's, so a stack of non-DNG
    /// RAW frames can be written to DNG. Metadata only, no pixels.
    pub fn from_raw_metadata(meta: &deptheos_raw::RawMetadata) -> Result<Self> {
        let mut status = DtStatus::new();
        let ptr = unsafe { dt_dng_from_raw_metadata(meta.as_ffi(), &mut status) };
        Self::from_ptr(ptr, &status, DngError::MetadataFailed)
    }

    /// Own a handle the C API returned, or report `err` with the cause
    /// `status` gives for a null one.
    fn from_ptr(
        ptr: *mut DtDngImage,
        status: &DtStatus,
        err: impl FnOnce(Cause) -> DngError,
    ) -> Result<Self> {
        NonNull::new(ptr).map(|ptr| Self { ptr }).ok_or_else(|| err(status.cause()))
    }

    pub fn width(&self) -> u32 {
        unsafe { dt_dng_width(self.ptr.as_ptr()) }.max(0) as u32
    }

    pub fn height(&self) -> u32 {
        unsafe { dt_dng_height(self.ptr.as_ptr()) }.max(0) as u32
    }

    pub fn channels(&self) -> u32 {
        unsafe { dt_dng_channels(self.ptr.as_ptr()) }.max(0) as u32
    }

    /// Interleaved 16-bit pixels, borrowed from the C++ buffer: a rendered
    /// image's ([`SrgbImage::pixels`]); a read keeps none.
    fn pixels(&self) -> &[u16] {
        let ptr = unsafe { dt_dng_pixels(self.ptr.as_ptr()) };
        let count = unsafe { dt_dng_pixel_count(self.ptr.as_ptr()) };
        if ptr.is_null() || count <= 0 {
            return &[];
        }
        // SAFETY: the buffer holds `count` u16 and only goes away through
        // `Drop`, which cannot run while this borrow of `self` lives.
        unsafe { std::slice::from_raw_parts(ptr, count as usize) }
    }

    /// Render linear pixels (a stack's composite) through this image's color
    /// science. The result exposes only pixels and geometry, so
    /// display-referred pixels can't be written or re-rendered as linear.
    pub fn render_linear_to_srgb(
        &self,
        pixels: &[u16],
        width: u32,
        height: u32,
        channels: u32,
    ) -> Result<SrgbImage> {
        self.render_linear_to_srgb_at(pixels, width, height, channels, None, 0.0)
    }

    /// [`Self::render_linear_to_srgb`] at `white` (`None`: as shot) and
    /// `exposure` stops added to the baseline, as the SDK's render task
    /// adds them: the reference for the GPU render's. A monochrome file
    /// ignores the white. A white that is not valid, or an exposure
    /// not finite, is refused.
    pub fn render_linear_to_srgb_at(
        &self,
        pixels: &[u16],
        width: u32,
        height: u32,
        channels: u32,
        white: Option<WhiteXy>,
        exposure: f64,
    ) -> Result<SrgbImage> {
        check_pixel_count(pixels.len(), width, height, channels)?;
        if let Some(cause) = WhiteXy::refused(white) {
            return Err(DngError::RenderFailed(cause));
        }
        if !exposure.is_finite() {
            let message = format!("an exposure of {exposure} stops");
            return Err(DngError::RenderFailed(Cause::Other { code: 0, message }));
        }
        let mut status = DtStatus::new();
        let ptr = unsafe {
            dt_dng_render_linear_to_srgb(
                self.ptr.as_ptr(),
                pixels.as_ptr(),
                pixels.len(),
                width as i32,
                height as i32,
                channels as i32,
                WhiteXy::c_ptr(&white),
                exposure,
                &mut status,
            )
        };
        Self::from_ptr(ptr, &status, DngError::RenderFailed).map(SrgbImage)
    }

    /// Apply an Adobe DCP camera profile to this image's metadata (matrices,
    /// illuminants, LUTs, profile name, baseline-exposure offset), for later
    /// [`DngImage::render_params`] and [`DngImage::write`] calls.
    pub fn apply_dcp(&mut self, dcp_path: &Path) -> Result<()> {
        let c = c_path(dcp_path)?;
        let mut status = DtStatus::new();
        let ok = unsafe { dt_dng_apply_dcp(self.ptr.as_ptr(), c.as_ptr(), &mut status) };
        check(ok, &status, |cause| read_failed(dcp_path, cause))
    }

    /// Everything a GPU pipeline needs to render this image's linear pixels
    /// to sRGB with the source's color science, at the white as shot.
    pub fn render_params(&self) -> Result<RenderParams> {
        self.render_params_at(None)
    }

    /// [`Self::render_params`] at `white` (`None`: as shot): the matrix,
    /// camera white and a dual-illuminant profile's HueSatMap are the
    /// SDK's for it. A monochrome file, or one without a usable profile,
    /// gets the same parameters at any white. A white that is not
    /// valid ([`WhiteXy::is_valid`]) is refused.
    pub fn render_params_at(&self, white: Option<WhiteXy>) -> Result<RenderParams> {
        if let Some(cause) = WhiteXy::refused(white) {
            return Err(DngError::ParamsFailed(cause));
        }
        let mut status = DtStatus::new();
        let ptr = unsafe {
            dt_dng_compute_render_params(self.ptr.as_ptr(), WhiteXy::c_ptr(&white), &mut status)
        };
        if ptr.is_null() {
            return Err(DngError::ParamsFailed(status.cause()));
        }
        // Copy everything out, then hand the C allocation straight back.
        // SAFETY: non-null and owned by us until the free below.
        let c = unsafe { &*ptr };
        let params = RenderParams {
            camera_to_prophoto: c.camera_to_prophoto,
            camera_white: c.camera_white,
            black_level: c.black_level,
            baseline_exposure: c.baseline_exposure,
            shadow_scale: c.shadow_scale,
            hue_sat_map: hue_sat_from_c(
                c.hue_sat_dims,
                c.hue_sat_encoding,
                c.hue_sat_data,
                c.hue_sat_count,
            ),
            look_table: hue_sat_from_c(
                c.look_table_dims,
                c.look_table_encoding,
                c.look_table_data,
                c.look_table_count,
            ),
            tone_curve: c.tone_curve.to_vec(),
        };
        unsafe { dt_dng_render_params_free(ptr) };
        Ok(params)
    }

    /// The white [`Self::render_params`] renders at: the camera neutral's,
    /// else D55. `None` for a monochrome file or one without a usable
    /// profile, whose render has no white to change.
    pub fn as_shot_white(&self) -> Result<Option<WhiteXy>> {
        let mut white = WhiteXy { x: 0.0, y: 0.0 };
        let mut has = 0;
        let mut status = DtStatus::new();
        let ok =
            unsafe { dt_dng_as_shot_white(self.ptr.as_ptr(), &mut white, &mut has, &mut status) };
        check(ok, &status, DngError::ParamsFailed)?;
        Ok((has == 1).then_some(white))
    }

    /// Write pixels to a DNG that stands in for this image's metadata (the
    /// Library's color reference), without GPS or serial numbers, which are
    /// never read. Opcode list 3 is kept at any size, with the size it
    /// applies to as the original size, so an export of that size from the
    /// file read back gets it again ([`Self::write_tagged`] writes a
    /// picture). A failed write can leave part of a file at `out_path`, so a
    /// save goes through `deptheos_core`'s `io::write_replacing`.
    pub fn write(
        &self,
        out_path: &Path,
        pixels: &[u16],
        width: u32,
        height: u32,
        channels: u32,
    ) -> Result<()> {
        self.write_with(out_path, pixels, width, height, channels, None)
    }
}

/// What [`DngImage::write_tagged`] writes besides the carried metadata.
#[derive(Clone, Copy, Debug)]
pub struct DngTags<'a> {
    /// Replaces the source's `Software` tag.
    pub software: &'a str,
    /// The Exif `UserComment`.
    pub comment: &'a str,
    /// An XMP packet, merged with the XMP the SDK derives from the EXIF.
    pub xmp: &'a str,
    /// The capture time as ISO 8601, for metadata that has none (LibRaw's
    /// carries none); empty when unknown.
    pub capture_time: &'a str,
    /// The reference frame's own tags (the JPEG and TIFF exports'
    /// allowlist), written over the carried metadata's: LibRaw normalizes
    /// the model and reads no Artist from some RAW formats.
    pub fields: &'a [DngField],
    /// A white balance written as AsShotNeutral in place of the carried
    /// one: this white's camera neutral under the carried profile. Pixels
    /// are untouched; a monochrome file or one without a profile keeps its
    /// own. A white that is not valid is refused.
    pub white: Option<WhiteXy>,
}

/// One tag of IFD0 or the Exif IFD, by number, as a frame holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DngField {
    pub tag: u16,
    pub value: DngValue,
}

/// A [`DngField`]'s value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DngValue {
    Text(String),
    /// BYTE, SHORT or LONG values.
    Integers(Vec<u32>),
    /// Numerator, denominator.
    Rationals(Vec<[u32; 2]>),
    SRationals(Vec<[i32; 2]>),
}

impl DngImage {
    /// [`Self::write`] for an exported result, with `tags`. Opcode list 3
    /// goes with it only when the result is the size the list applies to:
    /// the source's whole image, upright, as read.
    pub fn write_tagged(
        &self,
        out_path: &Path,
        pixels: &[u16],
        width: u32,
        height: u32,
        channels: u32,
        tags: &DngTags,
    ) -> Result<()> {
        self.write_with(out_path, pixels, width, height, channels, Some(tags))
    }

    /// The one write: with `tags` as [`Self::write_tagged`] says, without
    /// as [`Self::write`] does.
    fn write_with(
        &self,
        out_path: &Path,
        pixels: &[u16],
        width: u32,
        height: u32,
        channels: u32,
        tags: Option<&DngTags>,
    ) -> Result<()> {
        check_pixel_count(pixels.len(), width, height, channels)?;
        let c = c_path(out_path)?;
        let white = tags.and_then(|t| t.white);
        if let Some(cause) = WhiteXy::refused(white) {
            return Err(DngError::WriteFailed { path: out_path.display().to_string(), cause });
        }
        let text = |s: &str| {
            CString::new(s).map_err(|_| DngError::InvalidPath(format!("{s:?} holds a NUL")))
        };
        // The strings and values outlive the call, which is the only time C
        // reads them.
        let strings = match tags {
            Some(tags) => Some([
                text(tags.software)?,
                text(tags.comment)?,
                text(tags.xmp)?,
                text(tags.capture_time)?,
            ]),
            None => None,
        };
        let owned: Vec<OwnedField> =
            tags.map_or_else(Vec::new, |t| t.fields.iter().map(OwnedField::new).collect());
        let fields: Vec<DtDngField> = owned.iter().map(OwnedField::raw).collect();
        let c_tags = strings.as_ref().map(|[software, comment, xmp, capture_time]| DtDngTags {
            software: software.as_ptr(),
            user_comment: comment.as_ptr(),
            xmp: xmp.as_ptr(),
            capture_time: capture_time.as_ptr(),
            fields: if fields.is_empty() { std::ptr::null() } else { fields.as_ptr() },
            field_count: fields.len(),
            white: WhiteXy::c_ptr(&white),
        });
        let mut status = DtStatus::new();
        let ok = unsafe {
            dt_dng_write(
                self.ptr.as_ptr(),
                c.as_ptr(),
                pixels.as_ptr(),
                pixels.len(),
                width as i32,
                height as i32,
                channels as i32,
                c_tags.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
                &mut status,
            )
        };
        check(ok, &status, |cause| DngError::WriteFailed {
            path: out_path.display().to_string(),
            cause,
        })
    }
}

/// C mirror of `native/dng_c_api.h`'s DtDngTags: [`DngTags`] as C strings,
/// which the caller keeps alive through the write.
#[repr(C)]
struct DtDngTags {
    software: *const c_char,
    user_comment: *const c_char,
    xmp: *const c_char,
    capture_time: *const c_char,
    fields: *const DtDngField,
    field_count: usize,
    white: *const WhiteXy,
}

/// C mirror of `native/dng_c_api.h`'s DtDngField, pointing into an
/// [`OwnedField`].
#[repr(C)]
struct DtDngField {
    tag: u16,
    kind: u16,
    count: u32,
    text: *const c_char,
    numbers: *const u32,
}

/// A [`DngField`] as C reads it: its text NUL-terminated (cut at a NUL it
/// holds), its numbers flat, a rational's parts in turn.
struct OwnedField {
    tag: u16,
    kind: u16,
    count: u32,
    text: Option<CString>,
    numbers: Vec<u32>,
}

impl OwnedField {
    /// DtDngField's kinds (`DT_DNG_FIELD_*`).
    const TEXT: u16 = 1;
    const INTEGERS: u16 = 2;
    const RATIONALS: u16 = 3;
    const SRATIONALS: u16 = 4;

    fn new(field: &DngField) -> Self {
        let (kind, text, numbers, count) = match &field.value {
            DngValue::Text(s) => {
                let s = s.split('\0').next().unwrap_or_default();
                let text = CString::new(s).expect("cut at the first NUL");
                (Self::TEXT, Some(text), Vec::new(), 0)
            }
            DngValue::Integers(v) => (Self::INTEGERS, None, v.clone(), v.len()),
            DngValue::Rationals(v) => {
                (Self::RATIONALS, None, v.iter().flatten().copied().collect(), v.len())
            }
            DngValue::SRationals(v) => (
                Self::SRATIONALS,
                None,
                v.iter().flatten().map(|&p| p.cast_unsigned()).collect(),
                v.len(),
            ),
        };
        Self { tag: field.tag, kind, count: count.try_into().unwrap_or(u32::MAX), text, numbers }
    }

    fn raw(&self) -> DtDngField {
        DtDngField {
            tag: self.tag,
            kind: self.kind,
            count: self.count,
            text: self.text.as_ref().map_or(std::ptr::null(), |t| t.as_ptr()),
            numbers: if self.numbers.is_empty() { std::ptr::null() } else { self.numbers.as_ptr() },
        }
    }
}

// The offsets dng_c_api.cpp pins for the C structs (64-bit pointers).
#[cfg(target_pointer_width = "64")]
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<DtDngTags>() == 56);
    assert!(offset_of!(DtDngTags, white) == 48);
    assert!(size_of::<WhiteXy>() == 16);
    assert!(offset_of!(WhiteXy, y) == 8);
    assert!(offset_of!(DtDngTags, software) == 0);
    assert!(offset_of!(DtDngTags, user_comment) == 8);
    assert!(offset_of!(DtDngTags, xmp) == 16);
    assert!(offset_of!(DtDngTags, capture_time) == 24);
    assert!(offset_of!(DtDngTags, fields) == 32);
    assert!(offset_of!(DtDngTags, field_count) == 40);
    assert!(size_of::<DtDngField>() == 24);
    assert!(offset_of!(DtDngField, tag) == 0);
    assert!(offset_of!(DtDngField, kind) == 2);
    assert!(offset_of!(DtDngField, count) == 4);
    assert!(offset_of!(DtDngField, text) == 8);
    assert!(offset_of!(DtDngField, numbers) == 16);
};

/// Pixels rendered to sRGB by [`DngImage::render_linear_to_srgb`]. The
/// metadata inside is unreachable on purpose: it describes the linear data.
pub struct SrgbImage(DngImage);

impl SrgbImage {
    pub fn width(&self) -> u32 {
        self.0.width()
    }

    pub fn height(&self) -> u32 {
        self.0.height()
    }

    pub fn channels(&self) -> u32 {
        self.0.channels()
    }

    /// Interleaved 16-bit sRGB pixels, borrowed from the C++ buffer.
    pub fn pixels(&self) -> &[u16] {
        self.0.pixels()
    }
}

impl Drop for DngImage {
    fn drop(&mut self) {
        unsafe { dt_dng_free(self.ptr.as_ptr()) }
    }
}

/// C mirror of `native/dng_c_api.h`'s DtDngRenderParams.
#[repr(C)]
struct DtDngRenderParams {
    camera_to_prophoto: [f32; 9],
    camera_white: [f32; 3],
    black_level: f64,
    baseline_exposure: f64,
    shadow_scale: f64,

    hue_sat_dims: [u32; 3],
    hue_sat_encoding: u32,
    hue_sat_data: *mut f32,
    hue_sat_count: i64,

    look_table_dims: [u32; 3],
    look_table_encoding: u32,
    look_table_data: *mut f32,
    look_table_count: i64,

    tone_curve: [f32; 257],
}

// The offsets dng_c_api.cpp pins for the C struct, so a field added,
// removed or reordered on one side only fails the build (64-bit pointers).
#[cfg(target_pointer_width = "64")]
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<DtDngRenderParams>() == 1168);
    assert!(offset_of!(DtDngRenderParams, camera_to_prophoto) == 0);
    assert!(offset_of!(DtDngRenderParams, camera_white) == 36);
    assert!(offset_of!(DtDngRenderParams, black_level) == 48);
    assert!(offset_of!(DtDngRenderParams, baseline_exposure) == 56);
    assert!(offset_of!(DtDngRenderParams, shadow_scale) == 64);
    assert!(offset_of!(DtDngRenderParams, hue_sat_dims) == 72);
    assert!(offset_of!(DtDngRenderParams, hue_sat_encoding) == 84);
    assert!(offset_of!(DtDngRenderParams, hue_sat_data) == 88);
    assert!(offset_of!(DtDngRenderParams, hue_sat_count) == 96);
    assert!(offset_of!(DtDngRenderParams, look_table_dims) == 104);
    assert!(offset_of!(DtDngRenderParams, look_table_encoding) == 116);
    assert!(offset_of!(DtDngRenderParams, look_table_data) == 120);
    assert!(offset_of!(DtDngRenderParams, look_table_count) == 128);
    assert!(offset_of!(DtDngRenderParams, tone_curve) == 136);
};

/// A DNG-style HSV lookup table: `hue × sat × val` entries of
/// (hue shift in degrees, sat scale, val scale).
#[derive(Debug, Clone, PartialEq)]
pub struct HueSatMap {
    pub hue_divisions: u32,
    pub sat_divisions: u32,
    pub val_divisions: u32,
    /// Flat triples, 3 floats per entry, in the SDK's storage order.
    pub data: Vec<f32>,
    /// The table is indexed by sRGB-encoded value, not linear
    /// (ProfileHueSatMapEncoding / ProfileLookTableEncoding = 1).
    pub srgb_encoded: bool,
}

/// The SDK's `dng_color_spec` for the shot's white point, precomputed for a
/// GPU render of linear camera RGB to sRGB. Comparable, so a renderer can
/// tell whether its prepared buffers still apply.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderParams {
    /// Row-major camera RGB → ProPhoto RGB.
    pub camera_to_prophoto: [f32; 9],
    pub camera_white: [f32; 3],
    pub black_level: f64,
    /// Stops of exposure to apply.
    pub baseline_exposure: f64,
    /// The shadow ramp's multiplier: ShadowScale × Stage3Gain, or 0 when
    /// the profile's DefaultBlackRender turns the ramp off.
    pub shadow_scale: f64,
    pub hue_sat_map: Option<HueSatMap>,
    pub look_table: Option<HueSatMap>,
    /// Profile tone curve (or the ACR default) sampled at 257 points.
    pub tone_curve: Vec<f32>,
}

impl RenderParams {
    /// Identity params for sources that are already sRGB (JPEG/PNG): the
    /// sRGB→ProPhoto matrix so the shared pipeline's ProPhoto→sRGB tail
    /// round-trips, linear tone curve, no LUTs.
    pub fn srgb_identity() -> Self {
        #[rustfmt::skip]
        let srgb_to_prophoto = [
            0.529360, 0.330103, 0.140661,
            0.098388, 0.873463, 0.028098,
            0.016851, 0.117711, 0.865681,
        ];
        Self {
            camera_to_prophoto: srgb_to_prophoto,
            camera_white: [1.0, 1.0, 1.0],
            black_level: 0.0,
            baseline_exposure: 0.0,
            shadow_scale: 0.0,
            hue_sat_map: None,
            look_table: None,
            tone_curve: (0..=256).map(|i| i as f32 / 256.0).collect(),
        }
    }
}

fn hue_sat_from_c(dims: [u32; 3], encoding: u32, data: *mut f32, count: i64) -> Option<HueSatMap> {
    if data.is_null() || count <= 0 {
        return None;
    }
    // SAFETY: the C side allocated `count` floats at `data`, which live until
    // dt_dng_render_params_free; this copies them before that.
    let data = unsafe { std::slice::from_raw_parts(data, count as usize) }.to_vec();
    Some(HueSatMap {
        hue_divisions: dims[0],
        sat_divisions: dims[1],
        val_divisions: dims[2],
        data,
        // The SDK renders any other encoding as linear.
        srgb_encoded: encoding == 1,
    })
}

fn read_failed(path: &Path, cause: Cause) -> DngError {
    DngError::ReadFailed { path: path.display().to_string(), cause }
}

fn check_pixel_count(got: usize, width: u32, height: u32, channels: u32) -> Result<()> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(channels as usize));
    if expected == Some(got) {
        Ok(())
    } else {
        Err(DngError::PixelCountMismatch { got, width, height, channels })
    }
}

/// The path as the C API takes it: the raw bytes on Unix; UTF-8 on Windows
/// (the bridge widens it), so a non-Unicode path is rejected, not mangled.
fn c_path(path: &Path) -> Result<CString> {
    let invalid = || DngError::InvalidPath(path.display().to_string());
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let bytes = path.to_str().ok_or_else(invalid)?.as_bytes();
    CString::new(bytes).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_errors_cleanly() {
        assert!(matches!(
            DngImage::read_with_pixels(
                Path::new("/nonexistent/x.dng"),
                FileCache::Use,
                crate::Budget::unlimited()
            ),
            Err(DngError::ReadFailed { .. })
        ));
        assert!(matches!(
            DngImage::read_metadata(Path::new("/nonexistent/x.dng")),
            Err(DngError::ReadFailed { .. })
        ));
    }

    /// The SDK's codes a user can act on become their causes; the rest
    /// keep their code and message.
    #[test]
    fn a_status_names_its_cause() {
        let status = |code: i32, message: &str| {
            let mut status = DtStatus::new();
            status.code = code;
            for (to, from) in status.message.iter_mut().zip(message.bytes()) {
                *to = from as c_char;
            }
            status.cause()
        };
        for code in [100_006, 100_011, 100_012, 100_018] {
            assert_eq!(status(code, ""), Cause::Damaged, "{code}");
        }
        assert_eq!(status(100_001, ""), Cause::Unsupported);
        assert_eq!(status(100_015, ""), Cause::Unsupported);
        assert_eq!(status(100_005, "std::bad_alloc"), Cause::OutOfMemory);
        assert_eq!(status(200_001, ""), Cause::TooLarge);
        assert_eq!(
            status(100_000, "writing the file failed"),
            Cause::Other { code: 100_000, message: "writing the file failed".into() }
        );
        // A call that failed without throwing.
        assert_eq!(DtStatus::new().cause(), Cause::Other { code: 0, message: String::new() });
    }

    /// A header that claims a 24000×24000 image (576 million pixels, 3.5 GB
    /// as 16-bit RGB) is refused as too large before the SDK allocates for
    /// it, and so is a real image past the cap a read is given.
    #[test]
    fn a_dng_past_the_pixel_cap_is_refused() {
        use crate::test_dng::{Layout, TestDng};
        let frame = TestDng {
            width: 64,
            height: 48,
            cfa: false,
            big_endian: false,
            layout: Layout::Strips { rows: 48 },
            black: [0; 3],
            white: [65535; 3],
            active: [0, 0, 48, 64],
            crop: [0, 0, 64, 48],
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.dng");
        frame.write(&path, &frame.pixels());
        let too_large = |result: Result<DngRead>| {
            matches!(result, Err(DngError::ReadFailed { cause: Cause::TooLarge, .. }))
        };
        let capped = DngImage::read_capped(&path, FileCache::Use, Budget::unlimited(), 64 * 47);
        assert!(too_large(capped), "past a cap of 64×47");
        let read = DngImage::read_capped(&path, FileCache::Use, Budget::unlimited(), 64 * 48);
        assert!(matches!(read, Ok(DngRead::Linear(..))), "at the cap");

        // ImageWidth, ImageLength and RowsPerStrip (LONG, one value, in
        // the entry) say 24000.
        let mut bytes = std::fs::read(&path).unwrap();
        for (tag, value) in [(256u16, 64u32), (257, 48), (278, 48)] {
            let entry: Vec<u8> =
                [&tag.to_le_bytes()[..], &[4, 0, 1, 0, 0, 0], &value.to_le_bytes()].concat();
            let at = bytes.windows(entry.len()).position(|w| w == entry).unwrap();
            bytes[at + 8..at + 12].copy_from_slice(&24_000u32.to_le_bytes());
        }
        let hostile = dir.path().join("hostile.dng");
        std::fs::write(&hostile, bytes).unwrap();
        let read = DngImage::read_with_pixels(&hostile, FileCache::Use, Budget::unlimited());
        assert!(too_large(read), "the hostile header");
    }

    /// A lossy JPEG DNG whose tile libjpeg stops on fails its read: the
    /// SDK's libjpeg error handler throws out of libjpeg's frames, and the
    /// read returns the error.
    #[test]
    fn a_corrupt_jpeg_in_a_dng_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let read = |name: &str, jpeg: &[u8]| {
            let path = dir.path().join(format!("{name}.dng"));
            crate::test_dng::write_lossy_jpeg(&path, 64, 48, jpeg);
            DngImage::read_with_pixels(&path, FileCache::Use, Budget::unlimited())
        };
        let jpeg = crate::jpeg::corrupt::jpeg(64, 48);
        let good = read("good", &jpeg);
        assert!(matches!(good, Ok(DngRead::Linear(..))), "the DNG as made: {:?}", good.err());
        for (name, bad) in crate::jpeg::corrupt::kinds(&jpeg) {
            let read = read(name, &bad);
            assert!(matches!(read, Err(DngError::ReadFailed { .. })), "{name}: {:?}", read.err());
        }
    }

    /// A file that is no DNG fails as damaged, with the SDK's own code
    /// carried through the C API's status.
    #[test]
    fn a_file_that_is_no_dng_is_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.dng");
        // A TIFF header and nothing after it.
        std::fs::write(&path, b"II*\0\x08\0\0\0\0\0").unwrap();
        let read = DngImage::read_with_pixels(&path, FileCache::Use, crate::Budget::unlimited());
        assert!(
            matches!(read, Err(DngError::ReadFailed { cause: Cause::Damaged, .. })),
            "{:?}",
            read.err()
        );
        assert!(matches!(
            DngImage::read_metadata(&path),
            Err(DngError::ReadFailed { cause: Cause::Damaged, .. })
        ));
    }

    /// A DNG writes into a folder whose name is not ASCII, and a second
    /// write replaces the first. Both need care on Windows, where the ANSI
    /// code page cannot name the folder and a plain rename does not replace
    /// a file.
    #[test]
    fn writes_over_an_existing_file_in_a_unicode_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Café 日本");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("out.dng");
        let meta = deptheos_raw::TestMetadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            cam_mul: [1.0; 4],
            raw_colors: 3,
            ..Default::default()
        };
        let image = DngImage::from_raw_metadata(&meta.into()).unwrap();
        let (w, h) = (48, 32);
        for value in [1000u16, 2000] {
            let pixels = vec![value; (w * h * 3) as usize];
            image.write(&path, &pixels, w, h, 3).unwrap();
            let read =
                DngImage::read_with_pixels(&path, FileCache::Use, crate::Budget::unlimited())
                    .unwrap();
            let DngRead::Linear(back, _) = read else { panic!("read as a mosaic") };
            assert_eq!((back.width(), back.height(), back.channels()), (w, h, 3));
        }
        let files: Vec<_> = std::fs::read_dir(&folder).unwrap().collect();
        assert_eq!(files.len(), 1, "no temporary file is left behind");
    }

    /// A read gives back the pixels written. 36 MB, so the copy out runs in
    /// more than one band.
    #[test]
    fn read_with_pixels_reads_what_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.dng");
        let meta = deptheos_raw::TestMetadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            cam_mul: [1.0; 4],
            raw_colors: 3,
            ..Default::default()
        };
        let (w, h) = (3000u32, 2000u32);
        let pixels: Vec<u16> =
            (0..w * h * 3).map(|i| (i.wrapping_mul(2_654_435_761) >> 16) as u16).collect();
        DngImage::from_raw_metadata(&meta.into()).unwrap().write(&path, &pixels, w, h, 3).unwrap();

        let read =
            DngImage::read_with_pixels(&path, FileCache::Use, crate::Budget::unlimited()).unwrap();
        let DngRead::Linear(new, new_pixels) = read else { panic!("read as a mosaic") };
        assert_eq!((new.width(), new.height(), new.channels()), (w, h, 3));
        assert!(new.pixels().is_empty(), "the handle keeps no pixels");
        assert!(new_pixels == pixels, "the pixels written");
        // The metadata-only handle still renders the pixels it handed out.
        new.render_linear_to_srgb(&new_pixels, w, h, 3).unwrap();
    }

    /// The file `write_tagged` writes with `xmp` as the stack's packet.
    fn written_with_xmp(xmp: &str) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.dng");
        let meta = deptheos_raw::TestMetadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            cam_mul: [1.0; 4],
            raw_colors: 3,
            ..Default::default()
        };
        let (w, h) = (16, 16);
        let tags = DngTags {
            software: "Test",
            comment: "",
            xmp,
            capture_time: "",
            fields: &[],
            white: None,
        };
        DngImage::from_raw_metadata(&meta.into())
            .unwrap()
            .write_tagged(&path, &vec![1000u16; (w * h * 3) as usize], w, h, 3, &tags)
            .unwrap();
        std::fs::read(&path).unwrap()
    }

    fn contains(haystack: &[u8], needle: &str) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    /// An XMP packet with the predefined entities parses, and its property
    /// is written.
    #[test]
    fn xmp_with_predefined_entities_is_kept() {
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Label="Tom &amp; Jerry &lt;3"/></rdf:RDF></x:xmpmeta>"#;
        assert!(contains(&written_with_xmp(xmp), "Tom &amp; Jerry &lt;3"));
    }

    /// A packet that declares entities (the way to a billion laughs) is
    /// refused rather than expanded.
    #[test]
    fn xmp_declaring_entities_is_not_expanded() {
        let xmp = r#"<!DOCTYPE x [<!ENTITY a "Laugh"><!ENTITY b "&a;&a;">]><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Label="&b;"/></rdf:RDF></x:xmpmeta>"#;
        assert!(!contains(&written_with_xmp(xmp), "LaughLaugh"));
    }

    /// No write carries GPS or body and lens serials, the plain one included:
    /// it writes the Library's color reference (`color.dng`), which people
    /// share. The bridge never reads them (`DngCameraInfo` in
    /// `native/dng_bridge.h`). The Artist stays (ADR-133).
    #[test]
    fn a_write_leaves_out_gps_and_serial_numbers() {
        use crate::test_dng::{BODY_SERIAL, LENS_SERIAL, Layout, OWNER, TestDng};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.dng");
        let frame = TestDng {
            width: 64,
            height: 48,
            cfa: false,
            big_endian: false,
            layout: Layout::Strips { rows: 16 },
            black: [0; 3],
            white: [65535; 3],
            active: [0, 0, 48, 64],
            crop: [0, 0, 64, 48],
        };
        frame.write_identified(&source, &frame.pixels());
        let bytes = std::fs::read(&source).unwrap();
        assert!(has_gps(&bytes) && contains(&bytes, OWNER));
        assert!(contains(&bytes, BODY_SERIAL) && contains(&bytes, LENS_SERIAL));

        let metadata = DngImage::read_metadata(&source).unwrap();
        let path = dir.path().join("color.dng");
        let (w, h) = (16, 16);
        metadata.write(&path, &vec![0x4000u16; (w * h * 3) as usize], w, h, 3).unwrap();
        let written = std::fs::read(&path).unwrap();
        assert!(!has_gps(&written), "no GPS position");
        assert!(!contains(&written, BODY_SERIAL), "no body serial number");
        assert!(!contains(&written, LENS_SERIAL), "no lens serial number");
        assert!(contains(&written, OWNER), "the Artist, read and written");
    }

    /// OpcodeList3 (a lens warp about an off-center point) is in stage 3
    /// coordinates, so an export keeps it only when framed as the source's
    /// stage 3: the whole image, upright, the same size. The Library's color
    /// reference, a 16×16 stand-in, carries it to a later export of that size.
    #[test]
    fn opcode_list_3_is_kept_only_where_it_applies() {
        use crate::test_dng::{Layout, TestDng, warp_rectilinear_list};
        let list = warp_rectilinear_list(0.3, 0.6);
        let whole = TestDng {
            width: 64,
            height: 48,
            cfa: false,
            big_endian: false,
            layout: Layout::Strips { rows: 16 },
            black: [0; 3],
            white: [65535; 3],
            active: [0, 0, 48, 64],
            crop: [0, 0, 64, 48],
        };
        let cropped = TestDng { crop: [4, 2, 56, 40], ..whole };
        // Square, so only the turn tells it from its stage 3.
        let square = TestDng { width: 48, active: [0, 0, 48, 48], crop: [0, 0, 48, 48], ..whole };
        let tags = DngTags {
            software: "Test",
            comment: "",
            xmp: "",
            capture_time: "",
            fields: &[],
            white: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let export = |image: &DngImage, name: &str, (w, h): (u32, u32)| {
            let out = dir.path().join(format!("{name}.dng"));
            image.write_tagged(&out, &vec![0x2000; (w * h * 3) as usize], w, h, 3, &tags).unwrap();
            contains_bytes(&std::fs::read(&out).unwrap(), &list)
        };
        // The source, its orientation, the size written (the source's when
        // none); whether the list stays.
        let cases = [
            ("whole", &whole, 1, None, true),
            ("cropped", &cropped, 1, None, false),
            ("turned", &square, 6, None, false),
            ("smaller", &whole, 1, Some((62, 48)), false),
        ];
        for (name, frame, orientation, size, kept) in cases {
            let source = dir.path().join(format!("{name}-source.dng"));
            frame.write_with_opcode_list3(&source, &frame.pixels(), orientation, &list);
            assert!(contains_bytes(&std::fs::read(&source).unwrap(), &list), "{name}: source");
            let read = DngImage::read_with_pixels(&source, FileCache::Use, Budget::unlimited());
            let Ok(DngRead::Linear(image, _)) = read else { panic!("{name}: not read") };
            let size = size.unwrap_or((image.width(), image.height()));
            assert_eq!(export(&image, name, size), kept, "{name}");
            // A stack's metadata is read without the pixels.
            let metadata = DngImage::read_metadata(&source).unwrap();
            assert_eq!(export(&metadata, &format!("{name}-metadata"), size), kept, "{name}");
        }

        // Through the color reference and back.
        let source = dir.path().join("reference-source.dng");
        whole.write_with_opcode_list3(&source, &whole.pixels(), 1, &list);
        let color = dir.path().join("color.dng");
        DngImage::read_metadata(&source)
            .unwrap()
            .write(&color, &[0x4000; 16 * 16 * 3], 16, 16, 3)
            .unwrap();
        assert!(contains_bytes(&std::fs::read(&color).unwrap(), &list), "the color reference");
        let reference = DngImage::read_metadata(&color).unwrap();
        assert!(export(&reference, "from-reference", (64, 48)), "an export from the reference");
        assert!(!export(&reference, "from-reference-16", (16, 16)), "one of its own size");
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// Whether a TIFF's first IFD links a GPS IFD (GPSInfo, 34853).
    fn has_gps(bytes: &[u8]) -> bool {
        let big = bytes.starts_with(b"MM");
        let u16_at = |at: usize| {
            let b = [bytes[at], bytes[at + 1]];
            if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
        };
        let u32_at = |at: usize| {
            let b = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
            if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
        };
        let ifd = u32_at(4) as usize;
        (0..u16_at(ifd) as usize).any(|i| u16_at(ifd + 2 + 12 * i) == 34853)
    }

    /// The file a plain write of `meta`'s 16x16 frame writes.
    fn written_from(meta: deptheos_raw::TestMetadata) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.dng");
        let meta = deptheos_raw::TestMetadata {
            cam_xyz: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            cam_mul: [1.0; 4],
            raw_colors: 3,
            ..meta
        };
        let (w, h) = (16, 16);
        DngImage::from_raw_metadata(&meta.into())
            .unwrap()
            .write(&path, &vec![1000u16; (w * h * 3) as usize], w, h, 3)
            .unwrap();
        std::fs::read(&path).unwrap()
    }

    /// The file a tagged write of `meta`'s 16x16 frame writes with
    /// `fields` as the frame's own tags.
    fn written_tagged(meta: deptheos_raw::TestMetadata, fields: &[DngField]) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.dng");
        let meta = deptheos_raw::TestMetadata {
            cam_xyz: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            cam_mul: [1.0; 4],
            raw_colors: 3,
            ..meta
        };
        let (w, h) = (16, 16);
        let tags = DngTags {
            software: "Test",
            comment: "",
            xmp: "",
            capture_time: "",
            fields,
            white: None,
        };
        DngImage::from_raw_metadata(&meta.into())
            .unwrap()
            .write_tagged(&path, &vec![1000u16; (w * h * 3) as usize], w, h, 3, &tags)
            .unwrap();
        std::fs::read(&path).unwrap()
    }

    /// A tag of a written DNG's IFD0 or Exif IFD, read back.
    fn tag(file: &[u8], number: u16) -> Option<exif::Value> {
        let exif = exif::Reader::new().read_raw(file.to_vec()).unwrap();
        exif.fields()
            .find(|f| f.tag.number() == number && f.ifd_num == exif::In::PRIMARY)
            .map(|f| f.value.clone())
    }

    fn text_tag(file: &[u8], number: u16) -> Option<String> {
        match tag(file, number)? {
            exif::Value::Ascii(v) => Some(String::from_utf8_lossy(v.first()?).into_owned()),
            other => panic!("{number:#x} is {other:?}"),
        }
    }

    fn rationals(file: &[u8], number: u16) -> Option<Vec<(u32, u32)>> {
        match tag(file, number)? {
            exif::Value::Rational(v) => Some(v.iter().map(|r| (r.num, r.denom)).collect()),
            other => panic!("{number:#x} is {other:?}"),
        }
    }

    const MODEL: u16 = 0x0110;
    const ARTIST: u16 = 0x013B;
    const UNIQUE_CAMERA_MODEL: u16 = 0xC614;
    const LENS_INFO: u16 = 0xC630;
    const LENS_SPECIFICATION: u16 = 0xA432;

    /// A tagged write names the camera as the frame does, not by LibRaw's
    /// normalized model, which stays the UniqueCameraModel the color
    /// profiles are matched by (as the DNG converter does it).
    #[test]
    fn a_tagged_write_names_the_model_as_the_frame_does() {
        let file = written_tagged(
            deptheos_raw::TestMetadata {
                make: "Canon".into(),
                model: "EOS R5 Mark II".into(),
                unique_camera_model: "Canon EOS R5 Mark II".into(),
                ..Default::default()
            },
            &[DngField { tag: MODEL, value: DngValue::Text("Canon EOS R5m2".into()) }],
        );
        assert_eq!(text_tag(&file, MODEL).as_deref(), Some("Canon EOS R5m2"));
        assert_eq!(text_tag(&file, UNIQUE_CAMERA_MODEL).as_deref(), Some("Canon EOS R5 Mark II"));
    }

    /// The frame's Artist is written when LibRaw read none from the file.
    #[test]
    fn a_tagged_write_carries_the_frames_artist() {
        let meta = || deptheos_raw::TestMetadata {
            make: "Canon".into(),
            model: "EOS R5 Mark II".into(),
            ..Default::default()
        };
        assert_eq!(text_tag(&written_tagged(meta(), &[]), ARTIST), None);
        let artist = DngField { tag: ARTIST, value: DngValue::Text("A. Photographer".into()) };
        let file = written_tagged(meta(), &[artist]);
        assert_eq!(text_tag(&file, ARTIST).as_deref(), Some("A. Photographer"));
    }

    /// A lens's unknown apertures are written 0/1, never 0/0 (no number):
    /// from LibRaw's focal lengths alone, in a plain write and a tagged
    /// one, and from a frame's own LensSpecification that holds 0/0.
    #[test]
    fn unknown_lens_apertures_are_written_as_zero_over_one() {
        let meta = || deptheos_raw::TestMetadata {
            make: "Canon".into(),
            model: "EOS R5 Mark II".into(),
            lens_min_focal: 100.0,
            lens_max_focal: 100.0,
            ..Default::default()
        };
        let expected = Some(vec![(100, 1), (100, 1), (0, 1), (0, 1)]);
        for file in [written_from(meta()), written_tagged(meta(), &[])] {
            assert_eq!(rationals(&file, LENS_SPECIFICATION), expected);
            assert_eq!(rationals(&file, LENS_INFO), expected);
        }
        let spec = DngField {
            tag: LENS_SPECIFICATION,
            value: DngValue::Rationals(vec![[60, 1], [60, 1], [0, 0], [0, 0]]),
        };
        let file = written_tagged(meta(), &[spec]);
        assert_eq!(
            rationals(&file, LENS_SPECIFICATION),
            Some(vec![(60, 1), (60, 1), (0, 1), (0, 1)])
        );
    }

    /// A lens with no focal length known writes no lens specification at
    /// all, rather than one of zeros.
    #[test]
    fn no_lens_specification_without_focal_lengths() {
        let file = written_from(deptheos_raw::TestMetadata {
            make: "Canon".into(),
            model: "EOS R5 Mark II".into(),
            ..Default::default()
        });
        assert_eq!(rationals(&file, LENS_SPECIFICATION), None);
        assert_eq!(rationals(&file, LENS_INFO), None);
    }

    /// The export names the camera by LibRaw's name for it, as its profiles
    /// are named (`dt_raw_camera_name`, tested in deptheos_raw). (The tag is
    /// ASCII ending in NUL; Make and Model alone never spell the name.)
    #[test]
    fn a_raw_export_names_its_unique_camera_model() {
        let file = written_from(deptheos_raw::TestMetadata {
            make: "Testmaker".into(),
            model: "Model 7".into(),
            unique_camera_model: "Unique Seven".into(),
            ..Default::default()
        });
        assert!(contains(&file, "Unique Seven\0"));
        assert!(!contains(&file, "Testmaker Model 7\0"));
    }

    /// Exposure values no camera reports (NaN, infinite, negative, beyond
    /// any EXIF rational) write a DNG without them rather than casting them
    /// out of range.
    #[test]
    fn hostile_exposure_values_still_write() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 1e30, 1e-30] {
            let file = written_from(deptheos_raw::TestMetadata {
                make: "Testmaker".into(),
                model: "Model 7".into(),
                shutter: bad,
                aperture: bad,
                focal_len: bad,
                iso_speed: bad,
                lens_min_focal: bad,
                lens_max_focal: bad,
                ..Default::default()
            });
            assert!(file.starts_with(b"II") || file.starts_with(b"MM"), "{bad}");
        }
    }

    #[test]
    fn pixel_count_is_validated() {
        assert!(matches!(check_pixel_count(10, 4, 4, 3), Err(DngError::PixelCountMismatch { .. })));
        assert!(check_pixel_count(48, 4, 4, 3).is_ok());
        // A product that overflows usize never matches.
        assert!(check_pixel_count(0, u32::MAX, u32::MAX, u32::MAX).is_err());
    }
}
