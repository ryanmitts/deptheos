//! The log file: one per launch, rolled over by size, the folder pruned to
//! the newest few; every line redacted (`redact.rs`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

/// A file rolls over to the next once it holds this much.
pub(crate) const MAX_FILE_BYTES: u64 = 10 << 20;
/// The folder keeps this many files, at most about 100 MB.
pub(crate) const KEPT_FILES: usize = 10;

const PREFIX: &str = "deptheos-";
const EXTENSION: &str = "log";

/// This launch's file.
///
/// `Writing` → `Writing` (the next part, when a line would take the file
/// past [`MAX_FILE_BYTES`]) → `Off` (a write failed, or the next part
/// couldn't be opened; final, said once on stderr). The transitions can't
/// be logged through `tracing`: this is where its lines go.
enum State {
    Writing { file: File, path: PathBuf, written: u64 },
    Off,
}

pub(super) struct Sink {
    dir: PathBuf,
    /// The file name before any part number: the launch's start and pid.
    stem: String,
    part: u32,
    state: State,
}

impl Sink {
    /// Make `dir`, prune it to leave room, and open this launch's file.
    pub(super) fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let stem = format!("{PREFIX}{}-{}", file_stamp(SystemTime::now()), std::process::id());
        let mut sink = Self { dir: dir.to_path_buf(), stem, part: 1, state: State::Off };
        let (file, path) = sink.open_part()?;
        sink.state = State::Writing { file, path, written: 0 };
        Ok(sink)
    }

    /// Have the file's lines on disk.
    pub(super) fn flush(&mut self) {
        if let State::Writing { file, .. } = &mut self.state {
            let _ = std::io::Write::flush(file);
            let _ = file.sync_data();
        }
    }

    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(super) fn current_path(&self) -> Option<&Path> {
        match &self.state {
            State::Writing { path, .. } => Some(path),
            State::Off => None,
        }
    }

    /// Append `line` (already redacted), rolling over first when it would
    /// take the file past the limit.
    pub(super) fn write_line(&mut self, line: &[u8]) {
        let len = line.len() as u64;
        if let State::Writing { written, .. } = &self.state
            && *written > 0
            && *written + len > MAX_FILE_BYTES
        {
            self.roll_over();
        }
        let State::Writing { file, written, .. } = &mut self.state else { return };
        match file.write_all(line) {
            Ok(()) => *written += len,
            Err(e) => self.turn_off(&e),
        }
    }

    fn roll_over(&mut self) {
        self.part += 1;
        match self.open_part() {
            Ok((mut file, path)) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                if let State::Writing { file: old, .. } = &mut self.state {
                    let _ = writeln!(old, "(continued in {})", name.unwrap_or_default());
                }
                let _ = writeln!(file, "(continued from part {})", self.part - 1);
                self.state = State::Writing { file, path, written: 0 };
            }
            Err(e) => self.turn_off(&e),
        }
    }

    fn turn_off(&mut self, error: &std::io::Error) {
        eprintln!("the log file stopped: {error}");
        self.state = State::Off;
    }

    /// Prunes to leave room, then creates the next part's file.
    fn open_part(&self) -> std::io::Result<(File, PathBuf)> {
        prune(&self.dir, KEPT_FILES - 1);
        let name = match self.part {
            1 => format!("{}.{EXTENSION}", self.stem),
            n => format!("{}-{n}.{EXTENSION}", self.stem),
        };
        let path = self.dir.join(name);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok((file, path))
    }
}

/// The log files in `dir`, newest first.
pub(super) fn kept_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|entry| is_log_name(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    Ok(files.into_iter().map(|(_, path)| path).collect())
}

fn is_log_name(name: &str) -> bool {
    name.starts_with(PREFIX) && name.ends_with(&format!(".{EXTENSION}"))
}

