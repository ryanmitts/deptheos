//! Writes the website's `manifest.json`: `params`, the stackers' and
//! aligners' parameters computed by the app's own functions (see
//! `params.rs`), which fill the numbers in the articles' text at build time.
//! The file is committed, so the website builds without Rust.
//!
//! Usage: `cargo run -p deptheos_webexport -- [--check] [out_dir]` (default
//! `web/generated`, relative to the workspace root). `--check` writes
//! nothing and fails when the file there differs from what the code makes
//! now, as the Tests and Release workflows run it.

mod params;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let out = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("web/generated"));
    let file = out.join("manifest.json");
    let manifest = manifest();
    if check {
        // A Windows checkout may have the file's line endings as CRLF.
        if std::fs::read_to_string(&file).is_ok_and(|text| text.replace("\r\n", "\n") == manifest) {
            println!("{} is current", file.display());
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "{} differs from the engine's parameters: run `cd web && npm run export-params` and commit it",
            file.display()
        );
        return ExitCode::FAILURE;
    }
    std::fs::create_dir_all(&out).expect("create output dir");
    std::fs::write(&file, manifest).expect("write manifest");
    println!("wrote {}", file.display());
    ExitCode::SUCCESS
}

/// The manifest's text, as written.
fn manifest() -> String {
    let manifest = json!({ "params": params::params() });
    serde_json::to_string_pretty(&manifest).expect("serialize manifest") + "\n"
}
