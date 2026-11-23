//! The diagnostic log: what a user sends with a bug report.
//!
//! [`install`] logs to stderr and to a file per launch in [`default_dir`],
//! rolled at 10 MiB and pruned to ten (`sink.rs`), each opening with a
//! header ([`system`]). A panic is written straight to the file with its
//! backtrace and image addresses (`images.rs`) before the previous hook
//! runs, so the OS crash report still happens.
//!
//! The home folder is written `~` (`redact.rs`). Paths are logged at
//! `info`, so a log without Verbose Logging still says what was read.
//!
//! The level is `info` (GPU dependencies at `warn`), or `RUST_LOG`.
//! [`Logs::set_verbose`] adds `debug` for Deptheos's crates; front ends
//! don't keep it across launches.
//!
//! [`on_purpose`] crashes to test this (`docs/crash-testing.md`); on
//! Windows a native crash leaves a minidump ([`dumps`]).
//! [`Logs::write_bundle`] zips it all for a user to attach.

mod bundle;
pub mod dumps;
mod images;
pub mod on_purpose;
mod redact;
mod sink;
pub mod system;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt, reload};

use sink::{FileWriter, Sink, UtcTimer};

/// The filter without `RUST_LOG`: `info`, wgpu's at `warn`.
const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";
/// Added by [`Logs::set_verbose`]: every Deptheos crate's target starts
/// with `deptheos`.
const VERBOSE_DIRECTIVE: &str = "deptheos=debug";

/// Where the log files go: `~/Library/Logs/Deptheos` (macOS),
/// `%LOCALAPPDATA%\Deptheos\logs` (Windows), `$XDG_STATE_HOME/Deptheos`
/// (Linux, else `~/.local/state`). `None` on iOS, whose app passes its own.
pub fn default_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return dirs::home_dir().map(|home| home.join("Library").join("Logs").join("Deptheos"));
    #[cfg(windows)]
    return dirs::data_local_dir().map(|dir| dir.join("Deptheos").join("logs"));
    #[cfg(target_os = "linux")]
    return dirs::state_dir().or_else(dirs::data_local_dir).map(|dir| dir.join("Deptheos"));
    #[allow(unreachable_code)]
    None
}

/// What a front end says about itself in each launch's header.
pub struct App<'a> {
    /// "Deptheos (desktop)", "Deptheos (iOS)", "deptheos (CLI)".
    pub name: &'a str,
    /// The version people see.
    pub version: &'a str,
    /// The build: a commit and profile, or "" when unknown.
    pub build: &'a str,
}

/// The installed log: its folder, and the level while the app runs.
pub struct Logs {
    /// `None` when no file could be opened: stderr only.
    sink: Option<Arc<Mutex<Sink>>>,
    base: String,
    filter: reload::Handle<EnvFilter, Registry>,
    verbose: Mutex<bool>,
}

static INSTALLED: OnceLock<Logs> = OnceLock::new();

/// Sets up logging: stderr and, when `dir` can be written, a file there.
/// Later calls return the same [`Logs`]. Also routes in the DNG bridge's
/// diagnostics.
pub fn install(app: &App<'_>, dir: Option<&Path>) -> &'static Logs {
    INSTALLED.get_or_init(|| {
        let base = std::env::var("RUST_LOG")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_FILTER.to_owned());
        let (filter, handle) = reload::Layer::new(filter_for(&base, false));

        let (sink, failed) = match dir.map(Sink::open) {
            Some(Ok(sink)) => (Some(Arc::new(Mutex::new(sink))), None),
            Some(Err(e)) => (None, Some(e)),
            None => (None, None),
        };

        let stderr_ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let stderr =
            fmt::layer().with_timer(UtcTimer).with_ansi(stderr_ansi).with_writer(std::io::stderr);
        let file = sink.clone().map(|sink| {
            fmt::layer()
                .with_timer(UtcTimer)
                .with_ansi(false)
                .with_thread_names(true)
                .with_writer(FileWriter::new(sink))
        });
        let _ = tracing_subscriber::registry().with(filter).with(stderr).with(file).try_init();
        crate::route_logs_to_tracing();
        if let (Some(dir), Some(e)) = (dir, failed) {
            tracing::warn!("no log file in {}: {e}", redact::redact(&dir.display().to_string()));
        }

        let logs = Logs { sink, base, filter: handle, verbose: Mutex::new(false) };
        logs.install_panic_hook();
        logs.write_header(app);
        logs
    })
}

