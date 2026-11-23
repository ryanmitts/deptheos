//! Camera RAW decoding via LibRaw.
//!
//! `native/raw_c_api.h` is a small plain-C API over LibRaw (built by
//! `build.rs`); the declarations below must stay in step with it. Both
//! sides pin the struct's size and every field's offset at compile time
//! (`raw_c_api.cpp`'s `static_assert`s and the `const` block below).

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum RawError {
    #[error("path is not valid for the C API: {0}")]
    InvalidPath(String),

    #[error("failed to decode RAW file {path}: {reason}")]
    DecodeFailed { path: String, reason: String },

    /// The decode's cancel flag was set ([`Budget::cancel`]).
    #[error("canceled")]
    Canceled,
}

/// What a decode keeps to (ADR-143): the flag that stops it, the threads
/// its codec may start, and what is told how to stop it mid-loop.
#[derive(Clone, Copy)]
pub struct Budget<'a> {
    /// Set: the decode stops and fails as [`RawError::Canceled`].
    pub cancel: &'a AtomicBool,
    /// Threads the codec may start of its own; 0 for its own default.
    pub threads: usize,
    /// Handed an [`Interrupt`] once a decode has started; what it returns
    /// is dropped as the decode ends. The engine hangs a cancel hook on
    /// it, since a long LibRaw loop reports progress only at its ends.
    pub watch: Option<&'a (dyn Fn(Interrupt) -> Box<dyn std::any::Any> + Sync)>,
}

impl Budget<'static> {
    /// Never canceled, every core: for tools and tests.
    pub fn unlimited() -> Self {
        static NEVER: AtomicBool = AtomicBool::new(false);
        Self { cancel: &NEVER, threads: 0, watch: None }
    }
}

impl Budget<'_> {
    fn canceled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn flag(&self) -> *const u8 {
        self.cancel.as_ptr().cast_const().cast()
    }

    fn threads(&self) -> i32 {
        self.threads.min(i32::MAX as usize) as i32
    }
}

/// Stops one running LibRaw decode from another thread (LibRaw's cancel
/// flag, which its decoders check inside their loops); does nothing once
/// that decode has ended.
#[derive(Clone)]
pub struct Interrupt(Arc<Mutex<Option<DecodePtr>>>);

#[derive(Clone, Copy)]
struct DecodePtr(*mut DtRawDecode);

// SAFETY: the pointer is only used under the mutex, and only while the
// decode it names is alive (cleared before it is freed).
unsafe impl Send for DecodePtr {}

impl Interrupt {
    pub fn interrupt(&self) {
        if let Some(DecodePtr(decode)) = *self.0.lock().unwrap_or_else(|e| e.into_inner()) {
            // SAFETY: a live decode (see `DecodePtr`); setting LibRaw's
            // cancel flag is an atomic store, safe beside the decode.
            unsafe { dt_raw_interrupt(decode) };
        }
    }
}

pub type Result<T> = std::result::Result<T, RawError>;

#[cfg(windows)]
type PathChar = u16;
#[cfg(not(windows))]
type PathChar = c_char;

/// [`RawMetadata`]'s C struct (`DtRawMetadata` in `native/raw_c_api.h`) for
/// native code in other crates: take it with [`RawMetadata::as_ffi`] and
/// include the header through `DEP_DEPTHEOS_RAW_INCLUDE`. Passed only by
/// pointer; the fields are private.
pub mod ffi {
    use std::os::raw::c_char;

    #[repr(C)]
    pub struct DtRawMetadata {
        pub(crate) make: [c_char; 64],
        pub(crate) model: [c_char; 64],
        pub(crate) software: [c_char; 64],
        pub(crate) unique_camera_model: [c_char; 64],
        pub(crate) artist: [c_char; 64],

        pub(crate) lens_make: [c_char; 128],
        pub(crate) lens_model: [c_char; 128],
        pub(crate) lens_min_focal: f32,
        pub(crate) lens_max_focal: f32,
        pub(crate) focal_length_in_35mm: i32,

        pub(crate) iso_speed: f32,
        pub(crate) shutter: f32,
        pub(crate) aperture: f32,
        pub(crate) focal_len: f32,
        pub(crate) raw_colors: i32,

        pub(crate) cam_mul: [f32; 4],
        pub(crate) cam_xyz: [f32; 12],
        pub(crate) as_shot_neutral: [f32; 4],
        pub(crate) as_shot_neutral_count: i32,

