//! The Library's roots, `sources.json` (ADR-145). A frame is a
//! [`FrameRef`], a root and a path relative to it, so a folder that moves
//! rewrites one entry here and nothing else.
//!
//! A root is a [`Place`]: on a volume (with a bookmark), anchored in the
//! Library or the app's storage, or within another root. `split_from`
//! holds another root's frames under a subfolder split off from it.
//!
//! A frame's identity is its path from its deepest covering root
//! ([`Sources::identity`]), compared exactly; matching compares a [`key`]
//! (NFC, case folded). Paths are worked out when read
//! ([`Sources::resolve`]), never stored.
//!
//! File-system facts come through a [`Probe`] ([`Disk`] is the real one).
//! Front ends make and resolve bookmarks; [`Sources::find_again`] takes
//! back what they resolved to. A root's state ([`RootState`]) is worked
//! out each time, not kept.

mod change;
mod disk;
mod find;
mod probe;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

pub use disk::{Disk, canonical};
pub use find::Resolution;
pub use probe::{FINGERPRINT_END, FileId, Fingerprint, Mounted, Probe, Same, Stat, in_trash, key};

use crate::fsutil::{StoreError, Versioned, parse_versioned, write_json_atomic};
use crate::library::ids::{is_id, new_id};

/// The file in the Library root that lists the roots.
pub const SOURCES_FILE: &str = "sources.json";

/// A copy of `sources.json`, written after it, so one of the two always
/// holds the last write or the one before: what the Library opens from
/// when `sources.json` is damaged.
pub const PREVIOUS_SOURCES_FILE: &str = "sources.previous.json";

/// The Library's format version; an older build refuses a newer Library
/// whole (`fsutil.rs`'s module doc).
///
/// - 1: builds before 0.2.0 (the `library-0.2.0` fixture).
/// - 2: Recently Deleted, `Deleted/` (ADR-140); 0.2.0 on. A 1 is moved to
///   2 on open, changing only this number.
pub(crate) const SOURCES_SCHEMA: u32 = 2;

/// `sources.json`'s `written_by` for this build.
const THIS_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Error, Debug)]
pub enum SourcesError {
    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("{} is not inside {}", file.display(), folder.display())]
    NotInside { file: PathBuf, folder: PathBuf },

    #[error("{} has a name that isn't Unicode", .0.display())]
    NotUnicode(PathBuf),

    #[error("{} is not a folder", .0.display())]
    NotFolder(PathBuf),

    #[error("{} is not a file", .0.display())]
    NotFile(PathBuf),

    #[error("{0:?} is not a folder inside a root")]
    NotSubfolder(String),

    #[error("no root {0}")]
    UnknownRoot(RootId),

    /// Only a root on a volume keeps a bookmark.
    #[error("root {0} keeps no bookmark")]
    NoBookmark(RootId),

    /// A copy (Copy into Deptheos) missing these frames, by path, as
    /// checked by size and fingerprint.
    #[error("not copied whole: {}", .0.join(", "))]
    NotCopied(Vec<String>),

    /// `sources.json` can't be parsed: the Library is refused and the file
    /// left as it is, since no render can name its frames without it.
    #[error("{}: damaged", path.display())]
    Damaged {
        path: PathBuf,
        #[source]
        source: StoreError,
    },
}

type Result<T> = std::result::Result<T, SourcesError>;

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> SourcesError + '_ {
    move |source| SourcesError::Store(StoreError::Io { path: path.to_path_buf(), source })
}

/// A root's id; only an id this store could have written parses.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct RootId(String);

impl From<RootId> for String {
    fn from(id: RootId) -> Self {
        id.0
    }
}

impl TryFrom<String> for RootId {
    type Error = String;
    fn try_from(id: String) -> std::result::Result<Self, String> {
        Self::parse(&id).ok_or_else(|| format!("not a root id: {id:?}"))
    }
}

