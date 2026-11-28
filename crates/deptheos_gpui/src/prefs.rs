//! The user's preferences: appearance, external tools and folders, camera
//! RAW color, the grid aligner's limits, new-stack defaults, read-ahead and
//! the GPU.
//!
//! One small JSON file in the config directory, read at startup and
//! rewritten on every change. Every field has a default: missing fields,
//! unknown choices and wrong types take it, unknown keys are ignored, so
//! another version's file loads. An unreadable or non-object file is all
//! defaults, set aside first (`preferences.unreadable-<id>.json`) since the
//! next change rewrites it. Values are clamped to the window's ranges. Views
//! read the [`Prefs`] global with [`prefs`], change it with [`update_prefs`]
//! and observe it.
//!
//! Settings that change the result reach a stack through
//! [`Preferences::session_options`] and are recorded with each render. The
//! preferences shared with iOS are in `deptheos_app::preferences` (ADR-150),
//! flattened into this file under the same keys.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use deptheos_app::library::export::ExportFormat;
use deptheos_app::preferences::{AlignmentPreferences, ExportPreferences, tiff_compression_label};
use deptheos_app::{
    AlignMode, AlignSettings, Alignment, DcpPreference, DeviceChoice, RawBackend, SaveOptions,
    SessionOptions, TiffCompression,
};
use gpui::{App, Global};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::export_dialog::ExportPlace;
use crate::params::Algorithm;
use crate::words::{DesktopWords, say};

/// An enum stored by a stable key, and offered in a dropdown by its label.
pub trait Keyed: Copy + PartialEq + Default + 'static {
    /// Every value, in the order offered.
    const ALL: &'static [Self];

    fn key(self) -> &'static str;

    /// What the dropdown offers it as.
    fn label(self, words: &DesktopWords) -> String;

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.key() == key)
    }
}

/// [`Keyed`] from the type's `ALL` and `key()`, with `label` as given.
macro_rules! keyed {
    ($t:ty, $label:expr) => {
        impl Keyed for $t {
            const ALL: &'static [Self] = &<$t>::ALL;

            fn key(self) -> &'static str {
                <$t>::key(self)
            }

            fn label(self, words: &DesktopWords) -> String {
                let label: fn($t, &DesktopWords) -> String = $label;
                label(self, words)
            }
        }
    };
}

// A RAW decoder's and a file format's labels are their names.
keyed!(RawBackend, |b, _| b.label().into());
keyed!(ExportFormat, |f, _| f.label().into());
keyed!(TiffCompression, |c, w| tiff_compression_label(c, w.shared()));
keyed!(DcpPreference, |p, w| match p {
    DcpPreference::AdobeStandard => say!(w, "prefs-profile-adobe-standard"),
    DcpPreference::CameraStandard => say!(w, "prefs-profile-camera-standard"),
    DcpPreference::None => say!(w, "prefs-profile-none"),
});
keyed!(RawDecoder, RawDecoder::label);
keyed!(ExportPlace, ExportPlace::label);
keyed!(AlignMode, |m, w| deptheos_app::align_label(m, w.shared()));
keyed!(Algorithm, |a, w| a.label(w.shared()));

// The Preferences window's ranges for the desktop's own preferences; a
// loaded file is clamped into them (shared ones:
// `deptheos_app::preferences::Preference::range`).
pub const PREFETCH_MB: (u32, u32) = (256, 8192);
pub const CONVERTER_JOBS: (u32, u32) = (1, deptheos_app::MAX_CONVERTER_JOBS as u32);

/// Light or dark: following the system, or fixed
/// ([`crate::look::apply_appearance`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// What the preferences file calls it.
    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

keyed!(Appearance, |a, w| match a {
    Appearance::System => say!(w, "prefs-appearance-system"),
    Appearance::Light => say!(w, "prefs-appearance-light"),
    Appearance::Dark => say!(w, "prefs-appearance-dark"),
});

/// The default RAW decoder; Automatic is Adobe DNG Converter when installed,
/// else LibRaw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RawDecoder {
    #[default]
    Automatic,
    LibRaw,
    AdobeDng,
}

impl RawDecoder {
    pub const ALL: [Self; 3] = [Self::Automatic, Self::LibRaw, Self::AdobeDng];

