//! A render's paint on disk (ADR-148): `Renders/<id>/paint.sqlite`, holding
//! the canvas size, the source table and the painted tiles (codec in
//! `log.rs`).
//!
//! - **Reading** is one read transaction on a read-only connection, which
//!   writes nothing. Unusable paint is an [`Unread`]; [`open_doc`] sets
//!   damaged paint aside.
//! - **A commit** is one transaction by the store thread, from a
//!   [`Snapshot`] through the render's one [`Writer`]: changed tiles
//!   written, unpainted ones deleted, the source table replaced.
//! - **The undo spill**, `paint-undo.sqlite` ([`Spill`]), holds undo
//!   snapshots that left RAM; it is deleted with its doc.
//!
//! The store never makes a folder; a first commit syncs the folder once.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rayon::prelude::*;
use rusqlite::{Connection, ErrorCode, OpenFlags, params};

use super::canvas::Layer;
use super::doc::{PaintDoc, Storage, Unsaveable};
use super::log;
use super::source::{Source, SourceTable};
use super::tile::{TILE, TileData, TileId};
use super::undo::UndoStack;
use crate::frames::sources::{Disk, Fingerprint, FrameRef, Probe, RootId, VolumeKind};
use crate::fsutil::StoreError;
use crate::lock;

/// A render's paint, in its folder.
pub const PAINT_FILE: &str = "paint.sqlite";
/// Undo snapshots spilled from RAM this session.
pub(crate) const UNDO_FILE: &str = "paint-undo.sqlite";
/// Marks a render directory.
const RENDER_FILE: &str = "render.json";

/// `DPTH`: the file is a render's paint.
const APPLICATION_ID: i32 = 0x4450_5448;
/// The schema version, the file's `user_version`.
pub const USER_VERSION: i32 = 1;
/// Made with the file; it can't change after.
const PAGE_SIZE: u32 = 16 * 1024;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// 3.51.3: earlier versions can corrupt a WAL database when a checkpoint
/// races another connection's commit that resets the WAL.
const LEAST_SQLITE: i32 = 3_051_003;

const SCHEMA: &str = "
    CREATE TABLE canvas (
        only   INTEGER PRIMARY KEY CHECK (only = 1),
        width  INTEGER NOT NULL,
        height INTEGER NOT NULL,
        tile   INTEGER NOT NULL
    );
    CREATE TABLE sources (
        id          INTEGER PRIMARY KEY CHECK (id BETWEEN 0 AND 255),
        kind        TEXT NOT NULL CHECK (kind IN ('original', 'frame', 'render')),
        root        TEXT,
        path        TEXT,
        size        INTEGER,
        fingerprint BLOB,
        render      TEXT,
        CHECK ((kind = 'frame') = (root IS NOT NULL AND path IS NOT NULL)),
        CHECK ((kind = 'render') = (render IS NOT NULL))
    );
    CREATE TABLE tiles (
        y    INTEGER NOT NULL,
        x    INTEGER NOT NULL,
        data BLOB NOT NULL,
        PRIMARY KEY (y, x)
    );";

/// The paint file of the render at `render_dir`.
pub fn paint_file(render_dir: &Path) -> PathBuf {
    render_dir.join(PAINT_FILE)
}

/// Whether the linked SQLite is new enough; checked as the Library opens.
pub(crate) fn check_sqlite() -> Result<(), StoreError> {
    if rusqlite::version_number() < LEAST_SQLITE {
        return Err(StoreError::Invalid(format!(
            "this build's SQLite is {}, older than the 3.51.3 paint needs",
            rusqlite::version()
        )));
    }
    Ok(())
}

fn is_render(render_dir: &Path) -> bool {
    render_dir.join(RENDER_FILE).exists()
}

/// Why a render's saved paint can't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unread {
    /// Not paint of this format, the wrong canvas size, a bad tile, or a
    /// corrupt file. [`open_doc`] sets it aside.
    #[error("it is damaged")]
    Damaged,
    /// Reading failed for a reason unrelated to the paint (access, busy,
    /// device); it is left where it is.
    #[error("it couldn't be read just now")]
    Unreachable,
}

