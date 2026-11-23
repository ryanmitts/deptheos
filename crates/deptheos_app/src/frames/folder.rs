//! A folder of photos as shots: one shutter press, files sharing a name
//! (a RAW and its JPEG). The list shows each shot by its [`Source`] file.

use std::io;
use std::path::{Path, PathBuf};

// Pairing goes by file kind, not decoder: under Apple's (iOS) an ORF beside
// its JPEG is still a pair, though `is_raw_source` counts only DNG.
pub(crate) use deptheos_core::io::is_raw_file;
use deptheos_core::io::supported_extensions;
use serde::{Deserialize, Serialize};

use crate::frames::names::{natural_cmp, natural_path_cmp};

/// Which file of a shot to stack when it has both kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Camera RAW or DNG.
    #[default]
    Raw,
    /// JPEG, HEIF, TIFF, PNG and the other standard images.
    Standard,
}

impl Source {
    pub const ALL: [Self; 2] = [Self::Raw, Self::Standard];
}

/// One shot: the files in one folder that share a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shot {
    /// The shared file name without its extension.
    pub stem: String,
    /// Its files, best first: camera RAW, DNG, then JPEG, HEIF, TIFF, PNG.
    pub files: Vec<PathBuf>,
}

impl Shot {
    /// Its best RAW or DNG file.
    pub(crate) fn raw(&self) -> Option<&Path> {
        self.files.iter().map(PathBuf::as_path).find(|p| is_raw_file(p))
    }

    /// Its best standard image.
    pub(crate) fn standard(&self) -> Option<&Path> {
        self.files.iter().map(PathBuf::as_path).find(|p| !is_raw_file(p))
    }

    /// The file to stack: one of `source` when it has one, else its best.
    pub fn file(&self, source: Source) -> &Path {
        let preferred = match source {
            Source::Raw => self.raw(),
            Source::Standard => self.standard(),
        };
        preferred.unwrap_or(&self.files[0])
    }

    /// Whether `path` is one of its files.
    pub fn contains(&self, path: &Path) -> bool {
        self.files.iter().any(|f| f == path)
    }

    /// Whether it has both a RAW and a standard image.
    pub fn is_pair(&self) -> bool {
        self.raw().is_some() && self.standard().is_some()
    }
}

/// Whether `path` names a format Deptheos reads, by its extension.
pub fn is_image(path: &Path) -> bool {
    let extensions = supported_extensions();
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// The images directly in `dir`, in natural order, skipping hidden files
/// (macOS writes `._` twins on exFAT, FAT and SMB).
pub fn list_folder(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| !is_hidden(p) && is_image(p) && p.is_file())
        .collect();
    files.sort_by(|a, b| natural_path_cmp(a, b));
    Ok(files)
}

/// Whether `path`'s name starts with a dot.
pub(crate) fn is_hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
}

/// `paths` as shots (one folder, names equal ignoring case and extension),
/// in natural order.
pub fn shots(paths: impl IntoIterator<Item = PathBuf>) -> Vec<Shot> {
    let mut keyed: Vec<((PathBuf, String), PathBuf)> = paths
        .into_iter()
        .map(|p| {
            let folder = p.parent().map(Path::to_path_buf).unwrap_or_default();
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            ((folder, stem), p)
        })
        .collect();
    keyed.sort_by(|((fa, sa), pa), ((fb, sb), pb)| {
        natural_path_cmp(fa, fb)
            .then_with(|| sa.to_lowercase().cmp(&sb.to_lowercase()))
            .then_with(|| rank(pa).cmp(&rank(pb)))
            .then_with(|| pa.cmp(pb))
    });
    // Files sort together, best first; shots get natural order afterwards.
    let mut out: Vec<(PathBuf, Shot)> = Vec::new();
    for ((folder, stem), path) in keyed {
        match out.last_mut() {
            Some((f, shot)) if *f == folder && shot.stem.to_lowercase() == stem.to_lowercase() => {
                if !shot.files.contains(&path) {
                    shot.files.push(path);
                }
            }
            _ => out.push((folder, Shot { stem, files: vec![path] })),
        }
    }
    out.sort_by(|(fa, a), (fb, b)| {
        natural_path_cmp(fa, fb).then_with(|| natural_cmp(&a.stem, &b.stem))
    });
    out.into_iter().map(|(_, shot)| shot).collect()
}