    /// What the preferences file calls it: a backend's key, or "automatic".
    pub fn key(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::LibRaw => RawBackend::LibRaw.key(),
            Self::AdobeDng => RawBackend::AdobeDng.key(),
        }
    }

    pub fn label(self, words: &DesktopWords) -> String {
        match self {
            Self::Automatic => say!(words, "prefs-decoder-automatic"),
            Self::LibRaw => RawBackend::LibRaw.label().into(),
            Self::AdobeDng => RawBackend::AdobeDng.label().into(),
        }
    }

    /// The backend this means here and now.
    pub fn backend(self) -> RawBackend {
        match self {
            Self::Automatic if RawBackend::AdobeDng.is_available() => RawBackend::AdobeDng,
            Self::Automatic | Self::LibRaw => RawBackend::LibRaw,
            Self::AdobeDng => RawBackend::AdobeDng,
        }
    }
}

/// Serde for the GPU choice: "automatic", "software", or any other string as
/// the GPU's name.
mod device {
    use super::*;

    pub const AUTOMATIC: &str = "automatic";
    pub const SOFTWARE: &str = "software";

    pub fn key(choice: &DeviceChoice) -> &str {
        match choice {
            DeviceChoice::Automatic => AUTOMATIC,
            DeviceChoice::Software => SOFTWARE,
            DeviceChoice::Named(name) => name,
        }
    }

    pub fn from_key(key: &str) -> DeviceChoice {
        match key {
            AUTOMATIC | "" => DeviceChoice::Automatic,
            SOFTWARE => DeviceChoice::Software,
            name => DeviceChoice::Named(name.to_owned()),
        }
    }

    pub fn serialize<S: Serializer>(value: &DeviceChoice, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(key(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DeviceChoice, D::Error> {
        Ok(from_key(&String::deserialize(d)?))
    }
}

pub use device::{from_key as device_from_key, key as device_key};

/// Serde for a [`Keyed`] field: its key, an unknown key taking the default.
mod keyed {
    use super::*;

    pub fn serialize<T: Keyed, S: Serializer>(value: &T, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(value.key())
    }

    pub fn deserialize<'de, T: Keyed, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
        let key = String::deserialize(d)?;
        Ok(T::from_key(&key).unwrap_or_default())
    }
}

/// As [`keyed`], but an unknown key becomes DNG, this field's default.
mod keyed_raw_format {
    use super::*;

    pub use super::keyed::serialize;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ExportFormat, D::Error> {
        let key = String::deserialize(d)?;
        Ok(ExportFormat::from_key(&key).unwrap_or(ExportFormat::first_for(true)))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    // General
    /// Light, dark, or as the system is.
    #[serde(with = "keyed")]
    pub appearance: Appearance,
    /// Adobe DNG Converter, when not at its default install location.
    pub converter_path: Option<PathBuf>,
    /// How many files Adobe DNG Converter converts at once.
    pub converter_jobs: u32,
    /// Ask it for uncompressed DNGs: faster to decode, twice the disk.
    pub converter_uncompressed: bool,
    /// Where the history is kept, when not the default (read at startup).
    pub history_dir: Option<PathBuf>,
    /// Export's starting format for an ordinary stack: the last one used.
    #[serde(with = "keyed")]
    pub save_format: ExportFormat,
    /// The same for a RAW or DNG stack, which starts as DNG.
    #[serde(with = "keyed_raw_format")]
    pub save_format_raw: ExportFormat,
    /// The folder the last export went to (the save dialogs start there).
    pub save_dir: Option<PathBuf>,
    /// Where the Export sheet saves: beside the frames, or the last folder.
    #[serde(with = "keyed")]
    pub export_place: ExportPlace,
    /// Show an export in the Finder (or file manager) once it is saved.
    pub reveal_export: bool,
    /// Shared with iOS: `file_name_pattern`, `jpeg_quality`, `sixteen_bit`,
    /// `tiff_compression` and `export_space` in the file.
    #[serde(flatten)]
    pub export: ExportPreferences,
    /// Ask GitHub for a newer release at startup.
    pub check_for_updates: bool,

    // RAW
    #[serde(with = "keyed")]
    pub raw_backend: RawDecoder,
    #[serde(with = "keyed")]
    pub camera_profile: DcpPreference,
    /// Folders searched for `.dcp` profiles before Adobe's.
    pub profile_dirs: Vec<PathBuf>,

    // Alignment
    /// Shared with iOS: `grid_horizontal_pct`, `grid_vertical_pct`,
    /// `grid_rotation_deg`, `grid_magnification_pct` and
    /// `exposure_compensation` in the file.
    #[serde(flatten)]
    pub alignment: AlignmentPreferences,

    // Advanced
    /// Decoded JPEG/PNG/TIFF frames held ahead of the stacker, in MB.
    pub prefetch_mb: u32,
    /// The device that stacks and renders (read at startup).
    #[serde(with = "device")]
    pub gpu: DeviceChoice,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            converter_path: None,
            converter_jobs: deptheos_app::default_converter_jobs() as u32,
            converter_uncompressed: false,
            history_dir: None,
            save_format: ExportFormat::first_for(false),
            save_format_raw: ExportFormat::first_for(true),
            save_dir: None,
            export_place: ExportPlace::default(),
            reveal_export: true,
            export: ExportPreferences::default(),
            check_for_updates: true,
            raw_backend: RawDecoder::default(),
            camera_profile: DcpPreference::default(),
            profile_dirs: Vec::new(),
            alignment: AlignmentPreferences::default(),
            prefetch_mb: (SessionOptions::PREFETCH_BYTES >> 20) as u32,
            gpu: DeviceChoice::Automatic,
        }
    }
}

impl Preferences {
    /// A stack's options from the preferences, aligning by default; the
    /// sidebar fills in the rest (via [`Self::alignment`], and the RAW decoder).
    pub fn session_options(&self) -> SessionOptions {
        SessionOptions {
            alignment: self.alignment(AlignMode::default(), true),
            camera_profile: self.camera_profile,
            prefetch_bytes: u64::from(self.prefetch_mb) << 20,
            ..SessionOptions::default()
        }
    }

