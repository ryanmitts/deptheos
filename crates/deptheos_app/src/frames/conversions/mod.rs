//! The Adobe backend's RAW-to-DNG step: the cache of converted DNGs and
//! frames converted ahead of their stacks. A stack's own conversions are
//! [`StackConversions`] (`stack.rs`).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Instant, UNIX_EPOCH};

use deptheos_core::{Cancel, CoreError, converter};

use crate::{RawBackend, lock};

mod stack;

pub use stack::StackConversions;

/// Converted DNGs in one directory, each RAW converted once and reused
/// until the source changes; the directory goes with the cache. Shared
/// between threads; two requests for one source convert it once.
///
/// After each conversion and release, least recently used files are
/// deleted down to [`Self::with_limit`]. Held files ([`Self::open`]) are
/// never deleted, so a stack holding more than the limit exceeds it.
pub struct DngCache {
    dir: PathBuf,
    executable: Option<PathBuf>,
    /// Parent of every conversion's token; [`Self::clear`] cancels it.
    cancel: Cancel,
    /// Signaled when a name leaves `Entries::converting` or a waiter is
    /// canceled.
    finished: Condvar,
    /// The claim on `dir`, under the app's shared root.
    owner: Option<OwnerLock>,
    /// The conversions here and those in flight.
    entries: Mutex<Entries>,
    /// Most bytes of unheld conversions to keep.
    limit: AtomicU64,
}

/// The files in a cache's directory, by name.
#[derive(Default)]
struct Entries {
    files: HashMap<String, Entry>,
    /// Output names being converted.
    converting: HashSet<String>,
    bytes: u64,
    /// Counts uses, for least recently used.
    clock: u64,
}

struct Entry {
    bytes: u64,
    last_used: u64,
    /// Holders ([`CachedDng`]); a held file is never deleted.
    holds: u32,
}

impl Entries {
    /// Mark `name` used now, and held once more with `hold`.
    fn touch(&mut self, name: &str, hold: bool) {
        self.clock += 1;
        if let Some(entry) = self.files.get_mut(name) {
            entry.last_used = self.clock;
            entry.holds += u32::from(hold);
        }
    }

    fn insert(&mut self, name: &str, bytes: u64, hold: bool) {
        if let Some(old) = self.files.remove(name) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.files.insert(name.to_owned(), Entry { bytes, last_used: 0, holds: 0 });
        self.touch(name, hold);
    }

    fn remove(&mut self, name: &str) {
        if let Some(old) = self.files.remove(name) {
            self.bytes -= old.bytes;
        }
    }

    /// Whether `name` is already at `out`, touching it if so.
    fn reuse(&mut self, name: &str, out: &Path, hold: bool) -> bool {
        match std::fs::metadata(out) {
            Ok(meta) if meta.is_file() => {
                if self.files.contains_key(name) {
                    self.touch(name, hold);
                } else {
                    // Left in the directory before this cache counted it.
                    self.insert(name, meta.len(), hold);
                }
                true
            }
            _ => {
                self.remove(name);
                false
            }
        }
    }
}

/// A conversion held open: [`DngCache`] does not delete it until this drops.
pub struct CachedDng {
    cache: Arc<DngCache>,
    name: String,
    path: PathBuf,
}