/// A file's place among a shot's files, best first.
fn rank(path: &Path) -> u8 {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "dng" => 1,
        _ if is_raw_file(path) => 0,
        "jpg" | "jpeg" => 2,
        "heic" | "heif" => 3,
        "tif" | "tiff" => 4,
        "png" => 5,
        _ => 6,
    }
}

/// Folder names that are packages or app stores, never photo folders.
const PACKAGES: &[&str] =
    &["app", "bundle", "photoslibrary", "lrdata", "lrlibrary", "cocatalog", "deptheos", "partial"];

/// The folders directly in `dir`, in natural order, minus hidden ones and
/// packages.
pub(crate) fn subfolders(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
            let package = Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| PACKAGES.iter().any(|p| p.eq_ignore_ascii_case(e)));
            !name.starts_with('.') && !package
        })
        .collect();
    folders.sort_by(|a, b| natural_path_cmp(a, b));
    Ok(folders)
}

/// How deep [`folders_inside`] looks when asked to look deeper.
const MAX_DEPTH: usize = 8;

/// Folders below `parent` that hold images: direct children, or with
/// `deep` all descendants, parents first.
pub fn folders_inside(parent: &Path, deep: bool) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, deep: bool, out: &mut Vec<PathBuf>) {
        for child in subfolders(dir).unwrap_or_default() {
            if list_folder(&child).is_ok_and(|files| !files.is_empty()) {
                out.push(child.clone());
            }
            if deep && depth < MAX_DEPTH {
                walk(&child, depth + 1, deep, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(parent, 1, deep, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| PathBuf::from(format!("card/{n}"))).collect()
    }

    fn names(files: &[PathBuf]) -> Vec<String> {
        files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_raw_and_its_jpeg_are_one_shot() {
        let s = shots(paths(&["IMG_0002.JPG", "IMG_0001.CR3", "IMG_0001.JPG", "IMG_0002.CR3"]));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].stem, "IMG_0001");
        assert_eq!(names(&s[0].files), ["IMG_0001.CR3", "IMG_0001.JPG"]);
        assert!(s[0].is_pair());
    }

    #[test]
    fn the_source_picks_the_file() {
        let s = shots(paths(&["a.ORF", "a.jpg", "b.jpg", "c.ORF"]));
        let pick =
            |source| s.iter().map(|shot| shot.file(source).to_path_buf()).collect::<Vec<_>>();
        assert_eq!(names(&pick(Source::Raw)), ["a.ORF", "b.jpg", "c.ORF"]);
        assert_eq!(names(&pick(Source::Standard)), ["a.jpg", "b.jpg", "c.ORF"]);
        assert!(s[1].raw().is_none());
        assert!(s[2].standard().is_none());
    }

    #[test]
    fn pairing_ignores_case_and_stays_in_one_folder() {
        let s = shots(vec![
            PathBuf::from("card/IMG_0001.CR3"),
            PathBuf::from("card/img_0001.jpg"),
            PathBuf::from("other/IMG_0001.JPG"),
        ]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].files.len(), 2);
        assert_eq!(s[1].files, [PathBuf::from("other/IMG_0001.JPG")]);
    }

    #[test]
    fn camera_raw_comes_before_dng_and_jpeg_before_tiff() {
        let s = shots(paths(&["x.tif", "x.dng", "x.jpg", "x.CR3"]));
        assert_eq!(names(&s[0].files), ["x.CR3", "x.dng", "x.jpg", "x.tif"]);
        assert_eq!(s[0].file(Source::Standard), Path::new("card/x.jpg"));
    }

    #[test]
    fn shots_come_in_natural_order() {
        let s = shots(paths(&["BEE - 100.jpg", "BEE - 99.jpg", "BEE - 9.jpg"]));
        let stems: Vec<&str> = s.iter().map(|s| s.stem.as_str()).collect();
        assert_eq!(stems, ["BEE - 9", "BEE - 99", "BEE - 100"]);
    }

    #[test]
    fn single_files_are_shots_of_one() {
        let s = shots(paths(&["a.png", "b.jpg"]));
        assert!(s.iter().all(|s| s.files.len() == 1 && !s.is_pair()));
    }

    #[test]
    fn listing_skips_subfolders_and_other_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub.jpg")).unwrap();
        for name in ["b 10.jpg", "b 9.jpg", "notes.txt", "a.CR3", "._a.CR3", ".hidden.jpg"] {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        assert_eq!(names(&list_folder(dir.path()).unwrap()), ["a.CR3", "b 9.jpg", "b 10.jpg"]);
    }
}
