//! One stack's Adobe conversions, made on background threads while it
//! reads.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use deptheos_core::{Cancel, ConverterError, CoreError};

use super::{CachedDng, DngCache};
use crate::{RawBackend, lock};

/// One stack's Adobe conversions: background threads convert frames in
/// read order ([`Self::plan`]), at most `ahead` past the furthest asked
/// for, so stacking starts early and disk use stays bounded. Each file is
/// held until [`Self::release`]; one asked for again after that is
/// reconverted if evicted. A failure fails only its frame. Dropping
/// ([`Self::stop`]) kills running conversions.
pub struct StackConversions {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

struct Shared {
    cache: Arc<DngCache>,
    paths: Vec<PathBuf>,
    /// Each converted source's frame index (the first, if it repeats).
    index: HashMap<PathBuf, usize>,
    ahead: usize,
    state: Mutex<Conversions>,
    changed: Condvar,
    /// Canceled by [`StackConversions::stop`]: kills the converters running.
    halt: Cancel,
}

struct Conversions {
    /// Per frame index; unconverted frames stay `Waiting`, never asked for.
    slots: Vec<Slot>,
    /// The converted frames in read order, and each one's place in it.
    plan: Vec<usize>,
    place: HashMap<usize, usize>,
    /// The first place in `plan` not yet started.
    next: usize,
    /// The furthest place in `plan` asked for.
    asked: usize,
    /// No more conversions start: dropped.
    stopped: bool,
}

enum Slot {
    Waiting,
    Converting,
    Ready(CachedDng),
    /// The error for the first asker, its message for later ones (the
    /// session probes the first frame before reading it).
    Failed(Option<CoreError>, String),
    Released,
}

impl StackConversions {
    /// Start converting the frames of `paths` that `backend` converts, in
    /// their order, `jobs` at a time.
    pub fn start(
        cache: Arc<DngCache>,
        paths: Vec<PathBuf>,
        backend: RawBackend,
        jobs: usize,
    ) -> Self {
        let mut index = HashMap::new();
        for (i, path) in paths.iter().enumerate() {
            if backend.converts(path) {
                index.entry(path.clone()).or_insert(i);
            }
        }
        let jobs = jobs.clamp(1, index.len().max(1));
        let state = Conversions {
            slots: paths.iter().map(|_| Slot::Waiting).collect(),
            plan: Vec::new(),
            place: HashMap::new(),
            next: 0,
            asked: 0,
            stopped: false,
        };
        let shared = Arc::new(Shared {
            cache,
            paths,
            index,
            // Enough to keep every converter busy while the stack reads.
            ahead: 2 * jobs,
            state: Mutex::new(state),
            changed: Condvar::new(),
            halt: Cancel::root(),
        });
        shared.plan(&shared.paths);
        let workers = (0..jobs)
            .filter_map(|i| {
                let shared = shared.clone();
                crate::engine::work::thread(format!("dng-convert-{i}"))
                    .spawn(move || shared.work())
                    .inspect_err(|e| tracing::warn!("could not start a DNG conversion thread: {e}"))
                    .ok()
            })
            .collect();
        Self { shared, workers }
    }
}

impl Shared {
    fn plan(&self, order: &[PathBuf]) {
        let mut state = lock(&self.state);
        let mut seen = HashSet::new();
        state.plan = order
            .iter()
            .filter_map(|p| self.index.get(p).copied())
            .filter(|&i| seen.insert(i))
            .collect();
        state.place = state.plan.iter().enumerate().map(|(place, &i)| (i, place)).collect();
        state.next = 0;
        state.asked = 0;
        self.changed.notify_all();
    }

    /// One conversion thread: the next waiting frame within reach.
    fn work(&self) {
        loop {
            let i = {
                let mut state = lock(&self.state);
                loop {
                    if state.stopped {
                        return;
                    }
                    while state.next < state.plan.len()
                        && !matches!(state.slots[state.plan[state.next]], Slot::Waiting)
                    {
                        state.next += 1;
                    }
                    // Past the plan's end nothing waits: done.
                    if state.next >= state.plan.len() {
                        return;
                    }
                    if state.next <= state.asked + self.ahead {
                        let i = state.plan[state.next];
                        state.next += 1;
                        state.slots[i] = Slot::Converting;
                        break i;
                    }
                    state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
                }
            };
            let result = self.cache.open_until(&self.paths[i], &self.halt);
            let mut state = lock(&self.state);
            // A failed frame fails alone: the session leaves it out.
            state.slots[i] = match result {
                Ok(file) => Slot::Ready(file),
                Err(e) => {
                    let message = e.to_string();
                    Slot::Failed(Some(e), message)
                }
            };
            self.changed.notify_all();
        }
    }

