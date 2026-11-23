//! The preferences both apps offer (ADR-150): export format and naming, and
//! Grid search's limits. Each front end stores and shows them its own way;
//! this module holds the fields, defaults, ranges, clamping, labels, and how
//! they reach a stack ([`AlignmentPreferences::apply`]) and an export
//! ([`ExportPreferences::save_options`], [`ExportPreferences::file_stem`]).
//!
//! The serde names are the desktop `preferences.json` keys. A missing key
//! takes its default; an unknown TIFF compression is ZIP.
//!
//! None of these decide whether a working version is edited
//! ([`crate::library::stacks::Compared`]): they are the app's setup, not the
//! stack's.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::library::export::{self, NameParts};
use crate::runs::stacker::StackerKind;
use crate::words::{Words, say};
use crate::{GridConfig, SaveOptions, SessionOptions, TiffCompression};

/// How exports are written and named.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPreferences {
    /// The file name pattern; {stack}, {render}, {method} and {date} are
    /// replaced ([`export::file_stem`]).
    pub file_name_pattern: String,
    /// JPEG quality, in [`Preference::JpegQuality`]'s range.
    pub jpeg_quality: u8,
    /// PNG and TIFF keep 16 bits where the result has them: the last choice
    /// in an Export sheet or the Batch dialog, not a setting.
    pub sixteen_bit: bool,
    #[serde(with = "tiff_key")]
    pub tiff_compression: TiffCompression,
}

impl Default for ExportPreferences {
    fn default() -> Self {
        let save = SaveOptions::default();
        Self {
            file_name_pattern: export::DEFAULT_PATTERN.into(),
            jpeg_quality: save.jpeg_quality,
            sixteen_bit: save.sixteen_bit,
            tiff_compression: save.tiff_compression,
        }
    }
}

impl ExportPreferences {
    /// Every value within its range.
    pub fn sanitized(mut self) -> Self {
        let range = crate::JPEG_QUALITY_RANGE;
        self.jpeg_quality = self.jpeg_quality.clamp(*range.start(), *range.end());
        self
    }

    /// What a save is written with.
    pub fn save_options(&self) -> SaveOptions {
        SaveOptions {
            jpeg_quality: self.clone().sanitized().jpeg_quality,
            sixteen_bit: self.sixteen_bit,
            tiff_compression: self.tiff_compression,
        }
    }

    /// The file name (no extension) the pattern makes for `parts`.
    pub fn file_stem(&self, parts: &NameParts) -> String {
        export::file_stem(&self.file_name_pattern, parts)
    }

    /// "For example: bee-4.jpg": the pattern applied to a sample Depth render
    /// #4 of "bee", exported on `date` (`YYYY-MM-DD`).
    pub fn file_name_example(&self, extension: &str, date: &str, words: &Words) -> String {
        let (stack, method) =
            (say!(words, "pref-file-name-sample-stack"), StackerKind::Depth.label(words));
        let parts = NameParts { stack: &stack, render: Some(4), method: &method, date };
        let name = format!("{}.{extension}", self.file_stem(&parts));
        say!(words, "pref-file-name-example", name = name)
    }
}

/// Grid search's limits, and whether it compensates exposure.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlignmentPreferences {
    /// Percent of the frame's longer side.
    pub grid_horizontal_pct: f32,
    /// Percent of the frame's longer side.
    pub grid_vertical_pct: f32,
    /// Degrees; 0 turns the rotation search off.
    pub grid_rotation_deg: f32,
    /// Percent; 0 turns the magnification search off.
    pub grid_magnification_pct: f32,
    /// The brightness ratio Grid search measures is applied to each frame.
    pub exposure_compensation: bool,
}

impl Default for AlignmentPreferences {
    fn default() -> Self {
        let grid = GridConfig::default();
        Self {
            grid_horizontal_pct: grid.horizontal_pct,
            grid_vertical_pct: grid.vertical_pct,
            grid_rotation_deg: grid.rotation_deg,
            grid_magnification_pct: grid.magnification_pct,
            exposure_compensation: grid.apply_brightness,
        }
    }
}

