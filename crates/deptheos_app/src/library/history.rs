//! The history: every render made, and the list the next one is made from.
//!
//! A render is a snapshot of its result and the list and settings it was
//! made from. Renders are numbered within a stack
//! ([`crate::library::stacks`]); a batch run's carry its id and label.
//!
//! On disk, under the store root:
//!
//! ```text
//! sources.json              the roots every frame is named from (crate::frames::sources,
//!                           ADR-145), and the Library's format: read first
//! sources.previous.json     a copy of sources.json as last written, opened from
//!                           when sources.json is damaged
//! list.json                 the working version on screen: stack?, base?, the list
//!                           (crate::frames::list::List) and the live settings
//! Stacks/<stack id>.json    a stack's name, the last number given to a render of it,
//!                           and its working version while another stack is on
//!                           screen (crate::library::stacks)
//! Renders/<render id>/
//!   render.json             schema, created_unix_ms, frames [{root, path, size,
//!                           fingerprint}], included,
//!                           batch {id, split, skipped}?, stack, number, name?, picked?, stacker,
//!                           options, alignment, crop, reversed, left_out?, result, color,
//!                           frame_map
//!   result.jxl              the result's RGB u16 samples, lossless
//!   frame_map.jxl           the frame map, 16-bit gray, lossless
//!   thumb.jpg               at most THUMBNAIL_SIDE px, sRGB
//!   color.dng               linear stacks only: the reference frame's color metadata
//!   profile.icc             a stack of standard images not in sRGB: its frames' ICC profile
//!   paint.sqlite            the render's paint, when it has any (paint::store)
//!   paint-undo.sqlite       while Retouch has painted it this session: the undo
//!                           snapshots spilled from RAM
//! Failed/<id>.json          a batch stack that failed: schema, created_unix_ms, label,
//!                           frames [{root, …}], batch {id, split, skipped}, stack, stacker,
//!                           options, error
//! Deleted/<id>/             Recently Deleted: what one delete took of one stack, kept
//!                           30 days in the Library's own layout (deleted.rs, ADR-140)
//! ```
//!
//! Ids are UUIDv7, so names sort by creation time. A render is built under
//! `<render id>.partial` and renamed into place, manifest last, so a crash
//! leaves an orphan [`History::open`] sweeps. Listing reads only manifests.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use deptheos_core::io;
use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::frames::list::List;
use crate::frames::sources::{FrameRef, Places, RootId};
pub use crate::fsutil::StoreError;
use crate::fsutil::{
    PARTIAL_SUFFIX, Versioned, edit_versioned, io_error, json_files_by_id, read_versioned,
    remove_if_present, rename, write_json_atomic,
};
pub use crate::library::ids::{RenderId, StackId};
use crate::library::ids::{is_id, new_id};
use crate::library::render_file::*;
pub use crate::library::render_file::{EncodedPictures, THUMBNAIL_SIDE};
use crate::library::stacks::Settings;
use crate::runs::session::{CompletedStack, SessionOptions, StackGeometry};
use crate::runs::stacker::StackerConfig;
use crate::words::Words;

pub type Result<T> = std::result::Result<T, StoreError>;

pub mod deleted;
pub(crate) mod frames;
mod lock;
mod strip;
mod synced;
mod words;

pub use deleted::{DeletedId, DeletedItem, Deletion, KEPT_DAYS, Took};
pub use frames::{Frames, Kept, Located, Moves, Reach, TopFolder};
pub use lock::Holder;
use lock::LibraryLock;
pub use strip::{StackTile, StripEntry, StripRun, strip};
pub use synced::{SyncService, sync_service};
pub use words::{
    delete_library_button, delete_library_message, delete_library_title, deleted_days_left,
    deleted_notice, deleted_summary, deleted_title, empty_deleted_failed, empty_deleted_question,
    library_delete_failed, library_deleted, render_name, render_spoken, render_with_summary,
    restore_failed,
};

const RENDERS_DIR: &str = "Renders";
const FAILED_DIR: &str = "Failed";
/// A failure file's own format version.
pub(crate) const FAILURE_SCHEMA: u32 = 1;

/// Why [`History::save`] kept no render.
#[derive(Debug)]
pub enum Unsaved {
    /// The Library refused it or couldn't write it.
    Failed(StoreError),
    /// The encode or the write panicked, with what it said.
    Panicked(String),
}

/// A batch run: its renders share its id and show under its label.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Batch {
    pub id: String,
    pub label: BatchLabel,
}

/// What a batch run's header is worded from when shown: how it split its
/// folders and how many frames it skipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BatchLabel {
    pub split: crate::runs::batch::Split,
    pub skipped: u64,
}

impl BatchLabel {
    /// The header as it reads in `words`: "Auto sort · 9 frames skipped".
    pub fn text(&self, words: &Words) -> String {
        crate::runs::batch::label(self.split, self.skipped as usize, words)
    }
}

impl Batch {
    /// A new run of `plan`.
    pub fn of_plan(plan: &crate::runs::batch::Plan) -> Self {
        Self::new(BatchLabel { split: plan.split, skipped: plan.skipped as u64 })
    }

    /// A new run with header `label`.
    pub fn new(label: BatchLabel) -> Self {
        Self { id: new_id(), label }
    }
}

impl From<BatchFile> for Batch {
    fn from(file: BatchFile) -> Self {
        let label = BatchLabel { split: file.split, skipped: file.skipped };
        Self { id: file.id, label }
    }
}

impl From<&Batch> for BatchFile {
    fn from(batch: &Batch) -> Self {
        Self { id: batch.id.clone(), split: batch.label.split, skipped: batch.label.skipped }
    }
}

/// A render as listed: its list and settings, without its pixels.
#[derive(Clone, Debug)]
pub struct Render {
    pub id: RenderId,
    pub created: SystemTime,
    /// The name the user gave it, if any.
    pub name: Option<String>,
    /// The stack it is part of.
    pub stack: StackId,
    /// Its place in its stack, from 1; numbers never shift.
    pub number: u32,
    pub config: StackerConfig,
    pub options: SessionOptions,
    /// Every frame listed when the stack ran, in list order.
    pub frames: Vec<PathBuf>,
    /// Which of [`Self::frames`] were stacked, parallel to them.
    pub included: Vec<bool>,
    /// The batch run that made it, if one did.
    pub batch: Option<Batch>,
    /// The user starred it.
    pub picked: bool,
    pub width: u32,
    pub height: u32,
    /// Stacked from camera RAW/DNG, in linear camera RGB.
    pub linear: bool,
    /// Included frames the stack could not stack, and why.
    pub left_out: Vec<LeftOutFrame>,
}

/// A frame a render was to stack but left out ([`crate::LeftOut`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftOutFrame {
    pub path: PathBuf,
    pub reason: String,
}

/// `left_out`, by index into the stack's `paths`, as paths.
fn left_out_frames(
    paths: &[PathBuf],
    left_out: impl IntoIterator<Item = (usize, String)>,
) -> Vec<LeftOutFrame> {
    left_out
        .into_iter()
        .filter_map(|(index, reason)| {
            Some(LeftOutFrame { path: paths.get(index)?.clone(), reason })
        })
        .collect()
}

impl Render {
    /// What the stack left out, for a notice ([`crate::runs::session::left_out_message`]).
    pub fn left_out_message(&self, words: &Words) -> Option<String> {
        crate::runs::session::left_out_message(
            words,
            self.left_out.iter().map(|l| (l.path.as_path(), l.reason.as_str())),
        )
    }