        pub(crate) dng_illuminant: [u32; 2],
        pub(crate) dng_color_matrix: [[f32; 12]; 2],
        pub(crate) dng_forward_matrix: [[f32; 12]; 2],
        pub(crate) dng_calibration: [[f32; 16]; 2],
        pub(crate) dng_has_color_matrix: [i32; 2],
        pub(crate) dng_has_forward_matrix: [i32; 2],
        pub(crate) dng_has_calibration: [i32; 2],

        pub(crate) analog_balance: [f32; 4],
        pub(crate) analog_balance_count: i32,
        pub(crate) baseline_exposure: f32,
        pub(crate) has_baseline_exposure: i32,
        pub(crate) linear_response_limit: f32,
        pub(crate) has_linear_response_limit: i32,
    }

    // The offsets raw_c_api.cpp pins for the C struct, so a field added,
    // removed or reordered on one side only fails the build.
    const _: () = {
        use std::mem::{offset_of, size_of};
        assert!(size_of::<DtRawMetadata>() == 1080);
        assert!(offset_of!(DtRawMetadata, make) == 0);
        assert!(offset_of!(DtRawMetadata, model) == 64);
        assert!(offset_of!(DtRawMetadata, software) == 128);
        assert!(offset_of!(DtRawMetadata, unique_camera_model) == 192);
        assert!(offset_of!(DtRawMetadata, artist) == 256);
        assert!(offset_of!(DtRawMetadata, lens_make) == 320);
        assert!(offset_of!(DtRawMetadata, lens_model) == 448);
        assert!(offset_of!(DtRawMetadata, lens_min_focal) == 576);
        assert!(offset_of!(DtRawMetadata, lens_max_focal) == 580);
        assert!(offset_of!(DtRawMetadata, focal_length_in_35mm) == 584);
        assert!(offset_of!(DtRawMetadata, iso_speed) == 588);
        assert!(offset_of!(DtRawMetadata, shutter) == 592);
        assert!(offset_of!(DtRawMetadata, aperture) == 596);
        assert!(offset_of!(DtRawMetadata, focal_len) == 600);
        assert!(offset_of!(DtRawMetadata, raw_colors) == 604);
        assert!(offset_of!(DtRawMetadata, cam_mul) == 608);
        assert!(offset_of!(DtRawMetadata, cam_xyz) == 624);
        assert!(offset_of!(DtRawMetadata, as_shot_neutral) == 672);
        assert!(offset_of!(DtRawMetadata, as_shot_neutral_count) == 688);
        assert!(offset_of!(DtRawMetadata, dng_illuminant) == 692);
        assert!(offset_of!(DtRawMetadata, dng_color_matrix) == 700);
        assert!(offset_of!(DtRawMetadata, dng_forward_matrix) == 796);
        assert!(offset_of!(DtRawMetadata, dng_calibration) == 892);
        assert!(offset_of!(DtRawMetadata, dng_has_color_matrix) == 1020);
        assert!(offset_of!(DtRawMetadata, dng_has_forward_matrix) == 1028);
        assert!(offset_of!(DtRawMetadata, dng_has_calibration) == 1036);
        assert!(offset_of!(DtRawMetadata, analog_balance) == 1044);
        assert!(offset_of!(DtRawMetadata, analog_balance_count) == 1060);
        assert!(offset_of!(DtRawMetadata, baseline_exposure) == 1064);
        assert!(offset_of!(DtRawMetadata, has_baseline_exposure) == 1068);
        assert!(offset_of!(DtRawMetadata, linear_response_limit) == 1072);
        assert!(offset_of!(DtRawMetadata, has_linear_response_limit) == 1076);
    };
}
use ffi::DtRawMetadata;

/// Opaque `DtRawDecode`: a decoded frame LibRaw holds until it is copied out.
#[repr(C)]
struct DtRawDecode {
    _opaque: [u8; 0],
}

/// Opaque `DtRawPreview`: an embedded preview LibRaw holds until freed.
#[repr(C)]
struct DtRawPreview {
    _opaque: [u8; 0],
}

// `DT_RAW_PREVIEW_JPEG` / `DT_RAW_PREVIEW_RGB8` in `native/raw_c_api.h`.
const PREVIEW_JPEG: i32 = 1;
const PREVIEW_RGB8: i32 = 2;