impl AlignmentPreferences {
    /// Every value within its range; NaN becomes 0, since JSON would store
    /// it as `null` and the next read would refuse it.
    pub fn sanitized(mut self) -> Self {
        let clamp = |v: f32, preference: Preference| {
            let (lo, hi) = preference.range().expect("a range");
            if v.is_finite() { v } else { 0.0 }.clamp(lo.min, hi)
        };
        self.grid_horizontal_pct = clamp(self.grid_horizontal_pct, Preference::GridHorizontal);
        self.grid_vertical_pct = clamp(self.grid_vertical_pct, Preference::GridVertical);
        self.grid_rotation_deg = clamp(self.grid_rotation_deg, Preference::GridRotation);
        self.grid_magnification_pct =
            clamp(self.grid_magnification_pct, Preference::GridMagnification);
        self
    }

    /// Grid search's configuration: its default tuning with these limits.
    pub fn grid(&self) -> GridConfig {
        GridConfig {
            horizontal_pct: self.grid_horizontal_pct,
            vertical_pct: self.grid_vertical_pct,
            rotation_deg: self.grid_rotation_deg,
            magnification_pct: self.grid_magnification_pct,
            apply_brightness: self.exposure_compensation,
            ..GridConfig::default()
        }
    }

    /// `options` with these limits as Grid search's.
    pub fn apply(&self, options: SessionOptions) -> SessionOptions {
        SessionOptions { alignment: options.alignment.with_grid(self.grid()), ..options }
    }
}

/// One shared preference, for its label, help and range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// A number preference's lowest value and step ([`Preference::range`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lowest {
    pub min: f32,
    pub step: f32,
}

impl Preference {
    pub const ALL: [Self; 8] = [
        Self::JpegQuality,
        Self::TiffCompression,
        Self::FileName,
        Self::GridHorizontal,
        Self::GridVertical,
        Self::GridRotation,
        Self::GridMagnification,
        Self::ExposureCompensation,
    ];

    /// What the control is called.
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::JpegQuality => say!(words, "pref-jpeg-quality"),
            Self::TiffCompression => say!(words, "pref-tiff-compression"),
            Self::FileName => say!(words, "pref-file-name"),
            Self::GridHorizontal => say!(words, "pref-grid-horizontal"),
            Self::GridVertical => say!(words, "pref-grid-vertical"),
            Self::GridRotation => say!(words, "pref-grid-rotation"),
            Self::GridMagnification => say!(words, "pref-grid-magnification"),
            Self::ExposureCompensation => say!(words, "pref-exposure-compensation"),
        }
    }

    /// What it does, under the control (the desktop) or as its footer (iOS).
    pub fn help(self, words: &Words) -> String {
        match self {
            Self::JpegQuality => say!(words, "pref-jpeg-quality-help"),
            Self::TiffCompression => say!(words, "pref-tiff-compression-help"),
            Self::FileName => say!(words, "pref-file-name-help"),
            Self::GridHorizontal | Self::GridVertical => say!(words, "pref-grid-shift-help"),
            Self::GridRotation => say!(words, "pref-grid-rotation-help"),
            Self::GridMagnification => say!(words, "pref-grid-magnification-help"),
            Self::ExposureCompensation => say!(words, "pref-exposure-compensation-help"),
        }
    }

    /// A number's lowest value and step, and its highest; `None` otherwise.
    pub fn range(self) -> Option<(Lowest, f32)> {
        let range = |(min, max): (f32, f32), step| Some((Lowest { min, step }, max));
        let jpeg = crate::JPEG_QUALITY_RANGE;
        match self {
            Self::JpegQuality => range((f32::from(*jpeg.start()), f32::from(*jpeg.end())), 1.0),
            Self::GridHorizontal | Self::GridVertical => {
                range(GridConfig::SHIFT_PCT_RANGE, GridConfig::SHIFT_PCT_STEP)
            }
            Self::GridRotation => {
                range(GridConfig::ROTATION_DEG_RANGE, GridConfig::ROTATION_DEG_STEP)
            }
            Self::GridMagnification => {
                range(GridConfig::MAGNIFICATION_PCT_RANGE, GridConfig::MAGNIFICATION_PCT_STEP)
            }
            Self::TiffCompression | Self::FileName => None,
            Self::ExposureCompensation => None,
        }
    }
}

