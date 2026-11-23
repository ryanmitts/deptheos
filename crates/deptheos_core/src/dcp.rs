//! Adobe DCP (DNG Camera Profile) discovery.
//!
//! An installed profile for the camera overrides LibRaw's color science
//! ([`deptheos_dng::DngImage::apply_dcp`]). Profiles are named
//! `<Make Model> <Profile>.dcp`, matched by that name, never by substring
//! ("Canon EOS R5" never takes "Canon EOS R50" or "… R5 Mark II"), in
//! sorted order so a folder always gives the same profile.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Folders the user added, searched before Adobe's
/// ([`set_extra_directories`]).
static EXTRA_DIRECTORIES: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

/// Search `dirs` for profiles too (flat: `<model>.dcp` or
/// `<model> <anything>.dcp`).
pub fn set_extra_directories(dirs: Vec<PathBuf>) {
    *EXTRA_DIRECTORIES.write().unwrap_or_else(|e| e.into_inner()) = dirs;
}

/// Which DCP profile to prefer for camera RAW files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DcpPreference {
    /// "Adobe Standard" — matches what Adobe DNG Converter embeds.
    #[default]
    AdobeStandard,
    /// "Camera Standard" — the camera maker's look (in-camera JPEG).
    CameraStandard,
    /// Skip DCP entirely — LibRaw's basic color matrices only.
    None,
}

impl DcpPreference {
    pub const ALL: [Self; 3] = [Self::AdobeStandard, Self::CameraStandard, Self::None];

    /// What the preferences and render files call it.
    pub fn key(self) -> &'static str {
        match self {
            Self::AdobeStandard => "adobe_standard",
            Self::CameraStandard => "camera_standard",
            Self::None => "none",
        }
    }

    /// Profile names in the order this preference takes them (lower-case).
    fn profile_names(self) -> &'static [&'static str] {
        match self {
            Self::AdobeStandard => &["adobe standard", "camera standard"],
            Self::CameraStandard => &["camera standard", "adobe standard"],
            Self::None => &[],
        }
    }
}

/// Directories where Adobe keeps camera profiles, existing or not: the
/// user's own first, then the machine's.
pub fn adobe_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(user) = user_directory() {
        dirs.push(user);
    }
    let system: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Library/Application Support/Adobe/CameraRaw/CameraProfiles",
            "/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app/Contents/Resources/CameraProfiles",
            "/Applications/Adobe Lightroom CC/Adobe Lightroom.app/Contents/Resources/CameraProfiles",
        ]
    } else if cfg!(target_os = "windows") {
        &[r"C:\ProgramData\Adobe\CameraRaw\CameraProfiles"]
    } else {
        &[]
    };
    dirs.extend(system.iter().map(PathBuf::from));
    dirs
}

/// The user's own Adobe camera profile folder.
fn user_directory() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        Some(Path::new(&home).join("Library/Application Support/Adobe/CameraRaw/CameraProfiles"))
    } else if cfg!(target_os = "windows") {
        let app_data = std::env::var_os("APPDATA")?;
        Some(Path::new(&app_data).join(r"Adobe\CameraRaw\CameraProfiles"))
    } else {
        None
    }
}

/// Find a DCP profile for `model`: in [`set_extra_directories`]' folders
/// first, then Adobe's, `preference` picking within each. A model that
/// could name another folder ([`is_plain_model`]) finds none.
pub fn find_profile(model: &str, preference: DcpPreference) -> Option<PathBuf> {
    if preference == DcpPreference::None {
        return None;
    }
    if !is_plain_model(model) {
        tracing::debug!("not looking for a DCP profile under a model name that is a path");
        return None;
    }

    for dir in EXTRA_DIRECTORIES.read().unwrap_or_else(|e| e.into_inner()).iter() {
        if let Some(path) = find_in_folder(dir, model, preference) {
            return Some(path);
        }
    }

    // The user's own folder is often flat, beside Adobe's layout.
    if let Some(path) = user_directory().and_then(|dir| find_in_folder(&dir, model, preference)) {
        return Some(path);
    }
    let by_name = |name: &str| match name {
        "adobe standard" => find_adobe_standard(model),
        // The maker's standard look, under whatever name the maker gives
        // it (Fujifilm's is PROVIA).
        _ => find_camera_profile(model, |stem| stem.contains("camera ")),
    };
    preference
        .profile_names()
        .iter()
        .find_map(|name| by_name(name))
        // Last resort: the camera's profile most like a standard look.
        .or_else(|| find_camera_profile(model, |_| true))
}

/// Whether `model`, from the file's metadata, is safe to join into a path
/// (no separator or `..`).
fn is_plain_model(model: &str) -> bool {
    !model.is_empty() && !model.contains(['/', '\\', '\0']) && !model.contains("..")
}