/// `LIBRAW_SUCCESS`.
const SUCCESS: c_int = 0;

// The C API (`native/raw_c_api.h`). Every entry point turns C++ exceptions
// into error codes, so none unwinds into Rust.
unsafe extern "C" {
    fn dt_raw_read_metadata(path: *const PathChar, out: *mut DtRawMetadata) -> c_int;
    fn dt_raw_read_size(path: *const PathChar, width: *mut i32, height: *mut i32) -> c_int;
    fn dt_raw_decoder() -> *mut DtRawDecode;
    fn dt_raw_interrupt(decode: *mut DtRawDecode);
    fn dt_raw_decode(
        path: *const PathChar,
        meta: *mut DtRawMetadata,
        decode: *mut DtRawDecode,
        width: *mut i32,
        height: *mut i32,
        channels: *mut i32,
        cancel: *const u8,
        threads: i32,
    ) -> c_int;
    fn dt_raw_copy_pixels(decode: *mut DtRawDecode, dest: *mut u16, count: usize) -> c_int;
    fn dt_raw_decode_free(decode: *mut DtRawDecode);
    fn dt_raw_preview(
        path: *const PathChar,
        min_size: i32,
        out: *mut *mut DtRawPreview,
        format: *mut i32,
        width: *mut i32,
        height: *mut i32,
        flip: *mut i32,
        data: *mut *const u8,
        len: *mut usize,
    ) -> c_int;
    fn dt_raw_preview_free(preview: *mut DtRawPreview);
    fn dt_raw_decode_half(
        path: *const PathChar,
        decode: *mut DtRawDecode,
        width: *mut i32,
        height: *mut i32,
        cancel: *const u8,
        threads: i32,
    ) -> c_int;
    fn dt_raw_copy_pixels8(decode: *mut DtRawDecode, dest: *mut u8, count: usize) -> c_int;
    fn dt_raw_strerror(code: c_int) -> *const c_char;
    #[cfg(test)]
    fn dt_raw_camera_name(
        make: *const c_char,
        model: *const c_char,
        unique: *const c_char,
        out: *mut c_char,
        capacity: usize,
    );
    #[cfg(test)]
    fn dt_raw_parallel_threads() -> c_int;
}

/// Camera and color metadata carried alongside the decoded pixels, as
/// LibRaw read it, for deptheos_dng to build a DNG's color science and tags
/// from. There is no orientation: LibRaw returns pixels already upright.
pub struct RawMetadata(Box<ffi::DtRawMetadata>);

impl RawMetadata {
    /// All zeros, a valid `DtRawMetadata` for the C side to fill.
    fn zeroed() -> Self {
        // SAFETY: every field is a number or an array of them.
        Self(Box::new(unsafe { std::mem::zeroed() }))
    }

    /// The name the camera goes by in a DNG's UniqueCameraModel and in
    /// Adobe's profile names: the file's own UniqueCameraModel, else
    /// "<make> <model>" (`dt_raw_camera_name` in `native/raw_c_api.h`).
    pub fn unique_camera_model(&self) -> String {
        c_string(&self.0.unique_camera_model)
    }

    /// The C struct, for native code in other crates ([`ffi`]).
    pub fn as_ffi(&self) -> &ffi::DtRawMetadata {
        &self.0
    }
}

/// DNG-style color matrices for one calibration illuminant, for
/// [`TestMetadata`]. An absent matrix is `None`.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Default)]
pub struct RawDngColor {
    /// EXIF light source code (e.g. 17 = Standard Light A, 21 = D65).
    pub illuminant: u32,
    /// 4x3 row-major XYZ→camera; only the first 3 rows matter for RGB.
    pub color_matrix: Option<[f32; 12]>,
    /// 3x4 row-major camera→XYZ; only the first 3 columns matter for RGB.
    pub forward_matrix: Option<[f32; 12]>,
    /// 4x4 row-major camera calibration (usually diagonal).
    pub calibration: Option<[f32; 16]>,
}

