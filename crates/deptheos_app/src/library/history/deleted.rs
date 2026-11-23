//! Recently Deleted (ADR-140): a deleted stack or render is moved into
//! `<Library>/Deleted/<id>/`, kept [`KEPT_DAYS`] days and restorable whole.
//!
//! A record holds one delete's share of one stack: the whole stack, or
//! some of its renders and failed runs. Its folder mirrors the Library's,
//! so a restore renames everything back:
//!
//! ```text
//! Deleted/<id>/
//!   deleted.json            schema, deleted_unix_ms, stack, name?, whole,
//!                           renders [{id, number}], failures [id], size
//!   Renders/<render id>/    each render's folder, whole, as it was
//!   Failed/<id>.json        each failed run
//!   Stacks/<stack id>.json  a whole stack's file: its name, numbering and
//!                           kept working version
//! ```
//!
//! The record is written first (via `<id>.partial`), then each thing is
//! renamed into it, so a crash leaves a restorable record. Roots and stack
//! numbers count what is in here too.
//!
//! Records older than [`KEPT_DAYS`] are removed as the Library opens; a
//! stack left with nothing is forgotten then.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use image::RgbImage;
use serde::{Deserialize, Serialize};

use super::{FAILED_DIR, History, PaintClosed, RENDERS_DIR, Render, Result, tree_size};
use crate::fsutil::{
    PARTIAL_SUFFIX, Versioned, io_error, read_versioned, remove_if_present, rename,
    write_json_atomic,
};
use crate::library::ids::{RenderId, StackId, is_id, new_id};
use crate::library::render_file::{THUMBNAIL_FILE, from_unix_ms, partial_of, to_unix_ms};
use crate::library::stacks::STACKS_DIR;

/// How long a delete is kept before it goes for good.
pub const KEPT_DAYS: u64 = 30;
const KEPT_FOR: Duration = Duration::from_secs(KEPT_DAYS * 24 * 60 * 60);

pub(crate) const DELETED_DIR: &str = "Deleted";
const RECORD_FILE: &str = "deleted.json";
/// A record's own format version.
pub(crate) const DELETED_SCHEMA: u32 = 1;

/// A record's id, its folder's name under `Deleted/`; parsed strictly since
/// it names a folder.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeletedId(String);

impl DeletedId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// An id as [`Self::as_str`] wrote it; `None` for anything else.
    pub fn parse(id: &str) -> Option<Self> {
        is_id(id).then(|| Self(id.to_string()))
    }

    /// When it was made: a UUIDv7 holds its time in its first 48 bits.
    fn made(&self) -> Option<SystemTime> {
        let ms = u64::from_str_radix(self.0.get(..12)?, 16).ok()?;
        Some(from_unix_ms(ms))
    }
}

/// What one delete put in Recently Deleted, one record per stack.
#[derive(Clone, Debug, Default)]
pub struct Deletion(pub Vec<DeletedItem>);

impl Deletion {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The records, for Undo ([`History::restore`]).
    pub fn ids(&self) -> Vec<DeletedId> {
        self.0.iter().map(|i| i.id.clone()).collect()
    }
}

/// What a record took of its stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Took {
    /// The whole stack, its file included.
    Stack { renders: usize },
    /// Renders by number and `failed` failed runs; the stack stays.
    Renders { numbers: Vec<u32>, failed: usize },
}

/// A record in Recently Deleted, as listed.
#[derive(Clone, Debug)]
pub struct DeletedItem {
    pub id: DeletedId,
    pub deleted: SystemTime,
    pub stack: StackId,
    /// What its stack was called when deleted.
    pub name: Option<String>,
    pub took: Took,
    /// What it takes on disk, in bytes.
    pub size: u64,
    /// Its renders, newest first: the first's thumbnail stands for it
    /// ([`History::deleted_thumbnail`]).
    pub renders: Vec<RenderId>,
}

impl DeletedItem {
    /// When it goes for good.
    pub fn purged_at(&self) -> SystemTime {
        self.deleted + KEPT_FOR
    }

    /// The days left at `now`, rounded up: 30 on the day, 0 once due.
    pub fn days_left(&self, now: SystemTime) -> u32 {
        let left = self.purged_at().duration_since(now).unwrap_or_default();
        left.as_secs().div_ceil(24 * 60 * 60) as u32
    }
}

