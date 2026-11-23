//! Checks on where a run writes, made before any work: no output replaces
//! an input frame or another output, and each is in a format Deptheos
//! writes, in a folder that exists.

use deptheos_app::is_dng;
use deptheos_app::library::export::ExportFormat;
use std::path::{Path, PathBuf};

use crate::Run;
use crate::error::Refusal;

type Result<T> = std::result::Result<T, Refusal>;

/// Refuses a sweep `--output` that is a file or named like one
/// (`-o out.jpg`).
pub(crate) fn check_sweep_dir(dir: &Path) -> Result<()> {
    let named_like_a_file = is_dng(dir) || image::ImageFormat::from_path(dir).is_ok();
    if dir.is_file() || (!dir.exists() && named_like_a_file) {
        return Err(Refusal::SweepOutputIsFile(dir.to_path_buf()));
    }
    Ok(())
}

/// Refuses, before any work, an output that is an input frame (no
/// override), that two outputs share, without a format we write, or in a
/// missing folder. Other existing files are replaced.
pub(crate) fn check_outputs(inputs: &[PathBuf], runs: &[Run]) -> Result<()> {
    for run in runs {
        // DNG output copies its camera metadata from the first frame.
        if is_dng(&run.output)
            && let Some(first) = inputs
                .first()
                .filter(|f| !deptheos_app::is_raw_source(f, deptheos_app::CameraRaw::LibRaw))
        {
            return Err(Refusal::DngNeedsRaw { output: run.output.clone(), first: first.clone() });
        }
    }
    let mut seen: Vec<(PathBuf, &Path)> = Vec::new();
    for run in runs {
        let output = run.output.as_path();
        check_writable(output)?;
        if let Some(input) = inputs.iter().find(|input| same_file(output, input)) {
            return Err(Refusal::OutputIsInput {
                output: output.to_path_buf(),
                input: input.clone(),
            });
        }
        let key = normalized(output);
        if let Some((_, first)) = seen.iter().find(|(k, _)| *k == key) {
            return Err(Refusal::SameOutput {
                first: first.to_path_buf(),
                second: output.to_path_buf(),
            });
        }
        seen.push((key, output));
    }
    Ok(())
}

/// Refuses a path without an export format's extension or whose folder is
/// missing. Other image formats (WebP, BMP…) can't take 16 bits and would
/// fail only after the stack ran.
fn check_writable(path: &Path) -> Result<()> {
    if ExportFormat::of(path).is_none() {
        return Err(Refusal::FileType {
            path: path.to_path_buf(),
            formats: ".dng, .tif, .png or .jpg",
        });
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.is_dir()
    {
        return Err(Refusal::NoFolder { path: path.to_path_buf(), folder: parent.to_path_buf() });
    }
    Ok(())
}

/// Whether `a` and `b` are one existing file, through links, `..` and a
/// case-insensitive file system alike.
#[cfg(unix)]
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

#[cfg(not(unix))]
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    a.exists() && b.exists() && normalized(a) == normalized(b)
}

/// `path` with its folder made absolute and resolved, for comparing outputs
/// that may not exist yet.
fn normalized(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    match (std::fs::canonicalize(parent), path.file_name()) {
        (Ok(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deptheos_app::{DepthConfig, StackerConfig};

    fn job(output: &Path) -> Run {
        Run {
            config: StackerConfig::Depth(DepthConfig {
                radius: 8.0,
                smoothing: 4.0,
                blend_range: 0.0,
            }),
            output: output.to_path_buf(),
        }
    }

    #[test]
    fn outputs_never_replace_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let frames = dir.path().join("frames");
        std::fs::create_dir(&frames).unwrap();
        let inputs: Vec<PathBuf> = (1..=2)
            .map(|n| {
                let p = frames.join(format!("IMG_000{n}.jpg"));
                std::fs::write(&p, b"frame").unwrap();
                p
            })
            .collect();
        let fresh = dir.path().join("stack.jpg");
        assert!(check_outputs(&inputs, &[job(&fresh)]).is_ok());
        // An existing file that is not a frame is replaced.
        std::fs::write(&fresh, b"old").unwrap();
        assert!(check_outputs(&inputs, &[job(&fresh)]).is_ok());

        // The same frame by another spelling.
        let dotted = frames.join("..").join("frames").join("IMG_0002.jpg");
        assert!(check_outputs(&inputs, &[job(&dotted)]).is_err());

        // A rerun whose glob picked up the last result.
        let mut rerun = inputs.clone();
        rerun.push(fresh.clone());
        assert!(check_outputs(&rerun, &[job(&fresh)]).is_err());
    }

    #[test]
    fn outputs_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("r8.jpg");
        let same = dir.path().join(".").join("r8.jpg");
        assert!(check_outputs(&[], &[job(&out), job(&same)]).is_err());
        let other = dir.path().join("r4.jpg");
        assert!(check_outputs(&[], &[job(&out), job(&other)]).is_ok());
    }

    #[test]
    fn outputs_are_writable() {
        let dir = tempfile::tempdir().unwrap();
        let ok = |p: &Path| check_outputs(&[], &[job(p)]).is_ok();
        assert!(ok(&dir.path().join("out.TIF")));
        assert!(ok(&dir.path().join("out.dng")));
        assert!(!ok(&dir.path().join("out")));
        assert!(!ok(&dir.path().join("out.xyz")));
        assert!(ok(&dir.path().join("out.png")) && ok(&dir.path().join("out.jpeg")));
        assert!(ok(&dir.path().join("out.tiff")) && ok(&dir.path().join("out.JPG")));
        // Formats that can't take a 16-bit result are refused up front.
        for other in ["out.webp", "out.bmp", "out.gif", "out.tga"] {
            assert!(!ok(&dir.path().join(other)), "{other}");
        }
        assert!(!ok(&dir.path().join("missing").join("out.jpg")));
    }

    #[test]
    fn dng_output_needs_raw_frames() {
        let out = Path::new("out.dng");
        let jpg = [PathBuf::from("a.jpg")];
        assert!(check_outputs(&jpg, &[job(out)]).is_err());
        assert!(check_outputs(&[PathBuf::from("a.CR3")], &[job(out)]).is_ok());
    }
}