/// LibRaw metadata made up for a test, for the fields tests set; the rest
/// as a file with no such tag has them.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Default)]
pub struct TestMetadata {
    pub make: String,
    pub model: String,
    /// As LibRaw's metadata names the camera ([`RawMetadata`]'s).
    pub unique_camera_model: String,
    pub lens_min_focal: f32,
    pub lens_max_focal: f32,
    pub iso_speed: f32,
    /// Exposure time in seconds.
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    /// Camera white balance multipliers (R, G1, B, G2).
    pub cam_mul: [f32; 4],
    /// 4x3 XYZ→camera (D65), row-major.
    pub cam_xyz: [f32; 12],
    /// Distinct colors in the sensor's filter array: 3, or 4 (e.g. CMYG).
    pub raw_colors: i32,
    /// Dual-illuminant DNG color matrices (index 0 = illuminant 1).
    pub dng_color: [RawDngColor; 2],
    pub baseline_exposure: Option<f32>,
}

#[cfg(any(test, feature = "test-support"))]
impl From<TestMetadata> for RawMetadata {
    /// Strings become NUL-terminated fixed arrays, cut between characters
    /// to fit; an `Option` a value and its has-flag.
    fn from(t: TestMetadata) -> Self {
        fn fixed<const N: usize>(out: &mut [c_char; N], s: &str) {
            let mut end = s.len().min(N - 1);
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            for (dst, &b) in out.iter_mut().zip(&s.as_bytes()[..end]) {
                *dst = b as c_char;
            }
        }
        fn flagged<const N: usize>(matrix: Option<[f32; N]>) -> ([f32; N], i32) {
            matrix.map_or(([0.0; N], 0), |m| (m, 1))
        }
        let mut meta = Self::zeroed();
        let m = &mut *meta.0;
        fixed(&mut m.make, &t.make);
        fixed(&mut m.model, &t.model);
        fixed(&mut m.unique_camera_model, &t.unique_camera_model);
        m.lens_min_focal = t.lens_min_focal;
        m.lens_max_focal = t.lens_max_focal;
        m.iso_speed = t.iso_speed;
        m.shutter = t.shutter;
        m.aperture = t.aperture;
        m.focal_len = t.focal_len;
        m.cam_mul = t.cam_mul;
        m.cam_xyz = t.cam_xyz;
        m.raw_colors = t.raw_colors;
        for (i, color) in t.dng_color.iter().enumerate() {
            m.dng_illuminant[i] = color.illuminant;
            (m.dng_color_matrix[i], m.dng_has_color_matrix[i]) = flagged(color.color_matrix);
            (m.dng_forward_matrix[i], m.dng_has_forward_matrix[i]) = flagged(color.forward_matrix);
            (m.dng_calibration[i], m.dng_has_calibration[i]) = flagged(color.calibration);
        }
        (m.baseline_exposure, m.has_baseline_exposure) =
            t.baseline_exposure.map_or((0.0, 0), |v| (v, 1));
        meta
    }
}

/// A decoded RAW frame: interleaved RGB, 16 bits per channel.
pub struct RawImage {
    pub pixels: Vec<u16>,
    pub width: u32,
    pub height: u32,
    pub metadata: RawMetadata,
}

/// File extensions LibRaw handles, lower-case. Used to route loading;
/// decoding still decides whether a file is really readable.
pub const RAW_EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "orf", "raf", "rw2", "pef",
    "raw", "dcr", "kdc", "mrw", "3fr", "mef", "iiq", "rwl", "srw", "x3f", "erf", "mos",
];

/// Whether the path's extension is one LibRaw is expected to decode.
pub fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTENSIONS.iter().any(|raw| e.eq_ignore_ascii_case(raw)))
}

/// A RAW file's camera, lens and color metadata without decoding it
/// (`open_file` + `identify`, milliseconds), unlike [`read`]'s demosaic.
pub fn read_metadata(path: &Path) -> Result<RawMetadata> {
    let c_path = c_path(path)?;
    let mut meta = RawMetadata::zeroed();
    // SAFETY: the C side fills the struct on success.
    check(path, unsafe { dt_raw_read_metadata(c_path.as_ptr(), &mut *meta.0) })?;
    Ok(meta)
}

/// The upright size [`read`] decodes `path` to, from metadata alone, so a
/// stack can be sized before any frame decodes.
pub fn read_size(path: &Path) -> Result<(u32, u32)> {
    let c_path = c_path(path)?;
    let (mut width, mut height) = (0, 0);
    // SAFETY: the C side writes the two out-parameters on success.
    check(path, unsafe { dt_raw_read_size(c_path.as_ptr(), &mut width, &mut height) })?;
    Ok((width as u32, height as u32))
}

