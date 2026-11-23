//! The turntable's store of renders while it runs: a temporary folder of
//! raw pixels (none for a grow-only run), kept for the sequence and
//! removed when the run ends.

use std::path::{Path, PathBuf};

use deptheos_app::runs::sequence::{StackStore, TurntableMode};
use deptheos_app::{CompletedStack, CoreError};

use crate::error::{Error, Refusal, Result};

/// A CLI turntable run's folder of raw pixels, removed when the run ends.
struct TempDir {
    dir: PathBuf,
    /// Held locked while the run lasts: a folder whose lock is free was
    /// left by a run that was killed, and the next run deletes it.
    _lock: std::fs::File,
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The folder `pixels` keeps a run's renders' pixels in, if any.
fn dir(pixels: &Option<TempDir>) -> Option<&Path> {
    pixels.as_ref().map(|temp| temp.dir.as_path())
}

/// A CLI turntable run's renders while it stacks.
///
/// ```text
/// TempStore ──push…──▶ TempStore ──keep(self)──▶ KeptStore ──load…
/// ```
///
/// Lifecycle: collecting (`TempStore`, changed by `push`) → kept
/// (`KeptStore`, changed by `load`), the one move being `keep(self)`, so
/// the type is the phase and nothing is pushed after it or loaded before.
/// The turntable runner's `renders` (`sequence/turntable.rs`) holds it.
pub(super) struct TempStore {
    /// The renders' raw pixels on disk, for the full sequence (fifty 45 MP
    /// results don't fit in memory); none for a grow-only run, which replays
    /// each stack from its frames.
    pixels: Option<TempDir>,
    /// The stacks without their pixels.
    stacks: Vec<CompletedStack>,
}

impl TempStore {
    /// A store in `parent`, with a pixel folder only for a full sequence.
    pub(super) fn new(parent: &Path, mode: TurntableMode) -> Result<Self> {
        sweep_temp_folders(parent);
        let pixels = match mode.full() {
            true => Some(temp_dir(parent)?),
            false => None,
        };
        Ok(Self { pixels, stacks: Vec::new() })
    }

    /// Keeps `stack` (its pixels on disk, if kept); returns its index.
    pub(super) fn push(&mut self, mut stack: CompletedStack) -> Result<usize> {
        let index = self.stacks.len();
        if let Some(dir) = dir(&self.pixels) {
            let file = dir.join(format!("{index}.rgb16"));
            std::fs::write(file, bytemuck::cast_slice::<u16, u8>(&stack.pixels))
                .map_err(CoreError::from)?;
        }
        stack.pixels = Vec::new();
        stack.frame_map = None;
        self.stacks.push(stack);
        Ok(index)
    }

    /// The stacks at `keep`, in that order and numbered from 0: what the
    /// sequence is made of. Each index is kept at most once.
    pub(super) fn keep(self, keep: &[usize]) -> Result<KeptStore> {
        let mut old: Vec<Option<CompletedStack>> = self.stacks.into_iter().map(Some).collect();
        let mut stacks = Vec::with_capacity(keep.len());
        for &i in keep {
            let stack = old.get_mut(i).and_then(Option::take).ok_or_else(|| {
                Error::Fault(format!("stack {i} is not in the store, or kept twice"))
            })?;
            stacks.push(stack);
        }
        tracing::debug!("turntable store: kept {} of {} stacks", keep.len(), old.len());
        Ok(KeptStore { pixels: self.pixels, stacks, pushed: keep.to_vec(), loaded: None })
    }
}

/// A new folder in `parent` for a run's pixels, held locked while it runs.
fn temp_dir(parent: &Path) -> Result<TempDir> {
    let dir = parent.join(format!("{TEMP_PREFIX}{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(CoreError::from)?;
    let lock = std::fs::File::create(dir.join(LOCK_FILE)).map_err(CoreError::from)?;
    lock.try_lock()
        .map_err(|e| Refusal::TempFolderLocked { folder: dir.clone(), why: e.to_string() })?;
    Ok(TempDir { dir, _lock: lock })
}

/// The renders a turntable sequence is made of, one in memory at a time.
pub(super) struct KeptStore {
    /// [`TempStore`]'s folder of pixels, if any.
    pixels: Option<TempDir>,
    /// The stacks, with pixels only for the one loaded.
    stacks: Vec<CompletedStack>,
    /// Each kept stack's index as it was pushed, which names its file.
    pushed: Vec<usize>,
    loaded: Option<usize>,
}

const TEMP_PREFIX: &str = ".deptheos-turntable-";
const LOCK_FILE: &str = "lock";

/// Deletes the temporary folders in `parent` that no running turntable
/// holds.
fn sweep_temp_folders(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX);
        if !ours || !path.is_dir() {
            continue;
        }
        // No lock file (a run killed before it made one) or one nobody
        // holds: abandoned.
        let free = match std::fs::File::open(path.join(LOCK_FILE)) {
            Ok(lock) => lock.try_lock().is_ok(),
            Err(_) => true,
        };
        if free {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    eprintln!(
                        "removed the temporary folder {}, left by an earlier run",
                        path.display()
                    )
                }
                Err(e) => tracing::warn!("could not remove {}: {e}", path.display()),
            }
        }
    }
}