/// Why a read stopped, before it is told as an [`Unread`].
#[derive(Debug, thiserror::Error)]
enum Bad {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(io::Error),
    #[error("{0}")]
    Damaged(&'static str),
}

impl Bad {
    /// A crash left a journal only a writer can roll back (ADR-148).
    fn is_hot_journal(&self) -> bool {
        matches!(self, Self::Sql(rusqlite::Error::SqliteFailure(e, _))
            if e.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
    }

    fn unread(&self) -> Unread {
        match self {
            Self::Damaged(_) => Unread::Damaged,
            Self::Io(_) => Unread::Unreachable,
            Self::Sql(e) => match e.sqlite_error_code() {
                // Corrupt, not a database, or not this schema.
                Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase | ErrorCode::Unknown) => {
                    Unread::Damaged
                }
                Some(_) => Unread::Unreachable,
                // A row of the wrong type, or none where one must be.
                None => Unread::Damaged,
            },
        }
    }
}

fn connect(path: &Path, flags: OpenFlags) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// `Ok(false)` for an empty database (a first commit that didn't finish),
/// `Err` when it isn't paint of this format.
fn header(conn: &Connection) -> Result<bool, Bad> {
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    let app: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    match (app, version) {
        (0, 0) if tables == 0 => Ok(false),
        (APPLICATION_ID, USER_VERSION) => Ok(true),
        _ => Err(Bad::Damaged("not a render's paint, or not of this format")),
    }
}

/// Run `read` in one read-only transaction; `Ok(None)` when there is no
/// file or nothing committed.
fn reading<T>(
    render_dir: &Path,
    read: impl FnOnce(&Connection) -> Result<T, Bad>,
) -> Result<Option<T>, Unread> {
    said(render_dir, try_reading(render_dir, read))
}

/// [`reading`], with what went wrong as it is.
fn try_reading<T>(
    render_dir: &Path,
    read: impl FnOnce(&Connection) -> Result<T, Bad>,
) -> Result<Option<T>, Bad> {
    let path = paint_file(render_dir);
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Bad::Io(e)),
    }
    let mut conn = connect(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tx = conn.transaction()?;
    if !header(&tx)? {
        return Ok(None);
    }
    // Dropped unfinished: a read transaction ends writing nothing.
    read(&tx).map(Some)
}

/// A read's failure, logged and classified.
fn said<T>(render_dir: &Path, read: Result<T, Bad>) -> Result<T, Unread> {
    read.map_err(|bad| {
        let unread = bad.unread();
        tracing::warn!(
            "the retouching of {} can't be read ({unread}): {bad}",
            render_dir.display()
        );
        unread
    })
}

/// What a read finds; `sources` is `None` when malformed, and alone is
/// replaced.
struct Found {
    layer: Layer,
    saved: Saved,
    sources: Option<SourceTable>,
}

/// The tiles a file holds, by identity: painting makes a new `Arc`, and a
/// `Weak` keeps the address from being reused.
type Saved = HashMap<TileId, Weak<TileData>>;

fn read(render_dir: &Path, width: u32, height: u32) -> Result<Option<Found>, Unread> {
    said(render_dir, try_read(render_dir, width, height))
}