impl std::fmt::Display for RootId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl RootId {
    fn new() -> Self {
        Self(new_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// An id as [`Self::as_str`] wrote it; `None` for anything else.
    pub fn parse(id: &str) -> Option<Self> {
        is_id(id).then(|| Self(id.to_string()))
    }
}

/// A frame: its root, its `/`-separated path from it, and its size and
/// fingerprint when added (evidence for matching, never a gate on reading).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameRef {
    pub root: RootId,
    pub path: String,
    pub size: u64,
    pub fingerprint: Fingerprint,
}

impl FrameRef {
    /// The same root and path, exactly. Two refs are one frame when their
    /// [`Sources::identity`] is.
    pub fn same_file(&self, other: &FrameRef) -> bool {
        self.root == other.root && self.path == other.path
    }
}

/// A volume's identity. `id` is `uuid:…` or `serial:…`, compared only
/// within a scheme; a share is named by server and share, never by mount
/// point or drive letter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub kind: VolumeKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VolumeKind {
    Disk,
    Share { server: String, share: String },
}

/// Folders that move as a whole (the user moves the Library; iOS moves the
/// app's container between installs), so roots in them are relative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Library,
    App,
}

/// A front end's bookmark for a root's folder (written as hex).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Bookmark(pub Vec<u8>);

impl std::fmt::Debug for Bookmark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bookmark({} bytes)", self.0.len())
    }
}

impl From<Bookmark> for String {
    fn from(b: Bookmark) -> Self {
        b.0.iter().map(|x| format!("{x:02x}")).collect()
    }
}

impl TryFrom<String> for Bookmark {
    type Error = String;
    fn try_from(hex: String) -> std::result::Result<Self, String> {
        let bad = || "a bookmark is hex".to_string();
        if !hex.len().is_multiple_of(2) {
            return Err(bad());
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
            .collect::<Option<Vec<u8>>>()
            .map(Self)
            .ok_or_else(bad)
    }
}

/// Where a root is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "snake_case")]
pub enum Place {
    /// `path` is within the volume (empty for its top); `mount` is where it
    /// was last seen.
    Volume {
        volume: Volume,
        path: String,
        mount: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bookmark: Option<Bookmark>,
    },
    /// Relative to the Library folder or the app's storage.
    Anchored { anchor: Anchor, path: String },
    /// Inside another root, covered by its bookmark.
    Within { root: RootId, path: String },
}

/// A root's subfolder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Under {
    pub root: RootId,
    pub path: String,
}

/// A root: where it is, and the root it was split off from, if any.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub place: Place,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_from: Option<Under>,
    /// Fields a newer build wrote, kept as they are.
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl Root {
    fn at(place: Place) -> Self {
        Self { place, split_from: None, extra: Map::new() }
    }
}

/// Frames from `from` under `path` are named from `to`: a subfolder split
/// off, or a root found inside another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Renamed {
    pub from: RootId,
    pub path: String,
    pub to: RootId,
}

impl Renamed {
    /// `frame` as renamed, if this renames it.
    pub fn apply(&self, frame: &FrameRef) -> Option<FrameRef> {
        if frame.root != self.from {
            return None;
        }
        let rest = strip_rel(&frame.path, &self.path)?;
        Some(FrameRef { root: self.to.clone(), path: rest.to_string(), ..frame.clone() })
    }
}

/// What a relink or find changed: roots rewritten (their bookmarks may want
/// remaking) and frames renamed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relinked {
    pub moved: Vec<RootId>,
    pub renamed: Vec<Renamed>,
}

/// Where a root is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootState {
    At(PathBuf),
    /// The volume is there but the folder isn't, or it is in a trash.
    Missing,
    /// Its volume isn't there.
    NotConnected,
}

/// What a frame resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    There(PathBuf),
    /// Size or fingerprint differs: used anyway, so renders from it may not
    /// match it.
    Changed(PathBuf),
    /// The volume is there but the file isn't, or it is in a trash.
    Missing,
    /// Its volume isn't there.
    NotConnected,
}

/// How a frame is checked when it is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// By size: a stat.
    Size,
    /// By size and fingerprint: 128 KiB read.
    Contents,
}

/// "Found N of M": a root's frames a folder has, by folded name, size and
/// fingerprint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Matched {
    pub found: usize,
    pub of: usize,
}

impl Matched {
    /// All of a nonempty sample.
    pub fn all(self) -> bool {
        self.of > 0 && self.found == self.of
    }
}