impl StackStore for KeptStore {
    fn load(&mut self, index: usize) -> std::result::Result<&CompletedStack, deptheos_app::Error> {
        if self.loaded != Some(index) {
            let Some(dir) = dir(&self.pixels) else {
                return Err(deptheos_app::Error::InvalidState(
                    "a grow-only run kept no pixels".into(),
                ));
            };
            let file = dir.join(format!("{}.rgb16", self.pushed[index]));
            let bytes = std::fs::read(file).map_err(CoreError::from)?;
            if let Some(last) = self.loaded.take() {
                self.stacks[last].pixels = Vec::new();
            }
            self.stacks[index].pixels = bytemuck::pod_collect_to_vec(&bytes);
            self.loaded = Some(index);
        }
        Ok(&self.stacks[index])
    }

    fn load_settings(
        &mut self,
        index: usize,
    ) -> std::result::Result<&CompletedStack, deptheos_app::Error> {
        self.stacks
            .get(index)
            .ok_or_else(|| deptheos_app::Error::InvalidState(format!("no stack {index} kept")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary folder whose run is gone (no lock, or a lock nobody
    /// holds) is swept; one a running turntable holds stays.
    #[test]
    fn sweeps_temp_folders_their_runs_left() {
        let parent = tempfile::tempdir().unwrap();
        let folder = |name: &str| {
            let dir = parent.path().join(format!("{TEMP_PREFIX}{name}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("0.rgb16"), b"pixels").unwrap();
            dir
        };
        let old = folder("1");
        let killed = folder("2");
        std::fs::File::create(killed.join(LOCK_FILE)).unwrap();
        let running = folder("3");
        let held = std::fs::File::create(running.join(LOCK_FILE)).unwrap();
        held.try_lock().unwrap();
        let unrelated = parent.path().join("scan1 grow");
        std::fs::create_dir_all(&unrelated).unwrap();

        let store = TempStore::new(parent.path(), TurntableMode::Full).unwrap();
        assert!(!old.exists() && !killed.exists(), "abandoned folders are swept");
        assert!(running.exists(), "a running turntable's folder stays");
        assert!(unrelated.exists());
        let own = dir(&store.pixels).unwrap().to_path_buf();
        assert!(own.exists());
        drop(store);
        assert!(!own.exists(), "a run removes its own on the way out");
    }

    /// The sequence's stacks come back in the order kept, whatever order
    /// they were made in, and the folder goes with the kept store.
    #[test]
    fn the_kept_stacks_load_in_the_order_kept() {
        let parent = tempfile::tempdir().unwrap();
        let stack = |v: u16| CompletedStack::srgb(1, 1, vec![v; 3], Vec::new());
        let mut store = TempStore::new(parent.path(), TurntableMode::Both).unwrap();
        for v in 0..4 {
            assert_eq!(store.push(stack(v)).unwrap(), v as usize);
        }
        let dir = dir(&store.pixels).unwrap().to_path_buf();
        let mut kept = store.keep(&[3, 0, 1]).unwrap();
        for (i, v) in [3, 0, 1].into_iter().enumerate() {
            assert_eq!(kept.load(i).unwrap().pixels, [v; 3]);
        }
        assert_eq!(kept.load(0).unwrap().pixels, [3; 3], "loaded again");
        assert!(kept.stacks[2].pixels.is_empty(), "one in memory at a time");
        drop(kept);
        assert!(!dir.exists());
    }

    /// A grow-only run keeps no pixels and makes no folder for them: its
    /// stacks are replayed from their frames, with the settings kept.
    #[test]
    fn a_grow_only_run_keeps_no_pixels() {
        let parent = tempfile::tempdir().unwrap();
        let mut store = TempStore::new(parent.path(), TurntableMode::Grow).unwrap();
        for v in 0..2 {
            store.push(CompletedStack::srgb(2, 1, vec![v; 6], Vec::new())).unwrap();
        }
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0, "no folder");
        let mut kept = store.keep(&[1, 0]).unwrap();
        assert_eq!(kept.load_settings(0).unwrap().width, 2);
        assert!(kept.load_settings(0).unwrap().pixels.is_empty());
        assert!(kept.load(0).is_err(), "no pixels to load");
        assert!(kept.load_settings(2).is_err());
    }

    #[test]
    fn a_stack_is_kept_at_most_once() {
        let parent = tempfile::tempdir().unwrap();
        let mut store = TempStore::new(parent.path(), TurntableMode::Full).unwrap();
        store.push(CompletedStack::srgb(1, 1, vec![0; 3], Vec::new())).unwrap();
        assert!(store.keep(&[0, 0]).is_err());
        let mut store = TempStore::new(parent.path(), TurntableMode::Full).unwrap();
        store.push(CompletedStack::srgb(1, 1, vec![0; 3], Vec::new())).unwrap();
        assert!(store.keep(&[1]).is_err());
    }
}