/// [`read`], with what went wrong as it is.
fn try_read(render_dir: &Path, width: u32, height: u32) -> Result<Option<Found>, Bad> {
    try_reading(render_dir, |conn| {
        let canvas: [u32; 3] =
            conn.query_row("SELECT width, height, tile FROM canvas", [], |r| {
                Ok([r.get(0)?, r.get(1)?, r.get(2)?])
            })?;
        if canvas != [width, height, TILE] {
            return Err(Bad::Damaged("painted on a canvas of another size"));
        }
        let sources = read_sources(conn)?;
        let mut layer = Layer::empty(width, height);
        let [cols, rows] = layer.grid();
        let mut seen = vec![false; layer.tiles.len()];
        let mut raw = Vec::new();
        let mut tiles = conn.prepare("SELECT y, x, data FROM tiles")?;
        let mut found = tiles.query([])?;
        while let Some(row) = found.next()? {
            let (y, x): (u32, u32) = (row.get(0)?, row.get(1)?);
            let i = (y < rows && x < cols).then(|| (y * cols + x) as usize);
            let Some(i) = i.filter(|&i| !std::mem::replace(&mut seen[i], true)) else {
                return Err(Bad::Damaged("a tile off the canvas"));
            };
            raw.push((i, TileId { x, y }, row.get::<_, Vec<u8>>(2)?));
        }
        let decoded = raw
            .into_par_iter()
            .map(|(i, id, bytes)| Some((i, id, Arc::new(log::decode(&bytes).ok()?))))
            .collect::<Option<Vec<_>>>()
            .ok_or(Bad::Damaged("a tile that doesn't decode"))?;
        let mut saved = HashMap::with_capacity(decoded.len());
        for (i, id, tile) in decoded {
            saved.insert(id, Arc::downgrade(&tile));
            layer.tiles[i] = Some(tile);
        }
        Ok(Found { layer, saved, sources })
    })
}

/// `None` when the table isn't one a doc could have made.
fn read_sources(conn: &Connection) -> Result<Option<SourceTable>, Bad> {
    let mut sources = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT id, kind, root, path, size, fingerprint, render FROM sources ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let kind: String = row.get(1)?;
        let source = match kind.as_str() {
            "original" => Some(Source::Original),
            "frame" => {
                frame_of(row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?).map(Source::Frame)
            }
            "render" => row.get::<_, Option<String>>(6)?.map(Source::Render),
            _ => None,
        };
        match source {
            Some(source) if id == sources.len() as i64 => sources.push(source),
            _ => return Ok(None),
        }
    }
    Ok(Some(SourceTable::of(sources)).filter(SourceTable::is_valid))
}

/// `None` when the row is malformed.
fn frame_of(
    root: Option<String>,
    path: Option<String>,
    size: Option<i64>,
    fingerprint: Option<Vec<u8>>,
) -> Option<FrameRef> {
    Some(FrameRef {
        root: RootId::parse(&root?)?,
        path: path?,
        size: u64::try_from(size?).ok()?,
        fingerprint: Fingerprint(u64::from_be_bytes(fingerprint?.try_into().ok()?)),
    })
}

/// Whether the render has a painted tile, without reading the tiles.
pub fn has_paint(render_dir: &Path) -> Result<bool, Unread> {
    let painted = reading(render_dir, |conn| {
        Ok(conn.query_row("SELECT EXISTS (SELECT 1 FROM tiles)", [], |r| r.get(0))?)
    })?;
    Ok(painted.unwrap_or(false))
}

/// The roots the render's frame sources name, for the Library's scan.
pub(crate) fn frame_roots(render_dir: &Path) -> Result<Vec<RootId>, Unread> {
    let roots = reading(render_dir, |conn| {
        let mut stmt = conn.prepare("SELECT root FROM sources WHERE root IS NOT NULL")?;
        let roots = stmt.query_map([], |r| r.get::<_, String>(0))?;
        roots.map(|root| RootId::parse(&root?).ok_or(Bad::Damaged("a source's root"))).collect()
    })?;
    Ok(roots.unwrap_or_default())
}

/// `render_dir`'s doc to paint: its saved paint, or a fresh doc (damaged
/// paint set aside first). A gone render or unreachable paint gives a doc
/// in RAM only ([`PaintDoc::unsaveable`]).
///
/// The caller has closed the store thread's writer, so damaged paint can
/// be set aside.
pub fn open_doc(render_dir: &Path, width: u32, height: u32) -> PaintDoc {
    open_doc_rolling_back(render_dir, width, height, || {})
}

