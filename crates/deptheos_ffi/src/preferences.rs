//! The preferences iOS shares with the desktop (ADR-150): defaults, ranges
//! and words, and the calls that use them. The app keeps the values, hands
//! them to every stack and export, and clamps what it reads through
//! [`sanitized_preferences`]. Defined in `deptheos_app::preferences`.

pub use deptheos_app::preferences::ExportPreferences;
use deptheos_app::preferences::{AlignmentPreferences, Preference};
use deptheos_app::{ExportSpace, TiffCompression};

use crate::SettingRange;

/// How TIFF files compress their pixels (`deptheos_app::TiffCompression`).
/// All three are lossless.
#[uniffi::remote(Enum)]
pub enum TiffCompression {
    /// Uncompressed: the largest files, the fastest to write and read.
    None,
    /// LZW, the most widely read compression.
    Lzw,
    /// ZIP (Adobe Deflate): the smallest. The default.
    Zip,
}

/// How exports are written and named
/// (`deptheos_app::preferences::ExportPreferences`).
#[uniffi::remote(Record)]
pub struct ExportPreferences {
    /// The file name's pattern: {stack}, {render}, {method} and {date} are
    /// replaced ([`StackResult::export_file_stem`]).
    pub file_name_pattern: String,
    /// 1–100 ([`preference_range`]).
    pub jpeg_quality: u8,
    /// PNG and TIFF keep 16 bits where the result has them: the Export
    /// sheet's last choice, which the next starts from.
    pub sixteen_bit: bool,
    pub tiff_compression: TiffCompression,
    /// The space an adjusted camera RAW render exports in: the Export
    /// sheet's last choice, which the next starts from.
    pub space: ExportSpace,
}

/// The color space an adjusted camera RAW render exports in
/// (`deptheos_app::ExportSpace`); others keep their own.
#[uniffi::remote(Enum)]
pub enum ExportSpace {
    Srgb,
    DisplayP3,
    AdobeRgb,
    /// At 16 bits only; at 8 it writes Adobe RGB.
    ProPhoto,
}

/// Grid search's limits, and whether it compensates exposure; only when
/// Grid search aligns (`deptheos_app::preferences::AlignmentPreferences`).
#[uniffi::remote(Record)]
pub struct AlignmentPreferences {
    /// Percent of the frame's longer side.
    pub grid_horizontal_pct: f32,
    /// Percent of the frame's longer side.
    pub grid_vertical_pct: f32,
    /// Degrees; 0 turns the rotation search off.
    pub grid_rotation_deg: f32,
    /// Percent; 0 turns the magnification search off.
    pub grid_magnification_pct: f32,
    /// Scale each frame to the first frame's brightness.
    pub exposure_compensation: bool,
}

/// Every preference the apps share.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct SharedPreferences {
    pub export: ExportPreferences,
    pub alignment: AlignmentPreferences,
}

/// The defaults, as on the desktop.
#[uniffi::export]
pub fn default_preferences() -> SharedPreferences {
    SharedPreferences {
        export: ExportPreferences::default(),
        alignment: AlignmentPreferences::default(),
    }
}

/// `preferences` with every value in range, for values read back from
/// storage. A NaN becomes the range's lowest.
#[uniffi::export]
pub fn sanitized_preferences(preferences: SharedPreferences) -> SharedPreferences {
    SharedPreferences {
        export: preferences.export.sanitized(),
        alignment: preferences.alignment.sanitized(),
    }
}

/// One shared preference, for its label, help and range
/// (`deptheos_app::preferences::Preference`).
#[uniffi::remote(Enum)]
pub enum Preference {
    JpegQuality,
    TiffCompression,
    FileName,
    GridHorizontal,
    GridVertical,
    GridRotation,
    GridMagnification,
    ExposureCompensation,
}

/// A number preference's range, step and default; `None` for a switch, a
/// choice or the file name.
#[uniffi::export]
pub fn preference_range(preference: Preference) -> Option<SettingRange> {
    let (lowest, max) = preference.range()?;
    let (export, alignment) = (ExportPreferences::default(), AlignmentPreferences::default());
    let default_value = match preference {
        Preference::JpegQuality => f32::from(export.jpeg_quality),
        Preference::GridHorizontal => alignment.grid_horizontal_pct,
        Preference::GridVertical => alignment.grid_vertical_pct,
        Preference::GridRotation => alignment.grid_rotation_deg,
        Preference::GridMagnification => alignment.grid_magnification_pct,
        Preference::TiffCompression | Preference::FileName | Preference::ExposureCompensation => {
            return None;
        }
    };
    Some(SettingRange { min: lowest.min, max, step: lowest.step, default_value })
}
