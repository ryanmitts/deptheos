//! The store's file I/O: atomic writes (the exports' [`io::write_replacing`]),
//! versioned JSON, setting unreadable files aside, and free space.
//!
//! # The Library's format
//!
//! A Library has one format, `sources.json`'s schema (`SOURCES_SCHEMA`),
//! checked once as it opens. Any change to any Library file's format bumps
//! it, with each changed file's own schema. Upgrades are one way: the first
//! build of a new format moves every file to it, then writes the schema. An
//! older build refuses a newer Library whole, untouched
//! ([`StoreError::NewerLibrary`]), naming `sources.json`'s `written_by`.
//! Every later 0.x build opens any earlier build's Library (the
//! `library-<version>` fixtures).
//!
//! Inside a Library that passed, a file of another schema or that doesn't
//! parse is damaged: a render or failed stack is skipped, paint set aside,
//! `list.json` replaced by the next write, a stack's file set aside by the
//! next write. A file that couldn't be read (a drive that didn't answer) is
//! never taken for damaged.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use deptheos_core::{CoreError, io};
use deptheos_dng::DngError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::library::ids::new_id;

/// Suffix of a render directory built beside its final place, then renamed.
pub(crate) const PARTIAL_SUFFIX: &str = ".partial";

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },

    #[error("{}: {source}", path.display())]
    Json { path: PathBuf, source: serde_json::Error },

    /// A Library file of another format version: damaged, since the
    /// Library's format was checked as it opened.
    #[error("{}: format version {found} is not supported (this build reads {expected})", path.display())]
    Schema { path: PathBuf, found: u32, expected: u32 },

    /// The Library is of a newer format: refused whole, untouched.
    /// `written_by` is the version that last used it.
    #[error("{} is of a newer format, last used by {}", library.display(), written_by.as_deref().unwrap_or("a newer version"))]
    NewerLibrary { library: PathBuf, written_by: Option<String> },

    /// A file that parsed but is not what it should be (not a JSON object).
    #[error("{}: not a JSON object", path.display())]
    Damaged { path: PathBuf },

    /// The Library is open in another app, which holds its lock
    /// (`history/lock.rs`).
    #[error("{} is in use by another app", library.display())]
    InUse { library: PathBuf, holder: Option<crate::library::history::Holder> },

    /// Neither the Library's folder nor its parent exists: its drive isn't
    /// connected. Nothing is made in its place.
    #[error("{} isn't there, nor the folder it is in", library.display())]
    NotConnected { library: PathBuf },

    /// The Library's folder is kept by a sync service, which can damage it
    /// (`history/synced.rs`).
    #[error("{} is synced by {}", library.display(), service.name())]
    Synced { library: PathBuf, service: crate::library::history::SyncService },

    /// The Library's volume can't lock a file, so nothing would keep a second
    /// app out.
    #[error("{} can't be locked: {source}", library.display())]
    NoLocks { library: PathBuf, source: std::io::Error },

    /// A fault: a file this build wrote isn't as written, or a misuse.
    #[error("{0}")]
    Invalid(String),

    #[error(transparent)]
    Core(#[from] CoreError),

    #[error(transparent)]
    Dng(#[from] DngError),

    #[error(transparent)]
    Image(#[from] image::ImageError),

    /// A frame the Library's roots couldn't name or find (ADR-145).
    #[error(transparent)]
    Sources(Box<crate::frames::sources::SourcesError>),
}

/// A file error of the roots is the file error it was.
impl From<crate::frames::sources::SourcesError> for StoreError {
    fn from(e: crate::frames::sources::SourcesError) -> Self {
        match e {
            crate::frames::sources::SourcesError::Store(e) => e,
            e => Self::Sources(Box::new(e)),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, StoreError>;

/// For `io::Result` code (paint): an I/O error keeps its kind, the rest is
/// invalid data.
impl From<StoreError> for std::io::Error {
    fn from(e: StoreError) -> Self {
        let kind = match &e {
            StoreError::Io { source, .. } => source.kind(),
            _ => std::io::ErrorKind::InvalidData,
        };
        std::io::Error::new(kind, e)
    }
}

pub(crate) fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io { path: path.to_path_buf(), source }
}

/// The bytes of `path`; `None` when there is no such file.
pub(crate) fn read_if_present(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Remove the file `path`; one already gone counts as removed.
pub(crate) fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::Io { path: path.to_path_buf(), source: e }),
    }
}

/// The `<id>.json` files in `dir` whose stem `parse` accepts, unordered;
/// none when `dir` is missing.
pub(crate) fn json_files_by_id<Id>(
    dir: &Path,
    parse: impl Fn(&str) -> Option<Id>,
) -> Result<Vec<(Id, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(StoreError::Io { path: dir.to_path_buf(), source: e }),
    };
    Ok(entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
                .and_then(&parse)?;
            Some((id, path))
        })
        .collect())
}

