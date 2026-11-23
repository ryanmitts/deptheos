//! Records the commit the CLI is built from, for bench's record (git at run
//! time would name whatever repository bench runs in). On Windows, embeds
//! the long-path-aware manifest (`windows.manifest`).

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = "../../packaging/windows/deptheos.ico";
        println!("cargo:rerun-if-changed=windows.manifest");
        println!("cargo:rerun-if-changed={icon}");
        // Absolute: the resource compiler runs on a script in OUT_DIR.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("windows.manifest");
        winresource::WindowsResource::new()
            .set_manifest_file(&manifest.to_string_lossy())
            .set_icon(icon)
            .set("ProductName", "Deptheos")
            .set("FileDescription", "Deptheos")
            .set("LegalCopyright", "© Ryan Mitts. GPL-3.0.")
            .compile()
            .expect("failed to embed the Windows resources");
    }

    // Not a checkout (a source tarball): no commit is recorded.
    let Some(commit) = git(&["rev-parse", "--short", "HEAD"]) else { return };
    println!("cargo:rustc-env=DEPTHEOS_GIT_COMMIT={commit}");
    // Rebuild when HEAD moves: a checkout changes HEAD, a commit the ref.
    for file in ["HEAD", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", file])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &head_ref])
        && Path::new(&path).exists()
    {
        println!("cargo:rerun-if-changed={path}");
    }
}