#[derive(Serialize, Deserialize)]
struct DeletedFile {
    schema: u32,
    deleted_unix_ms: u64,
    stack: StackId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    whole: bool,
    renders: Vec<DeletedRender>,
    failures: Vec<RenderId>,
    size: u64,
}

#[derive(Serialize, Deserialize)]
struct DeletedRender {
    id: RenderId,
    number: u32,
}

impl Versioned for DeletedFile {
    const SCHEMA: u32 = DELETED_SCHEMA;
}

impl DeletedFile {
    fn item(self, id: DeletedId) -> DeletedItem {
        let mut renders = self.renders;
        renders.sort_by_key(|r| std::cmp::Reverse(r.number));
        let took = match self.whole {
            true => Took::Stack { renders: renders.len() },
            false => {
                let mut numbers: Vec<u32> = renders.iter().map(|r| r.number).collect();
                numbers.sort_unstable();
                Took::Renders { numbers, failed: self.failures.len() }
            }
        };
        DeletedItem {
            id,
            deleted: from_unix_ms(self.deleted_unix_ms),
            stack: self.stack,
            name: self.name,
            took,
            size: self.size,
            renders: renders.into_iter().map(|r| r.id).collect(),
        }
    }
}

/// What to move into one record: one stack's.
pub(crate) struct Doomed<'a> {
    pub stack: &'a StackId,
    pub name: Option<String>,
    /// The stack's file goes too: the whole stack.
    pub whole: bool,
    pub renders: Vec<(RenderId, u32)>,
    pub failures: Vec<RenderId>,
}

impl History {
    /// Every record, newest first; an unreadable one is logged and skipped.
    pub fn deleted(&self) -> Result<Vec<DeletedItem>> {
        let mut items: Vec<DeletedItem> = self
            .records()?
            .into_iter()
            .filter_map(|(id, dir)| match read_versioned::<DeletedFile>(&dir.join(RECORD_FILE)) {
                Ok(file) => Some(file.item(id)),
                Err(e) => {
                    tracing::warn!("skipping deleted {}: {e}", dir.display());
                    None
                }
            })
            .collect();
        items.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(items)
    }

    /// A record's thumbnail: its newest render's, sRGB.
    pub fn deleted_thumbnail(&self, id: &DeletedId) -> Result<RgbImage> {
        let dir = self.deleted_path(id);
        let file: DeletedFile = read_versioned(&dir.join(RECORD_FILE))?;
        let newest = file.renders.iter().max_by_key(|r| r.number).ok_or_else(|| {
            crate::fsutil::StoreError::Invalid(format!("{} has no render", dir.display()))
        })?;
        let path = dir.join(RENDERS_DIR).join(newest.id.as_str()).join(THUMBNAIL_FILE);
        Ok(image::open(&path)?.to_rgb8())
    }