/// The log [`install`] set up, if any.
pub fn installed() -> Option<&'static Logs> {
    INSTALLED.get()
}

fn filter_for(base: &str, verbose: bool) -> EnvFilter {
    let spec = if verbose { format!("{base},{VERBOSE_DIRECTIVE}") } else { base.to_owned() };
    EnvFilter::try_new(&spec).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

impl Logs {
    /// Flushes this launch's file to disk.
    pub fn flush(&self) {
        if let Some(sink) = &self.sink {
            crate::lock(sink).flush();
        }
    }

    /// The folder the files are in, when there is one.
    pub fn dir(&self) -> Option<PathBuf> {
        self.sink.as_ref().map(|s| crate::lock(s).dir().to_path_buf())
    }

    /// This launch's file now (a rollover starts another).
    pub fn current_file(&self) -> Option<PathBuf> {
        self.sink.as_ref().and_then(|s| crate::lock(s).current_path().map(Path::to_path_buf))
    }

    /// Whether Deptheos's own crates log at `debug`.
    pub fn is_verbose(&self) -> bool {
        *crate::lock(&self.verbose)
    }

    /// Logs Deptheos's crates at `debug` or at the base level.
    pub fn set_verbose(&self, on: bool) {
        let mut verbose = crate::lock(&self.verbose);
        if *verbose == on {
            return;
        }
        match self.filter.reload(filter_for(&self.base, on)) {
            Ok(()) => {
                *verbose = on;
                tracing::info!("verbose logging {}", if on { "on" } else { "off" });
            }
            Err(e) => tracing::warn!("couldn't change the log level: {e}"),
        }
    }

    /// Zips the kept log files, newest first, and `extra` into `dest`;
    /// [`std::io::ErrorKind::NotFound`] without a log folder.
    pub fn write_bundle(&self, dest: &Path, extra: &[PathBuf]) -> std::io::Result<()> {
        let Some(sink) = &self.sink else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "this launch has no log folder",
            ));
        };
        let files = sink::kept_files(crate::lock(sink).dir())?;
        bundle::write(dest, &files, extra)
    }

    fn install_panic_hook(&self) {
        let Some(sink) = self.sink.clone() else { return };
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("<unnamed>");
            let backtrace = std::backtrace::Backtrace::force_capture();
            // A release backtrace names few functions; addresses let it be
            // symbolized (`images.rs`).
            let addresses = images::describe(&images::frames(), &images::ours());
            let line = format!(
                "{} PANIC thread '{name}' {info}\nbacktrace:\n{backtrace}\n{addresses}",
                sink::utc_now()
            );
            let line = redact::redact(&line);
            // Wait a little for the lock, never block: this thread may hold
            // it mid-write. Then the previous hook's stderr is all.
            for _ in 0..100 {
                match sink.try_lock() {
                    Ok(mut sink) => sink.write_line(line.as_bytes()),
                    Err(std::sync::TryLockError::Poisoned(p)) => {
                        p.into_inner().write_line(line.as_bytes());
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        continue;
                    }
                }
                break;
            }
            previous(info);
        }));
    }

    fn write_header(&self, app: &App<'_>) {
        let build = if app.build.is_empty() { String::new() } else { format!(" ({})", app.build) };
        tracing::info!(target: "deptheos::session", "{} {}{build}", app.name, app.version);
        for (key, value) in system::describe() {
            tracing::info!(target: "deptheos::session", "{key}: {value}");
        }
        if let Some(path) = self.current_file() {
            tracing::info!(
                target: "deptheos::session",
                "log: {} (times are UTC)",
                redact::redact(&path.display().to_string())
            );
        }
    }
}
