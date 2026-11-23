//! The one place a Library file's [`FrameRef`] becomes a path and back
//! (ADR-145). Every clone of a [`History`](super::History) shares one
//! [`Frames`]: the roots ([`Sources`]) and the frames named this session.
//! Front ends speak paths; a path handed back is named from the root
//! covering it, or its folder is added as one.
//!
//! Reads take a snapshot of the roots, so a change waiting on a slow drive
//! never blocks one. Changes run one at a time, each written to
//! `sources.json` before it returns.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::frames::sources::{
    Bookmark, Disk, Fingerprint, FrameRef, InUse, Matched, Place, Places, Probe, Resolution,
    RootId, RootState, Sources, SourcesError, Volume, in_trash, matched_under,
};

type Result<T> = std::result::Result<T, SourcesError>;

/// The folders a relink moved, as they were and are, derived from the roots
/// before and after.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moves(Vec<(PathBuf, PathBuf)>);

impl Moves {
    /// Each folder of `folders` (as it was, as it is) moved.
    pub fn new(mut folders: Vec<(PathBuf, PathBuf)>) -> Self {
        folders.sort_by_key(|(from, _)| std::cmp::Reverse(from.components().count()));
        Self(folders)
    }

    /// The folders moved, each as it was and as it is, deepest first.
    pub fn folders(&self) -> &[(PathBuf, PathBuf)] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Where `path` is now, if it was under a folder that moved.
    pub fn path(&self, path: &Path) -> Option<PathBuf> {
        self.0.iter().find_map(|(from, to)| {
            let rest = path.strip_prefix(from).ok()?;
            Some(if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) })
        })
    }

    /// The roots whose folder changed from `before` to `after`, including
    /// one split off from another's subfolder.
    fn between(before: (&Sources, &Folders), after: (&Sources, &Folders)) -> Self {
        let moves: Vec<(PathBuf, PathBuf)> = after
            .0
            .roots()
            .filter_map(|(id, root)| {
                let was = match before.0.root(id) {
                    Some(_) => folder(before, id)?,
                    None => {
                        let under = root.split_from.as_ref()?;
                        join(&folder(before, &under.root)?, &under.path)
                    }
                };
                let now = folder(after, id)?;
                (was != now).then_some((was, now))
            })
            .collect();
        Self::new(moves)
    }
}

/// What Locate… did: the folders moved and how many frames were found
/// ("found N of M").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Located {
    pub moves: Moves,
    pub matched: Matched,
}

/// What [`Frames::keep`] did: the top root covering the folder, and its
/// volume's folders that came back with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub root: RootId,
    pub moves: Moves,
}

/// A top root on a volume as Settings lists it: its folder, whether it can
/// be reached, and its volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopFolder {
    pub root: RootId,
    pub folder: PathBuf,
    pub reach: Reach,
    pub volume: Volume,
}

/// Whether a frame can be read now (ADR-145).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    There,
    /// Its volume is there but the file isn't (or is in a trash folder).
    Missing,
    /// Its volume isn't: connect it.
    NotConnected,
}

/// How many of a root's frames prove a place found by name alone.
const SAMPLE: usize = 8;

/// Each root's folder as a front end named it this session, given back in
/// that form whatever form is stored (`/var` for `/private/var`).
type Folders = HashMap<RootId, PathBuf>;

/// The root `id`'s folder: as it was named this session, else within the
/// one it is inside, else its place's ([`Sources::root_path`]).
fn folder((sources, named): (&Sources, &Folders), id: &RootId) -> Option<PathBuf> {
    let mut at = id.clone();
    let mut rest: Vec<String> = Vec::new();
    for _ in 0..=sources.roots().count() {
        if let Some(path) = named.get(&at) {
            let mut path = path.clone();
            path.extend(rest.iter().rev().flat_map(|r| r.split('/')).filter(|n| !n.is_empty()));
            return Some(path);
        }
        match &sources.root(&at)?.place {
            Place::Within { root, path } => {
                rest.push(path.clone());
                at = root.clone();
            }
            _ => break,
        }
    }
    let mut path = sources.root_path(&at)?;
    path.extend(rest.iter().rev().flat_map(|r| r.split('/')).filter(|n| !n.is_empty()));
    Some(path)
}

/// The missing file `path` as the deepest root covering it names it, with
/// no size or fingerprint.
fn covered((sources, named): (&Sources, &Folders), path: &Path) -> Option<FrameRef> {
    let (root, rest) = sources
        .roots()
        .flat_map(|(id, _)| {
            let forms = [folder((sources, named), id), sources.root_path(id)];
            forms.into_iter().flatten().map(move |at| (id, at))
        })
        .filter_map(|(id, at)| Some((id, path.strip_prefix(at).ok()?.to_path_buf())))
        .max_by_key(|(_, rest)| std::cmp::Reverse(rest.components().count()))?;
    let rest: Vec<&str> = rest.iter().map(|n| n.to_str()).collect::<Option<_>>()?;
    let frame =
        FrameRef { root: root.clone(), path: rest.join("/"), size: 0, fingerprint: Fingerprint(0) };
    (!frame.path.is_empty()).then(|| sources.identity(&frame))
}

/// The root whose folder is `path`, as named this session or as stored.
fn root_at((sources, named): (&Sources, &Folders), path: &Path) -> Option<RootId> {
    sources
        .roots()
        .find(|(id, _)| {
            [folder((sources, named), id), sources.root_path(id)]
                .iter()
                .flatten()
                .any(|at| at == path)
        })
        .map(|(id, _)| id.clone())
}

/// `path` with its deepest existing folder canonical.
pub(crate) fn canonical_under(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(at) {
            return Some(rest.iter().rev().fold(canonical, |p, n| p.join(n)));
        }
        rest.push(at.file_name()?);
        at = at.parent()?;
    }
}

fn join(base: &Path, rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    path.extend(rel.split('/').filter(|n| !n.is_empty()));
    path
}