    /// Put records back whole, a whole stack's file merged with one made
    /// since. One already gone counts as restored. Stops at the first
    /// failure, leaving the rest in its record.
    pub fn restore(&self, ids: &[DeletedId]) -> Result<()> {
        for id in ids {
            let dir = self.deleted_path(id);
            if !dir.is_dir() {
                tracing::debug!("deleted {}: restored already", id.as_str());
                continue;
            }
            let stack = read_versioned::<DeletedFile>(&dir.join(RECORD_FILE)).ok().map(|f| f.stack);
            for entry in entries(&dir.join(RENDERS_DIR)) {
                let name = entry.file_name();
                let to = self.renders_dir().join(&name);
                if to.exists() {
                    return Err(already_there(&to));
                }
                rename(&entry.path(), &to)?;
            }
            let failed = self.root().join(FAILED_DIR);
            for entry in entries(&dir.join(FAILED_DIR)) {
                std::fs::create_dir_all(&failed).map_err(io_error(&failed))?;
                rename(&entry.path(), &failed.join(entry.file_name()))?;
            }
            for entry in entries(&dir.join(STACKS_DIR)) {
                let Some(stack) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.strip_suffix(".json"))
                    .and_then(StackId::parse)
                else {
                    continue;
                };
                self.restore_stack_file(&stack, &entry.path())?;
            }
            std::fs::remove_dir_all(&dir).map_err(io_error(&dir))?;
            tracing::info!(
                "restored {} from Recently Deleted (stack {})",
                id.as_str(),
                stack.as_ref().map_or("unknown", StackId::as_str)
            );
        }
        Ok(())
    }

    /// Empty Recently Deleted. Every record is tried; the first failure is
    /// returned.
    pub fn empty_deleted(&self) -> Result<()> {
        let records = self.records()?;
        let removed = self.remove_records(records.iter().map(|(id, dir)| (id, dir.as_path())));
        tracing::info!("emptied Recently Deleted: {} records", records.len());
        removed
    }

    /// Remove the records due at `now`, as the Library opens. An unreadable
    /// record is due by the time in its id.
    pub(crate) fn purge_deleted(&self, now: SystemTime) {
        let Ok(records) = self.records() else { return };
        let due: Vec<&(DeletedId, PathBuf)> = records
            .iter()
            .filter(|(id, dir)| {
                let deleted = read_versioned::<DeletedFile>(&dir.join(RECORD_FILE))
                    .ok()
                    .map(|f| from_unix_ms(f.deleted_unix_ms))
                    .or_else(|| id.made());
                deleted.is_some_and(|d| d + KEPT_FOR <= now)
            })
            .collect();
        if due.is_empty() {
            return;
        }
        tracing::info!("removing {} records of Recently Deleted kept {KEPT_DAYS} days", due.len());
        if let Err(e) = self.remove_records(due.iter().map(|(id, dir)| (id, dir.as_path()))) {
            tracing::warn!("couldn't remove part of Recently Deleted: {e}");
        }
    }

    /// Remove `records` for good, then forget their stacks left with
    /// nothing. All are tried; the first failure is returned.
    fn remove_records<'a>(
        &self,
        records: impl IntoIterator<Item = (&'a DeletedId, &'a Path)>,
    ) -> Result<()> {
        let mut stacks = BTreeSet::new();
        let mut first = None;
        for (id, dir) in records {
            if let Ok(file) = read_versioned::<DeletedFile>(&dir.join(RECORD_FILE)) {
                stacks.insert(file.stack);
            }
            tracing::debug!("deleted {}: removed for good", id.as_str());
            if let Err(e) = std::fs::remove_dir_all(dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("couldn't remove {}: {e}", dir.display());
                let path = dir.to_path_buf();
                first.get_or_insert(crate::fsutil::StoreError::Io { path, source: e });
            }
        }
        let stacks: Vec<StackId> = stacks.into_iter().collect();
        self.forget_if_unused(&stacks);
        first.map_or(Ok(()), Err)
    }

    /// Move `doomed` into a new record; `None` when nothing was there.
    /// `_closed` is its renders' paint closed first.
    pub(crate) fn put_away(
        &self,
        doomed: Doomed<'_>,
        _closed: &PaintClosed,
    ) -> Result<Option<DeletedItem>> {
        let render_dir = |id: &RenderId| self.render_path(id);
        let failure_path = |id: &RenderId| self.failure_path(id);
        let stack_file = self.stack_path(doomed.stack);
        let renders: Vec<&(RenderId, u32)> =
            doomed.renders.iter().filter(|(id, _)| render_dir(id).is_dir()).collect();
        let failures: Vec<&RenderId> =
            doomed.failures.iter().filter(|id| failure_path(id).is_file()).collect();
        let takes_file = doomed.whole && stack_file.is_file();
        if renders.is_empty() && failures.is_empty() && !takes_file {
            return Ok(None);
        }
        let size = renders.iter().map(|(id, _)| tree_size(&render_dir(id))).sum::<u64>()
            + failures.iter().map(|id| tree_size(&failure_path(id))).sum::<u64>();
        let id = DeletedId(new_id());
        let file = DeletedFile {
            schema: DELETED_SCHEMA,
            deleted_unix_ms: to_unix_ms(SystemTime::now()),
            stack: doomed.stack.clone(),
            name: doomed.name,
            whole: doomed.whole,
            renders: renders
                .iter()
                .map(|(id, number)| DeletedRender { id: id.clone(), number: *number })
                .collect(),
            failures: failures.iter().map(|id| (*id).clone()).collect(),
            size,
        };
        // The record first, whole, so what moves into it is never lost.
        let dir = self.deleted_path(&id);
        let partial = partial_of(&dir);
        std::fs::create_dir_all(&partial).map_err(io_error(&partial))?;
        write_json_atomic(&partial.join(RECORD_FILE), &file)?;
        rename(&partial, &dir)?;
        let item = file.item(id.clone());

        if !renders.is_empty() {
            let into = dir.join(RENDERS_DIR);
            std::fs::create_dir_all(&into).map_err(io_error(&into))?;
            for (render, _) in &renders {
                let from = render_dir(render);
                // A finished Retouch session's undo: nothing restores it.
                remove_if_present(&from.join(crate::paint::store::UNDO_FILE))?;
                rename(&from, &into.join(render.as_str()))?;
            }
        }
        if !failures.is_empty() {
            let into = dir.join(FAILED_DIR);
            std::fs::create_dir_all(&into).map_err(io_error(&into))?;
            for failure in &failures {
                rename(&failure_path(failure), &into.join(format!("{}.json", failure.as_str())))?;
            }
        }
        if takes_file {
            let into = dir.join(STACKS_DIR);
            std::fs::create_dir_all(&into).map_err(io_error(&into))?;
            let _one_at_a_time = crate::fsutil::one_edit_at_a_time();
            rename(&stack_file, &into.join(stack_file.file_name().unwrap_or_default()))?;
        }
        tracing::info!(
            "moved to Recently Deleted as {}: stack {}{}, {} renders, {} failed runs",
            id.as_str(),
            doomed.stack.as_str(),
            if doomed.whole { " whole" } else { "" },
            renders.len(),
            failures.len()
        );
        Ok(Some(item))
    }

    /// The highest number Recently Deleted holds of `stack`, which a save's
    /// number must pass.
    pub(crate) fn deleted_numbers_of(&self, stack: &StackId) -> u32 {
        let Ok(records) = self.records() else { return 0 };
        let mut highest = 0;
        for (_, dir) in records {
            let Ok(file) = read_versioned::<DeletedFile>(&dir.join(RECORD_FILE)) else { continue };
            if file.stack != *stack {
                continue;
            }
            highest = highest.max(file.renders.iter().map(|r| r.number).max().unwrap_or(0));
            if file.whole {
                let kept = dir.join(STACKS_DIR).join(format!("{}.json", stack.as_str()));
                highest = highest.max(self.last_number_in(&kept));
            }
        }
        highest
    }

    /// Whether a record in Recently Deleted holds anything of `stack`.
    pub(crate) fn deleted_holds(&self, stack: &StackId) -> bool {
        let Ok(records) = self.records() else { return false };
        records.iter().any(|(_, dir)| {
            read_versioned::<DeletedFile>(&dir.join(RECORD_FILE)).is_ok_and(|f| f.stack == *stack)
        })
    }

    /// The files in Recently Deleted that name roots. `None` when a folder
    /// can't be read.
    pub(crate) fn deleted_files(&self) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
        let mut renders = Vec::new();
        let mut files = Vec::new();
        for (_, dir) in self.records().ok()? {
            for entry in listing(&dir.join(RENDERS_DIR))? {
                if entry.file_name().to_str().is_some_and(is_id) {
                    renders.push(entry.path());
                }
            }
            for sub in [FAILED_DIR, STACKS_DIR] {
                files.extend(listing(&dir.join(sub))?.into_iter().map(|e| e.path()));
            }
        }
        Some((renders, files))
    }

    /// Every record's id and folder, unordered.
    fn records(&self) -> Result<Vec<(DeletedId, PathBuf)>> {
        let dir = self.deleted_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(crate::fsutil::StoreError::Io { path: dir, source: e }),
        };
        Ok(entries
            .flatten()
            .filter_map(|e| {
                let id = e.file_name().to_str().and_then(DeletedId::parse)?;
                e.path().is_dir().then(|| (id, e.path()))
            })
            .collect())
    }

    pub(crate) fn deleted_dir(&self) -> PathBuf {
        self.root().join(DELETED_DIR)
    }

    fn deleted_path(&self, id: &DeletedId) -> PathBuf {
        self.deleted_dir().join(id.as_str())
    }

    /// The `.partial` records a crash left, for the sweep; they hold nothing.
    pub(crate) fn deleted_partials(&self) -> Vec<PathBuf> {
        entries(&self.deleted_dir())
            .into_iter()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_suffix(PARTIAL_SUFFIX))
                    .is_some_and(is_id)
            })
            .map(|e| e.path())
            .collect()
    }

    /// The records' folders, for the sweep's temporaries.
    pub(crate) fn deleted_folders(&self) -> Vec<PathBuf> {
        self.records().unwrap_or_default().into_iter().map(|(_, dir)| dir).collect()
    }

    /// Group `renders` and `failures` by stack, one record each.
    pub(crate) fn doomed_by_stack<'a>(
        &self,
        renders: &'a [Render],
        failures: &'a [super::Failure],
    ) -> Vec<Doomed<'a>> {
        let stacks: BTreeSet<&StackId> =
            renders.iter().map(|r| &r.stack).chain(failures.iter().map(|f| &f.stack)).collect();
        stacks
            .into_iter()
            .map(|stack| Doomed {
                stack,
                name: self.name_of(stack, renders, failures),
                whole: false,
                renders: renders
                    .iter()
                    .filter(|r| r.stack == *stack)
                    .map(|r| (r.id.clone(), r.number))
                    .collect(),
                failures: failures
                    .iter()
                    .filter(|f| f.stack == *stack)
                    .map(|f| f.id.clone())
                    .collect(),
            })
            .collect()
    }

    /// What `stack` is called as it is deleted: its name, else its renders'
    /// folder, a failed run's label, or its kept working version's folder.
    pub(crate) fn name_of(
        &self,
        stack: &StackId,
        renders: &[Render],
        failures: &[super::Failure],
    ) -> Option<String> {
        let info = self.stacks().ok().and_then(|mut s| s.remove(stack));
        crate::library::stacks::stack_name(stack, info.as_ref(), renders)
            .or_else(|| failures.iter().find(|f| f.stack == *stack).map(|f| f.label.clone()))
            .or_else(|| info.as_ref().and_then(|i| i.unstacked_name()))
    }
}