    /// Aligning by `mode`, with these grid limits.
    pub fn alignment(&self, mode: AlignMode, crop: bool) -> Alignment {
        Alignment::On(AlignSettings { mode, grid: self.alignment.grid(), crop })
    }

    /// Export's starting format for a stack, remembered per kind (linear or not).
    pub fn save_format_for(&self, linear: bool) -> ExportFormat {
        if linear { self.save_format_raw } else { self.save_format.for_stack(false) }
    }

    /// Remember `format` as the one last used for that kind of stack.
    pub fn remember_save_format(&mut self, linear: bool, format: ExportFormat) {
        if linear {
            self.save_format_raw = format;
        } else {
            self.save_format = format;
        }
    }

    pub fn save_options(&self) -> SaveOptions {
        self.export.save_options()
    }

    /// The history's folder: the chosen one, else the default.
    pub fn history_root(&self) -> Option<PathBuf> {
        self.history_dir.clone().or_else(deptheos_app::library::history::History::default_root)
    }

    /// Whether `other` gives the lower layers what this does.
    fn same_environment(&self, other: &Self) -> bool {
        self.converter_path == other.converter_path
            && self.converter_jobs == other.converter_jobs
            && self.converter_uncompressed == other.converter_uncompressed
            && self.profile_dirs == other.profile_dirs
    }

    /// Hand the process-wide settings (converter, profile folders) to the
    /// engine's lower layers.
    fn apply_environment(&self) {
        deptheos_app::set_converter_executable(self.converter_path.clone());
        deptheos_app::set_converter_jobs(Some(self.converter_jobs as usize));
        deptheos_app::set_converter_uncompressed(self.converter_uncompressed);
        deptheos_app::set_profile_directories(self.profile_dirs.clone());
    }

    /// Read `path`; a missing or bad file is the defaults (see the module
    /// doc), a bad one set aside first ([`deptheos_app::set_aside`]). Never
    /// keeps the app from starting.
    #[cfg(test)]
    pub fn load(path: &Path) -> Self {
        Self::load_noting(path).0
    }

