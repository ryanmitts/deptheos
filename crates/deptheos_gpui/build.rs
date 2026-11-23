//! The build the log's header names (`DEPTHEOS_BUILD`: short commit,
//! "-dirty" with changes, and the profile; the profile alone without git),
//! and on Windows the icon and version details embedded in deptheos-gui.exe.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=DEPTHEOS_BUILD={}", build());

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = "../../packaging/windows/deptheos.ico";
        println!("cargo:rerun-if-changed={icon}");
        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon)
            .set("ProductName", "Deptheos")
            .set("FileDescription", "Deptheos")
            .set("LegalCopyright", "© Ryan Mitts. GPL-3.0.");
        res.compile().expect("failed to embed the Windows icon");
    }
}

/// "a7e698fe debug", "a7e698fe-dirty release", or "release" without git.
fn build() -> String {
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    match commit() {
        Some(commit) => format!("{commit} {profile}"),
        None => profile,
    }
}

/// The short commit, "-dirty" when tracked files differ. Reruns when HEAD
/// moves, not on every edit, so "-dirty" is as of then (or build.rs's change).
fn commit() -> Option<String> {
    let hash = git(&["rev-parse", "--short", "HEAD"])?;
    watch_head();
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|out| !out.is_empty());
    Some(if dirty { format!("{hash}-dirty") } else { hash })
}

/// Rerun when HEAD or the branch it names changes. Only files that exist:
/// a missing one would make cargo rerun the script on every build.
fn watch_head() {
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) else {
        return;
    };
    // Branches live in the common folder (the same one outside a worktree).
    let common = git(&["rev-parse", "--git-common-dir"])
        .map(|dir| absolute(&git_dir, &dir))
        .unwrap_or_else(|| git_dir.clone());
    let mut watched = vec![git_dir.join("HEAD"), common.join("packed-refs")];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(common.join(branch));
    }
    for path in watched.iter().filter(|p| p.is_file()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// `dir` as git printed it, which may be relative to the working folder.
fn absolute(git_dir: &Path, dir: &str) -> PathBuf {
    let dir = Path::new(dir);
    if dir.is_absolute() {
        return dir.to_path_buf();
    }
    std::env::current_dir().map(|cwd| cwd.join(dir)).unwrap_or_else(|_| git_dir.to_path_buf())
}

/// git's trimmed output, or `None` when git is missing or fails (no `.git`).
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