/// Which roots the Library's files name, for [`Sources::prune`].
#[derive(Clone, Copy, Debug)]
pub enum InUse<'a> {
    /// The Library hasn't been read in full: nothing is pruned.
    Unread,
    Read(&'a HashSet<RootId>),
}

/// The folders anchored roots are relative to, as they are now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places {
    pub library: PathBuf,
    /// The app's storage, where the app has its own (iOS's container).
    pub app: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct SourcesFile {
    schema: u32,
    /// The app version that last opened the Library; none from 0.2.0 or
    /// earlier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    written_by: Option<String>,
    roots: BTreeMap<RootId, Root>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl Versioned for SourcesFile {
    const SCHEMA: u32 = SOURCES_SCHEMA;
}

/// The Library's roots. Every change is written before the call returns.
#[derive(Clone, Debug)]
pub struct Sources {
    file: PathBuf,
    /// The anchors, canonical: what folders are matched against.
    places: Places,
    /// The anchors as the front end named them, for [`Self::path_of`]: iOS
    /// names its container `/var/…` (canonically `/private/var/…`) and
    /// compares paths it made with ones it is given back.
    named: Places,
    roots: BTreeMap<RootId, Root>,
    extra: Map<String, Value>,
    /// The version that last wrote the file, as read.
    written_by: Option<String>,
}

impl Sources {
    /// Reads `sources.json` (none when absent). A newer format is refused
    /// ([`StoreError::NewerLibrary`]). A damaged file is set aside and
    /// restored from `sources.previous.json`; with no readable copy, or
    /// when the file can't be read at all, it is refused
    /// ([`SourcesError::Damaged`]) and left in place.
    pub fn open(probe: &dyn Probe, places: Places) -> Result<Self> {
        let canonical = |p: &Path| probe.canonical(p).unwrap_or_else(|_| p.to_path_buf());
        let named = places.clone();
        let places = Places {
            library: canonical(&places.library),
            app: places.app.as_deref().map(canonical),
        };
        let file = places.library.join(SOURCES_FILE);
        let (read, restored) = match crate::fsutil::read_if_present(&file) {
            Ok(Some(bytes)) => match read_sources(&places.library, &file, &bytes) {
                Ok(read) => (Some(read), false),
                Err(damaged @ SourcesError::Damaged { .. }) => {
                    (Some(read_previous(&places.library, &file, damaged)?), true)
                }
                Err(e) => return Err(e),
            },
            Ok(None) => (None, false),
            Err(source) => return Err(StoreError::Io { path: file, source }.into()),
        };
        let upgraded = read.as_ref().is_some_and(|r| r.schema < SOURCES_SCHEMA);
        if let Some(read) = read.as_ref().filter(|_| upgraded) {
            tracing::info!(
                "the Library's format is {}: moved to {SOURCES_SCHEMA}, nothing else changed",
                read.schema
            );
        }
        let (roots, extra, written_by) =
            read.map(|r| (r.roots, r.extra, r.written_by)).unwrap_or_default();
        tracing::debug!("sources: {} roots, last written by {written_by:?}", roots.len());
        let mut sources = Self { file, places, named, roots, extra, written_by };
        if restored || upgraded {
            sources.save()?;
        }
        Ok(sources)
    }

    /// Write the file as this version's, unless it is already, so an older
    /// build can say which version last used it.
    pub(crate) fn stamp(&mut self) -> Result<()> {
        if self.written_by.as_deref() == Some(THIS_VERSION) {
            return Ok(());
        }
        tracing::info!("the Library, last used by {:?}, is now {THIS_VERSION}'s", self.written_by);
        self.save()
    }

    /// Write the roots, and then their copy, `sources.previous.json`.
    fn save(&mut self) -> Result<()> {
        let file = SourcesFile {
            schema: SOURCES_SCHEMA,
            written_by: Some(THIS_VERSION.to_string()),
            roots: self.roots.clone(),
            extra: self.extra.clone(),
        };
        write_json_atomic(&self.file, &file)?;
        let copy = self.file.with_file_name(PREVIOUS_SOURCES_FILE);
        if let Err(e) = write_json_atomic(&copy, &file) {
            tracing::warn!("couldn't keep the copy of the roots: {e}");
        }
        self.written_by = file.written_by;
        Ok(())
    }

    pub fn roots(&self) -> impl Iterator<Item = (&RootId, &Root)> {
        self.roots.iter()
    }

    pub fn root(&self, id: &RootId) -> Option<&Root> {
        self.roots.get(id)
    }

    /// The bookmark of a root on a volume, for the front end to resolve.
    pub fn bookmark(&self, id: &RootId) -> Option<&Bookmark> {
        match &self.roots.get(id)?.place {
            Place::Volume { bookmark, .. } => bookmark.as_ref(),
            _ => None,
        }
    }

    /// Keep a new bookmark for a root on a volume (a stale one remade, or a
    /// root found elsewhere).
    pub fn set_bookmark(&mut self, id: &RootId, to: Option<Bookmark>) -> Result<()> {
        let root = self.roots.get_mut(id).ok_or_else(|| SourcesError::UnknownRoot(id.clone()))?;
        let Place::Volume { bookmark, .. } = &mut root.place else {
            return Err(SourcesError::NoBookmark(id.clone()));
        };
        if *bookmark == to {
            return Ok(());
        }
        *bookmark = to;
        tracing::debug!("sources: root {id} has a new bookmark");
        self.save()
    }

    /// `frame` named from its deepest covering root, following roots within
    /// or split off from its own.
    pub fn identity(&self, frame: &FrameRef) -> FrameRef {
        let mut frame = frame.clone();
        for _ in 0..=self.roots.len() {
            let deeper = self
                .roots
                .iter()
                .filter_map(|(id, root)| {
                    let under = match &root.place {
                        Place::Within { root, path } if *root == frame.root => Some(path),
                        _ => None,
                    };
                    let split = root.split_from.as_ref().filter(|u| u.root == frame.root);
                    [under, split.map(|u| &u.path)]
                        .into_iter()
                        .flatten()
                        .filter_map(|path| Some((id, path, strip_rel(&frame.path, path)?)))
                        .max_by_key(|(_, path, _)| path.len())
                })
                .max_by_key(|(_, path, _)| path.len());
            match deeper {
                Some((id, _, rest)) if *id != frame.root => {
                    frame = FrameRef { root: id.clone(), path: rest.to_string(), ..frame };
                }
                _ => break,
            }
        }
        frame
    }

    /// Where the root `id` is now. An unknown root is missing.
    pub fn root_state(&self, probe: &dyn Probe, id: &RootId) -> RootState {
        self.state_at(probe, id, 0)
    }

    fn state_at(&self, probe: &dyn Probe, id: &RootId, depth: usize) -> RootState {
        let Some(root) = self.roots.get(id).filter(|_| depth <= self.roots.len()) else {
            return RootState::Missing;
        };
        match &root.place {
            Place::Volume { volume, path, mount, .. } => {
                let folder = join(mount, path);
                // Ask the nearest existing folder, not the mount: on macOS
                // the boot volume's data is reached through `/`, another
                // volume.
                let there = folder.ancestors().take_while(|a| a.starts_with(mount)).find_map(|a| {
                    probe.stat(a).ok().flatten().filter(|s| s.folder).and(probe.volume(a).ok())
                });
                // Its own when the ids agree, or it was only ever known by
                // name. One known by id but matched only by name must be
                // proved by its frames ([`Self::find_again`]).
                let own = there.is_some_and(|m| {
                    m.mount == *mount
                        && match volume.same(&m.volume) {
                            Same::Yes => true,
                            Same::Maybe => volume.id.is_none(),
                            Same::No => false,
                        }
                });
                if own { folder_state(probe, folder) } else { RootState::NotConnected }
            }
            Place::Anchored { anchor, path } => match self.base(*anchor) {
                Some(base) => folder_state(probe, join(base, path)),
                None => RootState::Missing,
            },
            Place::Within { root, path } => match self.state_at(probe, root, depth + 1) {
                RootState::At(parent) => folder_state(probe, join(&parent, path)),
                other => other,
            },
        }
    }

    /// What `frame` resolves to now, checked by `check`.
    pub fn resolve(&self, probe: &dyn Probe, frame: &FrameRef, check: Check) -> Resolved {
        let named = self.identity(frame);
        let base = match self.root_state(probe, &named.root) {
            RootState::At(base) => base,
            RootState::Missing => return Resolved::Missing,
            RootState::NotConnected => return Resolved::NotConnected,
        };
        let path = join(&base, &named.path);
        match probe.stat(&path) {
            Ok(Some(stat)) if !stat.folder && !in_trash(&path) => {
                let differs = stat.size != frame.size
                    || (check == Check::Contents
                        && Fingerprint::of(probe, &path, stat.size)
                            .is_ok_and(|f| f != frame.fingerprint));
                if differs { Resolved::Changed(path) } else { Resolved::There(path) }
            }
            Ok(_) => Resolved::Missing,
            Err(_) => Resolved::NotConnected,
        }
    }

    /// Where the root is or was last seen, without touching the disk; its
    /// anchor as named. `None` for an unknown root or an anchor this
    /// platform lacks.
    pub fn root_path(&self, id: &RootId) -> Option<PathBuf> {
        self.root_path_at(id, 0)
    }

    fn root_path_at(&self, id: &RootId, depth: usize) -> Option<PathBuf> {
        let root = self.roots.get(id).filter(|_| depth <= self.roots.len())?;
        match &root.place {
            Place::Volume { path, mount, .. } => Some(join(mount, path)),
            Place::Anchored { anchor, path } => {
                let base = match anchor {
                    Anchor::Library => Some(&self.named.library),
                    Anchor::App => self.named.app.as_ref(),
                };
                Some(join(base?, path))
            }
            Place::Within { root, path } => Some(join(&self.root_path_at(root, depth + 1)?, path)),
        }
    }

    /// `frame`'s identity under [`Self::root_path`], whether or not it is
    /// there ([`Self::resolve`] checks).
    pub fn path_of(&self, frame: &FrameRef) -> Option<PathBuf> {
        let named = self.identity(frame);
        Some(join(&self.root_path(&named.root)?, &named.path))
    }

    /// `frame`'s path from `root`, when `root` covers it.
    pub fn path_under(&self, frame: &FrameRef, root: &RootId) -> Option<String> {
        let named = self.identity(frame);
        let (mut at, mut path) = (named.root, named.path);
        for _ in 0..=self.roots.len() {
            if at == *root {
                return Some(path);
            }
            let Place::Within { root: parent, path: sub } = &self.roots.get(&at)?.place else {
                return None;
            };
            path = join_rel(sub, &path);
            at = parent.clone();
        }
        None
    }

    /// How many of `frames` `folder` has in `root`'s layout, by name
    /// (folded where the exact one is absent), size and fingerprint.
    pub fn matched(
        &self,
        probe: &dyn Probe,
        folder: &Path,
        root: &RootId,
        frames: &[FrameRef],
    ) -> Matched {
        let under = frames.iter().filter_map(|f| Some((self.path_under(f, root)?, f)));
        matched_under(probe, folder, under)
    }
}

/// How many of `frames` (relative path, frame) `folder` has at the same
/// path, by name (folded where the exact one is absent), size and
/// fingerprint.
pub fn matched_under<'a>(
    probe: &dyn Probe,
    folder: &Path,
    frames: impl IntoIterator<Item = (String, &'a FrameRef)>,
) -> Matched {
    let mut matched = Matched::default();
    for (path, frame) in frames {
        matched.of += 1;
        let Some(file) = find_named(probe, folder, &path) else { continue };
        let same = probe.stat(&file).ok().flatten().is_some_and(|s| {
            !s.folder
                && s.size == frame.size
                && Fingerprint::of(probe, &file, s.size).is_ok_and(|f| f == frame.fingerprint)
        });
        matched.found += usize::from(same);
    }
    matched
}

/// The state of a root whose folder would be `path`.
fn folder_state(probe: &dyn Probe, path: PathBuf) -> RootState {
    if in_trash(&path) {
        return RootState::Missing;
    }
    match probe.stat(&path) {
        Ok(Some(stat)) if stat.folder => RootState::At(path),
        Ok(_) => RootState::Missing,
        Err(_) => RootState::NotConnected,
    }
}

/// Relative `path` as stored, `/`-joined; refused unless every name is
/// plain Unicode. `whole` is for the error.
fn rel_string(path: &Path, whole: &Path) -> Result<String> {
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => names
                .push(name.to_str().ok_or_else(|| SourcesError::NotUnicode(whole.to_path_buf()))?),
            Component::CurDir => {}
            _ => return Err(SourcesError::NotUnicode(whole.to_path_buf())),
        }
    }
    Ok(names.join("/"))
}

