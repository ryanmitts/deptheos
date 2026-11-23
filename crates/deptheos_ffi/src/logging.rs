//! The diagnostic log ([`deptheos_app::engine::logs`]) for iOS: installed at
//! launch into `Library/Caches/Logs`, the app's lines forwarded in
//! ([`log_line`]), verbose on request, zipped for a bug report.
//! [`crash_on_purpose`] tests what a crash leaves (`docs/crash-testing.md`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::engine::logs::on_purpose::{self, Kind};
use deptheos_app::engine::logs::{self, App};
use deptheos_app::words::ios::Line;

use deptheos_app::engine::work::Lane;
use deptheos_app::{Platform, Words as AppWords};

use crate::request::{Ask, on_pool};
use crate::{EngineError, Request, Result, Words};

/// What each launch's header names the app.
const APP_NAME: &str = "Deptheos (iOS)";

/// Set up the log on the pool: stderr (Xcode's console) and a file per
/// launch in `directory`, headed with `version` and `build`. Call at
/// launch, before the engine; only the first install counts (this or the
/// engine's file-less fallback).
#[uniffi::export]
pub async fn install_logging(directory: PathBuf, version: String, build: String) {
    let install = move || {
        let app = App { name: APP_NAME, version: &version, build: &build };
        logs::install(&app, Some(&directory));
        Ok(())
    };
    let words = AppWords::english(Platform::Ios);
    // Failing leaves stderr only, as the fallback does.
    let _ =
        on_pool(&words, &Request::lasting(), Lane::Interactive, Ask::Own("log-install"), install)
            .await;
}

/// A kind of crash on purpose ([`on_purpose::Kind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CrashKind {
    /// A Rust panic in this call: the bridge makes it a Swift error this
    /// non-throwing call can't return, which ends the app.
    Panic,
    /// A Rust panic on an engine thread: logged; the app goes on.
    ThreadPanic,
    /// `abort()`: a native crash, no panic.
    Abort,
    /// A bad access: a native crash, `EXC_BAD_ACCESS`.
    Fault,
}

/// Crash as `kind` says, to test the log and the system's crash report.
/// Offered only in Debug and TestFlight builds.
#[uniffi::export]
pub fn crash_on_purpose(kind: CrashKind) {
    on_purpose::crash(match kind {
        CrashKind::Panic => Kind::Panic,
        CrashKind::ThreadPanic => Kind::ThreadPanic,
        CrashKind::Abort => Kind::Abort,
        CrashKind::Fault => Kind::Fault,
    });
}

/// The fallback when the app never installed the log: stderr only, so the
/// engine's lines reach Xcode's console.
pub(crate) fn install_fallback() {
    let app = App { name: APP_NAME, version: env!("CARGO_PKG_VERSION"), build: "" };
    logs::install(&app, None);
}

/// Debug detail for Deptheos's own code on or off, until the app quits.
/// Nothing without an installed log.
#[uniffi::export]
pub fn set_verbose_logging(on: bool) {
    if let Some(logs) = logs::installed() {
        logs.set_verbose(on);
    }
}

/// Whether the log has debug detail now.
#[uniffi::export]
pub fn verbose_logging() -> bool {
    logs::installed().is_some_and(logs::Logs::is_verbose)
}

/// Zip the kept log files into `path` on the pool, for the user to send.
/// Fails when this launch has no log file, saying so in `words`' language.
#[uniffi::export]
pub async fn write_diagnostics(
    request: Arc<Request>,
    path: PathBuf,
    words: Arc<Words>,
) -> Result<()> {
    let said = words.clone();
    let write = move || zip_logs(&path, &said.words);
    on_pool(&words.words, &request, Lane::Background, Ask::Own("diagnostics"), write).await
}

fn zip_logs(path: &Path, words: &AppWords) -> Result<()> {
    // Installed without a file when the engine started first: nothing to
    // send either way.
    let Some(logs) = logs::installed().filter(|l| l.dir().is_some()) else {
        tracing::error!("no diagnostics: this launch has no log file");
        return Err(EngineError::user(Line::NoLogFile, words));
    };
    logs.write_bundle(path, &[]).map_err(|e| {
        tracing::error!("diagnostics not written: {e}");
        let line = match e.kind() {
            std::io::ErrorKind::StorageFull => Line::DeviceFull,
            _ => Line::TryAgain,
        };
        EngineError::user(line, words)
    })
}

/// How much a line from the app matters, as `os.Logger`'s levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

/// One line from the app (`category` is its `os.Logger` category) into the
/// log under target `deptheos::ios`, so its debug lines show only while
/// verbose.
#[uniffi::export]
pub fn log_line(level: LogLevel, category: String, message: String) {
    match level {
        LogLevel::Debug => tracing::debug!(target: "deptheos::ios", "{category}: {message}"),
        LogLevel::Info => tracing::info!(target: "deptheos::ios", "{category}: {message}"),
        LogLevel::Warning => tracing::warn!(target: "deptheos::ios", "{category}: {message}"),
        LogLevel::Error => tracing::error!(target: "deptheos::ios", "{category}: {message}"),
    }
}