/// Decode a RAW file into 16-bit interleaved linear camera RGB (no white
/// balance, gamma 1.0), upright, for the DNG color-science path, within
/// `budget`.
pub fn read(path: &Path, budget: Budget) -> Result<RawImage> {
    let c_path = c_path(path)?;
    let mut meta = RawMetadata::zeroed();
    let decode = Decode::new(path, &budget)?;
    let (mut width, mut height, mut channels) = (0, 0, 0);
    // SAFETY: the C side fills the struct and the out-parameters on success.
    decode.ended(path, &budget, unsafe {
        dt_raw_decode(
            c_path.as_ptr(),
            &mut *meta.0,
            decode.ptr,
            &mut width,
            &mut height,
            &mut channels,
            budget.flag(),
            budget.threads(),
        )
    })?;

    if width <= 0 || height <= 0 || channels != 3 {
        return Err(RawError::DecodeFailed {
            path: path.display().to_string(),
            reason: format!("decoder returned {width}x{height} with {channels} channels, need RGB"),
        });
    }
    let (width, height) = (width as u32, height as u32);
    let mut pixels = vec![0u16; width as usize * height as usize * 3];
    // SAFETY: `pixels` holds exactly the `count` u16 the C side writes.
    check(path, unsafe { dt_raw_copy_pixels(decode.ptr, pixels.as_mut_ptr(), pixels.len()) })?;

    Ok(RawImage { pixels, width, height, metadata: meta })
}

/// The pixels of an embedded preview.
pub enum PreviewData {
    /// A JPEG file, still compressed.
    Jpeg(Vec<u8>),
    /// Interleaved 8-bit RGB, `width * height * 3` bytes.
    Rgb8(Vec<u8>),
}

/// A preview image a RAW or DNG file carries for viewers, already rendered
/// by the camera or the converter that wrote it.
pub struct Preview {
    pub data: PreviewData,
    /// 0 when the container does not record a JPEG's size: the JPEG's own
    /// header has it.
    pub width: u32,
    pub height: u32,
    /// LibRaw's orientation still to apply (0 upright, 3 = 180°,
    /// 5 = 90° counter-clockwise, 6 = 90° clockwise).
    pub flip: i32,
}

/// The embedded preview best suited to a thumbnail `min_size` pixels on its
/// long edge: the smallest at least that big, else the largest, letterboxed
/// ones last. Reads only that preview; errors when none is a JPEG or bitmap.
pub fn read_preview(path: &Path, min_size: u32) -> Result<Preview> {
    let c_path = c_path(path)?;
    let mut preview = std::ptr::null_mut();
    let (mut format, mut width, mut height, mut flip) = (0, 0, 0, 0);
    let (mut data, mut len) = (std::ptr::null(), 0usize);
    // SAFETY: `c_path` is NUL-terminated and outlives the call; every
    // out-pointer is a live local.
    check(path, unsafe {
        dt_raw_preview(
            c_path.as_ptr(),
            min_size.min(i32::MAX as u32) as i32,
            &mut preview,
            &mut format,
            &mut width,
            &mut height,
            &mut flip,
            &mut data,
            &mut len,
        )
    })?;
    /// Frees the preview on every exit path.
    struct Owned(*mut DtRawPreview);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the pointer came from dt_raw_preview and is freed once.
            unsafe { dt_raw_preview_free(self.0) };
        }
    }
    let _owned = Owned(preview);
    // SAFETY: on success `data` points at `len` bytes that live until the
    // preview is freed, which `_owned` does after this copy.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let (width, height) = (width.max(0) as u32, height.max(0) as u32);
    let data = match format {
        PREVIEW_JPEG => PreviewData::Jpeg(bytes),
        PREVIEW_RGB8 if bytes.len() == width as usize * height as usize * 3 => {
            PreviewData::Rgb8(bytes)
        }
        _ => {
            return Err(RawError::DecodeFailed {
                path: path.display().to_string(),
                reason: format!("unexpected preview format {format} ({width}x{height})"),
            });
        }
    };
    Ok(Preview { data, width, height, flip })
}

