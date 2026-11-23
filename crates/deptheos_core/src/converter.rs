//! Camera RAW to linear DNG through the installed Adobe DNG Converter, so
//! frames decode with the DNG SDK's color science instead of LibRaw's.
//! macOS and Windows only.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::error::Result;

/// Why Adobe DNG Converter did not convert a file. Technical text, for the
/// log; `deptheos_app::words::explain` words it for a person.
#[derive(thiserror::Error, Debug)]
pub enum ConverterError {
    #[error("Adobe DNG Converter not found at {}", .0.display())]
    NotFound(PathBuf),

    #[error("Adobe DNG Converter is not available on this platform")]
    NotOnThisPlatform,

    #[error("could not run {}: {source}", path.display())]
    Start { path: PathBuf, source: std::io::Error },

    /// The conversion was cut short: the stack it was for stopped.
    #[error("converting {} was stopped", .0.display())]
    Stopped(PathBuf),

    #[error("converting {} took longer than {seconds} s", input.display())]
    TimedOut { input: PathBuf, seconds: u64 },

    /// The converter exited with an error; `message` is its exit status and
    /// what it printed.
    #[error("converting {}: {message}", input.display())]
    Failed { input: PathBuf, message: String },

    /// The converter exited cleanly but wrote nothing (an input it does not
    /// know, such as a camera newer than it).
    #[error("converting {} produced no output file", .0.display())]
    NoOutput(PathBuf),
}

/// A converter the user pointed at, in place of the default install path
/// ([`set_executable`]).
static EXECUTABLE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Adobe's page for the converter, where it is downloaded (free).
pub const DOWNLOAD_PAGE: &str =
    "https://helpx.adobe.com/camera-raw/desktop/dng-and-file-formats/adobe-dng-converter.html";

/// How many conversions may run at once ([`set_parallel_jobs`]); 0 means
/// [`default_parallel_jobs`].
static PARALLEL_JOBS: AtomicUsize = AtomicUsize::new(0);

/// The most conversions a caller may run at once.
pub const MAX_PARALLEL_JOBS: usize = 32;

/// Write uncompressed DNGs ([`set_uncompressed`]); off by default.
static UNCOMPRESSED: AtomicBool = AtomicBool::new(false);

/// Ask the converter for uncompressed tiles. A stack then runs about a
/// quarter faster (no lossless-JPEG decode) at twice the disk: ~270 MB a
/// 45 MP frame instead of 130 MB, kept until the stack's last read of it
/// (a 200-frame depth stack needs ~54 GB free instead of 26 GB).
pub fn set_uncompressed(on: bool) {
    UNCOMPRESSED.store(on, Ordering::Relaxed);
}

/// Whether conversions are written uncompressed.
pub fn uncompressed() -> bool {
    UNCOMPRESSED.load(Ordering::Relaxed)
}

/// A quarter of the CPU's threads, 2 to 8: each converter is itself partly
/// multithreaded.
pub fn default_parallel_jobs() -> usize {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    (threads / 4).clamp(2, 8)
}

/// Run up to `jobs` conversions at once from now on (`None`: back to
/// [`default_parallel_jobs`]). Clamped to 1..=[`MAX_PARALLEL_JOBS`].
pub fn set_parallel_jobs(jobs: Option<usize>) {
    let jobs = jobs.map_or(0, |n| n.clamp(1, MAX_PARALLEL_JOBS));
    PARALLEL_JOBS.store(jobs, Ordering::Relaxed);
}

/// How many conversions to run at once.
pub fn parallel_jobs() -> usize {
    match PARALLEL_JOBS.load(Ordering::Relaxed) {
        0 => default_parallel_jobs(),
        n => n,
    }
}

/// How long one conversion may run before it is killed: a frame takes
/// seconds, so this only catches a converter that never exits.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Where the converter installs by default on this platform, or `None` where
/// Adobe does not ship it.
fn default_executable() -> Option<&'static Path> {
    if cfg!(target_os = "macos") {
        Some(Path::new("/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"))
    } else if cfg!(target_os = "windows") {
        Some(Path::new(r"C:\Program Files\Adobe\Adobe DNG Converter\Adobe DNG Converter.exe"))
    } else {
        None
    }
}

/// Use the converter at `path` instead of the default (`None`).
pub fn set_executable(path: Option<PathBuf>) {
    *EXECUTABLE.write().unwrap_or_else(|e| e.into_inner()) = path;
}

/// The converter [`convert_until`] runs when given none: the one set with
/// [`set_executable`], else the default install location.
pub fn executable() -> Option<PathBuf> {
    let chosen = EXECUTABLE.read().unwrap_or_else(|e| e.into_inner()).clone();
    chosen.or_else(|| default_executable().map(Path::to_path_buf))
}

/// Whether the converter is installed where [`executable`] looks.
pub fn is_available() -> bool {
    executable().is_some_and(|p| p.is_file())
}

/// Convert a camera RAW file to *linear* (demosaiced) DNG, writing
/// `output_name` into `output_dir`. Returns the output path.
///
/// `executable` overrides [`executable`]. Tiles are compressed unless
/// [`set_uncompressed`]. Lens corrections stay as OpcodeList3, unapplied
/// on read and copied by DNG output framed as the converted image
/// (ADR-131). The converter is killed as soon as `stop` is set.
pub fn convert_until(
    input: &Path,
    output_dir: &Path,
    output_name: &str,
    executable: Option<&Path>,
    stop: &AtomicBool,
) -> Result<PathBuf> {
    let exe = match executable.map(Path::to_path_buf).or_else(self::executable) {
        Some(exe) if exe.is_file() => exe,
        Some(exe) => {
            return Err(ConverterError::NotFound(exe).into());
        }
        None => {
            return Err(ConverterError::NotOnThisPlatform.into());
        }
    };

    std::fs::create_dir_all(output_dir)?;
    // A leftover file would pass the output check below.
    let out_path = output_dir.join(output_name);
    match std::fs::remove_file(&out_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    let mut child = Command::new(&exe)
        .arg("-l") // linear (demosaiced) DNG
        // Lossless-JPEG or uncompressed tiles ([`set_uncompressed`]).
        .arg(if uncompressed() { "-u" } else { "-c" })
        .arg("-p0") // no embedded preview: nothing reads it
        .arg("-d")
        .arg(output_dir)
        .arg("-o")
        .arg(output_name)
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ConverterError::Start { path: exe.clone(), source })?;
    // Drained on its own thread so a full pipe can't block the converter.
    let stderr = child.stderr.take().and_then(|mut pipe| {
        let drain = crate::threads::thread("converter-stderr").spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        });
        drain.ok()
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let ended = if stop.load(Ordering::Relaxed) {
            Some(ConverterError::Stopped(input.to_path_buf()))
        } else if started.elapsed() > TIMEOUT {
            let seconds = TIMEOUT.as_secs();
            Some(ConverterError::TimedOut { input: input.to_path_buf(), seconds })
        } else {
            None
        };
        if let Some(error) = ended {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stderr = stderr.and_then(|t| t.join().ok()).unwrap_or_default();

    if !status.success() {
        return Err(ConverterError::Failed {
            input: input.to_path_buf(),
            message: format!("{status}: {}", stderr.trim()),
        }
        .into());
    }

    if !out_path.is_file() {
        // The converter can exit 0 without writing anything.
        return Err(ConverterError::NoOutput(input.to_path_buf()).into());
    }
    Ok(out_path)
}