/// The Library's roots and the frames named from them this session.
#[derive(Debug)]
pub struct Frames {
    places: Places,
    /// The roots as last written.
    current: Mutex<Arc<Sources>>,
    /// Held while the roots change, so changes are made one at a time.
    changing: Mutex<()>,
    named: Mutex<Named>,
    /// Bumped each time a relink moves a frame's path.
    generation: AtomicU64,
    /// The unused roots were dropped this session.
    pruned: AtomicBool,
    /// How the front end makes a top root's bookmark, where it does.
    make_bookmark: OnceLock<MakeBookmark>,
}

/// Makes a bookmark of a folder: the front end's ([`Frames::make_bookmarks_with`]).
struct MakeBookmark(Box<MakeFn>);

type MakeFn = dyn Fn(&Path) -> Option<Bookmark> + Send + Sync;

impl std::fmt::Debug for MakeBookmark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MakeBookmark")
    }
}

/// The frames named this session.
#[derive(Debug, Default)]
struct Named {
    /// Each path read or written, to its frame, so it is named again
    /// without reading a fingerprint.
    frames: HashMap<PathBuf, FrameRef>,
    folders: Folders,
}

impl Named {
    /// The roots `moved` from `before` (folders as named, `was`) to `after`:
    /// those `pointed` keep the front end's form, the others are stored
    /// form. The paths named under them follow.
    fn moved(
        &mut self,
        (before, was): (&Sources, &Folders),
        after: &Sources,
        moved: &[RootId],
        pointed: Vec<(RootId, PathBuf)>,
    ) -> Moves {
        for id in moved {
            self.folders.remove(id);
        }
        self.folders.extend(pointed);
        let moves = Moves::between((before, was), (after, &self.folders));
        let renamed: Vec<(PathBuf, FrameRef)> = self
            .frames
            .iter()
            .filter_map(|(path, frame)| Some((moves.path(path)?, frame.clone())))
            .collect();
        self.frames.extend(renamed);
        moves
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Frames {
    /// The roots of the Library at `places`, read before anything else
    /// ([`Sources::open`]).
    pub(crate) fn open(places: Places) -> Result<Self> {
        let mut sources = Sources::open(&Disk, places.clone())?;
        sources.stamp()?;
        Ok(Self {
            places,
            current: Mutex::new(Arc::new(sources)),
            changing: Mutex::new(()),
            named: Mutex::default(),
            generation: AtomicU64::new(0),
            pruned: AtomicBool::new(false),
            make_bookmark: OnceLock::new(),
        })
    }

    /// Make a bookmark with `make` for each top root as it is added or
    /// moves (ADR-145): the macOS desktop's. Called once, before the first
    /// change, or never.
    pub fn make_bookmarks_with(
        &self,
        make: impl Fn(&Path) -> Option<Bookmark> + Send + Sync + 'static,
    ) {
        if self.make_bookmark.set(MakeBookmark(Box::new(make))).is_err() {
            tracing::warn!("frames: bookmarks are already made one way");
        }
    }

    /// The roots as they are now.
    pub fn sources(&self) -> Arc<Sources> {
        lock(&self.current).clone()
    }

    /// How many relinks have moved frames this session: something read at
    /// another generation may name old places.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The paths of `frames`, read from a file; refused for an unknown root.
    pub fn paths(&self, frames: &[FrameRef]) -> Result<Vec<PathBuf>> {
        let sources = self.sources();
        let mut named = lock(&self.named);
        frames
            .iter()
            .map(|frame| {
                let identity = sources.identity(frame);
                let base = folder((&sources, &named.folders), &identity.root)
                    .ok_or_else(|| SourcesError::UnknownRoot(identity.root.clone()))?;
                let path = join(&base, &identity.path);
                named.frames.entry(path.clone()).or_insert(identity);
                Ok(path)
            })
            .collect()
    }

    /// `paths` as frames, to be written: as named this session, else from
    /// the root covering it, added when none (a stat and fingerprint each).
    /// A missing file is named with no size or fingerprint; refused when no
    /// root covers it.
    pub fn refs(&self, paths: &[PathBuf]) -> Result<Vec<FrameRef>> {
        let known = |sources: &Sources, named: &Named| -> Vec<Option<FrameRef>> {
            paths.iter().map(|p| named.frames.get(p).map(|f| sources.identity(f))).collect()
        };
        let mut found = known(&self.sources(), &lock(&self.named));
        let mut unknown: Vec<PathBuf> = Vec::new();
        for (path, frame) in paths.iter().zip(&found) {
            if frame.is_none() && !unknown.contains(path) && path.is_file() {
                unknown.push(path.clone());
            }
        }
        if !unknown.is_empty() {
            // Named within the change adding their roots, so a prune never
            // sees one unnamed.
            self.change(|sources| {
                let added = sources.add_files(&Disk, &unknown)?.0;
                let mut named = lock(&self.named);
                for (path, frame) in unknown.into_iter().zip(added) {
                    // The folder of its root as this path names it.
                    let mut base = Some(path.as_path());
                    for _ in frame.path.split('/').filter(|n| !n.is_empty()) {
                        base = base.and_then(Path::parent);
                    }
                    if let Some(base) = base {
                        let folder = named.folders.entry(frame.root.clone());
                        folder.or_insert_with(|| base.to_path_buf());
                    }
                    named.frames.insert(path, frame);
                }
                Ok(())
            })?;
            found = known(&self.sources(), &lock(&self.named));
        }
        let sources = self.sources();
        let named = lock(&self.named);
        paths
            .iter()
            .zip(found)
            .map(|(path, frame)| match frame {
                Some(frame) => Ok(frame),
                None => covered((&sources, &named.folders), path)
                    .or_else(|| covered((&sources, &named.folders), &canonical_under(path)?))
                    .ok_or_else(|| SourcesError::NotFile(path.clone())),
            })
            .collect()
    }

    /// The folder `old` is at `new`: the root at or above it points there (a
    /// subfolder is split off as its own root), else every root inside it
    /// moves. With `bookmark` (iOS's pick), the volume's missing roots are
    /// looked for under `new` first and `new` is kept as [`Self::keep`]
    /// does, in one change. Only `sources.json` is written. Returns what
    /// moved.
    pub fn relink(&self, old: &Path, new: &Path, bookmark: Option<Bookmark>) -> Result<Moves> {
        let samples = bookmark.as_ref().map(|_| self.samples()).unwrap_or_default();
        let kept = bookmark.map(|b| (b, samples));
        // As the front end named it, else as stored.
        let moves = match self.relink_from(old, new, kept.as_ref())? {
            moves if moves.is_empty() => match canonical_under(old) {
                Some(canonical) if canonical != old => {
                    self.relink_from(&canonical, new, kept.as_ref())?
                }
                _ => moves,
            },
            moves => moves,
        };
        Ok(self.bumped(moves))
    }

    /// Keep the folder `folder` the user picked, with its bookmark (iOS):
    /// its volume's missing roots are looked for under it
    /// ([`Sources::found_volume`]), then it becomes a root unless one
    /// covers it ([`Sources::keep_folder`]). Only `sources.json` is written.
    pub fn keep(&self, folder: &Path, bookmark: Bookmark) -> Result<Kept> {
        let samples = self.samples();
        let (moves, root) = self.change(|sources| {
            let before = sources.clone();
            let mut named = lock(&self.named);
            let was = named.folders.clone();
            let (moved, root) = keep_in(sources, folder, bookmark, &samples)?;
            // In the front end's form when it is the root's own folder.
            let own = sources
                .root_path(&root)
                .is_some_and(|at| std::fs::canonicalize(folder).is_ok_and(|c| c == at));
            let pointed = if own { vec![(root.clone(), folder.to_path_buf())] } else { Vec::new() };
            let moves = named.moved((&before, &was), sources, &moved, pointed);
            Ok((moves, root))
        })?;
        Ok(Kept { root, moves: self.bumped(moves) })
    }

    /// Locate…: the user picked `new` for `old`. Frames named under `old`
    /// are matched under `new` by name, size and fingerprint, then the
    /// folder is relinked ([`Self::relink`]) whatever was found.
    pub fn locate(&self, old: &Path, new: &Path, bookmark: Option<Bookmark>) -> Result<Located> {
        // As the front end named it, else as stored.
        let under = self.named_under_either(old);
        let moves = self.relink(old, new, bookmark)?;
        // The one picked, or its place on a drive whose top was picked.
        let at = moves.path(old).unwrap_or_else(|| new.to_path_buf());
        let matched = matched_under(&Disk, &at, under.iter().map(|(path, f)| (path.clone(), f)));
        tracing::debug!("frames: located {} of {}", matched.found, matched.of);
        Ok(Located { moves, matched })
    }

    /// The frames named this session under `folder`, by relative path: what
    /// Copy into Deptheos copies.
    pub fn named_in(&self, folder: &Path) -> Vec<String> {
        let mut paths: Vec<String> =
            self.named_under_either(folder).into_iter().map(|(path, _)| path).collect();
        paths.sort();
        paths
    }

    /// Copy into Deptheos: relink `old` to its copy `copy` ([`Self::relink`])
    /// only when every frame named under `old` matches there by size and
    /// fingerprint; else name those lacking ([`SourcesError::NotCopied`]).
    /// Nothing is relinked when no frame is named under `old`.
    pub fn adopt(&self, old: &Path, copy: &Path) -> Result<Moves> {
        let under = self.named_under_either(old);
        if under.is_empty() {
            return Ok(Moves::default());
        }
        let ends = |path: &Path| -> Option<(u64, Fingerprint)> {
            let stat = Disk.stat(path).ok().flatten().filter(|s| !s.folder)?;
            Some((stat.size, Fingerprint::of(&Disk, path, stat.size).ok()?))
        };
        let mut lacking: Vec<String> = under
            .into_iter()
            .filter(|(rel, _)| {
                let original = ends(&join(old, rel));
                original.is_none() || original != ends(&join(copy, rel))
            })
            .map(|(rel, _)| rel)
            .collect();
        if !lacking.is_empty() {
            lacking.sort();
            tracing::debug!("frames: a copy lacks {} frames", lacking.len());
            return Err(SourcesError::NotCopied(lacking));
        }
        self.relink(old, copy, None)
    }

    /// [`Self::named_under`] `folder` as named, else as stored.
    fn named_under_either(&self, folder: &Path) -> Vec<(String, FrameRef)> {
        let under = self.named_under(folder);
        match canonical_under(folder).filter(|c| c != folder) {
            Some(canonical) if under.is_empty() => self.named_under(&canonical),
            _ => under,
        }
    }

    /// The frames named this session under `folder`, with relative paths.
    fn named_under(&self, folder: &Path) -> Vec<(String, FrameRef)> {
        let sources = self.sources();
        let named = lock(&self.named);
        let mut seen = HashSet::new();
        named
            .frames
            .iter()
            .filter_map(|(path, frame)| {
                let rest = path.strip_prefix(folder).ok()?;
                let rest: Vec<&str> = rest.iter().map(|n| n.to_str()).collect::<Option<_>>()?;
                let identity = sources.identity(frame);
                seen.insert((identity.root, identity.path)).then(|| (rest.join("/"), frame.clone()))
            })
            .collect()
    }

    fn relink_from(
        &self,
        old: &Path,
        new: &Path,
        kept: Option<&(Bookmark, HashMap<RootId, Vec<FrameRef>>)>,
    ) -> Result<Moves> {
        self.change(|sources| {
            let before = sources.clone();
            let mut named = lock(&self.named);
            let was = named.folders.clone();
            let paths: Vec<(RootId, PathBuf)> = before
                .roots()
                .filter_map(|(id, _)| Some((id.clone(), folder((&before, &was), id)?)))
                .collect();
            let above = paths
                .iter()
                .filter(|(_, path)| old.starts_with(path))
                .max_by_key(|(_, path)| path.components().count());
            let mut pointed: Vec<(RootId, PathBuf)> = Vec::new();
            let mut moved: Vec<RootId> = Vec::new();
            // Picked (iOS): the volume's roots come back first; one among
            // them needs no relink.
            if let Some((_, samples)) = kept {
                moved.extend(sources.found_volume(&Disk, new, samples)?.moved);
            }
            let back = above.is_some_and(|(id, _)| moved.contains(&before.top_of(id)));
            match above {
                _ if back => {}
                Some((id, path)) if path == old => {
                    moved.extend(sources.relink(&Disk, id, new, None)?.moved);
                    pointed.push((id.clone(), new.to_path_buf()));
                }
                Some((id, path)) => {
                    let sub = old.strip_prefix(path).unwrap_or(old);
                    let sub = sub
                        .iter()
                        .map(|n| n.to_str().ok_or_else(|| SourcesError::NotUnicode(old.into())))
                        .collect::<Result<Vec<&str>>>()?;
                    let split = sources.split(&Disk, id, &sub.join("/"), new, None)?;
                    pointed.extend(split.moved.first().map(|r| (r.clone(), new.to_path_buf())));
                    moved.extend(split.moved);
                }
                None => {
                    let inside: Vec<&(RootId, PathBuf)> =
                        paths.iter().filter(|(_, path)| path.starts_with(old)).collect();
                    // The top ones: one inside another moves with it.
                    for (id, path) in &inside {
                        let nested = inside
                            .iter()
                            .any(|(other, at)| other != id && path.starts_with(at) && at != path);
                        if !nested {
                            let to = new.join(path.strip_prefix(old).unwrap_or(path));
                            moved.extend(sources.relink(&Disk, id, &to, None)?.moved);
                            pointed.push((id.clone(), to));
                        }
                    }
                }
            }
            if let Some((bookmark, _)) = kept.filter(|_| !moved.is_empty()) {
                sources.keep_folder(&Disk, new, bookmark.clone())?;
            }
            Ok(named.moved((&before, &was), sources, &moved, pointed))
        })
    }

    /// At launch: look for every missing root ([`Sources::find_again`])
    /// through `bookmarks`, its volume's id, name and last path, accepting
    /// a place only when a sample of its frames matches; else it stays
    /// missing until Locate…. A stale bookmark is replaced. Returns what
    /// moved.
    pub fn find_again(&self, bookmarks: &[Resolution]) -> Result<Moves> {
        let samples = self.samples();
        let moves = self.change(|sources| {
            for fresh in bookmarks.iter().filter(|r| r.fresh.is_some()) {
                let there = matches!(sources.root_state(&Disk, &fresh.root), RootState::At(_));
                if there && !in_trash(&fresh.path) {
                    sources.set_bookmark(&fresh.root, fresh.fresh.clone())?;
                }
            }
            let before = sources.clone();
            let found = sources.find_again(&Disk, bookmarks, &samples)?;
            let mut named = lock(&self.named);
            let was = named.folders.clone();
            Ok(named.moved((&before, &was), sources, &found.moved, Vec::new()))
        })?;
        Ok(self.bumped(moves))
    }

    /// Whether the root `id` is not where it was.
    pub fn is_lost(&self, id: &RootId) -> bool {
        !matches!(self.sources().root_state(&Disk, id), RootState::At(_))
    }

    /// The top root covering each of `paths`; `None` for one no root covers.
    /// Only paths not named this session are canonicalized.
    pub fn top_roots(&self, paths: &[PathBuf]) -> Vec<Option<RootId>> {
        let sources = self.sources();
        let named = lock(&self.named);
        paths
            .iter()
            .map(|p| {
                let frame = named.frames.get(p).cloned();
                let at = |path: &Path| {
                    covered((&sources, &named.folders), path)
                        .map(|f| sources.identity(&f).root)
                        .or_else(|| root_at((&sources, &named.folders), path))
                };
                let root = match frame {
                    Some(frame) => sources.identity(&frame).root,
                    None => at(p).or_else(|| at(&canonical_under(p)?))?,
                };
                Some(sources.top_of(&root))
            })
            .collect()
    }

    /// The roots the frames named this session are named from.
    pub(crate) fn roots_named_this_session(&self) -> HashSet<RootId> {
        lock(&self.named).frames.values().map(|f| f.root.clone()).collect()
    }

    /// The root `id`'s folder as named this session, else as stored
    /// ([`Sources::root_path`]).
    pub fn folder_of(&self, id: &RootId) -> Option<PathBuf> {
        let sources = self.sources();
        let named = lock(&self.named);
        folder((&sources, &named.folders), id)
    }

    /// Each of `paths` where it is now, following relinks this session. No
    /// file is looked at.
    pub fn now(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let sources = self.sources();
        let named = lock(&self.named);
        paths
            .iter()
            .map(|p| {
                let Some(frame) = named.frames.get(p) else { return p.clone() };
                let identity = sources.identity(frame);
                folder((&sources, &named.folders), &identity.root)
                    .map_or_else(|| p.clone(), |base| join(&base, &identity.path))
            })
            .collect()
    }

    /// Every top root, by folder name, with whether it can be reached.
    pub fn top_folders(&self) -> Vec<TopFolder> {
        let sources = self.sources();
        let named = lock(&self.named).folders.clone();
        let mut tops: Vec<TopFolder> = sources
            .roots()
            .filter_map(|(id, root)| {
                let Place::Volume { volume, .. } = &root.place else { return None };
                let reach = match sources.root_state(&Disk, id) {
                    RootState::At(_) => Reach::There,
                    RootState::Missing => Reach::Missing,
                    RootState::NotConnected => Reach::NotConnected,
                };
                let folder = folder((&sources, &named), id)?;
                Some(TopFolder { root: id.clone(), folder, reach, volume: volume.clone() })
            })
            .collect();
        tops.sort_by(|a, b| {
            (a.folder.file_name(), &a.folder).cmp(&(b.folder.file_name(), &b.folder))
        });
        tops
    }

    /// Up to [`SAMPLE`] frames named this session per root, spread out: the
    /// proof for a root found by name alone.
    fn samples(&self) -> HashMap<RootId, Vec<FrameRef>> {
        let sources = self.sources();
        let named = lock(&self.named);
        let mut frames: Vec<FrameRef> =
            named.frames.values().map(|f| sources.identity(f)).collect();
        frames.sort_by(|a, b| (&a.root, &a.path).cmp(&(&b.root, &b.path)));
        frames.dedup_by(|a, b| a.same_file(b));
        sources
            .roots()
            .map(|(id, _)| {
                let under: Vec<FrameRef> = frames
                    .iter()
                    .filter(|f| f.size > 0 && sources.path_under(f, id).is_some())
                    .cloned()
                    .collect();
                let step = under.len().div_ceil(SAMPLE).max(1);
                (id.clone(), under.into_iter().step_by(step).collect())
            })
            .collect()
    }

    /// Whether each of `paths` can be read now: not connected when its
    /// root's volume is gone, missing only when the system says it isn't
    /// there (one the app may not look at counts as there). One stat each.
    pub fn reach(&self, paths: &[PathBuf]) -> Vec<Reach> {
        let sources = self.sources();
        let frames: Vec<Option<FrameRef>> = {
            let named = lock(&self.named);
            paths
                .iter()
                .map(|p| {
                    let frame = named.frames.get(p).cloned();
                    frame.or_else(|| covered((&sources, &named.folders), p))
                })
                .map(|f| f.map(|f| sources.identity(&f)))
                .collect()
        };
        let mut roots: HashMap<RootId, RootState> = HashMap::new();
        let gone = |path: &Path| matches!(path.try_exists(), Ok(false));
        paths
            .iter()
            .zip(frames)
            .map(|(path, frame)| {
                let Some(frame) = frame else {
                    return if gone(path) { Reach::Missing } else { Reach::There };
                };
                let state = roots
                    .entry(frame.root.clone())
                    .or_insert_with(|| sources.root_state(&Disk, &frame.root));
                match state {
                    RootState::NotConnected => Reach::NotConnected,
                    RootState::Missing => Reach::Missing,
                    RootState::At(_) if gone(path) => Reach::Missing,
                    RootState::At(_) => Reach::There,
                }
            })
            .collect()
    }

    /// `moves`, counted as a relink when anything moved.
    fn bumped(&self, moves: Moves) -> Moves {
        if !moves.is_empty() {
            tracing::debug!("frames: {} folders moved", moves.folders().len());
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
        moves
    }

    /// Once a session, drop the roots no file and no frame this session
    /// names. `named` gives the files' roots; `None` drops nothing. A root
    /// added while `named` reads stays: its file may not be written yet.
    pub(crate) fn prune(&self, named: impl FnOnce() -> Option<HashSet<RootId>>) -> Result<()> {
        if self.pruned.load(Ordering::Relaxed) {
            return Ok(());
        }
        let before: HashSet<RootId> = self.sources().roots().map(|(id, _)| id.clone()).collect();
        let Some(mut in_use) = named() else {
            tracing::debug!("frames: not pruned, a file of the Library couldn't be read");
            return Ok(());
        };
        self.change(|sources| {
            // Within the change, so every earlier change is seen.
            in_use.extend(lock(&self.named).frames.values().map(|f| f.root.clone()));
            in_use.extend(
                sources.roots().map(|(id, _)| id).filter(|id| !before.contains(*id)).cloned(),
            );
            sources.prune(InUse::Read(&in_use)).map(drop)
        })?;
        self.pruned.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Change the roots, one change at a time, from the roots held (the only
    /// writer of `sources.json`). On an error they are read from disk again;
    /// if that fails they stay as they were.
    fn change<T>(&self, change: impl FnOnce(&mut Sources) -> Result<T>) -> Result<T> {
        let _one_at_a_time = lock(&self.changing);
        let mut sources = (*self.sources()).clone();
        let before = sources.clone();
        let changed = change(&mut sources);
        if changed.is_ok() {
            self.bookmark_moved(&before, &mut sources);
        }
        if changed.is_err() {
            match Sources::open(&Disk, self.places.clone()) {
                Ok(read) => sources = read,
                Err(e) => {
                    tracing::warn!("the roots couldn't be read again: {e}");
                    return changed;
                }
            }
        }
        *lock(&self.current) = Arc::new(sources);
        changed
    }

    /// A bookmark for each top root a change added or moved, or kept with
    /// none; a moved root whose bookmark fails drops its old one. Only with
    /// [`Self::make_bookmarks_with`].
    fn bookmark_moved(&self, before: &Sources, sources: &mut Sources) {
        let Some(make) = self.make_bookmark.get() else { return };
        let wanted: Vec<(RootId, bool)> = sources
            .roots()
            .filter_map(|(id, root)| {
                let Place::Volume { bookmark, .. } = &root.place else { return None };
                let moved = before.root(id).is_none_or(|was| !same_place(&was.place, &root.place));
                (moved || bookmark.is_none()).then(|| (id.clone(), moved))
            })
            .collect();
        for (id, moved) in wanted {
            let Some(folder) = sources.root_path(&id) else { continue };
            let made = (make.0)(&folder);
            if made.is_none() && !moved {
                continue;
            }
            match sources.set_bookmark(&id, made) {
                Ok(()) => tracing::debug!("frames: root {id}'s bookmark made again"),
                Err(e) => tracing::warn!("frames: root {id}'s bookmark wasn't kept: {e}"),
            }
        }
    }
}

/// [`Frames::keep`] within one change: the volume's missing roots found
/// under `folder`, proved by `samples`, then it kept. Returns the roots
/// moved and the top root covering it.
fn keep_in(
    sources: &mut Sources,
    folder: &Path,
    bookmark: Bookmark,
    samples: &HashMap<RootId, Vec<FrameRef>>,
) -> Result<(Vec<RootId>, RootId)> {
    let found = sources.found_volume(&Disk, folder, samples)?;
    let root = sources.keep_folder(&Disk, folder, bookmark)?;
    Ok((found.moved, root))
}

/// Two places of a root on a volume are one, whatever their bookmarks.
fn same_place(a: &Place, b: &Place) -> bool {
    match (a, b) {
        (
            Place::Volume { volume, path, mount, .. },
            Place::Volume { volume: v, path: p, mount: m, .. },
        ) => volume == v && path == p && mount == m,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Library in `dir/Library`, and photos beside it.
    fn library() -> (tempfile::TempDir, Frames) {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("Library");
        std::fs::create_dir_all(&library).unwrap();
        (dir, Frames::open(Places { library, app: None }).unwrap())
    }

    fn photos(dir: &Path, folder: &str, names: &[&str]) -> Vec<PathBuf> {
        let folder = dir.join(folder);
        std::fs::create_dir_all(&folder).unwrap();
        names
            .iter()
            .map(|n| {
                std::fs::write(folder.join(n), n.as_bytes()).unwrap();
                folder.join(n)
            })
            .collect()
    }

    #[test]
    fn a_path_handed_in_comes_back_as_it_was() {
        let (dir, frames) = library();
        let paths = photos(dir.path(), "Bee", &["1.tif", "2.tif"]);
        let refs = frames.refs(&[paths[1].clone(), paths[0].clone(), paths[1].clone()]).unwrap();
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0], refs[2]);
        assert_eq!(frames.sources().roots().count(), 1, "one root for the folder");
        assert_eq!(
            frames.paths(&refs).unwrap(),
            [&paths[1], &paths[0], &paths[1]].map(Clone::clone)
        );

        // A new session: from the roots, the folder in its canonical form.
        let again = Frames::open(frames.places.clone()).unwrap();
        let canon = std::fs::canonicalize(dir.path().join("Bee")).unwrap();
        assert_eq!(again.paths(&refs[..1]).unwrap(), [canon.join("2.tif")]);
        // Named again without a read; a missing unread file from its root.
        std::fs::remove_file(&paths[1]).unwrap();
        assert_eq!(again.refs(&[canon.join("2.tif")]).unwrap(), &refs[..1]);
        // A spelling not named before: through a link on Unix, where the temporary
        // folder may already be canonical.
        #[cfg(unix)]
        let unnamed = {
            let alias = dir.path().join("alias");
            std::os::unix::fs::symlink(&canon, &alias).unwrap();
            alias.join("2.tif")
        };
        #[cfg(not(unix))]
        let unnamed = paths[1].clone();
        let other = again.refs(std::slice::from_ref(&unnamed)).unwrap().remove(0);
        assert!(other.same_file(&refs[0]) && other.size == 0, "{other:?}");
    }

    #[test]
    fn a_frame_of_a_root_not_kept_is_refused() {
        let (dir, frames) = library();
        let paths = photos(dir.path(), "Bee", &["1.tif"]);
        let mut frame = frames.refs(&paths).unwrap().remove(0);
        frame.root = RootId::parse("0190a1b2c3d47e8f9a0b1c2d3e4f5a6b").unwrap();
        assert!(matches!(frames.paths(&[frame]), Err(SourcesError::UnknownRoot(_))));
    }

    /// A frame gone since it was read is named from the root covering it;
    /// one no root covers is refused.
    #[test]
    fn a_frame_no_longer_there_is_named_from_its_folder() {
        let (dir, frames) = library();
        photos(dir.path(), "Bee", &["1.tif"]);
        let gone = dir.path().join("Bee/2.tif");
        frames.refs(&[dir.path().join("Bee/1.tif")]).unwrap();
        let named = frames.refs(std::slice::from_ref(&gone)).unwrap().remove(0);
        assert_eq!((named.path.as_str(), named.size), ("2.tif", 0));
        assert_eq!(frames.paths(&[named]).unwrap(), [gone]);
        let nowhere = dir.path().join("Elsewhere/3.tif");
        assert!(matches!(frames.refs(&[nowhere]), Err(SourcesError::NotFile(_))));
    }

    #[test]
    fn relinking_moves_a_root_a_subfolder_or_the_roots_inside_a_folder() {
        let (dir, frames) = library();
        let at = |p: &str| dir.path().join(p);
        let top = photos(dir.path(), "trip", &["a.tif"]);
        let day = photos(dir.path(), "trip/day1", &["b.tif"]);
        let refs = frames.refs(&[top[0].clone(), day[0].clone()]).unwrap();
        assert_eq!(frames.sources().roots().count(), 1, "the folder added first covers the other");

        // The subfolder alone moved: split off, in the front end's form.
        std::fs::rename(at("trip/day1"), at("day1")).unwrap();
        let moves = frames.relink(&at("trip/day1"), &at("day1"), None).unwrap();
        assert_eq!(moves.folders(), [(at("trip/day1"), at("day1"))]);
        assert_eq!(moves.path(&day[0]), Some(at("day1/b.tif")));
        assert_eq!(moves.path(&top[0]), None);
        assert_eq!(frames.paths(&refs).unwrap(), [top[0].clone(), at("day1/b.tif")]);
        assert_eq!(frames.sources().roots().count(), 2);

        // A folder above both roots: both move with it.
        std::fs::create_dir_all(at("all")).unwrap();
        std::fs::rename(at("trip"), at("all/trip")).unwrap();
        std::fs::rename(at("day1"), at("all/day1")).unwrap();
        let moves = frames.relink(dir.path(), &at("all"), None).unwrap();
        assert_eq!(moves.folders().len(), 2);
        let now = [at("all/trip/a.tif"), at("all/day1/b.tif")];
        assert_eq!(frames.paths(&refs).unwrap(), now);
        assert_eq!(frames.generation(), 2);
        // Named as they were, under the new place too, without a read.
        std::fs::remove_file(&now[0]).unwrap();
        assert_eq!(frames.refs(&now[..1]).unwrap(), &refs[..1]);
        assert_eq!(frames.refs(&top).unwrap(), &refs[..1]);

        // The roots kept their place for the next session.
        let again = Frames::open(frames.places.clone()).unwrap();
        let canon = std::fs::canonicalize(at("all")).unwrap();
        assert_eq!(
            again.paths(&refs).unwrap(),
            [canon.join("trip/a.tif"), canon.join("day1/b.tif")]
        );
    }

    /// Locate… matches by name (folded), size and fingerprint and relinks
    /// whatever it found.
    #[test]
    fn locating_says_how_many_frames_the_folder_picked_has() {
        let (dir, frames) = library();
        let at = |p: &str| dir.path().join(p);
        let mut paths = photos(dir.path(), "card", &["a.tif", "b.tif", "c.tif"]);
        paths.extend(photos(dir.path(), "card/sub", &["d.tif"]));
        frames.refs(&paths).unwrap();
        std::fs::rename(at("card"), at("Bee")).unwrap();
        // One renamed in case only, one edited, one gone.
        std::fs::rename(at("Bee/a.tif"), at("Bee/A.TIF")).unwrap();
        std::fs::write(at("Bee/b.tif"), "edited").unwrap();
        std::fs::remove_file(at("Bee/c.tif")).unwrap();

        let located = frames.locate(&at("card"), &at("Bee"), None).unwrap();
        assert_eq!(located.matched, Matched { found: 2, of: 4 });
        assert_eq!(located.moves.folders(), [(at("card"), at("Bee"))]);
    }

    /// A top root gets a bookmark as it is added and as it moves.
    #[test]
    fn a_top_root_is_bookmarked_where_it_is_added_and_moved() {
        let (dir, frames) = library();
        let made = |path: &Path| Some(Bookmark(path.as_os_str().as_encoded_bytes().to_vec()));
        frames.make_bookmarks_with(made);
        let paths = photos(dir.path(), "card", &["a.tif"]);
        let root = frames.refs(&paths).unwrap().remove(0).root;
        let bookmark = |frames: &Frames| frames.sources().bookmark(&root).cloned();
        let at = |p: &str| std::fs::canonicalize(dir.path()).unwrap().join(p);
        assert_eq!(bookmark(&frames), made(&at("card")));

        std::fs::rename(dir.path().join("card"), dir.path().join("Bee")).unwrap();
        frames.relink(&dir.path().join("card"), &dir.path().join("Bee"), None).unwrap();
        assert_eq!(bookmark(&frames), made(&at("Bee")));
        let again = Frames::open(frames.places.clone()).unwrap();
        assert_eq!(bookmark(&again), made(&at("Bee")), "kept in sources.json");
    }

    /// At launch a moved root is found through its bookmark; moved where
    /// nothing leads, it stays missing.
    #[test]
    fn a_root_found_again_moves_its_frames() {
        let (dir, frames) = library();
        let at = |p: &str| dir.path().join(p);
        let refs = frames.refs(&photos(dir.path(), "card", &["a.tif"])).unwrap();
        let root = refs[0].root.clone();
        let places = frames.places.clone();
        drop(frames);
        std::fs::rename(at("card"), at("Bee")).unwrap();

        // A new session: the folder isn't where it was.
        let frames = Frames::open(places.clone()).unwrap();
        let was = frames.paths(&refs).unwrap().remove(0);
        assert!(frames.is_lost(&root));
        assert!(frames.find_again(&[]).unwrap().is_empty());
        let led = Resolution { root: root.clone(), path: at("Bee"), fresh: None };
        let moves = frames.find_again(&[led]).unwrap();
        let now = std::fs::canonicalize(at("Bee")).unwrap();
        assert_eq!(moves.path(&was), Some(now.join("a.tif")));
        assert_eq!(frames.paths(&refs).unwrap(), [now.join("a.tif")]);
        assert!(!frames.is_lost(&root));
        assert_eq!(frames.generation(), 1);

        // Moved again, with nothing leading there: missing until located.
        std::fs::rename(at("Bee"), at("Wasp")).unwrap();
        assert!(frames.find_again(&[]).unwrap().is_empty());
        assert!(frames.is_lost(&root));
        assert_eq!(Frames::open(places).unwrap().paths(&refs).unwrap(), [now.join("a.tif")]);
    }

    /// A picked top root keeps the pick's bookmark; one inside a root keeps
    /// none; a located folder keeps the picked one's.
    #[test]
    fn a_folder_picked_keeps_its_bookmark_at_the_top_only() {
        let (dir, frames) = library();
        let at = |p: &str| dir.path().join(p);
        let mark = |s: &str| Bookmark(s.as_bytes().to_vec());
        let paths = photos(dir.path(), "card", &["a.tif"]);
        photos(dir.path(), "card/sub", &["b.tif"]);
        let kept = frames.keep(&at("card"), mark("one")).unwrap();
        assert!(kept.moves.is_empty());
        let root = kept.root.clone();
        assert_eq!(frames.sources().bookmark(&root), Some(&mark("one")));
        assert_eq!(frames.keep(&at("card"), mark("two")).unwrap().root, root);
        assert_eq!(frames.sources().bookmark(&root), Some(&mark("two")), "the newer pick");
        assert_eq!(frames.keep(&at("card/sub"), mark("sub")).unwrap().root, root);
        assert_eq!(frames.sources().roots().count(), 1, "covered by the folder above");
        assert_eq!(frames.refs(&paths).unwrap()[0].root, root);

        std::fs::rename(at("card"), at("Bee")).unwrap();
        let located = frames.locate(&at("card"), &at("Bee"), Some(mark("bee"))).unwrap();
        assert_eq!((located.matched.found, located.matched.of), (1, 1), "the frame named");
        assert_eq!(located.moves.path(&at("card")), Some(at("Bee")));
        assert_eq!(frames.sources().bookmark(&root), Some(&mark("bee")));
        assert_eq!(
            frames.top_roots(&[at("Bee/sub/b.tif"), at("Bee"), at("x.tif")]),
            [Some(root.clone()), Some(root.clone()), None]
        );
        assert_eq!(frames.folder_of(&root), Some(at("Bee")));
        assert_eq!(frames.now(&[paths[0].clone(), at("x.tif")]), [at("Bee/a.tif"), at("x.tif")]);
        let tops = frames.top_folders();
        assert_eq!(tops.len(), 1);
        assert_eq!(
            (&tops[0].root, &tops[0].folder, tops[0].reach),
            (&root, &at("Bee"), Reach::There)
        );
    }

    /// Copy into Deptheos relinks only to a whole copy, else names what it
    /// lacks.
    #[test]
    fn a_copy_is_taken_only_whole() {
        let (dir, frames) = library();
        let at = |p: &str| dir.path().join(p);
        let mut paths = photos(dir.path(), "card", &["a.tif", "b.tif"]);
        paths.extend(photos(dir.path(), "card/sub", &["c.tif"]));
        photos(dir.path(), "card", &["unused.tif"]);
        let refs = frames.refs(&paths).unwrap();
        assert_eq!(frames.named_in(&at("card")), ["a.tif", "b.tif", "sub/c.tif"]);

        photos(dir.path(), "copy", &["a.tif"]);
        std::fs::write(at("copy/b.tif"), "not b").unwrap();
        let lacking = frames.adopt(&at("card"), &at("copy"));
        assert!(
            matches!(&lacking, Err(SourcesError::NotCopied(f)) if f == &["b.tif", "sub/c.tif"]),
            "{lacking:?}"
        );
        assert_eq!(frames.paths(&refs).unwrap(), paths, "left where it was");

        photos(dir.path(), "copy", &["b.tif"]);
        photos(dir.path(), "copy/sub", &["c.tif"]);
        let moves = frames.adopt(&at("card"), &at("copy")).unwrap();
        assert_eq!(moves.folders(), [(at("card"), at("copy"))]);
        assert_eq!(frames.paths(&refs).unwrap()[2], at("copy/sub/c.tif"));
        assert!(frames.adopt(&at("nothing"), &at("copy")).unwrap().is_empty());
    }

    /// A frame is there, missing (its folder gone, or the file) or, under
    /// a root whose volume isn't there, not connected.
    #[test]
    fn a_frame_on_a_volume_not_there_is_not_connected() {
        let (dir, frames) = library();
        let at = |p: &str| dir.path().join(p);
        let paths = photos(dir.path(), "card", &["a.tif", "b.tif"]);
        frames.refs(&paths).unwrap();
        std::fs::remove_file(&paths[1]).unwrap();
        let stray = at("elsewhere.tif");
        let reach = frames.reach(&[paths[0].clone(), paths[1].clone(), stray]);
        assert_eq!(reach, [Reach::There, Reach::Missing, Reach::Missing]);
        std::fs::remove_dir_all(at("card")).unwrap();
        assert_eq!(frames.reach(&paths[..1]), [Reach::Missing]);

        // The root's volume isn't mounted where it was.
        let root = frames.refs(&paths[..1]).unwrap().remove(0).root;
        let file = frames.places.library.join(crate::frames::sources::SOURCES_FILE);
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        json["roots"][root.as_str()]["place"]["mount"] = "/Volumes/Nowhere".into();
        std::fs::write(&file, json.to_string()).unwrap();
        let again = Frames::open(frames.places.clone()).unwrap();
        let named = again.paths(&frames.refs(&paths[..1]).unwrap()).unwrap();
        assert_eq!(again.reach(&named), [Reach::NotConnected]);
    }

    #[test]
    fn pruning_keeps_what_the_files_or_this_session_name() {
        let (dir, frames) = library();
        let kept = frames.refs(&photos(dir.path(), "Kept", &["1.tif"])).unwrap();
        let session = frames.refs(&photos(dir.path(), "Session", &["2.tif"])).unwrap();
        let places = frames.places.clone();
        drop(frames);

        // Unread files prune nothing.
        let frames = Frames::open(places.clone()).unwrap();
        frames.prune(|| None).unwrap();
        assert_eq!(frames.sources().roots().count(), 2);
        frames.paths(&session).unwrap();
        frames.prune(|| Some(HashSet::from([kept[0].root.clone()]))).unwrap();
        assert_eq!(frames.sources().roots().count(), 2, "this session read the other");

        let frames = Frames::open(places).unwrap();
        frames.prune(|| Some(HashSet::from([kept[0].root.clone()]))).unwrap();
        frames.prune(|| unreachable!("once a session")).unwrap();
        let roots: Vec<RootId> = frames.sources().roots().map(|(id, _)| id.clone()).collect();
        assert_eq!(roots, [kept[0].root.clone()]);
    }

    /// A root added while the Library is read for its roots stays.
    #[test]
    fn pruning_keeps_a_root_added_while_the_library_was_read() {
        let (dir, frames) = library();
        frames.refs(&photos(dir.path(), "Old", &["1.tif"])).unwrap();
        let places = frames.places.clone();
        drop(frames);

        let frames = Frames::open(places).unwrap();
        let folder = dir.path().join("Picked");
        std::fs::create_dir_all(&folder).unwrap();
        let mut picked = None;
        frames
            .prune(|| {
                picked = Some(frames.keep(&folder, Bookmark(vec![1, 2, 3])).unwrap().root);
                Some(HashSet::new())
            })
            .unwrap();
        let roots: Vec<RootId> = frames.sources().roots().map(|(id, _)| id.clone()).collect();
        assert_eq!(roots, [picked.unwrap()], "the old root went, the one just kept stayed");
    }
}