/// The relative path `rel` under the folder `base`.
fn join(base: &Path, rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    path.extend(rel.split('/').filter(|n| !n.is_empty()));
    path
}

/// Two relative paths, one after the other.
fn join_rel(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}/{b}"),
    }
}

/// Parse `sources.json`. A later schema is a newer Library, refused; an
/// earlier one reads as this build's (each format is a superset of the
/// last); anything else unparseable is damaged.
fn read_sources(library: &Path, file: &Path, bytes: &[u8]) -> Result<SourcesFile> {
    #[derive(Deserialize)]
    struct Header {
        schema: u32,
        #[serde(default)]
        written_by: Option<String>,
    }
    if let Ok(header) = serde_json::from_slice::<Header>(bytes)
        && header.schema > SOURCES_SCHEMA
    {
        tracing::warn!(
            "{}: the Library's format is {}, newer than {SOURCES_SCHEMA}: refused, left as it is",
            file.display(),
            header.schema
        );
        let written_by = header.written_by;
        return Err(StoreError::NewerLibrary { library: library.to_path_buf(), written_by }.into());
    }
    let parsed = match serde_json::from_slice::<Header>(bytes) {
        Ok(header) if (1..SOURCES_SCHEMA).contains(&header.schema) => serde_json::from_slice(bytes)
            .map_err(|source| StoreError::Json { path: file.to_path_buf(), source }),
        _ => parse_versioned::<SourcesFile>(file, bytes),
    };
    parsed.map_err(|e| {
        tracing::warn!("{e}: refused, left as it is");
        SourcesError::Damaged { path: file.to_path_buf(), source: e }
    })
}

