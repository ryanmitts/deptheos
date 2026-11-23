//! The log as the iOS app drives it. Its own test binary: the log installs
//! once per process, and the library's tests start engines whose fallback
//! would install it first, with no file.

use deptheos_ffi::{
    LogLevel, Request, Words, install_logging, log_line, set_verbose_logging, verbose_logging,
    write_diagnostics,
};
use futures::executor::block_on;

#[test]
fn installs_logs_the_apps_lines_and_zips_them() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("Logs");
    block_on(install_logging(logs.clone(), "1.2.3".into(), "45".into()));
    // A second install (the engine's fallback) changes nothing.
    block_on(install_logging(dir.path().join("Other"), "9".into(), "9".into()));
    assert!(!dir.path().join("Other").exists());

    log_line(LogLevel::Info, "camera".into(), "camera resumed".into());
    log_line(LogLevel::Debug, "state".into(), "quiet: not verbose".into());
    assert!(!verbose_logging());
    set_verbose_logging(true);
    assert!(verbose_logging());
    log_line(LogLevel::Debug, "state".into(), "stack: running".into());
    set_verbose_logging(false);
    assert!(!verbose_logging());

    // The launch's log.
    let files: Vec<_> = std::fs::read_dir(&logs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("Deptheos (iOS) 1.2.3 (45)"), "{text}");
    assert!(text.contains("deptheos::ios: camera: camera resumed"), "{text}");
    assert!(text.contains("state: stack: running"), "{text}");
    assert!(!text.contains("quiet: not verbose"), "{text}");

    let zip_path = dir.path().join("Deptheos Diagnostics 2026-11-11.zip");
    let words = Words::new("en".into());
    block_on(write_diagnostics(Request::new(), zip_path.clone(), words.clone())).unwrap();
    let zip = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let name = files[0].file_name().unwrap().to_string_lossy();
    assert!(zip.file_names().any(|n| n == format!("logs/{name}")));

    let bad = dir.path().join("missing").join("x.zip");
    assert!(block_on(write_diagnostics(Request::new(), bad, words)).is_err());
}