    /// The frames it stacked, in order: those included and not left out.
    pub fn stacked(&self) -> impl Iterator<Item = &PathBuf> {
        self.frames
            .iter()
            .zip(&self.included)
            .filter(|(f, i)| **i && !self.left_out.iter().any(|l| &l.path == *f))
            .map(|(f, _)| f)
    }

    /// The name to show: the user's, else [`Self::folder_name`].
    pub fn display_name(&self) -> Option<String> {
        self.name.clone().or_else(|| self.folder_name())
    }

    /// Its first stacked frame's folder, the stack's default name
    /// ([`crate::library::stacks::stack_name`]).
    pub(crate) fn folder_name(&self) -> Option<String> {
        self.stacked()
            .next()
            .or(self.frames.first())
            .and_then(|f| f.parent())
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    }

    /// [`matches_search`] over what it is called and its batch's label.
    #[cfg(test)]
    fn matches(&self, query: &str) -> bool {
        let words = Words::english(crate::Platform::Desktop);
        let batch = self.batch.as_ref().map(|b| b.label.text(&words));
        matches_search(&[&self.display_name().unwrap_or_default()], batch.as_deref(), query)
    }

    fn of(file: RenderFile, id: RenderId, frames: &Frames) -> Result<Self> {
        file.check()?;
        let linear = matches!(file.color, ColorFile::Linear);
        let listed = frames.paths(&file.frames)?;
        let paths = file.included_paths(&listed);
        let left_out =
            left_out_frames(&paths, file.left_out.into_iter().map(|l| (l.index, l.reason)));
        Ok(Self {
            stack: file.stack,
            number: file.number,
            id,
            created: from_unix_ms(file.created_unix_ms),
            name: file.name,
            config: file.stacker.config(),
            options: file.options.options(),
            frames: listed,
            included: file.included,
            batch: file.batch.map(Batch::from),
            picked: file.picked,
            width: file.result.width,
            height: file.result.height,
            linear,
            left_out,
        })
    }
}

/// Library search, for every front end: whether `query`, trimmed and
/// case-insensitive, is in one of `names` or the `batch` label. An empty
/// query matches everything.
pub fn matches_search(names: &[&str], batch: Option<&str>, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let hit = |s: &str| s.to_lowercase().contains(&query);
    names.iter().any(|n| hit(n)) || batch.is_some_and(hit)
}

/// A stack a batch run could not make, shown under the run's header to be
/// loaded, fixed and stacked again.
#[derive(Clone, Debug)]
pub struct Failure {
    pub id: RenderId,
    pub created: SystemTime,
    /// What the run called it ("Bee", "Bee · 3").
    pub label: String,
    /// The files it was to stack, in order, all included.
    pub frames: Vec<PathBuf>,
    pub config: StackerConfig,
    pub options: SessionOptions,
    pub batch: Batch,
    /// The stack it was to be: trying it again joins it.
    pub stack: StackId,
    /// Why it failed.
    pub error: String,
}

impl Failure {
    /// [`matches_search`] over its label and its batch's label.
    #[cfg(test)]
    fn matches(&self, query: &str) -> bool {
        let words = Words::english(crate::Platform::Desktop);
        matches_search(&[&self.label], Some(&self.batch.label.text(&words)), query)
    }

    /// The list to load to try it again: its frames, all included.
    pub fn list(&self) -> List {
        List::of_frames(&self.frames, self.options.order)
    }
}

#[derive(Serialize, Deserialize)]
struct FailureFile {
    schema: u32,
    created_unix_ms: u64,
    label: String,
    frames: Vec<FrameRef>,
    batch: BatchFile,
    stack: StackId,
    stacker: StackerFile,
    options: OptionsFile,
    error: String,
}

impl Versioned for FailureFile {
    const SCHEMA: u32 = FAILURE_SCHEMA;
}

impl FailureFile {
    fn failure(self, id: RenderId, frames: &Frames) -> Result<Failure> {
        Ok(Failure {
            stack: self.stack,
            id,
            created: from_unix_ms(self.created_unix_ms),
            label: self.label,
            frames: frames.paths(&self.frames)?,
            config: self.stacker.config(),
            options: self.options.options(),
            batch: self.batch.into(),
            error: self.error,
        })
    }
}

/// The history on disk. Clones share the root, the roots ([`Frames`]) and
/// the lock, which goes with the last clone.
#[derive(Clone, Debug)]
pub struct History {
    root: PathBuf,
    frames: Arc<Frames>,
    /// The running batch's records not stacked yet
    /// ([`Self::record_unstacked`]), hidden from [`Self::failures`] until
    /// their stack fails or the run ends. In memory only, so a run cut short
    /// shows them at the next open.
    unstacked: Arc<std::sync::Mutex<std::collections::HashSet<RenderId>>>,
    _lock: Arc<LibraryLock>,
}

/// Proof that the paint of renders about to be deleted was closed
/// ([`History::close_paint`], [`History::close_all_paint`]).
///
/// Separate from the delete because the close waits on the store thread:
/// a delete run there gets its close made before it is sent.
#[must_use = "a delete takes it"]
#[derive(Debug)]
pub struct PaintClosed(());

impl History {
    /// The per-user local data directory (`%LOCALAPPDATA%` on Windows, not
    /// the roaming profile: renders are large).
    pub fn default_root() -> Option<PathBuf> {
        dirs::data_local_dir().map(|dir| dir.join("Deptheos"))
    }