/// The entries of `dir`; none when it can't be read or isn't there.
fn entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().collect()
}

/// The entries of `dir`: none when missing, `None` when unreadable.
fn listing(dir: &Path) -> Option<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.map(|e| e.ok()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

fn already_there(path: &Path) -> crate::fsutil::StoreError {
    crate::fsutil::StoreError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::from(std::io::ErrorKind::AlreadyExists),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::history::tests::{frame, id, put_render_in, store};
    use crate::library::history::{Batch, BatchLabel};
    use crate::library::stacks::{Settings, Working};
    use crate::{SessionOptions, StackerConfig};

    fn render_id(n: u8) -> RenderId {
        RenderId::parse(&id(n)).unwrap()
    }

    fn stack_id(n: u8) -> StackId {
        StackId::parse(&id(n)).unwrap()
    }

    /// Stack 10 ("Bee"): #1 and #2 (renders 1 and 2), and a failed run;
    /// stack 20: #1 (render 3).
    fn library() -> (tempfile::TempDir, History) {
        let (root, history) = store();
        let (bee, ant) = (id(10), id(20));
        put_render_in(&history, &id(1), &["/a/Bee/1.jpg"], None, Some((&bee, 1)));
        put_render_in(&history, &id(2), &["/a/Bee/2.jpg"], None, Some((&bee, 2)));
        put_render_in(&history, &id(3), &["/a/Ant/1.jpg"], None, Some((&ant, 1)));
        history.rename_stack(&stack_id(10), Some("Bee")).unwrap();
        let settings = Settings {
            config: StackerConfig::Pyramid(crate::PyramidConfig::default()),
            options: SessionOptions::default(),
        };
        let batch = Batch::new(BatchLabel::default());
        let frames = [frame(&history, "/a/Bee/3.jpg")];
        history.save_failure("Bee", &frames, &settings, &stack_id(10), &batch, "boom").unwrap();
        (root, history)
    }

    /// Every file under `dir` with its bytes, by path within it.
    fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        let mut folders = vec![dir.to_path_buf()];
        while let Some(at) = folders.pop() {
            for entry in std::fs::read_dir(&at).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if entry.file_name() != crate::library::history::lock::LOCK_FILE {
                    // The lock is held, and on Windows locked against reading.
                    let bytes = std::fs::read(&path).unwrap();
                    files.push((path.strip_prefix(dir).unwrap().to_path_buf(), bytes));
                }
            }
        }
        files.sort();
        files
    }

    /// The Library's files but `Deleted/`, the frames and the roots.
    fn listed_files(history: &History) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = tree(history.root());
        files.retain(|(p, _)| {
            !p.starts_with(DELETED_DIR)
                && !p.starts_with("a")
                && !p.to_string_lossy().contains("sources")
                && !p.to_string_lossy().contains("library")
        });
        files
    }

    /// A deleted render is moved, listed with its size, and restored byte
    /// for byte.
    #[test]
    fn a_render_deleted_is_restored_whole() {
        let (_root, history) = library();
        let before = listed_files(&history);
        let one = [render_id(2)];
        let deletion = history.delete_renders(&one, history.close_paint(&one)).unwrap();
        assert_eq!(deletion.0.len(), 1);
        let item = &deletion.0[0];
        assert_eq!(item.name.as_deref(), Some("Bee"));
        assert_eq!(item.took, Took::Renders { numbers: vec![2], failed: 0 });
        assert!(item.size > 0);
        assert_eq!(item.days_left(item.deleted), KEPT_DAYS as u32);

        let listed: Vec<RenderId> = history.list().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(listed, [render_id(1), render_id(3)], "the listing ignores Deleted/");
        assert_eq!(history.deleted().unwrap().len(), 1);
        assert_eq!(history.stacks().unwrap()[&stack_id(10)].name.as_deref(), Some("Bee"));
        let words = crate::words::Words::english(crate::Platform::Desktop);
        assert_eq!(
            crate::library::history::deleted_notice(&words, &deletion, "Untitled stack"),
            "Bee #2 moved to Recently Deleted."
        );

        history.restore(&deletion.ids()).unwrap();
        assert_eq!(listed_files(&history), before, "back byte for byte");
        assert!(history.deleted().unwrap().is_empty());
        history.restore(&deletion.ids()).unwrap();
    }

    /// A stack deleted whole is one record and restores as it was.
    #[test]
    fn a_stack_deleted_whole_is_one_record_restored_whole() {
        let (_root, history) = library();
        // Bee's working version kept while Ant is on screen.
        let renders = history.list().unwrap();
        let mut bee = Working::of_render(&history.render(&render_id(2)).unwrap());
        bee.list.add_files(vec![frame(&history, "/a/Bee/9.jpg")]);
        history.write_working(&bee).unwrap();
        history.switch_stack(&bee, &stack_id(20), &renders, None).unwrap().unwrap();
        let before = listed_files(&history);
        let failure = history.failures().unwrap()[0].id.clone();

        let doomed = [render_id(1), render_id(2)];
        let item = history
            .delete_stack(&stack_id(10), &doomed, &[failure], history.close_paint(&doomed))
            .unwrap()
            .unwrap();
        assert_eq!(item.took, Took::Stack { renders: 2 });
        assert_eq!(item.name.as_deref(), Some("Bee"));
        assert_eq!(item.renders, [render_id(2), render_id(1)], "newest first");
        assert!(history.failures().unwrap().is_empty());
        assert!(!history.stacks().unwrap().contains_key(&stack_id(10)));
        assert!(history.deleted_thumbnail(&item.id).is_err(), "the test's renders have none");

        history.restore(&[item.id]).unwrap();
        assert_eq!(listed_files(&history), before);
        assert_eq!(history.stacks().unwrap()[&stack_id(10)].working, Some(bee));
    }

    /// A damaged stack file in a record is set aside on restore, not merged.
    #[test]
    fn a_damaged_stack_file_restored_is_set_aside() {
        let (_root, history) = library();
        let doomed = [render_id(1), render_id(2)];
        let failure = history.failures().unwrap()[0].id.clone();
        let item = history
            .delete_stack(&stack_id(10), &doomed, &[failure], history.close_paint(&doomed))
            .unwrap()
            .unwrap();
        let name = format!("{}.json", stack_id(10).as_str());
        let deleted = history.root().join(DELETED_DIR);
        let (kept, _) = tree(&deleted).into_iter().find(|(p, _)| p.ends_with(&name)).unwrap();
        std::fs::write(deleted.join(kept), "not json").unwrap();
        history.rename_stack(&stack_id(10), Some("Wasp")).unwrap();

        history.restore(&[item.id]).unwrap();
        assert_eq!(history.stacks().unwrap()[&stack_id(10)].name.as_deref(), Some("Wasp"));
        let aside: Vec<_> = tree(&history.stacks_dir())
            .into_iter()
            .filter(|(p, _)| p.to_string_lossy().contains(".unreadable-"))
            .map(|(_, bytes)| bytes)
            .collect();
        assert_eq!(aside, [b"not json".to_vec()]);
        assert_eq!(history.stack_files_set_aside().len(), 1);
    }

    /// A delete reaching several stacks makes one record for each, and
    /// its Undo restores them all.
    #[test]
    fn a_delete_of_several_stacks_renders_is_a_record_each() {
        let (_root, history) = library();
        let doomed = [render_id(1), render_id(3), render_id(1)];
        let deletion = history.delete_renders(&doomed, history.close_paint(&doomed)).unwrap();
        let stacks: Vec<&StackId> = deletion.0.iter().map(|i| &i.stack).collect();
        assert_eq!(stacks, [&stack_id(10), &stack_id(20)]);
        let words = crate::words::Words::english(crate::Platform::Desktop);
        assert_eq!(
            crate::library::history::deleted_notice(&words, &deletion, "Untitled stack"),
            "2 renders moved to Recently Deleted."
        );
        history.restore(&deletion.ids()).unwrap();
        assert_eq!(history.list().unwrap().len(), 3);
    }

    /// A stack deleted whole and stacked again numbers past the record, and
    /// a restore merges its file.
    #[test]
    fn numbers_count_what_is_deleted() {
        let (_root, history) = library();
        let doomed = [render_id(1), render_id(2)];
        let item = history
            .delete_stack(&stack_id(10), &doomed, &[], history.close_paint(&doomed))
            .unwrap()
            .unwrap();
        assert_eq!(history.claim_number(&stack_id(10)).unwrap(), 3, "not #1 again");
        history.restore(&[item.id]).unwrap();
        let info = history.stacks().unwrap().remove(&stack_id(10)).unwrap();
        assert_eq!((info.name.as_deref(), info.last_number), (Some("Bee"), 3));
    }

    /// Records go after 30 days, at open, with stacks left with nothing.
    #[test]
    fn records_go_for_good_after_thirty_days() {
        let (_root, history) = library();
        let bee = [render_id(1), render_id(2)];
        let failure = history.failures().unwrap()[0].id.clone();
        let mut ids = bee.to_vec();
        ids.push(failure);
        let old = history.delete_renders(&ids, history.close_paint(&bee)).unwrap();
        let ant = [render_id(3)];
        history.delete_renders(&ant, history.close_paint(&ant)).unwrap();
        assert!(history.stacks().unwrap().contains_key(&stack_id(10)), "kept while deleted");

        // Bee's record as if made 30 days ago.
        let record = history.deleted_path(&old.0[0].id).join(RECORD_FILE);
        let mut file: DeletedFile = read_versioned(&record).unwrap();
        file.deleted_unix_ms -= KEPT_FOR.as_millis() as u64;
        write_json_atomic(&record, &file).unwrap();

        let now = SystemTime::now();
        history.purge_deleted(now - Duration::from_secs(60));
        assert_eq!(history.deleted().unwrap().len(), 2, "not due a minute ago");
        history.purge_deleted(now);
        let left = history.deleted().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].stack, stack_id(20));
        assert!(!history.stacks().unwrap().contains_key(&stack_id(10)), "forgotten");
        assert!(!history.deleted_dir().join(old.0[0].id.as_str()).exists());
    }

    /// The roots only something in Recently Deleted names are kept, so a
    /// restore finds its frames.
    #[test]
    fn what_is_deleted_keeps_its_roots() {
        let (root, history) = store();
        put_render_in(&history, &id(1), &["/kept/1.jpg"], None, None);
        let one = [render_id(1)];
        let deletion = history.delete_renders(&one, history.close_paint(&one)).unwrap();
        drop(history);
        let again = History::open(root.path()).unwrap();
        again.list().unwrap();
        assert_eq!(again.frames().sources().roots().count(), 1);
        again.restore(&deletion.ids()).unwrap();
        assert_eq!(again.list().unwrap()[0].frames, [frame(&again, "/kept/1.jpg")]);
    }

    /// Empty and Delete Library remove every record; a `.partial` is swept
    /// at open.
    #[test]
    fn emptying_and_delete_library_remove_every_record() {
        let (root, history) = library();
        let one = [render_id(1)];
        history.delete_renders(&one, history.close_paint(&one)).unwrap();
        history.empty_deleted().unwrap();
        assert!(history.deleted().unwrap().is_empty());
        assert_eq!(history.list().unwrap().len(), 2);

        let two = [render_id(2)];
        history.delete_renders(&two, history.close_paint(&two)).unwrap();
        history.delete_library(history.close_all_paint()).unwrap();
        assert!(history.deleted().unwrap().is_empty());
        assert!(history.list().unwrap().is_empty());

        let partial = history.deleted_dir().join(format!("{}{PARTIAL_SUFFIX}", id(9)));
        std::fs::create_dir_all(&partial).unwrap();
        drop(history);
        History::open(root.path()).unwrap();
        assert!(!partial.exists());
    }

    #[test]
    fn records_are_worded_by_what_they_took() {
        let words = crate::words::Words::english(crate::Platform::Desktop);
        let item = |took| DeletedItem {
            id: DeletedId(id(1)),
            deleted: SystemTime::now(),
            stack: stack_id(10),
            name: Some("Bee".into()),
            took,
            size: 4,
            renders: Vec::new(),
        };
        let title =
            |took| crate::library::history::deleted_title(&words, &item(took), "Untitled stack");
        assert_eq!(title(Took::Stack { renders: 3 }), "Bee");
        assert_eq!(title(Took::Renders { numbers: vec![4], failed: 0 }), "Bee #4");
        assert_eq!(title(Took::Renders { numbers: vec![1, 4], failed: 0 }), "2 renders of Bee");
        assert_eq!(title(Took::Renders { numbers: vec![], failed: 1 }), "Failed stack Bee");
        assert_eq!(
            title(Took::Renders { numbers: vec![2], failed: 1 }),
            "1 render and 1 failed run of Bee"
        );
        let mut untitled = item(Took::Stack { renders: 1 });
        untitled.name = None;
        let untitled = crate::library::history::deleted_title(&words, &untitled, "Untitled stack");
        assert_eq!(untitled, "Untitled stack");
        let stacks =
            Deletion(vec![item(Took::Stack { renders: 1 }), item(Took::Stack { renders: 2 })]);
        assert_eq!(
            crate::library::history::deleted_notice(&words, &stacks, "Untitled stack"),
            "2 stacks moved to Recently Deleted."
        );
        let now = SystemTime::now();
        let day = Duration::from_secs(24 * 60 * 60);
        let aged = |days: u32| DeletedItem {
            deleted: now - day * days,
            ..item(Took::Stack { renders: 1 })
        };
        assert_eq!(
            crate::library::history::deleted_days_left(&words, &aged(0), now),
            "30 days left"
        );
        assert_eq!(
            crate::library::history::deleted_days_left(&words, &aged(29), now),
            "1 day left"
        );
        assert_eq!(
            crate::library::history::deleted_days_left(&words, &aged(31), now),
            "1 day left"
        );
        assert_eq!(crate::library::history::deleted_summary(&words, 2, "4 MB"), "2 items · 4 MB");
        assert_eq!(
            crate::library::history::deleted_summary(&words, 0, "0 bytes"),
            "Nothing was deleted in the last 30 days."
        );
        let (title, message, button) =
            crate::library::history::empty_deleted_question(&words, 3, "1.2 GB");
        assert_eq!((title.as_str(), button.as_str()), ("Empty Recently Deleted?", "Empty"));
        assert_eq!(
            message,
            "The 3 items in Recently Deleted are deleted for good, freeing 1.2 GB. Your frames \
             and exported files are left alone. This can't be undone."
        );
    }
}