/// [`open_doc`], with a hot journal rolled back by `roll_back` (the store
/// thread's writer) and the file read again; still there, it is
/// unreachable.
pub(crate) fn open_doc_rolling_back(
    render_dir: &Path,
    width: u32,
    height: u32,
    roll_back: impl FnOnce(),
) -> PaintDoc {
    let mut doc =
        PaintDoc::new(Layer::empty(width, height), UndoStack::new(None), Default::default());
    if !is_render(render_dir) {
        return doc;
    }
    let unwritable = |mut doc: PaintDoc, e: &dyn std::fmt::Display| {
        tracing::error!("retouching of {} is not saved: {e}", render_dir.display());
        doc.storage = Storage::Memory(Unsaveable::Unwritable);
        doc
    };
    let read = match try_read(render_dir, width, height) {
        Err(bad) if bad.is_hot_journal() => {
            tracing::info!(
                "the retouching of {} has a commit to roll back: its writer opens it",
                render_dir.display()
            );
            roll_back();
            try_read(render_dir, width, height)
        }
        read => read,
    };
    let saved = match said(render_dir, read) {
        Ok(Some(found)) => {
            let Found { layer, saved, sources } = found;
            doc = PaintDoc::new(layer, UndoStack::new(None), sources.clone().unwrap_or_default());
            if sources.is_none() {
                tracing::warn!(
                    "the source table of {} is malformed: replaced",
                    render_dir.display()
                );
                doc.mark_changed();
            }
            saved
        }
        Ok(None) => Saved::new(),
        // It may read fine next time: nothing is moved or written.
        Err(Unread::Unreachable) => {
            tracing::error!(
                "the retouching of {} couldn't be read just now, so it is left as it is and what is painted is not saved",
                render_dir.display()
            );
            doc.storage = Storage::Memory(Unsaveable::Unreachable);
            return doc;
        }
        // Never overwritten: a person may still recover it.
        Err(Unread::Damaged) => match set_aside(render_dir) {
            Ok(()) => Saved::new(),
            Err(e) => return unwritable(doc, &e),
        },
    };
    doc.storage = Storage::Saved(PaintFile::new(render_dir, saved));
    doc.history = UndoStack::new(Some(Spill::new(render_dir)));
    doc
}

/// The committed paint as a RAM-only doc, for showing or exporting; `None`
/// when there is none. Writes nothing.
pub fn load_doc(render_dir: &Path, width: u32, height: u32) -> Result<Option<PaintDoc>, Unread> {
    Ok(read(render_dir, width, height)?.map(|found| {
        let Found { layer, sources, .. } = found;
        PaintDoc::new(layer, UndoStack::new(None), sources.unwrap_or_default())
    }))
}