impl CachedDng {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CachedDng {
    fn drop(&mut self) {
        self.cache.let_go(&self.name);
    }
}

/// `<root>/<name>.lock`, locked while `<root>/<name>` is in use. The OS
/// releases it however the process ends, so a lock that can be taken marks
/// an orphaned cache.
struct OwnerLock {
    path: PathBuf,
    _file: File,
}

const LOCK_EXTENSION: &str = ".lock";

impl DngCache {
    /// A cache owning `dir`, reusing what is there only for the same
    /// source. `executable` overrides the converter's install path.
    pub fn new(dir: PathBuf, executable: Option<PathBuf>) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            executable,
            cancel: Cancel::root(),
            finished: Condvar::new(),
            owner: None,
            entries: Mutex::new(Entries::default()),
            limit: AtomicU64::new(u64::MAX),
        })
    }

    /// Room for a stack or two (a 45 MP linear DNG is about 270 MB), so
    /// browsing or painting from them doesn't convert again.
    const DEFAULT_LIMIT: u64 = 8 << 30;

    /// Keep at most `bytes` of conversions nobody holds (no limit by
    /// default).
    pub(crate) fn with_limit(self, bytes: u64) -> Self {
        self.limit.store(bytes, Ordering::Relaxed);
        self
    }

    /// This process's own directory under `…/Deptheos/DNGCache`, so
    /// instances side by side never touch each other's conversions.
    pub(crate) fn for_this_process(executable: Option<PathBuf>) -> std::io::Result<Self> {
        let root = default_root().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no local data directory")
        })?;
        Ok(Self::in_root(&root, &std::process::id().to_string(), executable)?
            .with_limit(Self::DEFAULT_LIMIT))
    }

    /// `<root>/<name>`, locked before the directory exists so a sweep never
    /// sees it unclaimed; a dead namesake's leftovers are cleared.
    fn in_root(root: &Path, name: &str, executable: Option<PathBuf>) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let lock_path = root.join(format!("{name}{LOCK_EXTENSION}"));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        file.try_lock().map_err(|e| {
            std::io::Error::other(format!("{} is in use: {e}", lock_path.display()))
        })?;
        let dir = root.join(name);
        remove_dir(&dir);
        let mut cache = Self::new(dir, executable)?;
        cache.owner = Some(OwnerLock { path: lock_path, _file: file });
        Ok(cache)
    }

    /// Delete, on a background thread, caches no running instance owns
    /// (left by a crash or a cut-short quit; they can be gigabytes).
    pub fn sweep_default_dir() {
        let Some(root) = default_root() else { return };
        let own = std::process::id().to_string();
        let spawned =
            crate::engine::work::thread("dng-cache-sweep").spawn(move || sweep(&root, &own));
        if let Err(e) = spawned {
            tracing::warn!("could not start the DNG cache sweep: {e}");
        }
    }

    /// For shutdown: kill running conversions and delete the directory on
    /// the store thread (ADR-143), without waiting. What is still held stays
    /// for the next [`Self::sweep_default_dir`]; later conversions fail as
    /// canceled.
    pub fn clear(&self) {
        self.cancel.cancel();
        let dir = self.dir.clone();
        let _deleting = crate::engine::store::store().write(move || remove_dir(&dir));
        *lock(&self.entries) = Entries::default();
    }

    /// The linear DNG for `path`, unheld (tests only; see [`Self::open`]).
    /// A source edited in place gets a new key; a source being converted
    /// elsewhere is waited for.
    #[cfg(test)]
    fn convert(self: &Arc<Self>, path: &Path) -> Result<PathBuf, CoreError> {
        Ok(self.acquire(path, false, &Cancel::root())?.1)
    }

    /// The DNG for `path`, held until the result drops, canceled with this
    /// thread's work ([`crate::engine::work::current`]).
    pub fn open(self: &Arc<Self>, path: &Path) -> Result<CachedDng, CoreError> {
        self.open_until(path, &crate::engine::work::current())
    }

    /// [`Self::open`], ended when `cancel` is canceled.
    fn open_until(self: &Arc<Self>, path: &Path, cancel: &Cancel) -> Result<CachedDng, CoreError> {
        let (name, path) = self.acquire(path, true, cancel)?;
        Ok(CachedDng { cache: self.clone(), name, path })
    }

    /// The conversion's name and path. Canceled by `cancel` or
    /// [`Self::clear`].
    fn acquire(
        self: &Arc<Self>,
        path: &Path,
        hold: bool,
        cancel: &Cancel,
    ) -> Result<(String, PathBuf), CoreError> {
        let name = format!("{}.dng", cache_key(path)?);
        let out = self.dir.join(&name);
        let work = cancel.child();
        let _cleared = self.cancel.on_cancel({
            let work = work.clone();
            move || work.cancel()
        });
        // A wait for another thread's conversion is woken by a cancel.
        let me = Arc::downgrade(self);
        let _wake = work.on_cancel(move || {
            if let Some(cache) = me.upgrade() {
                drop(lock(&cache.entries));
                cache.finished.notify_all();
            }
        });
        let claim = {
            let mut entries = lock(&self.entries);
            while entries.converting.contains(&name) {
                work.check()?;
                entries = self.finished.wait(entries).unwrap_or_else(|e| e.into_inner());
            }
            work.check()?;
            if entries.reuse(&name, &out, hold) {
                return Ok((name, out));
            }
            entries.converting.insert(name.clone());
            Claim { cache: self, name: &name }
        };
        let start = Instant::now();
        let converted = converter::convert_until(
            path,
            &self.dir,
            &name,
            self.executable.as_deref(),
            work.flag(),
        )
        .inspect_err(|_| {
            // Never leave a partial file to be mistaken for a conversion.
            let _ = std::fs::remove_file(&out);
        })?;
        tracing::info!("converted {} ({:.0?})", path.display(), start.elapsed());
        let bytes = std::fs::metadata(&converted)?.len();
        lock(&self.entries).insert(&name, bytes, hold);
        drop(claim);
        self.evict();
        Ok((name, converted))
    }

    /// Whether `path`'s conversion is cached (converting adds no disk use).
    fn has(&self, path: &Path) -> bool {
        cache_key(path)
            .is_ok_and(|key| lock(&self.entries).files.contains_key(&format!("{key}.dng")))
    }

    /// One holder of `name` is done with it.
    fn let_go(&self, name: &str) {
        if let Some(entry) = lock(&self.entries).files.get_mut(name) {
            entry.holds = entry.holds.saturating_sub(1);
        }
        self.evict();
    }

    /// Delete the least recently used conversions nobody holds until the
    /// directory fits the limit.
    fn evict(&self) {
        let limit = self.limit.load(Ordering::Relaxed);
        let mut entries = lock(&self.entries);
        if entries.bytes <= limit {
            return;
        }
        let mut free: Vec<(u64, String)> = entries
            .files
            .iter()
            .filter(|(_, e)| e.holds == 0)
            .map(|(name, e)| (e.last_used, name.clone()))
            .collect();
        free.sort_unstable();
        for (_, name) in free {
            if entries.bytes <= limit {
                break;
            }
            match std::fs::remove_file(self.dir.join(&name)) {
                Ok(()) => entries.remove(&name),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => entries.remove(&name),
                // Still open somewhere (Windows): tried again next time.
                Err(e) => tracing::debug!("could not delete conversion {name}: {e}"),
            }
        }
    }

    /// Convert and hold every path `backend` converts, `jobs` at a time.
    /// `on_progress(current, total)` runs on this thread, `current` the one
    /// under way (1-based). Canceling gives `Ok(None)`; the first failure
    /// stops new starts and is the error.
    pub(crate) fn hold_all(
        self: &Arc<Self>,
        paths: &[PathBuf],
        backend: RawBackend,
        jobs: usize,
        cancel: &Cancel,
        mut on_progress: impl FnMut(u32, u32),
    ) -> Result<Option<Converted>, CoreError> {
        let mut seen = HashSet::new();
        let todo: Vec<&PathBuf> =
            paths.iter().filter(|p| backend.converts(p) && seen.insert(*p)).collect();
        if todo.is_empty() {
            return Ok(Some(Converted::default()));
        }
        let total = todo.len() as u32;
        let next = AtomicUsize::new(0);
        // A failure stops new starts; running ones finish.
        let failed = AtomicBool::new(false);
        let mut held = HashMap::with_capacity(todo.len());
        let mut error = None;
        std::thread::scope(|scope| {
            let (sender, results) = mpsc::channel();
            for _ in 0..jobs.clamp(1, todo.len()) {
                let sender = sender.clone();
                let (todo, next, failed) = (&todo, &next, &failed);
                let spawned =
                    crate::engine::work::thread("dng-convert").spawn_scoped(scope, move || {
                        while !failed.load(Ordering::Relaxed) && !cancel.is_canceled() {
                            let Some(path) = todo.get(next.fetch_add(1, Ordering::Relaxed)) else {
                                break;
                            };
                            let result = self.open_until(path, cancel);
                            if result.is_err() {
                                failed.store(true, Ordering::Relaxed);
                            }
                            if sender.send((*path, result)).is_err() {
                                break;
                            }
                        }
                    });
                if let Err(e) = spawned {
                    error.get_or_insert(e.into());
                }
            }
            drop(sender);
            on_progress(1, total);
            let mut done = 0;
            for (path, result) in results {
                done += 1;
                match result {
                    Ok(file) => {
                        held.insert(path.clone(), file);
                    }
                    Err(e) => {
                        error.get_or_insert(e);
                    }
                }
                if done < total && error.is_none() && !cancel.is_canceled() {
                    on_progress(done + 1, total);
                }
            }
        });
        if cancel.is_canceled() {
            return Ok(None);
        }
        match error {
            Some(e) => Err(e),
            None => Ok(Some(Converted { files: held })),
        }
    }
}

