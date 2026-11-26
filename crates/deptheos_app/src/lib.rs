//! The application layer between the stacking engine and its front ends
//! (desktop, CLI, iOS bridge).
//!
//! A [`StackingSession`] runs a stack (per-frame events, cancellation between
//! frames, re-reading frames for two-pass blends) and yields a
//! [`CompletedStack`], from which every save, comparison and render in the
//! Library ([`library::history`]) starts. Front ends drive it through
//! [`Engine`], which converts and caches RAW frames and returns CPU images.

pub mod engine;
mod error;
pub mod frames;
mod fsutil;
pub mod library;
pub mod paint;
pub mod preferences;
pub mod runs;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod view;
pub mod words;

pub use deptheos_render::{Adjustments, OutputSpace, ProcessVersion, Reframe, reframe};
pub use engine::look::Look;
pub use engine::present::{
    CpuImage, PresentedImage, TILE_CORE, TiledLevel, fit_long_edge, level_index,
};
pub use engine::preview::decode_linear_with_metadata;
pub use engine::{Engine, LIVE_PREVIEW_SIDE, Overlay, PresentedStack};
pub use error::{Error, FrameKind, RenderUse, Result};
pub use frames::conversions::{Converted, DngCache, StackConversions};
pub use frames::order::FrameOrder;
pub use frames::raw_backend::RawBackend;
pub use frames::source_thumbnail::thumbnail;
pub use frames::sources::{FrameRef, RootId, Sources};
pub use fsutil::{check_space, set_aside, write_json_atomic};
pub use library::history::{Frames, Moves, RenderId, StoreError};
pub use runs::session::{
    AlignSettings, Alignment, CompletedStack, DcpPreference, Gray16Image, LeftOut, LiveMap,
    SaveOptions, SessionEvent, SessionOptions, SessionOutput, StackColor, StackGeometry,
    StackingSession, left_out_line, left_out_message,
};
pub use runs::stacker::{SettingHelp, StackerConfig, StackerKind, align_label};
pub use words::Words;
pub use words::explain::{
    Cause as ExplanationCause, Explain, Explanation, Kind as ExplanationKind, Platform,
};

pub use deptheos_aligner::{AlignMode, GridConfig};

pub use deptheos_core::converter::{
    DOWNLOAD_PAGE as CONVERTER_PAGE, MAX_PARALLEL_JOBS as MAX_CONVERTER_JOBS,
    default_parallel_jobs as default_converter_jobs, executable as converter_executable,
    parallel_jobs as converter_jobs, set_executable as set_converter_executable,
    set_parallel_jobs as set_converter_jobs, set_uncompressed as set_converter_uncompressed,
};
pub use deptheos_core::dcp::{
    adobe_directories as adobe_profile_directories,
    set_extra_directories as set_profile_directories,
};
pub use deptheos_core::depth::{
    BLEND_RANGE, BLEND_RANGE_STEP, DepthConfig, RADIUS_RANGE, RADIUS_STEP, SMOOTHING_RANGE,
    SMOOTHING_STEP,
};
pub use deptheos_core::io::{
    CameraRaw, JPEG_QUALITY, JPEG_QUALITY_RANGE, TiffCompression, header_info, is_dng,
    is_raw_source, load_image, supported_extensions,
};
pub use deptheos_core::pyramid::{PyramidConfig, SMOOTHNESS_RANGE, SMOOTHNESS_STEP};
pub use deptheos_core::wavelet::{DENOISE_RANGE, DENOISE_STEP, MAX_CONSISTENCY, WaveletConfig};
pub use deptheos_core::{Cancel, CoreError};
pub use deptheos_dng::route_logs_to_tracing;
pub use deptheos_gpu::{DeviceChoice, DeviceInfo, DeviceKind, GpuContext};
pub(crate) use engine::SourceInput;

/// Lock `mutex` even when poisoned: nothing this crate locks holds
/// invariants a panic could break.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
