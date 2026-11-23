//! The diagnostics zip: logs under `logs/`, crash reports under
//! `crash-reports/`, redacted (`redact.rs`). It goes to support, never a
//! public issue, as a crash report reveals much and a minidump can't be
//! redacted.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use zip::CompressionMethod;

use super::redact;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Writes the zip beside `dest` and moves it there once whole.
pub(super) fn write(
    dest: &Path,
    logs: &[PathBuf],
    crash_reports: &[PathBuf],
) -> std::io::Result<()> {
    let partial = dest.with_extension("zip.partial");
    let result =
        write_to(&partial, logs, crash_reports).and_then(|()| std::fs::rename(&partial, dest));
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn write_to(path: &Path, logs: &[PathBuf], crash_reports: &[PathBuf]) -> std::io::Result<()> {
    let mut zip = ZipWriter::new(File::create(path)?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let entries =
        logs.iter().map(|p| ("logs", p)).chain(crash_reports.iter().map(|p| ("crash-reports", p)));
    for (folder, file) in entries {
        let Some(name) = file.file_name() else { continue };
        // A log pruned since it was listed is skipped, not an error.
        let Ok(bytes) = std::fs::read(file) else { continue };
        let bytes = match String::from_utf8(bytes) {
            Ok(text) => {
                let text = redact::redact(&text);
                match folder {
                    "crash-reports" => redact::blank_machine_ids(&text).into_owned(),
                    _ => text.into_owned(),
                }
                .into_bytes()
            }
            Err(e) => e.into_bytes(),
        };
        zip.start_file(format!("{folder}/{}", name.to_string_lossy()), options)
            .map_err(std::io::Error::other)?;
        zip.write_all(&bytes)?;
    }
    zip.finish().map_err(std::io::Error::other)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zips_logs_and_reports_and_leaves_nothing_partial() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("deptheos-a.log");
        std::fs::write(&log, "a line\n").unwrap();
        let home = dirs::home_dir().unwrap();
        let report = dir.path().join("Deptheos.ips");
        let escaped = home.display().to_string().replace('/', "\\/");
        let report_text = format!(
            "at {}/Applications\n\"crashReporterKey\" : \"8C2A6B9E\", \"path\" : \"{escaped}\\/x\"",
            home.display()
        );
        std::fs::write(&report, report_text).unwrap();
        let missing = dir.path().join("deptheos-gone.log");

        let dest = dir.path().join("Deptheos Diagnostics.zip");
        write(&dest, &[log, missing], &[report]).unwrap();
        assert!(!dir.path().join("Deptheos Diagnostics.zip.partial").exists());

        let mut zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let names: Vec<_> = zip.file_names().map(str::to_owned).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        let mut text = String::new();
        std::io::Read::read_to_string(
            &mut zip.by_name("crash-reports/Deptheos.ips").unwrap(),
            &mut text,
        )
        .unwrap();
        assert_eq!(text, "at ~/Applications\n\"crashReporterKey\" : \"\", \"path\" : \"~\\/x\"");
    }
}