/// Frames converted ahead of their stacks ([`DngCache::hold_all`]), held
/// until this drops.
#[derive(Default)]
pub struct Converted {
    files: HashMap<PathBuf, CachedDng>,
}

impl Converted {
    /// `path`'s conversion, or `path` when not converted.
    pub fn file<'a>(&'a self, path: &'a Path) -> &'a Path {
        self.files.get(path).map_or(path, CachedDng::path)
    }

    /// How many frames were converted.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// A conversion in flight; dropping it, however it ended, clears the name
/// from `converting` and wakes waiters.
struct Claim<'a> {
    cache: &'a Arc<DngCache>,
    name: &'a str,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        lock(&self.cache.entries).converting.remove(self.name);
        self.cache.finished.notify_all();
    }
}

/// Where every instance keeps its cache, one directory each.
fn default_root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("Deptheos").join("DNGCache"))
}

/// Delete from `root` what no live process owns, sparing `own`: a lockable
/// `<name>.lock` goes with `<name>`, and anything without a lock file goes.
fn sweep(root: &Path, own: &str) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        match name.strip_suffix(LOCK_EXTENSION) {
            Some(cache) if cache == own => {}
            Some(cache) => {
                let free = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .ok()
                    .filter(|f| f.try_lock().is_ok());
                if let Some(_held) = free {
                    remove_dir(&root.join(cache));
                    let _ = std::fs::remove_file(&path);
                }
            }
            None if name == own => {}
            None if root.join(format!("{name}{LOCK_EXTENSION}")).exists() => {}
            None if path.is_dir() => remove_dir(&path),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

impl Drop for DngCache {
    fn drop(&mut self) {
        remove_dir(&self.dir);
        // Removed while still held, so no sweep can claim it in between.
        if let Some(owner) = self.owner.take() {
            let _ = std::fs::remove_file(&owner.path);
        }
    }
}

fn remove_dir(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("could not delete {}: {e}", dir.display());
        }
        _ => {}
    }
}