/// Find and apply the preferred profile for a LibRaw-decoded source. With
/// none, or one that fails, LibRaw's color science stays (logged).
pub fn apply_profile(
    image: &mut deptheos_dng::DngImage,
    meta: &deptheos_raw::RawMetadata,
    preference: DcpPreference,
) {
    let model = meta.unique_camera_model();
    let Some(path) = find_profile(&model, preference) else {
        tracing::debug!("no DCP profile installed for '{model}'");
        return;
    };
    match image.apply_dcp(&path) {
        Ok(()) => tracing::info!("applied DCP profile {}", path.display()),
        Err(e) => tracing::warn!("DCP profile {} failed to apply: {e}", path.display()),
    }
}

/// The best profile for `model` in a flat folder: `<model> <name>.dcp` for
/// the preference's names in order, else [`most_standard`] of the model's.
/// `<model> Mark …` is another camera.
fn find_in_folder(dir: &Path, model: &str, preference: DcpPreference) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    let model = model.to_lowercase();
    let prefix = format!("{model} ");
    let candidates: Vec<_> = dcp_files(dir)
        .into_iter()
        .filter(|(stem, _)| match stem.strip_prefix(&prefix) {
            // "Mark II" and on name a later camera, not a profile.
            Some(rest) => !(rest == "mark" || rest.starts_with("mark ")),
            None => *stem == model,
        })
        .collect();
    preference
        .profile_names()
        .iter()
        .find_map(|name| {
            let exact = format!("{prefix}{name}");
            candidates.iter().find(|(stem, _)| *stem == exact)
        })
        .or_else(|| most_standard(&candidates, &model))
        .map(|(_, path)| path.clone())
}

/// Words that mark a monochrome profile, a fallback only when no color one
/// is there.
const MONOCHROME_WORDS: &[&str] = &["mono", "acros", "sepia", "b&w"];

/// Words that mark the maker's standard look under its own name for it
/// (Fujifilm's PROVIA, Olympus's Natural).
const STANDARD_WORDS: &[&str] = &["standard", "std", "provia", "natural"];

/// Among a model's profiles (lower-case stems, sorted), the one most like a
/// standard look: a standard name, else the first color profile, else
/// (a monochrome camera's) the first.
fn most_standard<'a>(
    candidates: &'a [(String, PathBuf)],
    model: &str,
) -> Option<&'a (String, PathBuf)> {
    let model = model.to_lowercase();
    let rank = |stem: &str| {
        // The name after the model's own, which may say "Monochrom" itself.
        let name = stem.strip_prefix(&model).unwrap_or(stem);
        if MONOCHROME_WORDS.iter().any(|w| name.contains(w)) {
            2
        } else if STANDARD_WORDS.iter().any(|w| name.contains(w)) {
            0
        } else {
            1
        }
    };
    candidates.iter().min_by_key(|(stem, _)| rank(stem))
}

fn find_adobe_standard(model: &str) -> Option<PathBuf> {
    let filename = format!("{model} Adobe Standard.dcp");
    for base in adobe_directories() {
        let dir = base.join("Adobe Standard");
        // Exact path first (fast), then a case-insensitive scan.
        let exact = dir.join(&filename);
        if exact.is_file() {
            return Some(exact);
        }
        if let Some(path) = find_file_named(&dir, &filename) {
            return Some(path);
        }
    }
    None
}

/// From the camera's folder (`Camera/<model>/`), the [`most_standard`] of
/// the profiles whose lower-case stem `accept`s.
fn find_camera_profile(model: &str, accept: impl Fn(&str) -> bool) -> Option<PathBuf> {
    for base in adobe_directories() {
        let camera_dir = base.join("Camera");
        let exact = camera_dir.join(model);
        let dirs = std::iter::once(exact.clone())
            .chain(find_subdirectory(&camera_dir, model).filter(|d| *d != exact));
        for dir in dirs {
            let files: Vec<_> = dcp_files(&dir).into_iter().filter(|(s, _)| accept(s)).collect();
            if let Some((_, path)) = most_standard(&files, model) {
                return Some(path.clone());
            }
        }
    }
    None
}

fn is_dcp(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
}

/// The `.dcp` files in `directory` with their lower-cased stems, sorted by
/// file name (empty when the folder cannot be read).
fn dcp_files(directory: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_dcp(p) && p.is_file())
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().to_lowercase(), p)))
        .collect();
    files.sort_by(|a, b| a.1.file_name().cmp(&b.1.file_name()));
    files
}

/// Case-insensitive exact-filename lookup (the first by name, should the
/// folder hold several spellings).
fn find_file_named(directory: &Path, filename: &str) -> Option<PathBuf> {
    let filename = filename.to_lowercase();
    let mut matches: Vec<_> = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == filename))
        .collect();
    matches.sort();
    matches.into_iter().next()
}

