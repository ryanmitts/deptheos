// Source-file collection shared by the build scripts of deptheos_raw and
// deptheos_dng.

use std::path::{Path, PathBuf};

/// Every file under `dir` with extension `ext`, descending into
/// subdirectories when `recursive`, sorted so the build order does not
/// depend on the filesystem's `read_dir` order. An `exclude` entry matches a
/// file by name (`"example.c"`) or by its path relative to `dir`
/// (`"write/write_ph.cpp"`).
pub fn collect_sources(dir: &Path, ext: &str, recursive: bool, exclude: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, dir, ext, recursive, exclude, &mut out);
    out.sort();
    out
}

fn walk(
    root: &Path,
    dir: &Path,
    ext: &str,
    recursive: bool,
    exclude: &[&str],
    out: &mut Vec<PathBuf>,
) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                walk(root, &path, ext, recursive, exclude, out);
            }
        } else if path.extension().is_some_and(|e| e == ext) {
            let name = path.file_name().unwrap().to_string_lossy();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let relative = relative.to_string_lossy().replace('\\', "/");
            if !exclude.iter().any(|&x| x == name || x == relative) {
                out.push(path);
            }
        }
    }
}