/// The raw data rendered small, for files with no usable preview: half size
/// (no demosaic), camera white balance, sRGB, 8-bit interleaved RGB,
/// upright. Several times faster than [`read`], but it still reads every
/// raw pixel.
pub fn read_half_size(path: &Path, budget: Budget) -> Result<(Vec<u8>, u32, u32)> {
    let c_path = c_path(path)?;
    let decode = Decode::new(path, &budget)?;
    let (mut width, mut height) = (0, 0);
    // SAFETY: `c_path` is NUL-terminated and outlives the call; every
    // out-pointer is a live local, and the flag outlives the call.
    decode.ended(path, &budget, unsafe {
        dt_raw_decode_half(
            c_path.as_ptr(),
            decode.ptr,
            &mut width,
            &mut height,
            budget.flag(),
            budget.threads(),
        )
    })?;
    if width <= 0 || height <= 0 {
        return Err(RawError::DecodeFailed {
            path: path.display().to_string(),
            reason: format!("decoder returned {width}x{height}"),
        });
    }
    let (width, height) = (width as u32, height as u32);
    let mut pixels = vec![0u8; width as usize * height as usize * 3];
    // SAFETY: `pixels` holds exactly the `count` bytes the C side writes.
    check(path, unsafe { dt_raw_copy_pixels8(decode.ptr, pixels.as_mut_ptr(), pixels.len()) })?;
    Ok((pixels, width, height))
}

/// Owns a `DtRawDecode`, freeing it on every exit path, and the watch its
/// budget hung on it, dropped first.
struct Decode {
    ptr: *mut DtRawDecode,
    interrupt: Interrupt,
    watched: Option<Box<dyn std::any::Any>>,
}

impl Decode {
    /// A decode's handle, watched as `budget` asks.
    fn new(path: &Path, budget: &Budget) -> Result<Self> {
        // SAFETY: no arguments; NULL when out of memory.
        let ptr = unsafe { dt_raw_decoder() };
        if ptr.is_null() {
            return Err(RawError::DecodeFailed {
                path: path.display().to_string(),
                reason: "no memory for a decoder".into(),
            });
        }
        let interrupt = Interrupt(Arc::new(Mutex::new(Some(DecodePtr(ptr)))));
        let watched = budget.watch.map(|watch| watch(interrupt.clone()));
        Ok(Self { ptr, interrupt, watched })
    }

    /// A decode call's status: canceled if the budget's flag was set.
    fn ended(&self, path: &Path, budget: &Budget, code: c_int) -> Result<()> {
        if code != SUCCESS && budget.canceled() {
            return Err(RawError::Canceled);
        }
        check(path, code)
    }
}

impl Drop for Decode {
    fn drop(&mut self) {
        drop(self.watched.take());
        *self.interrupt.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // SAFETY: the pointer came from dt_raw_decoder and is freed once,
        // and no interrupt reaches it from now on.
        unsafe { dt_raw_decode_free(self.ptr) };
    }
}

/// A LibRaw status code as a `Result`.
fn check(path: &Path, code: c_int) -> Result<()> {
    if code == SUCCESS {
        return Ok(());
    }
    // SAFETY: dt_raw_strerror returns a static NUL-terminated string.
    let reason = unsafe { CStr::from_ptr(dt_raw_strerror(code)) };
    Err(RawError::DecodeFailed {
        path: path.display().to_string(),
        reason: reason.to_string_lossy().into_owned(),
    })
}

/// The path as the NUL-terminated string the C API takes: UTF-16 on Windows
/// (LibRaw's wide-path open), raw bytes elsewhere — never lossily converted.
fn c_path(path: &Path) -> Result<Vec<PathChar>> {
    #[cfg(windows)]
    let units: Vec<PathChar> = {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().collect()
    };
    #[cfg(not(windows))]
    let units: Vec<PathChar> = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().iter().map(|&b| b as PathChar).collect()
    };
    if units.contains(&0) {
        return Err(RawError::InvalidPath(path.display().to_string()));
    }
    Ok(units.into_iter().chain([0]).collect())
}