/// Set damaged paint aside with its journal, as
/// `paint.unreadable-<id>.sqlite`. Only while no connection has it open.
fn set_aside(render_dir: &Path) -> io::Result<()> {
    let path = paint_file(render_dir);
    let to = crate::fsutil::set_aside(&path)?;
    // `-shm` too: it only goes with its WAL.
    for suffix in ["-wal", "-journal", "-shm"] {
        let beside = |file: &Path| {
            let mut name = file.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        };
        match std::fs::rename(beside(&path), beside(&to)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// A saved doc's file, shared with the store thread: which change and
/// which tiles it holds.
pub(crate) struct PaintFile {
    render_dir: PathBuf,
    committed: AtomicU64,
    /// The newest change count sent ([`Snapshot::sent`]).
    sent: AtomicU64,
    saved: Mutex<Saved>,
}

impl PaintFile {
    fn new(render_dir: &Path, saved: Saved) -> Arc<Self> {
        Arc::new(Self {
            render_dir: render_dir.to_path_buf(),
            committed: AtomicU64::new(0),
            sent: AtomicU64::new(0),
            saved: Mutex::new(saved),
        })
    }

    pub(crate) fn render_dir(&self) -> &Path {
        &self.render_dir
    }

    /// The doc's change count the file holds.
    pub(crate) fn committed(&self) -> u64 {
        self.committed.load(Ordering::Acquire)
    }

    /// The last commit's tiles, for a released doc; an error changes
    /// nothing.
    pub(crate) fn read_back(&self, width: u32, height: u32) -> io::Result<Layer> {
        match read(&self.render_dir, width, height) {
            Ok(Some(found)) => {
                *lock(&self.saved) = found.saved;
                Ok(found.layer)
            }
            Ok(None) => Err(io::Error::new(io::ErrorKind::NotFound, "the paint file is gone")),
            Err(unread) => Err(io::Error::new(io::ErrorKind::InvalidData, unread.to_string())),
        }
    }
}

/// A doc at change `changes`, its tiles shared so painting goes on
/// meanwhile.
pub(crate) struct Snapshot {
    pub(crate) file: Arc<PaintFile>,
    pub(crate) changes: u64,
    pub(crate) size: [u32; 2],
    pub(crate) tiles: Vec<(TileId, Arc<TileData>)>,
    pub(crate) sources: Vec<Source>,
}

impl Snapshot {
    /// Record that this snapshot is sent to the store thread.
    pub(crate) fn sent(self) -> Self {
        self.file.sent.fetch_max(self.changes, Ordering::AcqRel);
        self
    }

    /// Whether a newer snapshot of the doc was sent after this one.
    pub(crate) fn is_overtaken(&self) -> bool {
        self.changes < self.file.sent.load(Ordering::Acquire)
    }
}

/// A commit's failure as a [`StoreError`].
fn failure(path: &Path, e: rusqlite::Error) -> StoreError {
    use io::ErrorKind::{NotFound, Other, PermissionDenied, StorageFull};
    let kind = match e.sqlite_error_code() {
        Some(ErrorCode::DiskFull) => StorageFull,
        Some(ErrorCode::PermissionDenied | ErrorCode::ReadOnly) => PermissionDenied,
        // No folder: the render is gone.
        Some(ErrorCode::CannotOpen) => NotFound,
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => {
            return StoreError::Damaged { path: path.to_path_buf() };
        }
        _ => Other,
    };
    StoreError::Io { path: path.to_path_buf(), source: io::Error::new(kind, e) }
}

/// WAL on a local volume; on a share or unknown volume a rollback journal
/// with extra sync, as WAL's shared memory can't be relied on (ADR-148).
fn journal(probe: &dyn Probe, render_dir: &Path) -> (&'static str, &'static str) {
    match probe.volume(render_dir) {
        Ok(mounted) if mounted.volume.kind == VolumeKind::Disk => ("WAL", "FULL"),
        Ok(_) => ("DELETE", "EXTRA"),
        Err(e) => {
            tracing::warn!("the volume of {} couldn't be told: {e}", render_dir.display());
            ("DELETE", "EXTRA")
        }
    }
}

/// The store thread's one connection writing a render's file, open while
/// Retouch is on it.
pub(crate) struct Writer {
    conn: Connection,
    path: PathBuf,
    made: Made,
}

/// Where a writer's file stands.
///
/// Lifecycle: fresh, unsynced, synced; changed only by its [`Writer`]'s
/// `commit`, on the store thread (ADR-148).
///
/// ```text
/// Fresh ──commit──▶ Unsynced ──folder synced──▶ Synced
/// ```
///
/// A file opened with paint starts `Synced`; a failed folder sync stays
/// `Unsynced` for the next commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Made {
    /// Nothing committed: the first commit writes the schema, then the
    /// journal is set.
    Fresh,
    /// Its folder not synced since the file was made: a power loss could
    /// lose it.
    Unsynced,
    Synced,
}

impl Writer {
    /// Open the render's file to write, making it if missing (never its
    /// folder). Refused when it isn't paint of this version.
    fn open(render_dir: &Path, probe: &dyn Probe) -> Result<Self, StoreError> {
        let path = paint_file(render_dir);
        let fail = |e| failure(&path, e);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = connect(&path, flags).map_err(fail)?;
        conn.pragma_update(None, "fullfsync", true).map_err(fail)?;
        conn.pragma_update(None, "checkpoint_fullfsync", true).map_err(fail)?;
        // Empty: made just now, or by a first commit that didn't finish.
        let empty = match header(&conn) {
            Ok(paint) => !paint,
            Err(Bad::Sql(e)) => return Err(fail(e)),
            Err(_) => return Err(StoreError::Damaged { path }),
        };
        if empty {
            // Only settable before anything is written.
            #[cfg(not(test))]
            let page_size = PAGE_SIZE;
            #[cfg(test)]
            let page_size = tests::NEW_PAGE_SIZE.get();
            conn.pragma_update(None, "page_size", page_size).map_err(fail)?;
            conn.pragma_update(None, "auto_vacuum", "INCREMENTAL").map_err(fail)?;
            conn.pragma_update(None, "synchronous", "FULL").map_err(fail)?;
        }
        let made = if empty { Made::Fresh } else { Made::Synced };
        let writer = Self { conn, path, made };
        if !empty {
            writer.set_journal(render_dir, probe)?;
        }
        Ok(writer)
    }

    /// Set at every open: a Library moved to or from a share switches.
    fn set_journal(&self, render_dir: &Path, probe: &dyn Probe) -> Result<(), StoreError> {
        let (mode, synchronous) = journal(probe, render_dir);
        let fail = |e| failure(&self.path, e);
        let set: String = self
            .conn
            .pragma_update_and_check(None, "journal_mode", mode, |r| r.get(0))
            .map_err(fail)?;
        if !set.eq_ignore_ascii_case(mode) {
            tracing::warn!("the paint of {} keeps journal {set}, not {mode}", render_dir.display());
        }
        self.conn.pragma_update(None, "synchronous", synchronous).map_err(fail)?;
        tracing::debug!(journal = mode, synchronous, "paint: a render's file opened to write");
        Ok(())
    }

    /// Write `snapshot` in one transaction; on an error it is rolled back
    /// and the doc stays unsaved.
    fn commit(&mut self, snapshot: &Snapshot, probe: &dyn Probe) -> Result<(), StoreError> {
        let saved = lock(&snapshot.file.saved).clone();
        let unchanged = |id: &TileId, tile: &Arc<TileData>| {
            saved.get(id).is_some_and(|w| w.as_ptr() == Arc::as_ptr(tile))
        };
        let encoded: Vec<(TileId, Vec<u8>)> = snapshot
            .tiles
            .par_iter()
            .filter(|(id, tile)| !unchanged(id, tile))
            .map(|(id, tile)| (*id, log::encode(tile)))
            .collect();
        let painted: HashSet<TileId> = snapshot.tiles.iter().map(|(id, _)| *id).collect();
        let gone: Vec<TileId> = saved.keys().filter(|id| !painted.contains(id)).copied().collect();

        let written = self.transaction(|conn, fresh| {
            if fresh {
                conn.pragma_update(None, "application_id", APPLICATION_ID)?;
                conn.pragma_update(None, "user_version", USER_VERSION)?;
                conn.execute_batch(SCHEMA)?;
                let [width, height] = snapshot.size;
                conn.execute(
                    "INSERT INTO canvas (only, width, height, tile) VALUES (1, ?1, ?2, ?3)",
                    params![width, height, TILE],
                )?;
            }
            let mut put =
                conn.prepare("INSERT OR REPLACE INTO tiles (y, x, data) VALUES (?1, ?2, ?3)")?;
            for (id, bytes) in &encoded {
                put.execute(params![id.y, id.x, bytes])?;
            }
            let mut delete = conn.prepare("DELETE FROM tiles WHERE y = ?1 AND x = ?2")?;
            for id in &gone {
                delete.execute(params![id.y, id.x])?;
            }
            conn.execute("DELETE FROM sources", [])?;
            let mut insert = conn.prepare(
                "INSERT INTO sources (id, kind, root, path, size, fingerprint, render)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (id, source) in (0i64..).zip(&snapshot.sources) {
                let (kind, frame, render) = match source {
                    Source::Original => ("original", None, None),
                    Source::Frame(frame) => ("frame", Some(frame), None),
                    Source::Render(render) => ("render", None, Some(render)),
                };
                insert.execute(params![
                    id,
                    kind,
                    frame.map(|f| f.root.as_str()),
                    frame.map(|f| &f.path),
                    frame.map(|f| i64::try_from(f.size).unwrap_or(i64::MAX)),
                    frame.map(|f| f.fingerprint.0.to_be_bytes().to_vec()),
                    render,
                ])?;
            }
            Ok(())
        });
        written.map_err(|e| failure(&self.path, e))?;
        let dir = snapshot.file.render_dir();
        if self.made == Made::Fresh {
            self.set_made(Made::Unsynced);
            self.set_journal(dir, probe)?;
        }
        if self.made == Made::Unsynced {
            deptheos_core::io::sync_folder(dir)
                .map_err(|e| StoreError::Io { path: dir.to_path_buf(), source: e })?;
            self.set_made(Made::Synced);
        }
        *lock(&snapshot.file.saved) =
            snapshot.tiles.iter().map(|(id, tile)| (*id, Arc::downgrade(tile))).collect();
        snapshot.file.committed.store(snapshot.changes, Ordering::Release);
        Ok(())
    }

    fn set_made(&mut self, next: Made) {
        tracing::debug!(from = ?self.made, to = ?next, "paint file");
        self.made = next;
    }

    /// Run `write` in a `BEGIN IMMEDIATE` transaction, rolled back on an
    /// error unless SQLite already did.
    fn transaction(
        &mut self,
        write: impl FnOnce(&Connection, bool) -> rusqlite::Result<()>,
    ) -> rusqlite::Result<()> {
        #[cfg(test)]
        tests::full::limit(&self.conn, &self.path)?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let fresh = self.made == Made::Fresh;
        let done = write(&self.conn, fresh).and_then(|()| self.conn.execute_batch("COMMIT"));
        if done.is_err() && !self.conn.is_autocommit() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
        done
    }

    /// Close the file as Retouch leaves: free pages given back past a
    /// quarter of it, the WAL truncated.
    fn leave(self) {
        let tidy = || -> rusqlite::Result<()> {
            let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let free: i64 = self.conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            if free * 4 > pages {
                // One page per step.
                let mut vacuum = self.conn.prepare("PRAGMA incremental_vacuum")?;
                let mut steps = vacuum.query([])?;
                while steps.next()?.is_some() {}
            }
            self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        };
        // Nothing to tidy in a file nothing was committed to.
        if let Err(e) = if self.made == Made::Fresh { Ok(()) } else { tidy() } {
            tracing::warn!(
                "the paint file {} wasn't tidied as it closed: {e}",
                self.path.display()
            );
        }
        if let Err((_, e)) = self.conn.close() {
            tracing::warn!("the paint file {} didn't close: {e}", self.path.display());
        }
    }
}

/// Commit `doc` and close its file, off the paint worker (tools and
/// tests); whether it is saved.
pub fn save(doc: &PaintDoc) -> Result<bool, StoreError> {
    let Some(snapshot) = doc.snapshot() else { return Ok(false) };
    let mut writers = Writers::default();
    let committed = writers.commit(&snapshot);
    writers.leave_all();
    committed.map(|()| !doc.is_unsaved())
}

/// The store thread's files: each render's [`Writer`], by its folder.
#[derive(Default)]
pub(crate) struct Writers {
    open: HashMap<PathBuf, Writer>,
}

impl Writers {
    /// Commit `snapshot`, opening its render's writer if needed.
    pub(crate) fn commit(&mut self, snapshot: &Snapshot) -> Result<(), StoreError> {
        self.commit_on(snapshot, &Disk)
    }

    fn commit_on(&mut self, snapshot: &Snapshot, probe: &dyn Probe) -> Result<(), StoreError> {
        let dir = snapshot.file.render_dir();
        if !self.open.contains_key(dir) {
            self.open.insert(dir.to_path_buf(), Writer::open(dir, probe)?);
        }
        self.open.get_mut(dir).expect("opened above").commit(snapshot, probe)
    }

    /// Retouch left the render: tidy and close its file.
    pub(crate) fn leave(&mut self, render_dir: &Path) {
        if let Some(writer) = self.open.remove(render_dir) {
            writer.leave();
        }
    }

    /// Roll back a hot journal by opening a writer, closed again at once.
    pub(crate) fn roll_back(&mut self, render_dir: &Path) {
        if self.open.contains_key(render_dir) {
            return;
        }
        if let Err(e) = Writer::open(render_dir, &Disk) {
            tracing::warn!("the retouching of {} wasn't rolled back: {e}", render_dir.display());
        }
    }

    /// Close the file as it is: the render is being deleted.
    pub(crate) fn close(&mut self, render_dir: &Path) {
        self.open.remove(render_dir);
    }

    /// Tidy and close every file.
    pub(crate) fn leave_all(&mut self) {
        for (_, writer) in self.open.drain() {
            writer.leave();
        }
    }
}

/// A doc's undo snapshots that left RAM, in `paint-undo.sqlite` on its own
/// connection, so an undo never waits behind a commit. Written by the store
/// thread, read by the worker, never synced: it doesn't outlive a crash.
/// Deleted with the spill; one a crash left goes as the Library opens.
pub(crate) struct Spill {
    path: PathBuf,
    /// Made by the first write.
    conn: Mutex<Option<Connection>>,
    /// Rows no history holds, deleted with the next write.
    freed: Mutex<Vec<i64>>,
    /// Tests: run before each snapshot write; an error fails it.
    #[cfg(test)]
    pub(crate) before_write: Mutex<Option<BeforeWrite>>,
}

/// A test's hook run before each snapshot is written.
#[cfg(test)]
pub(crate) type BeforeWrite = Box<dyn FnMut() -> rusqlite::Result<()> + Send>;

impl Spill {
    pub(crate) fn new(render_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            path: render_dir.join(UNDO_FILE),
            conn: Mutex::new(None),
            freed: Mutex::new(Vec::new()),
            #[cfg(test)]
            before_write: Mutex::new(None),
        })
    }

    fn make(path: &Path) -> rusqlite::Result<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = connect(path, flags)?;
        conn.pragma_update(None, "page_size", PAGE_SIZE)?;
        conn.pragma_update(None, "journal_mode", "MEMORY")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        #[cfg(test)]
        tests::full::limit(&conn, path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS snapshots (id INTEGER PRIMARY KEY, data BLOB NOT NULL)",
        )?;
        Ok(conn)
    }

    /// Write `tiles` in one transaction, returning their rows in order.
    pub(crate) fn write(&self, tiles: &[Vec<u8>]) -> rusqlite::Result<Vec<i64>> {
        let mut conn = lock(&self.conn);
        if conn.is_none() {
            *conn = Some(Self::make(&self.path)?);
        }
        let conn = conn.as_mut().expect("made above");
        #[cfg(test)]
        tests::full::limit(conn, &self.path)?;
        let freed = std::mem::take(&mut *lock(&self.freed));
        let tx = conn.transaction()?;
        let rows = (|| {
            let mut delete = tx.prepare("DELETE FROM snapshots WHERE id = ?1")?;
            for row in &freed {
                delete.execute([row])?;
            }
            let mut insert = tx.prepare("INSERT INTO snapshots (data) VALUES (?1)")?;
            let mut rows = Vec::with_capacity(tiles.len());
            for bytes in tiles {
                #[cfg(test)]
                if let Some(hook) = &mut *lock(&self.before_write) {
                    hook()?;
                }
                insert.execute([bytes])?;
                rows.push(tx.last_insert_rowid());
            }
            Ok(rows)
        })();
        let committed = rows.and_then(|rows| tx.commit().map(|()| rows));
        if committed.is_err() {
            // Rolled back: the freed rows wait for the next write.
            lock(&self.freed).extend(freed);
        }
        committed
    }

    pub(crate) fn read(&self, row: i64) -> io::Result<Vec<u8>> {
        let conn = lock(&self.conn);
        let conn = conn.as_ref().ok_or_else(|| io::Error::other("the undo file isn't made"))?;
        conn.query_row("SELECT data FROM snapshots WHERE id = ?1", [row], |r| r.get(0))
            .map_err(io::Error::other)
    }

    /// Row `row` is no history's.
    pub(crate) fn free(&self, row: i64) {
        lock(&self.freed).push(row);
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        // Closed first; removed even if unwritten, as a failed first write
        // may have left it.
        drop(self.conn.get_mut().unwrap_or_else(|p| p.into_inner()).take());
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                tracing::warn!("the undo file {} wasn't deleted: {e}", self.path.display());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
pub(crate) mod tests;