/// Rename `from` to `to` ([`io::rename_retrying`]) and flush both folders,
/// so the rename survives a crash.
pub(crate) fn rename(from: &Path, to: &Path) -> Result<()> {
    io::rename_retrying(from, to).map_err(io_error(to))?;
    let (into, out_of) = (to.parent(), from.parent());
    sync_folder(into.unwrap_or(Path::new("")))?;
    if out_of != into {
        sync_folder(out_of.unwrap_or(Path::new("")))?;
    }
    Ok(())
}

/// Flush `folder` so its entries survive a crash ([`io::sync_folder`]).
pub(crate) fn sync_folder(folder: &Path) -> Result<()> {
    io::sync_folder(folder).map_err(io_error(folder))
}

/// Flush a written file to disk ([`io::sync_file`]). Opened for writing:
/// Windows refuses `FlushFileBuffers` on a read-only handle.
pub(crate) fn sync(path: &Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| io::sync_file(&f))
        .map_err(io_error(path))
}

pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(path).map_err(io_error(path))?;
    file.write_all(bytes).and_then(|()| io::sync_file(&file)).map_err(io_error(path))
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_file(path, &to_json(path, value)?)
}

fn to_json<T: Serialize>(path: &Path, value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value)
        .map_err(|source| StoreError::Json { path: path.to_path_buf(), source })
}

/// Write `bytes` to `path` through a temporary beside it
/// ([`io::write_replacing`]): a crash leaves the old file or the new.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_replacing(path, |tmp| Ok(std::fs::write(tmp, bytes)?))
}

/// [`io::write_replacing`] with store errors that name `path`.
pub(crate) fn write_replacing(
    path: &Path,
    write: impl FnOnce(&Path) -> deptheos_core::Result<()>,
) -> Result<()> {
    io::write_replacing(path, write).map_err(|e| match e {
        CoreError::Io(source) => StoreError::Io { path: path.to_path_buf(), source },
        CoreError::Dng(e) => StoreError::Dng(e),
        e => StoreError::Core(e),
    })
}

/// [`write_atomic`] for `value` as pretty JSON.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, &to_json(path, value)?)
}

/// A JSON file with a `schema` field, the only version this build reads
/// or writes; another is [`StoreError::Schema`]. A format's first change
/// adds its migration here.
pub(crate) trait Versioned {
    /// The format version this build writes.
    const SCHEMA: u32;
}

/// The `schema` a JSON file declares, read before anything else in it.
fn schema_of(path: &Path, bytes: &[u8]) -> Result<u32> {
    #[derive(serde::Deserialize)]
    struct Header {
        schema: u32,
    }
    let header: Header = serde_json::from_slice(bytes)
        .map_err(|source| StoreError::Json { path: path.to_path_buf(), source })?;
    Ok(header.schema)
}

/// Refuse `bytes` unless they are at version `T::SCHEMA`.
fn check_schema<T: Versioned>(path: &Path, bytes: &[u8]) -> Result<()> {
    match schema_of(path, bytes)? {
        found if found == T::SCHEMA => Ok(()),
        found => Err(StoreError::Schema { path: path.to_path_buf(), found, expected: T::SCHEMA }),
    }
}

/// [`read_versioned`] of bytes already read from `path`.
pub(crate) fn parse_versioned<T: Versioned + DeserializeOwned>(
    path: &Path,
    bytes: &[u8],
) -> Result<T> {
    check_schema::<T>(path, bytes)?;
    serde_json::from_slice(bytes)
        .map_err(|source| StoreError::Json { path: path.to_path_buf(), source })
}

/// Read the versioned JSON file `path` as `T` ([`Versioned`]).
pub(crate) fn read_versioned<T: Versioned + DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    parse_versioned(path, &bytes)
}

/// [`read_versioned`], `None` when there is no such file.
pub(crate) fn read_versioned_if_present<T: Versioned + DeserializeOwned>(
    path: &Path,
) -> Result<Option<T>> {
    match read_if_present(path).map_err(io_error(path))? {
        Some(bytes) => parse_versioned(path, &bytes).map(Some),
        None => Ok(None),
    }
}

/// Edits are one at a time, so two edits of one file don't undo each other.
static EDITING: Mutex<()> = Mutex::new(());

/// Hold across a store file's read-modify-write so another thread's edit
/// isn't undone. Not re-entrant.
pub(crate) fn one_edit_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    crate::lock(&EDITING)
}

/// Read-modify-write the versioned JSON file `path` as `T`; another version
/// is refused, unchanged.
pub(crate) fn edit_versioned<T: Versioned + Serialize + DeserializeOwned>(
    path: &Path,
    edit: impl FnOnce(&mut T),
) -> Result<()> {
    let _one_at_a_time = one_edit_at_a_time();
    let mut value: T = read_versioned(path)?;
    edit(&mut value);
    write_json_atomic(path, &value)
}

/// Rename the unreadable `path` aside to an unused
/// `<stem>.unreadable-<id>[.<ext>]`, so it can still be recovered. Returns
/// where it went.
pub fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    set_aside_as(path, path)
}

