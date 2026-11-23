//! The license files the About window opens. Release bundles carry them
//! (script/bundle-mac.sh, packaging/windows/deptheos.iss, and on Linux
//! script/bundle-linux.sh and packaging/linux/nfpm.yaml into
//! share/doc/deptheos); a repository build finds them at the workspace root.

use std::path::{Path, PathBuf};

/// The GPL and its additional permission. Windows builds ship it as .txt,
/// which Windows can open without asking which app to use.
pub const LICENSE: &[&str] = &["LICENSE.txt", "LICENSE.md"];
/// Every bundled library's license and copyright notice.
pub const NOTICES: &[&str] = &["THIRD_PARTY_NOTICES.txt"];

/// Where to open one of the files above: a local copy if there is one,
/// else the source repository's.
pub enum Location {
    File(PathBuf),
    Url(String),
}

pub fn locate(names: &[&str]) -> Location {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(Path::parent);
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe_dir {
        // Deptheos.app/Contents/MacOS/deptheos-gui → Contents/Resources.
        dirs.push(dir.join("../Resources"));
        // Linux, installed (/usr/bin/deptheos-gui) or from the tarball
        // (bin/deptheos-gui) → share/doc/deptheos.
        dirs.push(dir.join("../share/doc/deptheos"));
        dirs.push(dir.to_path_buf());
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    for dir in &dirs {
        for name in names {
            let path = dir.join(name);
            if path.is_file() {
                return Location::File(path);
            }
        }
    }
    let name = names.last().expect("at least one name");
    Location::Url(crate::updates::repo_page(&format!("/blob/main/{name}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A development build finds the repository's own copies.
    #[test]
    fn finds_the_repository_files() {
        for names in [LICENSE, NOTICES] {
            assert!(matches!(locate(names), Location::File(_)), "{names:?}");
        }
    }
}
