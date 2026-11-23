//! Damaged frames: a JPEG, TIFF, PNG, DNG or CR3 cut short (as a card
//! pulled mid-copy leaves one) is an error from every read the app makes
//! of a frame (its header, its pixels, its linear decode), never a panic,
//! a crash or a read that doesn't end. The whole files are the known
//! images (`known.rs`); the CR3 is the first 256 KB of one.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use deptheos_core::io::{self, CameraRaw, FileCache, Target};

use crate::known::fixture;

/// The longest a read of a damaged file may take.
const DEADLINE: Duration = Duration::from_secs(30);

/// `name`'s first `keep` of its bytes (a share of it), in `dir`.
fn cut(dir: &Path, name: &str, keep: f64) -> PathBuf {
    let bytes = std::fs::read(fixture(name)).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, &bytes[..(bytes.len() as f64 * keep) as usize]).unwrap();
    path
}

/// What each of the app's reads of `path` came to, on its own thread so a
/// read that never ends fails rather than hangs; a panic is caught.
fn reads(path: &Path) -> Vec<(&'static str, Result<(), String>)> {
    let (tx, rx) = mpsc::channel();
    let owned = path.to_path_buf();
    std::thread::spawn(move || {
        let path = owned.as_path();
        let caught = |f: &dyn Fn() -> Result<(), String>| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .unwrap_or_else(|_| Err("PANICKED".into()))
        };
        let mut out = vec![
            (
                "header",
                caught(&|| match io::header_info(path, CameraRaw::LibRaw) {
                    Some(_) => Ok(()),
                    None => Err("no header".into()),
                }),
            ),
            (
                "load",
                caught(&|| {
                    io::load_frame(path, FileCache::Use, CameraRaw::LibRaw, Target::Own)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }),
            ),
        ];
        if io::is_raw_file(path) {
            out.push((
                "linear",
                caught(&|| io::decode_linear(path).map(|_| ()).map_err(|e| e.to_string())),
            ));
        }
        let _ = tx.send(out);
    });
    rx.recv_timeout(DEADLINE)
        .unwrap_or_else(|_| panic!("{}: a read didn't end in {DEADLINE:?}", path.display()))
}

/// Each damaged frame fails to load with an error that says so; a header,
/// read from the start of the file, may still be there.
#[test]
fn a_frame_cut_short_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let damaged = vec![
        cut(dir.path(), "chart-p3.jpg", 0.5),
        cut(dir.path(), "chart-16.tif", 0.5),
        cut(dir.path(), "chart-16.png", 0.5),
        cut(dir.path(), "mosaic.dng", 0.5),
        cut(dir.path(), "converter.dng", 0.5),
        // The CR3 is a cut one already.
        fixture("truncated.cr3"),
    ];
    // The CR3's head is whole, so its decode fails in its pixels.
    let cr3 = fixture("truncated.cr3");
    assert!(io::header_info(&cr3, CameraRaw::LibRaw).is_some(), "the CR3 is identified");
    for path in damaged {
        for (read, result) in reads(&path) {
            eprintln!("{} {read}: {result:?}", path.display());
            if let Err(e) = &result {
                assert!(e != "PANICKED", "{}: its {read} panicked", path.display());
            }
            if read != "header" {
                assert!(result.is_err(), "{}: its {read} took a damaged file", path.display());
            }
        }
    }
}