    /// [`Self::load`], and the [`Reset`] when the file existed but was unusable.
    pub fn load_noting(path: &Path) -> (Self, Option<Reset>) {
        let reset = || {
            let kept = deptheos_app::set_aside(path)
                .inspect_err(|e| tracing::warn!("could not keep {} aside: {e}", path.display()))
                .ok();
            (Self::default(), Some(Reset { kept }))
        };
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                tracing::warn!("could not read preferences {}: {e}", path.display());
                return reset();
            }
        };
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(serde_json::Value::Object(fields)) => (Self::field_by_field(fields), None),
            Ok(_) => {
                tracing::warn!(
                    "preferences {} are not an object; using the defaults",
                    path.display()
                );
                reset()
            }
            Err(e) => {
                tracing::warn!(
                    "preferences {} did not parse ({e}); using the defaults",
                    path.display()
                );
                reset()
            }
        }
    }

    /// Each field of `fields` read alone, an unreadable one left at its
    /// default, so one bad value never costs the others (`history_dir`
    /// above all, which picks the Library).
    fn field_by_field(fields: serde_json::Map<String, serde_json::Value>) -> Self {
        let mut kept = serde_json::Map::new();
        for (key, value) in fields {
            kept.insert(key.clone(), value);
            if serde_json::from_value::<Self>(serde_json::Value::Object(kept.clone())).is_err() {
                tracing::warn!("preference {key} could not be read; using its default");
                kept.remove(&key);
            }
        }
        serde_json::from_value::<Self>(serde_json::Value::Object(kept))
            .unwrap_or_default()
            .sanitized()
    }

    /// Every value within the range the window offers for it.
    fn sanitized(mut self) -> Self {
        // The shared ones clamped by the engine (a non-number becomes 0,
        // since NaN would save as `null` and fail the next read).
        self.export = self.export.sanitized();
        self.alignment = self.alignment.sanitized();
        self.converter_jobs = self.converter_jobs.clamp(CONVERTER_JOBS.0, CONVERTER_JOBS.1);
        self.prefetch_mb = self.prefetch_mb.clamp(PREFETCH_MB.0, PREFETCH_MB.1);
        self
    }

    /// Write through a temporary file, so a crash leaves the old one whole.
    pub fn save(&self, path: &Path) -> Result<(), deptheos_app::StoreError> {
        save_json(path, self)
    }

    /// `…/Deptheos/preferences.json` in the platform's config directory.
    fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("Deptheos").join("preferences.json"))
    }
}

/// Write `value` as JSON through a temporary file, making its folder if needed.
pub(crate) fn save_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), deptheos_app::StoreError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|source| deptheos_app::StoreError::Io { path: dir.to_path_buf(), source })?;
    }
    deptheos_app::write_json_atomic(path, value)
}

/// The file existed but couldn't be read or parsed; defaults are in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reset {
    /// Where the old file was kept; none when it couldn't be set aside.
    pub kept: Option<PathBuf>,
}

/// The preferences in effect, and their file (none in tests).
pub struct Prefs {
    values: Preferences,
    path: Option<PathBuf>,
    environment: Environment,
}

/// Whether converter and profile-folder changes reach the lower layers at
/// once, or wait for the running stack, batch or sequence so nothing
/// changes under its decodes.
///
/// Lifecycle: `Free`, or `Held` while a run runs, changed only by
/// [`hold_environment`] and [`release_environment`], from the app's
/// `set_running`.
///
/// ```text
/// Free ──a run starts (hold_environment)──▶ Held { applied }
/// Held ──it ends (release_environment)──▶ Free, what changed meanwhile applied
/// ```
enum Environment {
    Free,
    /// What the lower layers were given as the run started.
    Held {
        applied: Box<Preferences>,
    },
}

impl Global for Prefs {}

impl Prefs {
    /// The preferences from their default file, and the file. Read before
    /// the engine, which needs the GPU choice.
    pub fn load_default() -> (Preferences, Option<PathBuf>, Option<Reset>) {
        let path = Preferences::default_path();
        let (values, reset) = path.as_deref().map(Preferences::load_noting).unwrap_or_default();
        (values, path, reset)
    }

    /// Make `values` the preferences in effect, kept at `path`.
    pub fn install(values: Preferences, path: Option<PathBuf>, cx: &mut App) {
        // Only a setup unlike the defaults' is handed down (one set by hand,
        // as tests do, is left alone).
        if !values.same_environment(&Preferences::default()) {
            values.apply_environment();
        }
        cx.set_global(Self { values, path, environment: Environment::Free });
    }
}

