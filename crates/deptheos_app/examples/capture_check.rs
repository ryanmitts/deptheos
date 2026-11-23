//! Checks the capture-time reader against ExifTool on a folder of photos:
//! capture time, sub-second, model and OM shot number, plus bytes read.
//!
//! Run with (ExifTool on PATH, or EXIFTOOL set to it):
//! cargo run --release -p deptheos_app --features examples --example capture_check -- <folder>

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use deptheos_app::frames::capture::{CaptureInfo, read_from};

/// Counts the bytes read through it.
struct Counting<R> {
    inner: R,
    read: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// What ExifTool reports for one file, in the reader's terms.
#[derive(Debug, PartialEq)]
struct Expected {
    seconds: Option<i64>,
    micros: Option<u32>,
    model: Option<String>,
    shot: Option<u16>,
}

fn main() {
    let folder = PathBuf::from(std::env::args().nth(1).expect("a folder to check"));
    let exiftool = std::env::var("EXIFTOOL").unwrap_or_else(|_| "exiftool".into());
    let extensions = deptheos_app::supported_extensions();
    // -T: tab-separated, -f: "-" for a missing tag, `#`: the number.
    let output = Command::new(&exiftool)
        .args(["-T", "-f", "-fast", "-r", "-FilePath", "-DateTimeOriginal"])
        .args(["-SubSecTimeOriginal", "-Model", "-DriveMode"])
        .args(extensions.iter().flat_map(|e| ["-ext", e]))
        .arg(&folder)
        .output()
        .expect("ExifTool runs (set EXIFTOOL to its path)");
    let text = String::from_utf8_lossy(&output.stdout);
    let expected: HashMap<PathBuf, Expected> = text.lines().filter_map(parse_line).collect();
    assert!(!expected.is_empty(), "ExifTool found no images under {}", folder.display());

    let (mut files, mut mismatches, mut total_bytes, mut max_bytes) = (0, 0, 0u64, 0u64);
    // How many files had each field, so a pass is not a run of empty reads.
    let (mut timed, mut subsec, mut shots) = (0, 0, 0);
    let start = Instant::now();
    let mut paths: Vec<&PathBuf> = expected.keys().collect();
    paths.sort();
    for path in paths {
        let mut counting = Counting { inner: File::open(path).expect("opens"), read: 0 };
        let info = read_from(&mut counting).expect("reads");
        files += 1;
        total_bytes += counting.read;
        max_bytes = max_bytes.max(counting.read);
        let got = from_info(&info);
        timed += usize::from(got.seconds.is_some());
        subsec += usize::from(got.micros.is_some());
        shots += usize::from(got.shot.is_some());
        if got != expected[path] {
            mismatches += 1;
            println!("{}\n  reader:   {got:?}\n  exiftool: {:?}", path.display(), expected[path]);
        }
    }
    println!("{timed} with a capture time, {subsec} with sub-seconds, {shots} with a shot number");
    println!(
        "{files} files, {mismatches} differ; read {:.1} KB per file on average, {:.1} KB at most, in {:.2?}",
        total_bytes as f64 / files as f64 / 1024.0,
        max_bytes as f64 / 1024.0,
        start.elapsed()
    );
    if mismatches > 0 {
        std::process::exit(1);
    }
}

fn from_info(info: &CaptureInfo) -> Expected {
    Expected {
        seconds: info.time.map(|t| t.seconds),
        micros: info.time.and_then(|t| t.micros),
        model: info.model.clone(),
        shot: info.shot,
    }
}

fn parse_line(line: &str) -> Option<(PathBuf, Expected)> {
    let cols: Vec<&str> = line.split('\t').collect();
    let [path, time, subsec, model, drive] = cols[..] else { return None };
    let some = |s: &str| (s != "-" && !s.trim().is_empty()).then(|| s.trim().to_string());
    let seconds = some(time).and_then(|t| seconds_of(&t));
    Some((
        PathBuf::from(path),
        Expected {
            seconds,
            micros: some(subsec).filter(|_| seconds.is_some()).map(|s| {
                let digits: String = s.chars().take_while(char::is_ascii_digit).take(6).collect();
                format!("{digits:0<6}").parse().unwrap_or(0)
            }),
            model: some(model),
            // "Sequential, Shot 19", "Focus Bracketing, Shot 10".
            shot: some(drive)
                .and_then(|d| d.split("Shot ").nth(1).and_then(|n| n.trim().parse().ok())),
        },
    ))
}

/// Seconds since 1970 of `YYYY:MM:DD HH:MM:SS`, as the reader counts them.
fn seconds_of(text: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if y < 1900 || mo == 0 || d == 0 {
        return None;
    }
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + s)
}