    /// Open the history at `root`, in order: refuse a synced folder
    /// ([`StoreError::Synced`]); make a missing Library only inside an
    /// existing folder ([`StoreError::NotConnected`]), but for the default
    /// one; take the lock ([`StoreError::InUse`], ADR-140); read
    /// `sources.json` and the format, refusing a newer one
    /// ([`StoreError::NewerLibrary`]) and opening a damaged one from its
    /// copy; sweep what a crash left; purge Recently Deleted records older
    /// than [`deleted::KEPT_DAYS`] days.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        Self::open_in(root, None)
    }

    /// [`Self::open`] for an app with storage of its own (iOS's container),
    /// which frames in it are kept relative to, since it moves.
    pub fn open_in(root: impl Into<PathBuf>, app: Option<PathBuf>) -> Result<Self> {
        crate::paint::store::check_sqlite()?;
        let root = root.into();
        if let Some(service) = sync_service(&root) {
            tracing::warn!("the Library at {} is synced by {}", root.display(), service.name());
            return Err(StoreError::Synced { library: root, service });
        }
        make_root(&root)?;
        let lock = Arc::new(LibraryLock::take(&root)?);
        // Checked before anything but the lock file is written.
        let frames = Frames::open(Places { library: root.clone(), app })?;
        lock.name_holder(&root);
        let renders = root.join(RENDERS_DIR);
        std::fs::create_dir_all(&renders).map_err(io_error(&renders))?;
        let history =
            Self { root, frames: Arc::new(frames), unstacked: Arc::default(), _lock: lock };
        history.sweep();
        history.purge_deleted(SystemTime::now());
        Ok(history)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The Library's roots, and the frames named from them.
    pub fn frames(&self) -> &Arc<Frames> {
        &self.frames
    }

    /// Every readable render, oldest first; an unreadable one is logged and
    /// skipped.
    pub fn list(&self) -> Result<Vec<Render>> {
        let dir = self.renders_dir();
        let mut renders = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io_error(&dir))?.flatten() {
            let Some(id) = entry.file_name().to_str().and_then(RenderId::parse) else { continue };
            match read_versioned::<RenderFile>(&entry.path().join(RENDER_FILE))
                .and_then(|file| Render::of(file, id, &self.frames))
            {
                Ok(render) => renders.push(render),
                Err(e) => tracing::warn!("skipping render {}: {e}", entry.path().display()),
            }
        }
        renders.sort_by(|a, b| a.id.cmp(&b.id));
        // Once a session, with every render read: the roots nothing uses.
        if let Err(e) = self.frames.prune(|| self.roots_named()) {
            tracing::warn!("the unused folders couldn't be forgotten: {e}");
        }
        Ok(renders)
    }

    /// The folders of the roots in use: those [`Self::roots_named`] gives
    /// and those named this session. What iOS's Delete Unused Copies keeps.
    /// `None` when a Library file couldn't be read.
    pub fn folders_in_use(&self) -> Option<Vec<PathBuf>> {
        let mut roots = self.roots_named()?;
        roots.extend(self.frames.roots_named_this_session());
        let mut folders: Vec<PathBuf> =
            roots.iter().filter_map(|id| self.frames.folder_of(id)).collect();
        folders.sort();
        folders.dedup();
        Some(folders)
    }

    /// Every root a Library file names, Recently Deleted's included, read
    /// as plain JSON so any version counts. `None` when one can't be read.
    fn roots_named(&self) -> Option<HashSet<RootId>> {
        let mut named = HashSet::new();
        let mut files = vec![self.root.join(crate::library::stacks::LIST_FILE)];
        for dir in [self.stacks_dir(), self.failed_dir()] {
            files.extend(
                json_files_by_id(&dir, |id| is_id(id).then_some(()))
                    .ok()?
                    .into_iter()
                    .map(|(_, p)| p),
            );
        }
        let mut renders = Vec::new();
        for entry in std::fs::read_dir(self.renders_dir()).ok()? {
            // Only renders, as the listing reads them: not a `.DS_Store`.
            let entry = entry.ok()?;
            if entry.file_name().to_str().and_then(RenderId::parse).is_some() {
                renders.push(entry.path());
            }
        }
        // Recently Deleted may be restored.
        let (deleted_renders, deleted_files) = self.deleted_files()?;
        files.extend(deleted_files);
        for dir in renders.into_iter().chain(deleted_renders) {
            files.push(dir.join(RENDER_FILE));
            named.extend(crate::paint::store::frame_roots(&dir).ok()?);
        }
        for file in files {
            let Some(bytes) = crate::fsutil::read_if_present(&file).ok()? else { continue };
            let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            roots_in(&json, &mut named);
        }
        Some(named)
    }

    /// One render as listed.
    pub fn render(&self, id: &RenderId) -> Result<Render> {
        let file = read_versioned::<RenderFile>(&self.render_path(id).join(RENDER_FILE))?;
        Render::of(file, id.clone(), &self.frames)
    }

    /// Save `stack` as the next render of stack `of` ([`Self::save_render`]):
    /// encoded on the pool as a write quit waits for, then written on the
    /// store thread in order (ADR-143). Never canceled: dropping the future
    /// only stops the wait.
    pub fn save(
        self: &Arc<Self>,
        frames: Vec<PathBuf>,
        stack: Arc<CompletedStack>,
        thumbnail: RgbImage,
        of: StackId,
        batch: Option<Batch>,
    ) -> impl std::future::Future<Output = std::result::Result<Render, Unsaved>> + Send + 'static
    {
        let (tx, rx) = futures::channel::oneshot::channel();
        let history = self.clone();
        // The job holds the token quit waits on until the write is queued.
        let save = move || match history.encode_render(&stack, &thumbnail) {
            Err(e) => {
                let _ = tx.send(Err(Unsaved::Failed(e)));
            }
            Ok(pictures) => {
                // Not awaited: its result goes to `tx`.
                let _written = crate::engine::store::store().write(move || {
                    let saved =
                        history.save_encoded(&frames, &stack, pictures, &of, batch.as_ref());
                    let _ = tx.send(saved.map_err(Unsaved::Failed));
                });
            }
        };
        crate::engine::work::pool()
            .run(
                crate::engine::work::Lane::Background,
                crate::engine::work::Key::kind("render-encode"),
                &crate::engine::work::must_finish(),
                save,
            )
            .detach();
        async move {
            rx.await.unwrap_or_else(|_| {
                Err(Unsaved::Panicked("the encode or the write panicked".into()))
            })
        }
    }

    /// Keep `stack` as the next render of stack `of`, synchronously (the
    /// store thread, tests). `frames` is every file listed, in order, the
    /// stack's paths among them in the same order; the rest are excluded.
    /// `thumbnail` is sRGB, at least [`THUMBNAIL_SIDE`] on its long side.
    pub fn save_render(
        &self,
        frames: &[PathBuf],
        stack: &CompletedStack,
        thumbnail: &RgbImage,
        of: &StackId,
        batch: Option<&Batch>,
    ) -> Result<Render> {
        let pictures = self.encode_render(stack, thumbnail)?;
        self.save_encoded(frames, stack, pictures, of, batch)
    }

    /// The pool half of [`Self::save`]: the pictures encoded into a
    /// `.partial` folder for [`Self::save_encoded`].
    pub fn encode_render(
        &self,
        stack: &CompletedStack,
        thumbnail: &RgbImage,
    ) -> Result<EncodedPictures> {
        let id = RenderId(new_id());
        let partial = partial_of(&self.render_path(&id));
        encode_pictures(id, partial, stack, thumbnail)
    }

    /// [`Self::save_render`] with its pictures encoded already.
    pub fn save_encoded(
        &self,
        frames: &[PathBuf],
        stack: &CompletedStack,
        pictures: EncodedPictures,
        of: &StackId,
        batch: Option<&Batch>,
    ) -> Result<Render> {
        let included = included_frames(frames, &stack.paths)?;
        if stack.geometry.transforms.len() != stack.paths.len() {
            return Err(StoreError::Invalid(format!(
                "the stack records alignment for {} frames but has {}",
                stack.geometry.transforms.len(),
                stack.paths.len()
            )));
        }
        let named = self.frames.refs(frames)?;
        let number = self.claim_number(of)?;
        let id = pictures.id.clone();
        let created = SystemTime::now();
        let dir = self.render_path(&id);
        let partial = &pictures.partial;
        let place = Place { stack: of.clone(), number, batch: batch.map(BatchFile::from) };
        // Failed, the folder goes with `pictures`.
        let built = write_render(partial, &named, &included, stack, created, place)
            .and_then(|file| rename(partial, &dir).map(|()| file));
        let file = built?;
        // A failed batch stack stacked again is not failed.
        if let Err(e) = self.delete_failures_of(of) {
            tracing::warn!("could not clear the failure of a stack now stacked: {e}");
        }
        Render::of(file, id, &self.frames)
    }

    /// Rebuild the stack a render was saved from: pixels, map, color
    /// science and geometry, with the paths of the frames it included.
    pub fn load_render(&self, id: &RenderId) -> Result<CompletedStack> {
        let (dir, file, paths) = self.read_render(id)?;
        load_stack(&dir, file, paths)
    }

    /// [`Self::load_render`] without decoding pixels or map, for a replay of
    /// its frames.
    pub fn load_settings(&self, id: &RenderId) -> Result<CompletedStack> {
        let (dir, file, paths) = self.read_render(id)?;
        load_settings(&dir, file, paths)
    }

    /// Which frames a render stacked and where each landed, without its
    /// pixels.
    pub fn load_geometry(&self, id: &RenderId) -> Result<(Vec<PathBuf>, StackGeometry)> {
        let (_, file, paths) = self.read_render(id)?;
        Ok((paths, file.geometry()))
    }

    fn read_render(&self, id: &RenderId) -> Result<(PathBuf, RenderFile, Vec<PathBuf>)> {
        let dir = self.render_path(id);
        let file: RenderFile = read_versioned(&dir.join(RENDER_FILE))?;
        file.check()?;
        let paths = file.included_paths(&self.frames.paths(&file.frames)?);
        Ok((dir, file, paths))
    }

    /// Name a render; `None` or blank removes the name.
    pub fn rename_render(&self, id: &RenderId, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        self.edit_manifest(id, |file| file.name = name.map(str::to_string))
    }

    /// Star a render, or take its star away.
    pub fn set_render_picked(&self, id: &RenderId, picked: bool) -> Result<()> {
        self.edit_manifest(id, |file| file.picked = picked)
    }

    /// Read-modify-write a render's manifest.
    fn edit_manifest(&self, id: &RenderId, edit: impl FnOnce(&mut RenderFile)) -> Result<()> {
        edit_versioned::<RenderFile>(&self.render_path(id).join(RENDER_FILE), edit)
    }

    /// A render's thumbnail, sRGB.
    pub fn load_thumbnail(&self, id: &RenderId) -> Result<RgbImage> {
        let path = self.render_path(id).join(THUMBNAIL_FILE);
        Ok(image::open(&path)?.to_rgb8())
    }

    /// Move renders and failed batch stacks into Recently Deleted
    /// ([`deleted`]), one record per stack, returned for Undo. One already
    /// gone counts as deleted; stops at the first stack that fails. Their
    /// stacks stay. `closed` is their paint closed first
    /// ([`Self::close_paint`]).
    pub fn delete_renders(&self, ids: &[RenderId], closed: PaintClosed) -> Result<Deletion> {
        let mut ids = ids.to_vec();
        ids.sort();
        ids.dedup();
        let renders: Vec<Render> = ids.iter().filter_map(|id| self.render(id).ok()).collect();
        let failures: Vec<Failure> =
            self.failures()?.into_iter().filter(|f| ids.contains(&f.id)).collect();
        let mut deletion = Deletion::default();
        let mut stopped = None;
        for doomed in self.doomed_by_stack(&renders, &failures) {
            match self.put_away(doomed, &closed) {
                Ok(made) => deletion.0.extend(made),
                Err(e) => {
                    stopped = Some(e);
                    break;
                }
            }
        }
        // Only those that went.
        let gone: Vec<Render> =
            renders.into_iter().filter(|r| !self.render_path(&r.id).exists()).collect();
        self.renders_deleted(&gone);
        stopped.map_or(Ok(deletion), Err)
    }

    /// Each render's size on disk in bytes, its whole folder; what can't be
    /// read counts as nothing.
    pub fn render_sizes(&self) -> HashMap<RenderId, u64> {
        let Ok(entries) = std::fs::read_dir(self.renders_dir()) else { return HashMap::new() };
        entries
            .flatten()
            .filter_map(|entry| {
                let id = entry.file_name().to_str().and_then(RenderId::parse)?;
                Some((id, tree_size(&entry.path())))
            })
            .collect()
    }

    /// Delete the whole Library for good, as a first launch finds it: every
    /// stack it lists, `list.json` and its set-aside copies, and Recently
    /// Deleted. What the listing can't read, others' files, the frames,
    /// exports and preferences stay. Every stack is tried; the first error
    /// is returned. `_closed` is every render's paint closed first
    /// ([`Self::close_all_paint`]).
    pub fn delete_library(&self, _closed: PaintClosed) -> Result<()> {
        let renders = self.list()?;
        let failures = self.failures()?;
        let named = self.stacks()?;
        let mut stacks: Vec<&StackId> = renders
            .iter()
            .map(|r| &r.stack)
            .chain(failures.iter().map(|f| &f.stack))
            .chain(named.keys())
            .collect();
        stacks.sort();
        stacks.dedup();
        let mut failed: Vec<StoreError> = Vec::new();
        let mut note = |removed: Result<()>| {
            if let Err(e) = removed {
                tracing::warn!("couldn't delete part of the Library: {e}");
                failed.push(e);
            }
        };
        for stack in stacks {
            let of = |s: &StackId| s == stack;
            let renders: Vec<RenderId> =
                renders.iter().filter(|r| of(&r.stack)).map(|r| r.id.clone()).collect();
            let failures: Vec<RenderId> =
                failures.iter().filter(|f| of(&f.stack)).map(|f| f.id.clone()).collect();
            // Closed above, with every render's.
            note(self.remove_stack(stack, &renders, &failures, PaintClosed(())));
        }
        for path in self.working_files().into_iter().chain(self.stack_files_set_aside()) {
            note(remove_if_present(&path));
        }
        note(self.empty_deleted());
        let first = failed.into_iter().next();
        match &first {
            None => tracing::info!("deleted the Library at {}", self.root.display()),
            Some(_) => {
                tracing::warn!("deleted only part of the Library at {}", self.root.display())
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Have the paint worker close the files of renders `ids` and wait: the
    /// first step of deleting them ([`PaintClosed`],
    /// [`crate::paint::worker::close`]). Never from the store thread, which
    /// the worker waits on: there it is logged and skipped.
    pub fn close_paint(&self, ids: &[RenderId]) -> PaintClosed {
        crate::paint::worker::close(&self.root, ids);
        PaintClosed(())
    }

    /// [`Self::close_paint`] for every render, for [`Self::delete_library`].
    pub fn close_all_paint(&self) -> PaintClosed {
        let renders = std::fs::read_dir(self.renders_dir()).into_iter().flatten().flatten();
        let ids: Vec<RenderId> =
            renders.filter_map(|e| e.file_name().to_str().and_then(RenderId::parse)).collect();
        self.close_paint(&ids)
    }

    pub(crate) fn delete_render_dirs(&self, ids: &[RenderId]) -> Result<()> {
        for id in ids {
            remove_render_dir(&self.render_path(id))?;
        }
        Ok(())
    }

    /// Keep a failed batch stack under its run; `of` is the stack it was to
    /// be. A record from [`Self::record_unstacked`] is written over and
    /// shown.
    pub fn save_failure(
        &self,
        label: &str,
        frames: &[PathBuf],
        settings: &Settings,
        of: &StackId,
        batch: &Batch,
        error: &str,
    ) -> Result<Failure> {
        let kept = self.read_failures()?.into_iter().find(|f| f.stack == *of).map(|f| f.id);
        let id = kept.unwrap_or_else(|| RenderId(new_id()));
        let failure = self.write_failure(&id, label, frames, settings, of, batch, error)?;
        crate::lock(&self.unstacked).remove(&id);
        Ok(failure)
    }

    /// Record each of `jobs` as a failed stack of `batch` saying `why` ("not
    /// stacked yet") as the run starts. Hidden from [`Self::failures`] until
    /// [`Self::save_failure`] or [`Self::batch_ended`]; a render saved into
    /// its stack removes one ([`Self::save_render`]).
    pub fn record_unstacked(
        &self,
        jobs: &[(&str, &[PathBuf], &StackId)],
        settings: &Settings,
        batch: &Batch,
        why: &str,
    ) -> Result<()> {
        for &(label, frames, of) in jobs {
            let id = RenderId(new_id());
            crate::lock(&self.unstacked).insert(id.clone());
            self.write_failure(&id, label, frames, settings, of, batch, why)?;
        }
        tracing::debug!("{} batch stacks recorded as not stacked yet", jobs.len());
        Ok(())
    }

    /// The running batch ended: its unstacked records show as failures.
    pub fn batch_ended(&self) {
        let mut unstacked = crate::lock(&self.unstacked);
        tracing::debug!("batch ended: {} stacks left not stacked yet", unstacked.len());
        unstacked.clear();
    }

    #[allow(clippy::too_many_arguments)]
    fn write_failure(
        &self,
        id: &RenderId,
        label: &str,
        frames: &[PathBuf],
        settings: &Settings,
        of: &StackId,
        batch: &Batch,
        error: &str,
    ) -> Result<Failure> {
        let dir = self.failed_dir();
        std::fs::create_dir_all(&dir).map_err(io_error(&dir))?;
        let id = id.clone();
        let file = FailureFile {
            schema: FAILURE_SCHEMA,
            created_unix_ms: to_unix_ms(SystemTime::now()),
            label: label.to_string(),
            frames: self.frames.refs(frames)?,
            batch: batch.into(),
            stack: of.clone(),
            stacker: StackerFile::from(&settings.config),
            options: OptionsFile::from(&settings.options),
            error: error.to_string(),
        };
        write_json_atomic(&self.failure_path(&id), &file)?;
        file.failure(id, &self.frames)
    }

    /// Every failed batch stack, oldest first, but the running batch's not
    /// stacked yet ([`Self::record_unstacked`]). Unreadable ones are skipped.
    pub fn failures(&self) -> Result<Vec<Failure>> {
        let mut failures = self.read_failures()?;
        let unstacked = crate::lock(&self.unstacked);
        failures.retain(|f| !unstacked.contains(&f.id));
        Ok(failures)
    }

    /// [`Self::failures`], those not stacked yet included.
    fn read_failures(&self) -> Result<Vec<Failure>> {
        let mut failures = Vec::new();
        for (id, path) in json_files_by_id(&self.failed_dir(), RenderId::parse)? {
            match read_versioned::<FailureFile>(&path).and_then(|f| f.failure(id, &self.frames)) {
                Ok(failure) => failures.push(failure),
                Err(e) => tracing::warn!("skipping failure {}: {e}", path.display()),
            }
        }
        failures.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(failures)
    }

    /// Remove the failures recorded for `stack`: it has a render now.
    pub(crate) fn delete_failures_of(&self, stack: &StackId) -> Result<()> {
        let ids: Vec<RenderId> =
            self.read_failures()?.into_iter().filter(|f| f.stack == *stack).map(|f| f.id).collect();
        self.delete_failures(&ids)
    }

    /// Remove failures for good. One already gone counts as removed.
    pub(crate) fn delete_failures(&self, ids: &[RenderId]) -> Result<()> {
        for id in ids {
            remove_if_present(&self.failure_path(id))?;
        }
        Ok(())
    }

    /// The folder `old` is at `new`: the root covering it points there
    /// ([`Frames::relink`]); files name frames by root, so nothing else is
    /// written. Returns what moved.
    pub fn relink_folder(&self, old: &Path, new: &Path) -> Result<Moves> {
        Ok(self.frames.relink(old, new, None)?)
    }

    /// Locate…: [`Self::relink_folder`], with how many frames the folder
    /// picked has ([`Frames::locate`]).
    pub fn locate_folder(&self, old: &Path, new: &Path) -> Result<Located> {
        Ok(self.frames.locate(old, new, None)?)
    }

    /// A render's directory, where its paint lives.
    pub fn render_path(&self, id: &RenderId) -> PathBuf {
        self.renders_dir().join(id.as_str())
    }

    pub(crate) fn renders_dir(&self) -> PathBuf {
        self.root.join(RENDERS_DIR)
    }

    fn failed_dir(&self) -> PathBuf {
        self.root.join(FAILED_DIR)
    }

    fn failure_path(&self, id: &RenderId) -> PathBuf {
        self.failed_dir().join(format!("{}.json", id.as_str()))
    }

    /// Remove what a crash left: `.partial` render and Recently Deleted
    /// folders, temporary files ([`io::is_temporary`]) and undo spills. Run
    /// at open by the lock's holder, the only writer, so anything there is
    /// stale whatever its age.
    fn sweep(&self) {
        let mut folders = vec![self.root.clone(), self.stacks_dir(), self.failed_dir()];
        for partial in self.deleted_partials() {
            tracing::debug!("deleting a record a crashed delete left: {}", partial.display());
            let _ = std::fs::remove_dir_all(&partial);
        }
        folders.extend(self.deleted_folders());
        let renders = std::fs::read_dir(self.renders_dir()).into_iter().flatten().flatten();
        for entry in renders {
            let (path, name) = (entry.path(), entry.file_name());
            let Some(name) = name.to_str() else { continue };
            if name.strip_suffix(PARTIAL_SUFFIX).is_some_and(is_id) && path.is_dir() {
                tracing::debug!("deleting what a crashed save left: {}", path.display());
                let _ = std::fs::remove_dir_all(&path);
            } else if is_id(name) {
                let undo = path.join(crate::paint::store::UNDO_FILE);
                if undo.exists() {
                    tracing::debug!("deleting the undo snapshots a crash left: {}", undo.display());
                    let _ = std::fs::remove_file(&undo);
                }
                folders.push(path);
            }
        }
        for folder in folders {
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if entry.file_name().to_str().is_some_and(io::is_temporary) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}

/// Make the Library folder `root` inside an existing folder (anywhere for
/// [`History::default_root`]).
fn make_root(root: &Path) -> Result<()> {
    match std::fs::metadata(root) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(StoreError::Io { path: root.to_path_buf(), source }),
    }
    if Some(root) == History::default_root().as_deref() {
        return std::fs::create_dir_all(root).map_err(io_error(root));
    }
    if !root.parent().is_some_and(Path::is_dir) {
        tracing::warn!("the Library at {} isn't there, nor its folder", root.display());
        return Err(StoreError::NotConnected { library: root.to_path_buf() });
    }
    tracing::info!("a new Library at {}", root.display());
    std::fs::create_dir(root).map_err(io_error(root))
}

/// Every root named in `json`: the value of each `root` field.
fn roots_in(json: &serde_json::Value, into: &mut HashSet<RootId>) {
    match json {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                match value.as_str().and_then(RootId::parse) {
                    Some(root) if key == "root" => {
                        into.insert(root);
                    }
                    _ => roots_in(value, into),
                }
            }
        }
        serde_json::Value::Array(values) => values.iter().for_each(|v| roots_in(v, into)),
        _ => {}
    }
}

/// `path`'s size on disk in bytes, a folder's whole; links count as
/// themselves, unreadable entries as nothing.
fn tree_size(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::read_dir(path)
            .map(|entries| entries.flatten().map(|entry| tree_size(&entry.path())).sum())
            .unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

/// Remove a render's folder, renamed to `.partial` first so a half-removed
/// one is never listed. One already gone counts as removed.
fn remove_render_dir(dir: &Path) -> Result<()> {
    let partial = partial_of(dir);
    let doomed = match std::fs::rename(dir, &partial) {
        Ok(()) => partial.as_path(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => dir,
    };
    std::fs::remove_dir_all(doomed).map_err(io_error(doomed))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::runs::batch::Split;

    pub(crate) fn store() -> (tempfile::TempDir, History) {
        let root = tempfile::tempdir().unwrap();
        let history = History::open(root.path()).unwrap();
        (root, history)
    }

    /// A file under the store standing for frame `path`, made if missing.
    pub(crate) fn frame(history: &History, path: &str) -> PathBuf {
        let file = match Path::new(path).starts_with(history.root()) {
            true => PathBuf::from(path),
            false => history.root().join(path.trim_start_matches('/')),
        };
        if !file.exists() {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, path.as_bytes()).unwrap();
        }
        file
    }

    /// A current manifest for a render of `frames`, all included, as #1 of
    /// its own stack.
    fn put_render(history: &History, id: &str, frames: &[&str], batch: Option<(&str, Split)>) {
        put_render_in(history, id, frames, batch, None);
    }

    /// [`put_render`], as render `number` of stack `stack` when given.
    pub(crate) fn put_render_in(
        history: &History,
        id: &str,
        frames: &[&str],
        batch: Option<(&str, Split)>,
        stack: Option<(&str, u32)>,
    ) {
        let (stack, number) = stack.unwrap_or((id, 1));
        let dir = history.renders_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<PathBuf> = frames.iter().map(|f| frame(history, f)).collect();
        let frames_json = history.frames().refs(&paths).unwrap();
        let mut manifest = serde_json::json!({
            "schema": RENDER_SCHEMA, "created_unix_ms": 5,
            "frames": frames_json, "included": vec![true; frames.len()],
            "stacker": {"algorithm": "pyramid", "smoothness": 0.5},
            "options": {
                "align": true, "align_mode": "grid", "crop": true, "reverse_check": true,
                "raw_backend": "libraw", "camera_profile": "adobe_standard", "order": "name",
                "grid": {
                    "horizontal_pct": 5.0, "vertical_pct": 5.0, "rotation_deg": 1.0,
                    "magnification_pct": 5.0, "apply_brightness": false,
                },
            },
            "alignment": vec![serde_json::Value::Null; frames.len()],
            "crop": null, "reversed": false,
            "result": {"width": 4, "height": 3},
            "color": {"kind": "srgb", "sixteen_bit": false, "profile": false},
            "frame_map": false, "stack": stack, "number": number,
        });
        if let Some((id, split)) = batch {
            manifest["batch"] = serde_json::json!({ "id": id, "split": split, "skipped": 0 });
        }
        std::fs::write(dir.join(RENDER_FILE), manifest.to_string()).unwrap();
        // Numbered as a save numbers it: its stack's file has it.
        if let Some(stack) = StackId::parse(stack) {
            history.note_number(&stack, number);
        }
    }

    pub(crate) fn id(n: u8) -> String {
        format!("{n:032x}")
    }

    #[test]
    fn renders_list_oldest_first_with_their_batch() {
        let (_root, history) = store();
        put_render(&history, &id(2), &["/a/Bee/2.jpg"], Some(("run", Split::AutoSort)));
        put_render(&history, &id(1), &["/a/Bee/1.jpg"], None);
        std::fs::create_dir_all(history.renders_dir().join("notes")).unwrap();

        let renders = history.list().unwrap();
        let ids: Vec<&str> = renders.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [id(1), id(2)]);
        assert_eq!(renders[0].batch, None);
        assert_eq!(
            renders[1].batch,
            Some(Batch {
                id: "run".into(),
                label: crate::library::history::BatchLabel {
                    split: crate::runs::batch::Split::AutoSort,
                    skipped: 0
                }
            })
        );
        assert_eq!(renders[1].display_name().as_deref(), Some("Bee"));
        // Each its own stack, as #1.
        assert_eq!(renders[0].stack.as_str(), id(1));
        assert_eq!((renders[1].stack.as_str(), renders[1].number), (id(2).as_str(), 1));
    }

    /// A render of another schema is skipped and left as it was.
    #[test]
    fn renders_of_another_schema_are_skipped_and_left() {
        let (_root, history) = store();
        put_render(&history, &id(1), &["/a/Bee/1.jpg"], None);
        put_render(&history, &id(2), &["/a/Bee/2.jpg"], None);
        put_render(&history, &id(3), &["/a/Bee/3.jpg"], None);
        let file = |n| history.renders_dir().join(id(n)).join(RENDER_FILE);
        let set_schema = |n, schema: u32| {
            let mut json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(file(n)).unwrap()).unwrap();
            json["schema"] = schema.into();
            std::fs::write(file(n), json.to_string()).unwrap();
        };
        set_schema(2, RENDER_SCHEMA + 1);
        set_schema(3, RENDER_SCHEMA - 1);
        let before = std::fs::read(file(2)).unwrap();

        let renders = history.list().unwrap();
        let ids: Vec<&str> = renders.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [id(1)]);
        assert_eq!(std::fs::read(file(2)).unwrap(), before, "left as it was");
    }

    #[test]
    fn rename_pick_and_delete() {
        let (_root, history) = store();
        put_render(&history, &id(1), &["/a/Bee/1.jpg"], None);
        let render = RenderId::parse(&id(1)).unwrap();
        history.rename_render(&render, Some("  Beetle  ")).unwrap();
        history.set_render_picked(&render, true).unwrap();
        let listed = history.render(&render).unwrap();
        assert_eq!((listed.name.as_deref(), listed.picked), (Some("Beetle"), true));
        let one = [render];
        history.delete_renders(&one, history.close_paint(&one)).unwrap();
        assert!(history.list().unwrap().is_empty());
    }

    /// Renaming and starring change only those fields; another schema is
    /// refused.
    #[test]
    fn rename_and_star_change_only_their_fields() {
        let (_root, history) = store();
        put_render(&history, &id(1), &["/a/Bee/1.jpg"], None);
        let render = RenderId::parse(&id(1)).unwrap();
        let file = history.renders_dir().join(id(1)).join(RENDER_FILE);
        let json = || -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap()
        };
        let before = json();
        history.rename_render(&render, Some(" Beetle ")).unwrap();
        history.set_render_picked(&render, true).unwrap();
        let mut named = before.clone();
        named["name"] = "Beetle".into();
        named["picked"] = true.into();
        assert_eq!(json(), named);
        history.rename_render(&render, None).unwrap();
        history.set_render_picked(&render, false).unwrap();
        assert_eq!(json(), before);

        let mut newer = before.clone();
        newer["schema"] = (RENDER_SCHEMA + 1).into();
        std::fs::write(&file, newer.to_string()).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        assert!(history.rename_render(&render, Some("Wasp")).is_err());
        assert!(history.set_render_picked(&render, true).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "left as it was");
    }

    #[test]
    fn search_matches_names_and_batches_not_folders() {
        let (_root, history) = store();
        put_render(&history, &id(1), &["/shoots/Spider/1.jpg"], Some(("r", Split::PerFolder)));
        let render = &history.list().unwrap()[0];
        assert!(render.matches("spid"));
        assert!(!render.matches("shoots"));
        assert!(render.matches("per folder"));
        assert!(render.matches("  "));
        assert!(!render.matches("beetle"));
    }

    #[test]
    fn search_is_trimmed_any_case_and_names_only() {
        let names = ["Spider", "Red eyes"];
        assert!(matches_search(&names, None, "  red EYES "));
        assert!(matches_search(&names, Some("Auto sort"), "sort"));
        assert!(!matches_search(&names, None, "#2"));
        assert!(matches_search(&[], None, ""));
        assert!(!matches_search(&[], None, "spider"));
    }

    /// A relinked folder moves every frame under it, at any depth, and
    /// writes only `sources.json`.
    #[test]
    fn relinking_a_folder_moves_the_frames_of_every_render() {
        let (root, history) = store();
        let frames = ["/old/Bee/1.jpg", "/old/Bee/deep/2.jpg"];
        put_render(&history, &id(1), &frames, None);
        put_render(&history, &id(2), &["/other/3.jpg"], None);
        // A broken render doesn't stop the others.
        let broken = history.renders_dir().join(id(0));
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(RENDER_FILE), b"{").unwrap();
        let manifest = history.render_path(&RenderId::parse(&id(1)).unwrap()).join(RENDER_FILE);
        let written = std::fs::read(&manifest).unwrap();

        let (old, new) = (root.path().join("old"), root.path().join("new"));
        std::fs::rename(&old, &new).unwrap();
        let moves = history.relink_folder(&old.join("Bee"), &new.join("Bee")).unwrap();
        assert_eq!(moves.path(&old.join("Bee/1.jpg")), Some(new.join("Bee/1.jpg")));
        let renders = history.list().unwrap();
        assert_eq!(renders[0].frames, [new.join("Bee/1.jpg"), new.join("Bee/deep/2.jpg")]);
        assert_eq!(renders[1].frames, [frame(&history, "/other/3.jpg")]);
        assert_eq!(std::fs::read(&manifest).unwrap(), written, "the render is as it was");
        // Read afresh: a folder in the Library stays relative to it.
        drop(history);
        let again = History::open(root.path()).unwrap();
        assert_eq!(again.list().unwrap()[0].frames[0], new.join("Bee/1.jpg"));
    }

    /// One app at a time: a second open is refused until the first one's
    /// last clone goes.
    #[test]
    fn a_library_opens_in_one_app_at_a_time() {
        let (root, history) = store();
        let clone = history.clone();
        drop(history);
        let Err(refused) = History::open(root.path()) else { panic!("opened twice") };
        assert!(matches!(refused, StoreError::InUse { .. }), "{refused:?}");
        let words = crate::words::Words::english(crate::Platform::Desktop);
        let said = crate::words::explain::Explain::explain(&refused, &words);
        assert!(said.reason.contains("open in another"), "{said:?}");
        drop(clone);
        History::open(root.path()).expect("free once the first has gone");
    }

    /// A Library whose parent folder is missing isn't made (a drive to
    /// connect); one in an existing folder is.
    #[test]
    fn a_library_on_a_drive_not_connected_is_not_made() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("Volumes/Photos/Deptheos");
        let Err(refused) = History::open(&gone) else { panic!("opened") };
        assert!(matches!(refused, StoreError::NotConnected { .. }), "{refused:?}");
        assert!(!dir.path().join("Volumes").exists(), "nothing made in its place");
        let words = crate::words::Words::english(crate::Platform::Desktop);
        let said = crate::words::explain::Explain::explain(&refused, &words);
        assert!(said.reason.contains("isn't connected"), "{said:?}");
        assert_eq!(said.cause, crate::words::explain::Cause::NotFound);

        let new = dir.path().join("Deptheos");
        History::open(&new).unwrap();
        assert!(new.join(RENDERS_DIR).is_dir());
    }

    /// A Library in a synced folder is refused, nothing made.
    #[test]
    fn a_library_in_a_synced_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".dropbox"), "{}").unwrap();
        let library = dir.path().join("Deptheos");
        let Err(refused) = History::open(&library) else { panic!("opened") };
        assert!(
            matches!(&refused, StoreError::Synced { service: SyncService::Dropbox, .. }),
            "{refused:?}"
        );
        assert!(!library.exists());
        let words = crate::words::Words::english(crate::Platform::Desktop);
        let said = crate::words::explain::Explain::explain(&refused, &words);
        assert!(said.reason.contains("Dropbox"), "{said:?}");
        assert_eq!(said.advice, "Choose a folder that isn't synced.");
    }

    /// Roots no file names go once a new session lists the Library.
    #[test]
    fn a_folder_no_file_names_is_forgotten_once_listed() {
        let (root, history) = store();
        put_render(&history, &id(1), &["/kept/1.jpg"], None);
        history.frames().refs(&[frame(&history, "/dropped/2.jpg")]).unwrap();
        assert_eq!(history.frames().sources().roots().count(), 2);
        drop(history);
        let again = History::open(root.path()).unwrap();
        again.list().unwrap();
        assert_eq!(again.frames().sources().roots().count(), 1);
        assert_eq!(again.list().unwrap()[0].frames, [frame(&again, "/kept/1.jpg")]);
    }

    /// A non-render file in `Renders/` (`.DS_Store`) doesn't keep unused
    /// roots.
    #[test]
    fn a_stray_file_among_the_renders_still_lets_roots_go() {
        let (root, history) = store();
        put_render(&history, &id(1), &["/kept/1.jpg"], None);
        history.frames().refs(&[frame(&history, "/dropped/2.jpg")]).unwrap();
        std::fs::write(history.renders_dir().join(".DS_Store"), b"\0\0\0\x01Bud1").unwrap();
        drop(history);
        let again = History::open(root.path()).unwrap();
        again.list().unwrap();
        assert_eq!(again.frames().sources().roots().count(), 1);
    }

    /// The folders in use are those named by renders, Recently Deleted and
    /// this session; an unreadable file gives no answer.
    #[test]
    fn the_folders_in_use_count_recently_deleted_and_refuse_when_unread() {
        let (root, history) = store();
        put_render(&history, &id(1), &["/kept/1.jpg"], None);
        put_render(&history, &id(2), &["/deleted/2.jpg"], None);
        let two = [RenderId::parse(&id(2)).unwrap()];
        history.delete_renders(&two, history.close_paint(&two)).unwrap();
        // A folder that was a root and no file names any more.
        put_render(&history, &id(3), &["/gone/3.jpg"], None);
        std::fs::remove_dir_all(history.render_path(&RenderId::parse(&id(3)).unwrap())).unwrap();
        drop(history);

        let history = History::open(root.path()).unwrap();
        history.frames().refs(&[frame(&history, "/listed/4.jpg")]).unwrap();
        let folder = |name: &str| history.root().join(name);
        let mut want = ["deleted", "kept", "listed"].map(folder).to_vec();
        want.sort();
        assert_eq!(history.folders_in_use().unwrap(), want);

        std::fs::write(history.stacks_dir().join(format!("{}.json", id(9))), b"{ not json")
            .unwrap();
        assert_eq!(history.folders_in_use(), None);
    }

    /// A damaged `sources.json` with no copy refuses the Library, untouched;
    /// one damaged mid-session is overwritten by the next change.
    #[test]
    fn a_damaged_sources_file_is_refused() {
        let (root, history) = store();
        put_render(&history, &id(1), &["/kept/1.jpg"], None);
        let file = root.path().join(crate::frames::sources::SOURCES_FILE);
        let manifest = history.render_path(&RenderId::parse(&id(1)).unwrap()).join(RENDER_FILE);
        let written = std::fs::read(&manifest).unwrap();

        // Mid-session: the next change starts from the roots held.
        std::fs::write(&file, b"{ not json").unwrap();
        history.frames().refs(&[frame(&history, "/added/2.jpg")]).unwrap();
        assert_eq!(history.frames().sources().roots().count(), 2);
        drop(history);
        assert!(History::open(root.path()).is_ok(), "written whole again");

        // At open, with no copy: refused, both files byte for byte.
        std::fs::write(&file, b"{ not json").unwrap();
        std::fs::remove_file(root.path().join(crate::frames::sources::PREVIOUS_SOURCES_FILE))
            .unwrap();
        let Err(refused) = History::open(root.path()) else { panic!("opened") };
        assert!(
            matches!(&refused, StoreError::Sources(e) if matches!(**e, crate::frames::sources::SourcesError::Damaged { .. })),
            "{refused:?}"
        );
        let said = crate::words::explain::Explain::explain(
            &refused,
            &crate::words::Words::english(crate::Platform::Desktop),
        );
        assert!(said.reason.contains("sources.json"), "{said:?}");
        assert!(said.reason.contains("untouched"), "{said:?}");
        assert!(said.advice.contains("backup"), "{said:?}");
        assert_eq!(std::fs::read(&file).unwrap(), b"{ not json");
        assert_eq!(std::fs::read(&manifest).unwrap(), written);
        let mut names: Vec<_> = std::fs::read_dir(root.path()).unwrap().flatten().collect();
        names.retain(|e| e.file_name().to_string_lossy().contains("unreadable"));
        assert!(names.is_empty(), "nothing set aside");
    }

    /// A damaged `sources.json` is set aside and the Library opens from its
    /// copy, written back.
    #[test]
    fn a_damaged_sources_file_opens_from_the_one_before() {
        let (root, history) = store();
        put_render(&history, &id(1), &["/kept/1.jpg"], None);
        let frames = history.list().unwrap().remove(0).frames;
        history.frames().refs(&[frame(&history, "/added/2.jpg")]).unwrap();
        drop(history);
        let file = root.path().join(crate::frames::sources::SOURCES_FILE);
        let previous = root.path().join(crate::frames::sources::PREVIOUS_SOURCES_FILE);
        std::fs::write(&file, b"{ not json").unwrap();

        let history = History::open(root.path()).unwrap();
        assert_eq!(history.frames().sources().roots().count(), 2, "every root");
        let render = history.list().unwrap().remove(0);
        assert_eq!(render.frames, frames);
        let read: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(read["schema"], crate::frames::sources::SOURCES_SCHEMA, "written back whole");
        let aside: Vec<_> = std::fs::read_dir(root.path()).unwrap().flatten().collect();
        let aside: Vec<_> = aside
            .iter()
            .filter(|e| e.file_name().to_string_lossy().contains("unreadable"))
            .collect();
        assert_eq!(aside.len(), 1, "the damaged one set aside");
        assert_eq!(std::fs::read(aside[0].path()).unwrap(), b"{ not json");
        assert_eq!(std::fs::read(&previous).unwrap(), std::fs::read(&file).unwrap());

        // Both damaged: refused, as with no copy.
        drop(history);
        std::fs::write(&file, b"{ not json").unwrap();
        std::fs::write(&previous, b"{ not json either").unwrap();
        assert!(matches!(History::open(root.path()), Err(StoreError::Sources(_))));
        assert_eq!(std::fs::read(&file).unwrap(), b"{ not json", "nothing moved");
    }

    /// A render naming frames by path is skipped and left as it is.
    #[test]
    fn an_unreadable_render_is_skipped() {
        let (_root, history) = store();
        let dir = history.renders_dir().join(id(1));
        put_render(&history, &id(1), &["/a/1.jpg"], None);
        let file = dir.join(RENDER_FILE);
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        json["frames"] = serde_json::json!([{"path": "/a/1.jpg"}]);
        std::fs::write(&file, json.to_string()).unwrap();
        assert!(history.list().unwrap().is_empty());
        assert!(file.exists());
    }

    #[test]
    fn failures_round_trip_relink_and_delete() {
        let (root, history) = store();
        assert!(history.failures().unwrap().is_empty());
        let (old, new) = (root.path().join("old"), root.path().join("new"));
        let batch =
            Batch::new(BatchLabel { split: crate::runs::batch::Split::AutoSort, skipped: 0 });
        let settings = Settings {
            config: StackerConfig::Pyramid(crate::PyramidConfig::default()),
            options: SessionOptions::default(),
        };
        let frames = [frame(&history, "/old/Bee/1.jpg"), frame(&history, "/old/Bee/2.jpg")];
        let saved = history
            .save_failure("Bee", &frames, &settings, &StackId::new(), &batch, "boom")
            .unwrap();

        let listed = history.failures().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, saved.id);
        assert_eq!((listed[0].label.as_str(), listed[0].error.as_str()), ("Bee", "boom"));
        assert_eq!(listed[0].batch, batch);
        assert!(listed[0].matches("bee") && listed[0].matches("auto"));
        assert_eq!(listed[0].list().files, frames);

        std::fs::rename(&old, &new).unwrap();
        history.relink_folder(&old.join("Bee"), &new.join("Bee")).unwrap();
        assert_eq!(
            history.failures().unwrap()[0].frames,
            [new.join("Bee/1.jpg"), new.join("Bee/2.jpg")]
        );

        history.delete_failures(&[saved.id.clone(), saved.id]).unwrap();
        assert!(history.failures().unwrap().is_empty());
    }

    #[test]
    fn deleting_a_render_already_gone_is_fine() {
        let (_root, history) = store();
        put_render(&history, &id(1), &["/a/1.jpg"], None);
        let render = RenderId::parse(&id(1)).unwrap();
        let twice = [render.clone(), render];
        history.delete_renders(&twice, history.close_paint(&twice)).unwrap();
        assert!(std::fs::read_dir(history.renders_dir()).unwrap().next().is_none());
    }

    /// The open's sweep removes partials and temporaries, however new, and
    /// nothing else.
    #[test]
    fn a_sweep_removes_only_partials_and_temporaries() {
        let (_root, history) = store();
        let renders = history.renders_dir();
        let render = renders.join(id(2));
        let dirs =
            [renders.join(format!("{}.partial", id(1))), renders.join("Foo.partial"), render];
        for dir in dirs.iter().chain([&history.stacks_dir(), &history.failed_dir()]) {
            std::fs::create_dir_all(dir).unwrap();
        }
        let temporary = |dir: &Path, name: &str| dir.join(format!(".{name}.12-3.tmp"));
        let temporaries = [
            temporary(history.root(), "list.json"),
            temporary(&history.stacks_dir(), &format!("{}.json", id(3))),
            temporary(&history.failed_dir(), &format!("{}.json", id(4))),
            temporary(&dirs[2], "render.json"),
        ];
        let kept = [
            history.root().join("list.json"),
            dirs[2].join("render.json"),
            temporary(&dirs[1], "render.json"),
            dirs[2].join(".render.json.tmp"),
        ];
        for file in temporaries.iter().chain(&kept) {
            std::fs::write(file, b"x").unwrap();
        }
        let exists = |paths: &[PathBuf]| paths.iter().map(|p| p.exists()).collect::<Vec<_>>();

        // Left by a crashed session: the next open sweeps them.
        let root = history.root().to_path_buf();
        drop(history);
        let history = History::open(&root).unwrap();
        assert_eq!(history.root(), root);
        assert_eq!(exists(&dirs), [false, true, true]);
        assert_eq!(exists(&temporaries), [false; 4]);
        assert_eq!(exists(&kept), [true; 4]);
    }
}
