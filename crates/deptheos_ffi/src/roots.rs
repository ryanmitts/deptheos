//! The Library's roots as iOS reaches them (ADR-145): the frame folders,
//! each a root in `sources.json` with the bookmark Swift made when it was
//! picked. Swift opens a root's scope from that bookmark to read frames (a
//! lease), resolving it off the main thread with a timeout, and hands back
//! where it led ([`Engine::find_again`]). Every relink returns the folders
//! moved, which the app follows in one place (`AppModel.relinked`).
//!
//! Paths come back as the app named them this session, else as stored, so
//! the app never compares `/var` with `/private/var` itself.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::frames::sources::{Bookmark, Place, Resolution, VolumeKind};
use deptheos_app::library::history::Moves;
pub use deptheos_app::library::history::Reach;
use deptheos_app::{RootId, Sources};

use super::{Engine, FolderMove, Records, Request, Result};
use crate::request::Ask;
use deptheos_app::engine::work::Lane;

/// Whether a frame can be read now: there, missing (its drive is there but
/// not the file: Locate…), or not connected.
#[uniffi::remote(Enum)]
pub enum Reach {
    There,
    Missing,
    NotConnected,
}

/// A top root on a drive, share or the device: the folder a stack's frames
/// were added from, with what its scope opens from.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RootInfo {
    pub root: String,
    /// Where it is, or was last seen, in the form the app named it.
    pub folder: PathBuf,
    /// A drive's or a share's.
    pub volume: String,
    /// On a network share.
    pub share: bool,
    /// Made by Swift when picked; none for a folder never picked (read in
    /// place on the simulator).
    pub bookmark: Option<Vec<u8>>,
}

/// Where Swift's resolving of a bookmark led, and the bookmark remade when
/// stale.
#[derive(Clone, Debug, uniffi::Record)]
pub struct BookmarkResolution {
    pub root: String,
    pub path: PathBuf,
    pub fresh: Option<Vec<u8>>,
}

/// Locate…: the folders moved, and how many frames under the folder the
/// pick has ("Found 12 of 14 frames").
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LocatedInfo {
    pub moved: Vec<FolderMove>,
    pub found: u32,
    pub of: u32,
}

/// A folder read in place, as Settings lists it: its root, reachability,
/// and how many stacks use frames under it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ReadFolder {
    pub root: RootInfo,
    pub reach: Reach,
    pub stacks: u32,
}