/// [`set_aside`], named for and put beside `like` (Recently Deleted).
pub fn set_aside_as(path: &Path, like: &Path) -> std::io::Result<PathBuf> {
    let stem = like.file_stem().unwrap_or_default().to_string_lossy();
    let mut name = format!("{stem}.unreadable-{}", new_id());
    if let Some(ext) = like.extension() {
        name.push('.');
        name.push_str(&ext.to_string_lossy());
    }
    let to = like.with_file_name(name);
    rename(path, &to).map_err(std::io::Error::from)?;
    tracing::warn!("kept unreadable {} as {}", path.display(), to.display());
    Ok(to)
}

/// Left free on a volume for everything else that writes there.
const SPARE: u64 = 1 << 30;

/// The free bytes on `folder`'s volume when `bytes` plus [`SPARE`] won't
/// fit; `None` when they fit or it can't be told.
pub(crate) fn no_room_for(folder: &Path, bytes: u64) -> Option<u64> {
    let free = free_space(folder)?;
    (bytes.saturating_add(SPARE) > free).then_some(free)
}

/// Refuse when `bytes` won't fit on `folder`'s volume with 1 GB to spare.
pub fn check_space(folder: &Path, bytes: u64) -> crate::Result<()> {
    match no_room_for(folder, bytes) {
        Some(free) => {
            Err(crate::Error::NoSpace { folder: folder.to_path_buf(), needed: bytes, free })
        }
        None => Ok(()),
    }
}

/// Free bytes on the volume of `path` or its nearest existing parent.
pub(crate) fn free_space(path: &Path) -> Option<u64> {
    let mut dir = path;
    while !dir.exists() {
        dir = dir.parent()?;
    }
    free_space_of(dir)
}

#[cfg(unix)]
fn free_space_of(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `stat` a writable statvfs.
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(windows)]
fn free_space_of(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is a NUL-terminated wide string that outlives the call.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }.ok()?;
    Some(available)
}

#[cfg(not(any(unix, windows)))]
fn free_space_of(_: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file's schema changed without `SOURCES_SCHEMA` fails here; bump it
    /// with its upgrade (ADR-141), then update the pins.
    #[test]
    fn a_file_schema_changes_only_with_the_library_format() {
        use crate::library::{history, render_file, stacks};
        let pinned = [
            ("SOURCES_SCHEMA", 2),
            ("RENDER_SCHEMA", 1),
            ("LIST_SCHEMA", 1),
            ("STACK_SCHEMA", 1),
            ("FAILURE_SCHEMA", 1),
            ("DELETED_SCHEMA", 1),
            ("paint USER_VERSION", 1),
        ];
        let now = [
            ("SOURCES_SCHEMA", crate::frames::sources::SOURCES_SCHEMA),
            ("RENDER_SCHEMA", render_file::RENDER_SCHEMA),
            ("LIST_SCHEMA", stacks::LIST_SCHEMA),
            ("STACK_SCHEMA", stacks::STACK_SCHEMA),
            ("FAILURE_SCHEMA", history::FAILURE_SCHEMA),
            ("DELETED_SCHEMA", history::deleted::DELETED_SCHEMA),
            ("paint USER_VERSION", crate::paint::store::USER_VERSION as u32),
        ];
        assert_eq!(
            now, pinned,
            "a file's schema changed: bump SOURCES_SCHEMA with its upgrade, then update these pins"
        );

        // Every `*_SCHEMA` in the crate is pinned above.
        let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut found = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for line in text.lines() {
                        let Some((_, rest)) = line.split_once("const ") else { continue };
                        let name = rest.split(':').next().unwrap_or_default().trim();
                        if name.ends_with("_SCHEMA") {
                            found.push(name.to_string());
                        }
                    }
                }
            }
        }
        for name in found {
            assert!(pinned.iter().any(|(p, _)| *p == name), "{name} is not pinned here");
        }
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Note {
        schema: u32,
        title: String,
    }

    impl Versioned for Note {
        const SCHEMA: u32 = 1;
    }

    fn file(json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.json");
        std::fs::write(&path, json).unwrap();
        (dir, path)
    }

    /// A file of another version is neither read nor edited.
    #[test]
    fn other_versions_are_refused_untouched() {
        for (json, found) in [(r#"{"schema": 2, "title": "Bee"}"#, 2), (r#"{"schema": 0}"#, 0)] {
            let (_dir, path) = file(json);
            let read = read_versioned::<Note>(&path);
            assert!(
                matches!(&read, Err(StoreError::Schema { found: f, expected: 1, .. }) if *f == found)
            );
            assert!(edit_versioned::<Note>(&path, |_| {}).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), json);
        }
    }

    #[test]
    fn set_aside_keeps_the_extension_and_never_collides() {
        let (dir, path) = file("{ not json");
        let first = set_aside(&path).unwrap();
        std::fs::write(&path, "again").unwrap();
        let second = set_aside(&path).unwrap();
        assert_ne!(first, second);
        for kept in [&first, &second] {
            let name = kept.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("note.unreadable-") && name.ends_with(".json"), "{name}");
        }
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "{ not json");
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn an_atomic_write_leaves_no_partial() {
        let (dir, path) = file("old");
        write_json_atomic(&path, &serde_json::json!({"schema": 1})).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["note.json"]);
    }
}
