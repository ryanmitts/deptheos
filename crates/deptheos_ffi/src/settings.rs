//! A stack's settings: its method, alignment and their ranges and words.

use deptheos_app::preferences::AlignmentPreferences;
use deptheos_app::{
    AlignMode, AlignSettings, Alignment, DepthConfig, GridConfig, PyramidConfig, RawBackend,
    SessionOptions, SettingHelp, StackerConfig, WaveletConfig,
};

/// The stacking algorithm and its settings (ranges: [`algorithm_ranges`]).
#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum Algorithm {
    Pyramid {
        smoothness: f32,
    },
    Depth {
        radius: f32,
        smoothing: f32,
        blend_range: f32,
    },
    /// `consistency` is set from the command line only; kept so a restored
    /// render stacks the same.
    Wavelet {
        denoise: f32,
        consistency: u32,
    },
}

impl Algorithm {
    pub(crate) fn config(self) -> StackerConfig {
        match self {
            Self::Pyramid { smoothness } => StackerConfig::Pyramid(PyramidConfig { smoothness }),
            Self::Depth { radius, smoothing, blend_range } => {
                StackerConfig::Depth(DepthConfig { radius, smoothing, blend_range })
            }
            Self::Wavelet { denoise, consistency } => {
                StackerConfig::Wavelet(WaveletConfig { denoise, consistency })
            }
        }
    }

    fn of(config: &StackerConfig) -> Self {
        match *config {
            StackerConfig::Pyramid(c) => Self::Pyramid { smoothness: c.smoothness },
            StackerConfig::Depth(c) => {
                Self::Depth { radius: c.radius, smoothing: c.smoothing, blend_range: c.blend_range }
            }
            StackerConfig::Wavelet(c) => {
                Self::Wavelet { denoise: c.denoise, consistency: c.consistency }
            }
        }
    }
}

/// How frames are aligned (`deptheos_app::AlignMode`).
#[uniffi::remote(Enum)]
pub enum AlignMode {
    /// Chain ECC, full affine: the default.
    Ecc,
    /// Grid search, similarity and brightness.
    Grid,
}

/// Who decodes camera RAW and mosaic DNGs (`deptheos_app::RawBackend`,
/// ADR-131). iOS offers `Apple` (the default, rendered: TIFF, JPEG or PNG)
/// and `LibRaw` (linear: DNG too); `AdobeDng` is the desktop's. A linear
/// DNG goes through the DNG SDK under either.
#[uniffi::remote(Enum)]
pub enum RawBackend {
    LibRaw,
    AdobeDng,
    Apple,
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct StackSettings {
    pub algorithm: Algorithm,
    pub align: bool,
    /// Which aligner, while `align` is on.
    pub align_method: AlignMode,
    /// Crop the result to the area every aligned frame covers.
    pub crop: bool,
    /// Who decodes the stack's camera RAW and mosaic DNGs.
    pub raw_decoder: RawBackend,
}

impl StackSettings {
    /// Alignment off keeps the aligner and crop it resumes with.
    pub(crate) fn of(config: &StackerConfig, options: &SessionOptions) -> Self {
        let alignment = options.alignment.settings();
        Self {
            algorithm: Algorithm::of(config),
            align: options.alignment.is_on(),
            align_method: alignment.mode,
            crop: alignment.crop,
            raw_decoder: options.raw_backend,
        }
    }

    /// The session options, off remembering the aligner and crop, with
    /// Grid search's default limits ([`stack_options`] sets the
    /// preferences').
    pub(crate) fn options(&self) -> SessionOptions {
        let alignment =
            AlignSettings { mode: self.align_method, grid: GridConfig::default(), crop: self.crop };
        SessionOptions {
            alignment: Alignment::new(self.align, alignment),
            raw_backend: self.raw_decoder,
            ..SessionOptions::default()
        }
    }
}

/// What [`Engine::stack`] stacks with: `settings`, and Grid search as
/// `alignment` sets it (clamped). The read-ahead is the session's, within
/// what the app has left (`deptheos_app::engine::memory::prefetch_bytes`).
pub(crate) fn stack_options(
    settings: &StackSettings,
    alignment: AlignmentPreferences,
) -> SessionOptions {
    alignment.sanitized().apply(settings.options())
}

/// A slider's range, step and default.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct SettingRange {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub default_value: f32,
}

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct AlgorithmRanges {
    pub smoothness: SettingRange,
    pub radius: SettingRange,
    pub smoothing: SettingRange,
    pub blend_range: SettingRange,
    pub denoise: SettingRange,
}

/// How much to let go of when memory runs short
/// ([`deptheos_app::engine::memory::Shed`]).
#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum MemoryShed {
    /// Short, or backgrounded: paint's previous and prefetched sources go
    /// (the app drops its off-screen thumbnails).
    Moderate,
    /// About to be ended: the same of the engine's (the app drops an unshown
    /// map pyramid and a replaced compare too).
    Critical,
}

/// Let go of what the engine can at `level`, at once, from any thread: it
/// only asks what holds the memory, without waiting. The footprint is
/// logged first.
#[uniffi::export]
pub fn shed_memory(level: MemoryShed) {
    deptheos_app::engine::memory::shed(level.into());
}

impl From<MemoryShed> for deptheos_app::engine::memory::Shed {
    fn from(level: MemoryShed) -> Self {
        match level {
            MemoryShed::Moderate => Self::Moderate,
            MemoryShed::Critical => Self::Critical,
        }
    }
}

/// "footprint 812 MB, 2304 MB left", for the app's logs at each transition
/// ([`deptheos_app::engine::memory::reading`]).
#[uniffi::export]
pub fn memory_reading() -> String {
    deptheos_app::engine::memory::reading()
}

/// The engine's own ranges and defaults, so the app's sliders match it.
#[uniffi::export]
pub fn algorithm_ranges() -> AlgorithmRanges {
    let range = |(min, max): (f32, f32), step, default_value| SettingRange {
        min,
        max,
        step,
        default_value,
    };
    let depth = DepthConfig::default();
    AlgorithmRanges {
        smoothness: range(
            deptheos_app::SMOOTHNESS_RANGE,
            deptheos_app::SMOOTHNESS_STEP,
            PyramidConfig::default().smoothness,
        ),
        radius: range(deptheos_app::RADIUS_RANGE, deptheos_app::RADIUS_STEP, depth.radius),
        smoothing: range(
            deptheos_app::SMOOTHING_RANGE,
            deptheos_app::SMOOTHING_STEP,
            depth.smoothing,
        ),
        blend_range: range(
            deptheos_app::BLEND_RANGE,
            deptheos_app::BLEND_RANGE_STEP,
            depth.blend_range,
        ),
        denoise: range(
            deptheos_app::DENOISE_RANGE,
            deptheos_app::DENOISE_STEP,
            WaveletConfig::default().denoise,
        ),
    }
}

/// A stack setting with a help line (`deptheos_app::SettingHelp`). iOS
/// shows the method's sliders', the aligner's and the crop's; it has no RAW
/// decoder control, so no `Decoder` line.
#[uniffi::remote(Enum)]
pub enum SettingHelp {
    Smoothness,
    Smoothing,
    Denoise,
    Radius,
    BlendRange,
    Aligner,
    Crop,
    Decoder,
}