/// "Grid search limits": the heading over the four limits.
pub fn grid_limits_label(words: &Words) -> String {
    say!(words, "pref-grid-limits")
}

/// What the four limits do, under their heading.
pub fn grid_limits_help(words: &Words) -> String {
    say!(words, "pref-grid-limits-help")
}

/// A TIFF compression as the choice offers it: "ZIP", "LZW", "None".
pub fn tiff_compression_label(compression: TiffCompression, words: &Words) -> String {
    match compression {
        TiffCompression::Zip => "ZIP".into(),
        TiffCompression::Lzw => "LZW".into(),
        TiffCompression::None => say!(words, "pref-tiff-none"),
    }
}

/// Serde for the TIFF compression; an unknown key is ZIP.
mod tiff_key {
    use super::*;

    pub(crate) fn serialize<S: Serializer>(
        value: &TiffCompression,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_str(value.key())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<TiffCompression, D::Error> {
        let key = String::deserialize(d)?;
        Ok(TiffCompression::from_key(&key).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::words::explain::Platform;

    /// The limits reach a stack's alignment, on or off.
    #[test]
    fn limits_apply_to_the_alignment() {
        let limits =
            AlignmentPreferences { grid_rotation_deg: 4.0, ..AlignmentPreferences::default() };
        for alignment in [crate::Alignment::default(), crate::Alignment::default().turned_over()] {
            let options = SessionOptions { alignment, ..Default::default() };
            let applied = limits.apply(options).alignment;
            assert_eq!(applied.settings().grid.rotation_deg, 4.0);
            assert_eq!(applied.is_on(), alignment.is_on());
        }
    }

    #[test]
    fn defaults_match_the_engine() {
        let export = ExportPreferences::default();
        assert_eq!(export.save_options(), SaveOptions::default());
        assert_eq!(export.file_name_pattern, export::DEFAULT_PATTERN);
        let alignment = AlignmentPreferences::default();
        let engine = GridConfig::default();
        let grid = alignment.grid();
        assert_eq!(
            (grid.horizontal_pct, grid.vertical_pct, grid.rotation_deg, grid.magnification_pct),
            (
                engine.horizontal_pct,
                engine.vertical_pct,
                engine.rotation_deg,
                engine.magnification_pct
            )
        );
        assert_eq!(grid.apply_brightness, engine.apply_brightness);
        assert_eq!(ExportPreferences::default().sanitized(), export);
        assert_eq!(alignment.sanitized(), alignment);
        for preference in Preference::ALL {
            if let Some((lo, hi)) = preference.range() {
                assert!(lo.min < hi && lo.step > 0.0, "{preference:?}");
            }
        }
    }

    #[test]
    fn stored_values_are_clamped_and_keys_tolerated() {
        let export: ExportPreferences =
            serde_json::from_str(r#"{"jpeg_quality": 0, "tiff_compression": "jpeg2000"}"#).unwrap();
        let export = export.sanitized();
        assert_eq!(export.jpeg_quality, 1);
        assert_eq!(export.tiff_compression, TiffCompression::Zip);
        assert_eq!(export.file_name_pattern, export::DEFAULT_PATTERN, "a key left out");
        let alignment = AlignmentPreferences {
            grid_rotation_deg: f32::NAN,
            grid_magnification_pct: 1000.0,
            grid_horizontal_pct: -4.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(alignment.grid_rotation_deg, 0.0);
        assert_eq!(alignment.grid_magnification_pct, GridConfig::MAGNIFICATION_PCT_RANGE.1);
        assert_eq!(alignment.grid_horizontal_pct, 0.0);
    }

    #[test]
    fn the_example_names_a_sample_render() {
        let words = Words::english(Platform::Ios);
        let export = ExportPreferences::default();
        assert_eq!(export.file_name_example("jpg", "2026-11-14", &words), "For example: bee-4.jpg");
        let dated = ExportPreferences { file_name_pattern: "{date} {method}".into(), ..export };
        assert_eq!(
            dated.file_name_example("tif", "2026-11-14", &words),
            "For example: 2026-11-14 Depth.tif"
        );
        assert_eq!(tiff_compression_label(TiffCompression::None, &words), "None");
    }
}