/// The roots from `sources.previous.json`, with the damaged `file` set
/// aside. With no readable copy, `damaged` stands and nothing moves.
fn read_previous(library: &Path, file: &Path, damaged: SourcesError) -> Result<SourcesFile> {
    let previous = file.with_file_name(PREVIOUS_SOURCES_FILE);
    let read = match crate::fsutil::read_if_present(&previous) {
        Ok(Some(bytes)) => read_sources(library, &previous, &bytes),
        Ok(None) => return Err(damaged),
        Err(source) => Err(StoreError::Io { path: previous.clone(), source }.into()),
    };
    match read {
        Ok(read) => {
            let aside = crate::fsutil::set_aside(file)
                .map_err(|source| StoreError::Io { path: file.to_path_buf(), source })?;
            tracing::warn!(
                "{} is damaged (set aside as {}): the Library opens from its copy, {}",
                file.display(),
                aside.display(),
                previous.display()
            );
            Ok(read)
        }
        // A newer build's copy: the Library is that build's.
        Err(newer @ SourcesError::Store(StoreError::NewerLibrary { .. })) => Err(newer),
        Err(e) => {
            tracing::warn!("{} can't stand in: {e}", previous.display());
            Err(damaged)
        }
    }
}

/// `path` without the folder `prefix` (whole names, exact); an empty
/// `prefix` covers everything.
fn strip_rel<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(path);
    }
    match path.strip_prefix(prefix)? {
        "" => Some(""),
        rest => rest.strip_prefix('/'),
    }
}

fn depth_of(rel: &str) -> usize {
    rel.split('/').filter(|n| !n.is_empty()).count()
}

/// The file at `rel` under `base`, each name exact or else matched by
/// [`key`].
fn find_named(probe: &dyn Probe, base: &Path, rel: &str) -> Option<PathBuf> {
    let exact = join(base, rel);
    if probe.stat(&exact).ok().flatten().is_some() {
        return Some(exact);
    }
    let mut at = base.to_path_buf();
    for name in rel.split('/').filter(|n| !n.is_empty()) {
        let next = at.join(name);
        if probe.stat(&next).ok().flatten().is_some() {
            at = next;
            continue;
        }
        let want = key(name);
        let found = probe.list(&at).ok()?.into_iter().find(|n| key(n) == want)?;
        at.push(found);
    }
    Some(at)
}