/// 16 hex digits over the path, size and modification time. Not stable
/// across builds, which is fine: a cache lives for one run.
fn cache_key(path: &Path) -> Result<String, CoreError> {
    let meta = std::fs::metadata(path)?;
    let modified = meta.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    (path, meta.len(), modified).hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(test)]
mod sweep_tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A running instance holds its lock; an exited one left it free; no
    /// lock file means no owner.
    #[test]
    fn a_sweep_takes_only_caches_no_running_instance_owns() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let held = File::create(root.join("111.lock")).unwrap();
        held.try_lock().unwrap();
        std::fs::create_dir(root.join("111")).unwrap();
        std::fs::write(root.join("111").join("a.dng"), "live").unwrap();
        File::create(root.join("222.lock")).unwrap();
        std::fs::create_dir(root.join("222")).unwrap();
        std::fs::write(root.join("222").join("b.dng"), "dead").unwrap();
        std::fs::create_dir(root.join("333")).unwrap();
        std::fs::write(root.join("loose.dng"), "orphan").unwrap();

        sweep(root, "999");
        assert_eq!(names(root), ["111", "111.lock"]);
        assert_eq!(names(&root.join("111")), ["a.dng"]);

        // Once it exits its cache goes too, though another test's fork may
        // hold the lock until it execs, hence the retry.
        drop(held);
        let start = Instant::now();
        sweep(root, "999");
        while !names(root).is_empty() && start.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            sweep(root, "999");
        }
        assert!(names(root).is_empty());
    }

    #[test]
    fn an_instance_cache_is_claimed_until_dropped() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::create_dir_all(root.join("42")).unwrap();
        std::fs::write(root.join("42").join("stale.dng"), "a dead process's").unwrap();

        let cache = DngCache::in_root(root, "42", None).unwrap();
        assert_eq!(names(root), ["42", "42.lock"]);
        assert!(names(&root.join("42")).is_empty(), "a dead namesake's files are cleared");
        assert!(DngCache::in_root(root, "42", None).is_err(), "one owner at a time");

        // Neither another instance's sweep nor this one's touches it.
        sweep(root, "7");
        sweep(root, "42");
        assert_eq!(names(root), ["42", "42.lock"]);

        drop(cache);
        assert!(names(root).is_empty());
    }
}