/// Removes all but the newest `keep` log files, best effort.
fn prune(dir: &Path, keep: usize) {
    let Ok(files) = kept_files(dir) else { return };
    for path in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

/// The per-event writer for the file layer: an event's line is gathered,
/// redacted and appended whole.
pub(super) struct FileWriter {
    sink: Arc<Mutex<Sink>>,
}

impl FileWriter {
    pub(super) fn new(sink: Arc<Mutex<Sink>>) -> Self {
        Self { sink }
    }
}

impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = EventLine<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        EventLine { buf: Vec::with_capacity(256), sink: &self.sink }
    }
}

pub(super) struct EventLine<'a> {
    buf: Vec<u8>,
    sink: &'a Mutex<Sink>,
}

impl Write for EventLine<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for EventLine<'_> {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&self.buf);
        let line = super::redact::redact(&text);
        crate::lock(self.sink).write_line(line.as_bytes());
    }
}

/// The time on every line: `2026-11-11 09:30:12.345Z`.
pub(super) struct UtcTimer;

impl FormatTime for UtcTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        write!(w, "{}", utc_now())
    }
}

pub(super) fn utc_now() -> String {
    utc(SystemTime::now()).format("%Y-%m-%d %H:%M:%S%.3fZ").to_string()
}

/// A file name's time: `2026-11-11T09-30-12Z` (no colons, for Windows).
fn file_stamp(time: SystemTime) -> String {
    utc(time).format("%Y-%m-%dT%H-%M-%SZ").to_string()
}

/// `time` in UTC, a clock set before 1970 read as 1970.
fn utc(time: SystemTime) -> chrono::DateTime<chrono::Utc> {
    time.max(UNIX_EPOCH).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dates_from_days() {
        let day = |days: u64| file_stamp(UNIX_EPOCH + Duration::from_secs(days * 86_400));
        assert_eq!(day(0), "1970-01-01T00-00-00Z");
        assert_eq!(day(11_016), "2000-02-29T00-00-00Z");
        assert_eq!(day(20_403), "2025-11-11T00-00-00Z");
        assert_eq!(file_stamp(UNIX_EPOCH - Duration::from_secs(1)), "1970-01-01T00-00-00Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_762_860_612);
        assert_eq!(file_stamp(t), "2025-11-11T11-30-12Z");
        let t = t + Duration::from_micros(45_999);
        assert_eq!(utc(t).format("%Y-%m-%d %H:%M:%S%.3fZ").to_string(), "2025-11-11 11:30:12.045Z");
    }

    #[test]
    fn rolls_over_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        // Older launches' files, and one that isn't a log.
        for i in 0..KEPT_FILES + 3 {
            std::fs::write(dir.path().join(format!("{PREFIX}old-{i:02}.log")), "x").unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), "keep").unwrap();

        let mut sink = Sink::open(dir.path()).unwrap();
        assert_eq!(kept_files(dir.path()).unwrap().len(), KEPT_FILES);
        let first = sink.current_path().unwrap().to_path_buf();

        let line = vec![b'a'; (MAX_FILE_BYTES / 2) as usize + 1];
        sink.write_line(&line);
        sink.write_line(&line);
        let second = sink.current_path().unwrap().to_path_buf();
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("-2.log"));
        let old = std::fs::read_to_string(&first).unwrap();
        assert!(old.ends_with(&format!(
            "(continued in {})\n",
            second.file_name().unwrap().to_string_lossy()
        )));
        assert!(old.starts_with(&"a".repeat(line.len())));

        let kept = kept_files(dir.path()).unwrap();
        assert_eq!(kept.len(), KEPT_FILES);
        assert!(kept.contains(&first) && kept.contains(&second));
        assert!(dir.path().join("notes.txt").exists());
    }

    #[test]
    fn a_single_long_line_still_goes_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = Sink::open(dir.path()).unwrap();
        let first = sink.current_path().unwrap().to_path_buf();
        sink.write_line(&vec![b'a'; MAX_FILE_BYTES as usize + 10]);
        assert_eq!(sink.current_path().unwrap(), first);
    }
}