/// A run starts: converter and profile-folder changes wait for
/// [`release_environment`].
pub fn hold_environment(cx: &mut App) {
    if cx.try_global::<Prefs>().is_none() {
        Prefs::install(Preferences::default(), None, cx);
    }
    let prefs = cx.global::<Prefs>();
    if let Environment::Free = prefs.environment {
        let applied = Box::new(prefs.values.clone());
        tracing::debug!("preferences: the converter and profile folders held");
        cx.global_mut::<Prefs>().environment = Environment::Held { applied };
    }
}

/// The run ended: what was changed meanwhile goes down now.
pub fn release_environment(cx: &mut App) {
    let Some(prefs) = cx.try_global::<Prefs>() else { return };
    let Environment::Held { applied } = &prefs.environment else { return };
    let changed = !prefs.values.same_environment(applied);
    tracing::debug!(changed, "preferences: the converter and profile folders let go");
    if changed {
        prefs.values.apply_environment();
    }
    cx.global_mut::<Prefs>().environment = Environment::Free;
}

/// Whether those changes wait for a run ([`hold_environment`]), for Settings
/// to say so.
pub fn environment_held(cx: &App) -> bool {
    cx.try_global::<Prefs>().is_some_and(|p| matches!(p.environment, Environment::Held { .. }))
}

/// The preferences in effect (the defaults before [`Prefs::install`]).
pub fn prefs(cx: &App) -> &Preferences {
    static DEFAULTS: LazyLock<Preferences> = LazyLock::new(Preferences::default);
    cx.try_global::<Prefs>().map_or(&DEFAULTS, |p| &p.values)
}