/// Case-insensitive subdirectory lookup (the first by name).
fn find_subdirectory(parent: &Path, name: &str) -> Option<PathBuf> {
    let name = name.to_lowercase();
    let mut matches: Vec<_> = std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == name)
        })
        .collect();
    matches.sort();
    matches.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that names another folder finds no profile, even where the
    /// folder it names holds one.
    #[test]
    fn a_model_with_a_path_in_it_finds_nothing() {
        for model in ["../Canon EOS R5", "Canon/EOS R5", "Canon\\EOS R5", "Canon EOS ..", ""] {
            assert!(!is_plain_model(model), "{model:?}");
            assert!(find_profile(model, DcpPreference::AdobeStandard).is_none(), "{model:?}");
            assert!(find_profile(model, DcpPreference::CameraStandard).is_none(), "{model:?}");
        }
        assert!(is_plain_model("Canon EOS R5 Mark II"));
        assert!(is_plain_model("LEICA M MONOCHROM Typ 246"));
    }

    fn folder(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in files {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        dir
    }

    fn found_in(dir: &tempfile::TempDir, model: &str, preference: DcpPreference) -> Option<String> {
        find_in_folder(dir.path(), model, preference)
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
    }

    /// The only test in this binary that sets the extra folders.
    #[test]
    fn extra_directory_flat_match() {
        let dir = folder(&["FUJIFILM X-T5 Custom.dcp", "unrelated.txt"]);
        set_extra_directories(vec![dir.path().to_path_buf()]);

        let found = find_profile("FUJIFILM X-T5", DcpPreference::AdobeStandard);
        assert!(found.is_some_and(|p| p.file_name().unwrap() == "FUJIFILM X-T5 Custom.dcp"));
        assert!(find_profile("FUJIFILM X-T5", DcpPreference::None).is_none());

        // A model no vendor ships, so Adobe's real folders can't match.
        assert!(find_profile("Nonexistent TestCam 9000", DcpPreference::AdobeStandard).is_none());
        set_extra_directories(Vec::new());
    }

    /// A model never takes the profile of another whose name extends its
    /// own, nor one that merely mentions it.
    #[test]
    fn a_longer_model_name_does_not_match() {
        let dir = folder(&[
            "Canon EOS R5 Mark II Adobe Standard.dcp",
            "My Canon EOS R5 Look.dcp",
            "Canon EOS R50 Camera Standard.dcp",
        ]);
        assert_eq!(found_in(&dir, "Canon EOS R5", DcpPreference::AdobeStandard), None);
        assert_eq!(
            found_in(&dir, "Canon EOS R5 Mark II", DcpPreference::AdobeStandard).as_deref(),
            Some("Canon EOS R5 Mark II Adobe Standard.dcp")
        );
    }

    /// The fallback is the maker's standard look, else any color profile,
    /// never monochrome while a color one is there; a monochrome camera
    /// still gets its own.
    #[test]
    fn the_fallback_skips_monochrome_looks() {
        let fuji = folder(&[
            "FUJIFILM X-T5 Camera ACROS.dcp",
            "FUJIFILM X-T5 Camera ASTIA_Soft.dcp",
            "FUJIFILM X-T5 Camera MONOCHROME.dcp",
            "FUJIFILM X-T5 Camera PROVIA_Standard.dcp",
        ]);
        assert_eq!(
            found_in(&fuji, "FUJIFILM X-T5", DcpPreference::CameraStandard).as_deref(),
            Some("FUJIFILM X-T5 Camera PROVIA_Standard.dcp")
        );
        let no_standard =
            folder(&["Olympus E-M1 Camera Monotone.dcp", "Olympus E-M1 Camera Vivid.dcp"]);
        assert_eq!(
            found_in(&no_standard, "Olympus E-M1", DcpPreference::AdobeStandard).as_deref(),
            Some("Olympus E-M1 Camera Vivid.dcp")
        );
        let leica = folder(&["LEICA M MONOCHROM Typ 246 Camera Default.dcp"]);
        assert_eq!(
            found_in(&leica, "LEICA M MONOCHROM Typ 246", DcpPreference::AdobeStandard).as_deref(),
            Some("LEICA M MONOCHROM Typ 246 Camera Default.dcp")
        );
    }

    /// Within a folder the preference decides, case-insensitively, then the
    /// other standard, then the first by name.
    #[test]
    fn the_preference_picks_within_a_folder() {
        let dir = folder(&[
            "Canon EOS R5 Z Custom.dcp",
            "canon eos r5 camera standard.dcp",
            "Canon EOS R5 Adobe Standard.dcp",
            "Canon EOS R5 A Custom.dcp",
        ]);
        let pick = |p| found_in(&dir, "Canon EOS R5", p);
        assert_eq!(
            pick(DcpPreference::AdobeStandard).as_deref(),
            Some("Canon EOS R5 Adobe Standard.dcp")
        );
        assert_eq!(
            pick(DcpPreference::CameraStandard).as_deref(),
            Some("canon eos r5 camera standard.dcp")
        );

        let only_custom = folder(&["Canon EOS R5 Z Custom.dcp", "Canon EOS R5 A Custom.dcp"]);
        assert_eq!(
            found_in(&only_custom, "Canon EOS R5", DcpPreference::CameraStandard).as_deref(),
            Some("Canon EOS R5 A Custom.dcp")
        );
        let only_adobe = folder(&["Canon EOS R5 Adobe Standard.dcp"]);
        assert_eq!(
            found_in(&only_adobe, "Canon EOS R5", DcpPreference::CameraStandard).as_deref(),
            Some("Canon EOS R5 Adobe Standard.dcp")
        );
    }
}
