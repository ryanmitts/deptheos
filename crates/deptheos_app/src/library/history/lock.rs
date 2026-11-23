//! The Library's lock (ADR-140): [`History::open`] takes an OS lock on
//! `<Library>/library.lock`, held until the last clone is dropped, so no
//! other app on any computer writes the Library meanwhile. A volume without
//! locks is refused.
//!
//! `library-owner.json` names the holder (computer and app version) for a
//! refused open to show. It is read only while someone else holds the
//! lock, so a stale one never misleads.
//!
//! [`History::open`]: super::History::open

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fsutil::{StoreError, io_error, write_json_atomic};

/// The file the Library's lock is taken on.
pub(crate) const LOCK_FILE: &str = "library.lock";
/// Who holds the lock ([`Holder`]).
pub(crate) const OWNER_FILE: &str = "library-owner.json";

/// The app that holds a Library's lock, as it said when it took it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    /// The computer's name; empty when it couldn't be told.
    pub host: String,
    /// The version of the app ("0.2.0").
    pub version: String,
}

impl Holder {
    /// This app, on this computer.
    pub fn this_app() -> Self {
        Self {
            host: host_name().unwrap_or_default(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Whether it runs on this computer.
    pub fn is_here(&self) -> bool {
        !self.host.is_empty() && host_name().is_some_and(|here| here == self.host)
    }
}

/// The lock on one Library, held while it lives.
#[derive(Debug)]
pub(crate) struct LibraryLock {
    _file: File,
}

impl LibraryLock {
    /// Take the lock on the Library at the existing folder `root`:
    /// [`StoreError::InUse`] when held, [`StoreError::NoLocks`] when the
    /// volume can't lock.
    pub(crate) fn take(root: &Path) -> Result<Self, StoreError> {
        let path = root.join(LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io_error(&path))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let holder = read_holder(root);
                tracing::info!("the Library at {} is in use", root.display());
                return Err(StoreError::InUse { library: root.to_path_buf(), holder });
            }
            Err(TryLockError::Error(source)) => {
                tracing::warn!("the Library at {} can't be locked: {source}", root.display());
                return Err(StoreError::NoLocks { library: root.to_path_buf(), source });
            }
        }
        tracing::debug!("library lock: taken on {}", root.display());
        Ok(Self { _file: file })
    }

    /// Write `library-owner.json`, for a refused open ([`StoreError::InUse`])
    /// to name.
    pub(crate) fn name_holder(&self, root: &Path) {
        let owner = root.join(OWNER_FILE);
        if let Err(e) = write_json_atomic(&owner, &Holder::this_app()) {
            // Only what a refused open says depends on it.
            tracing::warn!("who holds the Library couldn't be written: {e}");
        }
    }
}

/// Who holds the lock on the Library at `root`, as it wrote; `None` when it
/// can't be read.
fn read_holder(root: &Path) -> Option<Holder> {
    let bytes = std::fs::read(root.join(OWNER_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// This computer's name without `.local` or `.localdomain`; `None` when
/// unknown.
fn host_name() -> Option<String> {
    let name = system_host_name()?;
    let name = [".localdomain", ".local"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(&name)
        .trim()
        .to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(unix)]
fn system_host_name() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its whole length, and one byte is
    // kept for the terminator.
    let ok = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) } == 0;
    let end = buffer.iter().position(|&b| b == 0)?;
    ok.then(|| String::from_utf8_lossy(&buffer[..end]).into_owned())
}

#[cfg(windows)]
fn system_host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

#[cfg(not(any(unix, windows)))]
fn system_host_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time_and_the_refused_one_is_told_who() {
        let root = tempfile::tempdir().unwrap();
        let held = LibraryLock::take(root.path()).unwrap();
        held.name_holder(root.path());
        let Err(StoreError::InUse { holder, .. }) = LibraryLock::take(root.path()) else {
            panic!("taken twice")
        };
        let holder = holder.expect("the holder said who it is");
        assert_eq!(holder, Holder::this_app());
        assert!(holder.is_here() || holder.host.is_empty());
        assert!(root.path().join(LOCK_FILE).exists());
        drop(held);
        // Another test's forked child shares the lock's file until it
        // execs, so the lock may briefly outlive the drop.
        let start = std::time::Instant::now();
        while let Err(e) = LibraryLock::take(root.path()) {
            assert!(start.elapsed().as_secs() < 5, "free again once dropped: {e:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