/// Read a fixed-size C char array up to its NUL terminator.
fn c_string(bytes: &[c_char]) -> String {
    let bytes: Vec<u8> = bytes.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up name is cut between characters to fit the C array, less
    /// its NUL: 31 two-byte characters fit 63 bytes, not 31 and a half.
    #[test]
    fn a_test_name_is_cut_to_fit() {
        let meta = RawMetadata::from(TestMetadata {
            unique_camera_model: "é".repeat(40),
            ..Default::default()
        });
        assert_eq!(meta.unique_camera_model(), "é".repeat(31));
    }

    fn camera_name(make: &str, model: &str, unique: &str) -> String {
        let c = |s: &str| std::ffi::CString::new(s).unwrap();
        let (make, model, unique) = (c(make), c(model), c(unique));
        let mut out = [0 as c_char; 64];
        // SAFETY: three NUL-terminated strings, and `out` holds the
        // capacity given.
        unsafe {
            dt_raw_camera_name(
                make.as_ptr(),
                model.as_ptr(),
                unique.as_ptr(),
                out.as_mut_ptr(),
                out.len(),
            )
        };
        c_string(&out)
    }

    /// A camera's name for its profiles and its DNG's UniqueCameraModel:
    /// the make once, nothing but what the file says, and one the file
    /// names kept as it is.
    #[test]
    fn a_camera_is_named_by_make_and_model() {
        assert_eq!(camera_name("Testmaker", "Model 7", ""), "Testmaker Model 7");
        assert_eq!(camera_name("Testmaker", "TESTMAKER Model 7", ""), "TESTMAKER Model 7");
        assert_eq!(camera_name(" fujifilm ", "FUJIFILM X-T5", ""), "FUJIFILM X-T5");
        assert_eq!(camera_name(" Canon ", " EOS R5 ", ""), "Canon EOS R5");
        assert_eq!(camera_name("Canon", "", ""), "Canon");
        assert_eq!(camera_name("Canon", "  ", ""), "Canon");
        assert_eq!(camera_name("", " EOS R5", ""), "EOS R5");
        assert_eq!(camera_name("", "", ""), "");
        assert_eq!(camera_name("Testmaker", "Model 7", "Unique Seven"), "Unique Seven");
        assert_eq!(camera_name("Testmaker", "Model 7", " Unique Seven\t"), "Unique Seven");
        assert_eq!(camera_name("Testmaker", "Model 7", " \t "), "Testmaker Model 7");
        // Cut to the buffer, less its NUL.
        assert_eq!(camera_name(&"m".repeat(40), &"x".repeat(40), "").len(), 63);
    }

    #[test]
    fn recognizes_raw_extensions() {
        assert!(is_raw_path(Path::new("a.CR2")));
        assert!(is_raw_path(Path::new("a.dng")));
        assert!(!is_raw_path(Path::new("a.jpg")));
        assert!(!is_raw_path(Path::new("a")));
    }

    /// A thread that ran an OpenMP parallel region exits, and threads start
    /// after it. The stacker decodes each frame on a thread that then exits;
    /// a runtime joining its team at that exit would, on Windows, deadlock
    /// under the loader lock (see `deptheos_dng`'s `jxl_c_api.cpp`). Broken,
    /// this hangs rather than fails, so run it under a timeout.
    #[test]
    fn a_thread_that_used_openmp_exits() {
        let (done, finished) = std::sync::mpsc::channel();
        // Started first, so the watch needs no thread made after OpenMP ran.
        std::thread::spawn(move || {
            for _ in 0..8 {
                // SAFETY: no arguments, and it keeps nothing.
                let threads =
                    std::thread::spawn(|| unsafe { dt_raw_parallel_threads() }).join().unwrap();
                assert!(threads >= 1);
                // The MSVC build is the one with `/openmp`: a region on one
                // thread there would make this test check nothing.
                if cfg!(target_env = "msvc")
                    && std::thread::available_parallelism().is_ok_and(|n| n.get() > 1)
                {
                    assert!(threads > 1, "the OpenMP region ran on {threads} thread");
                }
            }
            std::thread::spawn(|| ()).join().unwrap();
            let _ = done.send(());
        });
        finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("a thread that used OpenMP never exited, or no thread could start after it");
    }

    #[test]
    fn missing_file_errors_cleanly() {
        let err = read(Path::new("/nonexistent/frame.cr2"), Budget::unlimited());
        assert!(matches!(err, Err(RawError::DecodeFailed { .. })));
        let err = read_metadata(Path::new("/nonexistent/frame.cr2"));
        assert!(matches!(err, Err(RawError::DecodeFailed { .. })));
        let err = read_size(Path::new("/nonexistent/frame.cr2"));
        assert!(matches!(err, Err(RawError::DecodeFailed { .. })));
    }
}