/// Change, apply and save the preferences; [`Prefs`] observers are notified
/// only on a change.
pub fn update_prefs(cx: &mut App, f: impl FnOnce(&mut Preferences)) {
    if cx.try_global::<Prefs>().is_none() {
        Prefs::install(Preferences::default(), None, cx);
    }
    let mut next = cx.global::<Prefs>().values.clone();
    f(&mut next);
    let current = &cx.global::<Prefs>().values;
    if next == *current {
        return;
    }
    // Process-wide: only on change, and not under a run, which applies them
    // as it ends.
    let held = matches!(cx.global::<Prefs>().environment, Environment::Held { .. });
    if !next.same_environment(current) && !held {
        next.apply_environment();
    }
    // Written on the store thread once settled (a slider drag writes once),
    // never here (ADR-143).
    if let Some(path) = cx.global::<Prefs>().path.clone() {
        let values = next.clone();
        deptheos_app::engine::store::store().keep("preferences", move || {
            if let Err(e) = values.save(&path) {
                tracing::error!("could not save preferences to {}: {e}", path.display());
            }
        });
    }
    cx.global_mut::<Prefs>().values = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use deptheos_app::GridConfig;

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("preferences.json");
        let prefs = Preferences {
            converter_path: Some("/opt/dng".into()),
            camera_profile: DcpPreference::CameraStandard,
            alignment: AlignmentPreferences { grid_rotation_deg: 2.5, ..Default::default() },
            export: ExportPreferences {
                tiff_compression: TiffCompression::Lzw,
                file_name_pattern: "{date} {stack}".into(),
                ..Default::default()
            },
            save_format: ExportFormat::Tiff,
            profile_dirs: vec!["/a".into(), "/b".into()],
            gpu: DeviceChoice::Named("AMD Radeon(TM) Graphics".into()),
            appearance: Appearance::Dark,
            ..Preferences::default()
        };
        prefs.save(&path).unwrap();
        assert_eq!(Preferences::load(&path), prefs);
    }

    /// An unknown value takes the default, an unknown key is ignored, a
    /// missing field takes its default.
    #[test]
    fn tolerates_other_versions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        std::fs::write(
            &path,
            r#"{"jpeg_quality": 80, "save_format": "webp", "future_field": [1, 2]}"#,
        )
        .unwrap();
        let loaded = Preferences::load(&path);
        assert_eq!(loaded.export.jpeg_quality, 80);
        assert_eq!(loaded.save_format, ExportFormat::Jpeg);
        assert_eq!(loaded.save_format_raw, ExportFormat::Dng, "a field the file leaves out");
        assert_eq!(loaded.converter_jobs, Preferences::default().converter_jobs);
    }

    /// Export starts as JPEG, or DNG for RAW/DNG; each kind remembers its last.
    #[test]
    fn export_starts_as_jpeg_or_dng_for_raw() {
        let mut p = Preferences::default();
        assert_eq!(p.save_format_for(false), ExportFormat::Jpeg);
        assert_eq!(p.save_format_for(true), ExportFormat::Dng);
        p.remember_save_format(true, ExportFormat::Tiff);
        assert_eq!(p.save_format_for(true), ExportFormat::Tiff);
        assert_eq!(p.save_format_for(false), ExportFormat::Jpeg, "the other kind keeps its own");
        // DNG set by hand for ordinary stacks can't be had: TIFF in its place.
        p.save_format = ExportFormat::Dng;
        assert_eq!(p.save_format_for(false), ExportFormat::Tiff);
        let unknown: Preferences = serde_json::from_str(r#"{"save_format_raw": "webp"}"#).unwrap();
        assert_eq!(unknown.save_format_raw, ExportFormat::Dng);
    }

    /// The GPU is "automatic", "software" or a name; the engine falls back
    /// when no GPU has that name.
    #[test]
    fn gpu_choice_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        for (json, gpu) in [
            ("automatic", DeviceChoice::Automatic),
            ("software", DeviceChoice::Software),
            ("NVIDIA GeForce RTX 5090", DeviceChoice::Named("NVIDIA GeForce RTX 5090".into())),
        ] {
            std::fs::write(&path, format!(r#"{{"gpu": "{json}"}}"#)).unwrap();
            assert_eq!(Preferences::load(&path).gpu, gpu);
        }
        assert_eq!(Preferences::default().gpu, DeviceChoice::Automatic);
    }

    /// The contents of every preferences file set aside in `dir`.
    fn kept_aside(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let name = p.file_name().unwrap().to_string_lossy();
                name.starts_with("preferences.unreadable-") && name.ends_with(".json")
            })
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect()
    }

    #[test]
    fn unreadable_or_missing_file_is_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        assert_eq!(Preferences::load(&path), Preferences::default());
        assert!(kept_aside(dir.path()).is_empty(), "nothing to keep aside");
        assert_eq!(Preferences::load_noting(&path).1, None, "a first launch is no reset");
        std::fs::write(&path, b"not json").unwrap();
        let (loaded, reset) = Preferences::load_noting(&path);
        assert_eq!(loaded, Preferences::default());
        assert_eq!(kept_aside(dir.path()), ["not json"], "kept aside");
        let kept = reset.expect("the window is told").kept.expect("and where the file went");
        assert_eq!(std::fs::read(kept).unwrap(), b"not json");
    }

    /// A non-number never reaches the file: as `null` it would lose every
    /// preference next launch.
    #[test]
    fn a_value_that_is_not_a_number_is_not_kept() {
        let alignment = AlignmentPreferences { grid_rotation_deg: f32::NAN, ..Default::default() };
        let prefs = Preferences { alignment, ..Default::default() }.sanitized();
        assert!(prefs.alignment.grid_rotation_deg.is_finite());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        prefs.save(&path).unwrap();
        assert_eq!(Preferences::load_noting(&path), (prefs, None));
    }

    /// A wrong-typed field takes its default alone; the others (the Library's
    /// folder above all) stay, and nothing is set aside.
    #[test]
    fn a_bad_field_falls_back_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let text = r#"{"jpeg_quality": "high", "sixteen_bit": false,
            "history_dir": "/Volumes/Photos/Deptheos", "prefetch_mb": [1],
            "grid_rotation_deg": null, "converter_jobs": 3}"#;
        std::fs::write(&path, text).unwrap();
        let (loaded, reset) = Preferences::load_noting(&path);
        assert_eq!(reset, None);
        let defaults = Preferences::default();
        assert_eq!(loaded.history_dir, Some(PathBuf::from("/Volumes/Photos/Deptheos")));
        assert_eq!(loaded.export.jpeg_quality, defaults.export.jpeg_quality);
        assert!(!loaded.export.sixteen_bit);
        assert_eq!(loaded.prefetch_mb, defaults.prefetch_mb);
        assert_eq!(loaded.alignment.grid_rotation_deg, defaults.alignment.grid_rotation_deg);
        assert_eq!(loaded.converter_jobs, 3);
        assert!(kept_aside(dir.path()).is_empty());
        assert!(path.exists());
    }

    /// JSON that isn't an object is all defaults, set aside first.
    #[test]
    fn a_file_that_is_not_an_object_is_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let text = r#"["sixteen_bit"]"#;
        std::fs::write(&path, text).unwrap();
        assert_eq!(Preferences::load(&path), Preferences::default());
        assert_eq!(kept_aside(dir.path()), [text]);
        assert!(!path.exists(), "the next save writes a fresh file");
    }

    /// An unreadable file is set aside too; the next change writes defaults.
    #[cfg(unix)]
    #[test]
    fn a_file_that_cant_be_read_is_kept_aside() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        std::fs::write(&path, r#"{"sixteen_bit": false}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            eprintln!("skipping: the file is readable anyway (root)");
            return;
        }
        assert_eq!(Preferences::load(&path), Preferences::default());
        assert!(!path.exists(), "the next save writes a fresh file");
        let aside: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].file_name().to_string_lossy().starts_with("preferences.unreadable-"));
    }

    /// Values the window could not have set are clamped into its ranges.
    #[test]
    fn loaded_values_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let text = r#"{"prefetch_mb": 0, "grid_rotation_deg": -4, "jpeg_quality": 0,
                       "grid_magnification_pct": 1000}"#;
        std::fs::write(&path, text).unwrap();
        let loaded = Preferences::load(&path);
        assert_eq!(loaded.prefetch_mb, PREFETCH_MB.0);
        assert_eq!(loaded.alignment.grid_rotation_deg, 0.0);
        assert_eq!(loaded.export.jpeg_quality, 1);
        assert_eq!(
            loaded.alignment.grid_magnification_pct,
            deptheos_app::GridConfig::MAGNIFICATION_PCT_RANGE.1
        );
        assert!(kept_aside(dir.path()).is_empty(), "every field parsed");
        assert_eq!(Preferences::default().sanitized(), Preferences::default());
    }

    #[test]
    fn defaults_match_the_engine() {
        let prefs = Preferences::default();
        let options = prefs.session_options();
        let engine = SessionOptions::default();
        let (grid, defaults) = (prefs.alignment.grid(), GridConfig::default());
        assert_eq!(grid.horizontal_pct, defaults.horizontal_pct);
        assert_eq!(grid.magnification_pct, defaults.magnification_pct);
        assert_eq!(grid.apply_brightness, defaults.apply_brightness);
        assert_eq!(options.alignment.mode(), engine.alignment.mode());
        assert_eq!(options.alignment.crops(), engine.alignment.crops());
        assert_eq!(options.camera_profile, engine.camera_profile);
        assert_eq!(options.prefetch_bytes, engine.prefetch_bytes);
        assert_eq!(prefs.save_options(), SaveOptions::default());
    }

    /// A file with every key round-trips, the shared preferences flattened
    /// in, under the same keys and values.
    #[test]
    fn a_file_loads_and_is_written_back_unchanged() {
        let old = r#"{
  "appearance": "dark",
  "converter_path": null,
  "converter_jobs": 2,
  "converter_uncompressed": false,
  "history_dir": null,
  "save_format": "tiff",
  "save_format_raw": "dng",
  "save_dir": "/Users/someone/Exports",
  "export_place": "last",
  "reveal_export": false,
  "export_space": "display_p3",
  "file_name_pattern": "{date} {stack}-{render}",
  "jpeg_quality": 92,
  "sixteen_bit": false,
  "tiff_compression": "lzw",
  "check_for_updates": true,
  "raw_backend": "libraw",
  "camera_profile": "camera_standard",
  "profile_dirs": [],
  "grid_horizontal_pct": 7.5,
  "grid_vertical_pct": 4.0,
  "grid_rotation_deg": 2.0,
  "grid_magnification_pct": 6,
  "exposure_compensation": true,
  "reverse_check": false,
  "prefetch_mb": 1200,
  "gpu": "automatic"
}"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        std::fs::write(&path, old).unwrap();
        let (loaded, reset) = Preferences::load_noting(&path);
        assert_eq!(reset, None);
        assert_eq!(
            loaded.export,
            ExportPreferences {
                file_name_pattern: "{date} {stack}-{render}".into(),
                jpeg_quality: 92,
                sixteen_bit: false,
                tiff_compression: TiffCompression::Lzw,
                space: deptheos_app::ExportSpace::DisplayP3,
            }
        );
        assert_eq!(
            loaded.alignment,
            AlignmentPreferences {
                grid_horizontal_pct: 7.5,
                grid_vertical_pct: 4.0,
                grid_rotation_deg: 2.0,
                grid_magnification_pct: 6.0,
                exposure_compensation: true,
            }
        );
        assert_eq!(loaded.appearance, Appearance::Dark);
        assert_eq!(loaded.save_format, ExportFormat::Tiff);
        assert!(!loaded.reveal_export);
        let grid = loaded.alignment.grid();
        assert_eq!(grid.horizontal_pct, 7.5);
        assert!(grid.apply_brightness);
        // Written back: the same keys, with the same values.
        loaded.save(&path).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut before: serde_json::Value = serde_json::from_str(old).unwrap();
        before["grid_magnification_pct"] = 6.0.into();
        before.as_object_mut().unwrap().remove("reverse_check");
        assert_eq!(written, before);
        assert_eq!(Preferences::load(&path), loaded);
    }

    /// The decoder defaults to Automatic; a file that names one keeps it.
    #[test]
    fn the_raw_decoder_is_automatic_unless_chosen() {
        assert_eq!(Preferences::default().raw_backend, RawDecoder::Automatic);
        let automatic = RawDecoder::Automatic.backend();
        assert_eq!(automatic == RawBackend::AdobeDng, RawBackend::AdobeDng.is_available());
        assert_eq!(RawDecoder::LibRaw.backend(), RawBackend::LibRaw);
        assert_eq!(RawDecoder::AdobeDng.backend(), RawBackend::AdobeDng);
        let kept: Preferences = serde_json::from_str(r#"{"raw_backend": "libraw"}"#).unwrap();
        assert_eq!(kept.raw_backend, RawDecoder::LibRaw);
    }

    #[test]
    fn every_key_is_unique_and_round_trips() {
        fn check<T: Keyed + std::fmt::Debug>() {
            for &value in T::ALL {
                assert_eq!(T::from_key(value.key()), Some(value));
            }
            let mut keys: Vec<&str> = T::ALL.iter().map(|v| v.key()).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), T::ALL.len());
        }
        check::<RawBackend>();
        check::<RawDecoder>();
        check::<DcpPreference>();
        check::<TiffCompression>();
        check::<AlignMode>();
        check::<Algorithm>();
        check::<ExportFormat>();
        check::<Appearance>();
    }

    /// Appearance follows the system unless chosen, also for a file without
    /// it or an unknown choice.
    #[test]
    fn appearance_follows_the_system_unless_chosen() {
        assert_eq!(Preferences::default().appearance, Appearance::System);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        for (json, appearance) in [
            (r#"{"jpeg_quality": 80}"#, Appearance::System),
            (r#"{"appearance": "light"}"#, Appearance::Light),
            (r#"{"appearance": "dark"}"#, Appearance::Dark),
            (r#"{"appearance": "sepia"}"#, Appearance::System),
        ] {
            std::fs::write(&path, json).unwrap();
            assert_eq!(Preferences::load(&path).appearance, appearance, "{json}");
        }
        let dark = Preferences { appearance: Appearance::Dark, ..Default::default() };
        dark.save(&path).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains(r#""appearance": "dark""#));
        assert_eq!(Preferences::load(&path), dark);
    }

    /// A converter change during a run waits for its end; after, it applies
    /// at once.
    #[gpui::test]
    fn a_converter_change_during_a_run_applies_when_it_ends(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            Prefs::install(Preferences::default(), None, cx);
            let before = deptheos_app::converter_jobs();
            let other = if before == 1 { 2 } else { 1 };
            hold_environment(cx);
            assert!(environment_held(cx), "Settings says so");
            update_prefs(cx, |p| p.converter_jobs = other as u32);
            assert_eq!(deptheos_app::converter_jobs(), before, "held while the run lasts");
            release_environment(cx);
            assert!(!environment_held(cx));
            assert_eq!(deptheos_app::converter_jobs(), other, "applied as it ends");
            update_prefs(cx, |p| p.converter_jobs = before as u32);
            assert_eq!(deptheos_app::converter_jobs(), before, "at once with nothing running");
        });
    }
}