fn folder_moves(moves: &Moves) -> Vec<FolderMove> {
    moves
        .folders()
        .iter()
        .map(|(old, new)| FolderMove { old: old.clone(), new: new.clone() })
        .collect()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[uniffi::export]
impl Engine {
    /// Keep `folder`, just picked in Files, with Swift's `bookmark`: roots
    /// of its drive or share not where they were are looked for under it
    /// (picking a drive's top brings back every root on it), then it is a
    /// root unless one covers it, keeping the bookmark when at the top.
    pub async fn keep_folder(
        self: Arc<Self>,
        folder: PathBuf,
        bookmark: Vec<u8>,
    ) -> Result<Vec<FolderMove>> {
        self.store(move |e| {
            let frames = e.history.frames();
            let kept = e.told(frames.keep(&folder, Bookmark(bookmark)))?;
            e.relisted(&kept.moves);
            Ok(folder_moves(&kept.moves))
        })
        .await
    }

    /// The top root covering each of `paths`; `None` in the app's own
    /// storage or uncovered. No file is looked at.
    pub async fn roots_of(
        self: Arc<Self>,
        request: Arc<Request>,
        paths: Vec<PathBuf>,
    ) -> Result<Vec<Option<RootInfo>>> {
        self.pool(&request, Lane::Interactive, Ask::Own("roots-of"), move |e| {
            let frames = e.history.frames();
            let sources = frames.sources();
            let tops = frames.top_roots(&paths);
            Ok(tops.into_iter().map(|id| e.root_info(&sources, &id?)).collect())
        })
        .await
    }

    /// How many roots have a bookmark (the folders read in place, as
    /// Settings counts them). In memory, so quick.
    pub fn bookmarked_count(&self) -> u32 {
        let sources = self.history.frames().sources();
        sources.roots().filter(|(id, _)| sources.bookmark(id).is_some()).count() as u32
    }

    /// Every top root with a bookmark: what launch resolves to find them.
    pub async fn bookmarked_roots(self: Arc<Self>, request: Arc<Request>) -> Result<Vec<RootInfo>> {
        self.pool(&request, Lane::Interactive, Ask::Own("roots-bookmarked"), |e| {
            let sources = e.history.frames().sources();
            let ids: Vec<RootId> = sources
                .roots()
                .filter(|(id, _)| sources.bookmark(id).is_some())
                .map(|(id, _)| id.clone())
                .collect();
            Ok(ids.iter().filter_map(|id| e.root_info(&sources, id)).collect())
        })
        .await
    }

    /// Find moved roots from `resolutions` (where Swift's bookmarks led, a
    /// stale one remade): by bookmark unless it led into a trash, then by
    /// volume id, then by name and last path, proved by sampled frames. A
    /// partial match is never offered; the root stays missing until
    /// Locate…. A root in place keeps a remade bookmark. Returns the moves.
    pub async fn find_again(
        self: Arc<Self>,
        resolutions: Vec<BookmarkResolution>,
    ) -> Result<Vec<FolderMove>> {
        let led: Vec<Resolution> = resolutions
            .into_iter()
            .filter_map(|r| {
                Some(Resolution {
                    root: RootId::parse(&r.root)?,
                    path: r.path,
                    fresh: r.fresh.map(Bookmark),
                })
            })
            .collect();
        self.store(move |e| {
            let moves = e.told(e.history.frames().find_again(&led))?;
            e.relisted(&moves);
            Ok(folder_moves(&moves))
        })
        .await
    }

    /// Locate…: `old` is at `new`, picked in Files with Swift's bookmark.
    /// Frames are matched under `new` by name, size and fingerprint at the
    /// same paths, and the folder relinked whatever was found; `new` keeps
    /// the bookmark as [`Self::keep_folder`] does.
    pub async fn locate(
        self: Arc<Self>,
        old: PathBuf,
        new: PathBuf,
        bookmark: Option<Vec<u8>>,
    ) -> Result<LocatedInfo> {
        self.store(move |e| {
            let frames = e.history.frames();
            let located = e.told(frames.locate(&old, &new, bookmark.map(Bookmark)))?;
            e.relisted(&located.moves);
            Ok(LocatedInfo {
                moved: folder_moves(&located.moves),
                found: count(located.matched.found),
                of: count(located.matched.of),
            })
        })
        .await
    }

    /// Whether each of `paths` can be read now. Asked with their roots'
    /// scopes open: a closed one can't be looked at, so is not connected.
    pub async fn reach(
        self: Arc<Self>,
        request: Arc<Request>,
        paths: Vec<PathBuf>,
    ) -> Result<Vec<Reach>> {
        self.pool(&request, Lane::Interactive, Ask::Own("reach"), move |e| {
            Ok(e.history.frames().reach(&paths))
        })
        .await
    }

    /// The roots of [`Self::read_folders`]' folders, whose scopes the app
    /// opens first so reachability can be told.
    pub async fn read_folder_roots(
        self: Arc<Self>,
        request: Arc<Request>,
    ) -> Result<Vec<RootInfo>> {
        self.pool(&request, Lane::Background, Ask::Own("read-folder-roots"), |e| {
            let uses = e.folder_uses()?;
            let sources = e.history.frames().sources();
            Ok(uses.keys().filter_map(|root| e.root_info(&sources, root)).collect())
        })
        .await
    }

    /// The folders read in place, as Settings lists them: every top root a
    /// stack has a frame under (working version, render or failed run), with
    /// how many stacks. One no stack uses isn't listed. Asked with scopes
    /// open ([`Self::read_folder_roots`]), as [`Self::reach`].
    pub async fn read_folders(self: Arc<Self>, request: Arc<Request>) -> Result<Vec<ReadFolder>> {
        self.pool(&request, Lane::Background, Ask::Own("read-folders"), |e| {
            let uses = e.folder_uses()?;
            Ok(e.read_folders_now(&uses))
        })
        .await
    }

    /// Copy into Deptheos: the frames the Library names under `folder`, by
    /// paths from it, which a copy must hold.
    pub async fn frames_to_copy(
        self: Arc<Self>,
        request: Arc<Request>,
        folder: PathBuf,
    ) -> Result<Vec<String>> {
        self.pool(&request, Lane::Interactive, Ask::Own("frames-to-copy"), move |e| {
            Ok(e.history.frames().named_in(&folder))
        })
        .await
    }

    /// Copy into Deptheos: `copy`, in the app's storage, copies `folder`.
    /// Only if it holds every frame of [`Self::frames_to_copy`] unchanged
    /// (size and fingerprint) is the folder pointed at it; else nothing
    /// changes and the error names what's missing.
    pub async fn take_copy(
        self: Arc<Self>,
        folder: PathBuf,
        copy: PathBuf,
    ) -> Result<Vec<FolderMove>> {
        self.store(move |e| {
            let frames = e.history.frames();
            let moves = e.told(frames.adopt(&folder, &copy))?;
            e.relisted(&moves);
            Ok(folder_moves(&moves))
        })
        .await
    }

    /// Each of `paths` now: one a relink moved is named where its folder is.
    pub fn paths_now(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        self.history.frames().now(&paths)
    }

    /// Relinks this session: what was read at another count names frames
    /// where they were.
    pub fn relinks(&self) -> u64 {
        self.history.frames().generation()
    }
}

impl Engine {
    /// Each top root a stack has a frame under, with those stacks (working
    /// versions on screen or kept, renders, failed runs). Only stack files
    /// changed since the last ask are reread.
    fn folder_uses(&self) -> Result<HashMap<RootId, HashSet<String>>> {
        let shown = self.told(self.history.read_working())?;
        let stacks = self.stacks()?;
        let renders = self.told(self.history.list())?;
        let failures = self.told(self.history.failures())?;
        let kept = stacks.iter().filter_map(|(stack, info)| {
            Some((stack.as_str(), info.working.as_ref()?.list.files.as_slice()))
        });
        let uses = std::iter::once((shown.stack.as_str(), shown.list.files.as_slice()))
            .chain(kept)
            .chain(renders.iter().map(|r| (r.stack.as_str(), r.frames.as_slice())))
            .chain(failures.iter().map(|f| (f.stack.as_str(), f.frames.as_slice())));
        let frames = self.history.frames();
        let mut roots: HashMap<RootId, HashSet<String>> = HashMap::new();
        for (stack, files) in uses {
            for root in frames.top_roots(files).into_iter().flatten() {
                roots.entry(root).or_default().insert(stack.to_string());
            }
        }
        Ok(roots)
    }

    /// [`Self::read_folders`], here, of `uses` ([`Self::folder_uses`]).
    fn read_folders_now(&self, uses: &HashMap<RootId, HashSet<String>>) -> Vec<ReadFolder> {
        let frames = self.history.frames();
        let sources = frames.sources();
        frames
            .top_folders()
            .into_iter()
            .filter_map(|top| {
                let stacks = uses.get(&top.root).map_or(0, |s| count(s.len()));
                Some(ReadFolder {
                    root: self.root_info(&sources, &top.root)?,
                    reach: top.reach,
                    stacks,
                })
                .filter(|f| f.stacks > 0)
            })
            .collect()
    }

    /// The top root `id` on a volume, as the app sees it.
    fn root_info(&self, sources: &Sources, id: &RootId) -> Option<RootInfo> {
        let Place::Volume { volume, .. } = &sources.root(id)?.place else { return None };
        let folder = self.history.frames().folder_of(id)?;
        Some(RootInfo {
            root: id.as_str().to_string(),
            folder,
            volume: volume.name.clone(),
            share: matches!(volume.kind, VolumeKind::Share { .. }),
            bookmark: sources.bookmark(id).map(|b| b.0.clone()),
        })
    }

    /// After `moves`: reread every render's frames now, on the caller's
    /// worker, rather than one by one as the app draws.
    fn relisted(&self, moves: &Moves) {
        if moves.is_empty() {
            return;
        }
        *self.records() = match self.history.list() {
            Ok(renders) => Records::listed(&renders),
            Err(e) => {
                tracing::warn!("couldn't list the renders after relinking: {e}");
                Records::default()
            }
        };
    }
}