    fn file(&self, path: &Path) -> Result<PathBuf, CoreError> {
        let Some(&i) = self.index.get(path) else { return Ok(path.to_path_buf()) };
        let mut state = lock(&self.state);
        if let Some(&place) = state.place.get(&i)
            && place > state.asked
        {
            state.asked = place;
            self.changed.notify_all();
        }
        loop {
            let stopped = state.stopped;
            match state.slots[i] {
                Slot::Ready(ref file) => return Ok(file.path().to_path_buf()),
                Slot::Failed(ref mut error, ref message) => {
                    return Err(error.take().unwrap_or_else(|| {
                        ConverterError::Failed {
                            input: path.to_path_buf(),
                            message: message.clone(),
                        }
                        .into()
                    }));
                }
                Slot::Released => break,
                Slot::Waiting if stopped => {
                    return Err(ConverterError::Stopped(path.to_path_buf()).into());
                }
                Slot::Waiting | Slot::Converting => {}
            }
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        // Read again after release (a restarted pass): held again, and
        // reconverted if evicted.
        state.slots[i] = Slot::Converting;
        drop(state);
        let result = self.cache.open_until(path, &self.halt);
        let mut state = lock(&self.state);
        let (slot, result) = match result {
            Ok(file) => {
                let path = file.path().to_path_buf();
                (Slot::Ready(file), Ok(path))
            }
            Err(e) => (Slot::Released, Err(e)),
        };
        state.slots[i] = slot;
        self.changed.notify_all();
        result
    }

    fn release(&self, path: &Path) {
        let Some(&i) = self.index.get(path) else { return };
        let mut state = lock(&self.state);
        if matches!(state.slots[i], Slot::Ready(_)) {
            let held = std::mem::replace(&mut state.slots[i], Slot::Released);
            drop(state);
            // Let go outside the lock: the cache may delete files now.
            drop(held);
        }
    }
}

/// The disk space a stack's conversions still need
/// ([`StackConversions::space_needed`]).
pub(crate) struct SpaceNeeded {
    /// The cache's folder, where they are written.
    pub folder: PathBuf,
    pub bytes: u64,
}

impl StackConversions {
    /// Disk space the remaining conversions need, estimated from one made
    /// (one camera, similar sizes). With `held_through` (a blend pass reads
    /// each frame twice) all are on disk at once; without, the cache keeps
    /// to its limit. `None` before the first conversion or when done.
    pub(crate) fn space_needed(&self, held_through: bool) -> Option<SpaceNeeded> {
        let shared = &self.shared;
        let (each, to_come) = {
            let state = lock(&shared.state);
            let each = state.slots.iter().find_map(|slot| match slot {
                Slot::Ready(file) => std::fs::metadata(file.path()).ok().map(|m| m.len()),
                _ => None,
            })?;
            let to_come: Vec<usize> = shared
                .index
                .values()
                .copied()
                .filter(|&i| matches!(state.slots[i], Slot::Waiting | Slot::Converting))
                .collect();
            (each, to_come)
        };
        // A frame converted for an earlier stack is read from the cache.
        let to_come =
            to_come.into_iter().filter(|&i| !shared.cache.has(&shared.paths[i])).count() as u64;
        let all = each.saturating_mul(to_come);
        let bytes = if held_through {
            all
        } else {
            let window = (shared.ahead + self.workers.len()) as u64;
            let limit = shared.cache.limit.load(Ordering::Relaxed);
            all.min(limit.saturating_add(each.saturating_mul(window)))
        };
        (bytes > 0).then(|| SpaceNeeded { folder: shared.cache.dir.clone(), bytes })
    }

    /// The session reads the frames in `order` from now on, from its start.
    pub fn plan(&self, order: &[PathBuf]) {
        self.shared.plan(order);
    }

    /// The file to decode for frame `path`, waiting for it if need be.
    pub fn file(&self, path: &Path) -> Result<PathBuf, CoreError> {
        self.shared.file(path)
    }

    /// The session is done with `path`, unless a restarted pass asks again.
    pub fn release(&self, path: &Path) {
        self.shared.release(path);
    }

    /// Kill running conversions and start none (a canceled stack); every
    /// unconverted frame fails.
    pub fn stop(&self) {
        self.shared.halt.cancel();
        lock(&self.shared.state).stopped = true;
        self.shared.changed.notify_all();
    }
}

impl Drop for StackConversions {
    fn drop(&mut self) {
        self.stop();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
