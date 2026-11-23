//! The paint file: what a commit writes and a read finds, damaged and
//! newer paint, the journal on a share, a full disk.

use super::*;
use crate::CompletedStack;
use crate::paint::source::SourceKind;
use crate::paint::stroke::tests::stack;
use crate::paint::undo::tests::{flat, paint, redo_all, undo_all};

/// A full disk in tests: `PRAGMA max_page_count` held to the file's size,
/// so growing fails with `SQLITE_FULL`.
pub(crate) mod full {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static FULL: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// Put the paint files in `dir` on a full disk, or not.
    pub(crate) fn set(dir: &Path, full: bool) {
        let mut dirs = crate::lock(&FULL);
        dirs.retain(|d| d != dir);
        if full {
            dirs.push(dir.to_path_buf());
        }
    }

    /// Before a write: no page past its size while its folder is full.
    pub(crate) fn limit(conn: &rusqlite::Connection, path: &Path) -> rusqlite::Result<()> {
        let full = path.parent().is_some_and(|dir| crate::lock(&FULL).iter().any(|d| d == dir));
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let max = if full { pages.max(1) } else { 1 << 30 };
        conn.query_row(&format!("PRAGMA max_page_count = {max}"), [], |_| Ok(()))
    }
}

pub(crate) fn render_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(RENDER_FILE), "{}").unwrap();
    dir
}

fn base() -> Arc<CompletedStack> {
    stack(300, 200, 20000)
}

fn open(dir: &Path, base: &Arc<CompletedStack>) -> PaintDoc {
    let mut doc = open_doc(dir, base.width, base.height);
    doc.open(base.clone());
    doc
}

fn load(dir: &Path, base: &Arc<CompletedStack>) -> PaintDoc {
    let mut doc = load_doc(dir, base.width, base.height).unwrap().expect("committed");
    doc.open(base.clone());
    doc
}

fn painted(dir: &Path, base: &Arc<CompletedStack>, strokes: usize) -> PaintDoc {
    let mut doc = open(dir, base);
    for i in 0..strokes {
        paint(&mut doc, i);
    }
    doc
}

/// Commit `doc` as the store thread does; whether it is saved.
pub(crate) fn save_in(writers: &mut Writers, doc: &mut PaintDoc) -> Result<bool, StoreError> {
    let Some(snapshot) = doc.snapshot() else { return Ok(false) };
    writers.commit(&snapshot)?;
    Ok(!doc.is_unsaved())
}

/// A plain connection, to look at the file or break it.
fn sql(dir: &Path) -> Connection {
    Connection::open(paint_file(dir)).unwrap()
}

fn tile_rows(dir: &Path) -> i64 {
    sql(dir).query_row("SELECT count(*) FROM tiles", [], |r| r.get(0)).unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The files set aside in `dir`, journals excluded.
fn set_aside_in(dir: &Path) -> Vec<String> {
    let aside = names(dir).into_iter().filter(|n| n.starts_with("paint.unreadable-"));
    aside
        .filter(|n| !n.ends_with("-wal") && !n.ends_with("-shm") && !n.ends_with("-journal"))
        .collect()
}

fn erase_all(doc: &mut PaintDoc) {
    let canvas = doc.canvas_mut().unwrap();
    let ids: Vec<_> = canvas.painted().map(|(id, _)| id).collect();
    for id in ids {
        canvas.replace(id, None);
    }
    doc.mark_changed();
}

#[test]
fn this_builds_sqlite_is_new_enough() {
    check_sqlite().unwrap();
    assert!(rusqlite::version_number() >= LEAST_SQLITE, "{}", rusqlite::version());
}

#[test]
fn a_saved_doc_reopens_with_its_pixels_and_a_fresh_history() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 5);
    let (canvas, history) = doc.parts_mut().unwrap();
    history.undo(canvas).unwrap();
    let shown = flat(&doc);
    assert!(save(&doc).unwrap());
    assert!(!doc.is_unsaved());
    drop(doc);

    let mut doc = open(dir.path(), &base);
    assert_eq!(flat(&doc), shown);
    assert_eq!(doc.history.counts(), [0, 0], "the history was the session's");
    assert!(!doc.is_unsaved(), "nothing needed fixing");
    paint(&mut doc, 6);
    let painted = flat(&doc);
    assert!(save(&doc).unwrap());
    undo_all(&mut doc);
    assert_eq!(flat(&doc), shown);
    drop(doc);
    assert_eq!(flat(&open(dir.path(), &base)), painted, "the last commit was before the undo");

    // Made as ADR-148 has it.
    let conn = sql(dir.path());
    let pragma = |name: &str| -> i64 {
        conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0)).unwrap()
    };
    assert_eq!(pragma("application_id"), i64::from(APPLICATION_ID));
    assert_eq!(pragma("user_version"), 1);
    assert_eq!(pragma("page_size"), i64::from(PAGE_SIZE));
    assert_eq!(pragma("auto_vacuum"), 2, "incremental");
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "wal", "on a local disk");
}

/// The undo spill is made by the first write and goes with its doc.
#[test]
fn the_undo_file_goes_with_its_doc() {
    let (dir, base) = (render_dir(), base());
    let undo_file = dir.path().join(UNDO_FILE);
    let mut doc = open(dir.path(), &base);
    paint(&mut doc, 0);
    assert!(!undo_file.exists(), "nothing to write yet");
    paint(&mut doc, 1);
    assert!(undo_file.exists());
    assert!(save(&doc).unwrap());
    assert!(undo_file.exists(), "kept by the commit");
    drop(doc);
    assert!(!undo_file.exists());
}

/// A closed, committed doc releases its tiles and restores them with its
/// history; unsaved ones stay.
#[test]
fn a_closed_committed_doc_lets_go_of_its_tiles() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 5);
    let (canvas, history) = doc.parts_mut().unwrap();
    history.undo(canvas).unwrap();
    let shown = flat(&doc);
    let counts = doc.history.counts();
    let tiles: Vec<_> = doc.layer().unwrap().painted().map(|(_, t)| Arc::downgrade(t)).collect();
    assert!(!tiles.is_empty());

    // Unsaved: kept.
    doc.close();
    assert!(!doc.release());
    assert!(tiles.iter().all(|t| t.upgrade().is_some()));
    let mut writers = Writers::default();
    assert!(save_in(&mut writers, &mut doc).unwrap());
    assert!(doc.release());
    assert!(doc.is_released() && doc.is_painted() && doc.layer().is_none());
    assert!(tiles.iter().all(|t| t.upgrade().is_none()), "the layer's RAM is freed");

    doc.restore().unwrap();
    doc.open(base.clone());
    assert_eq!(flat(&doc), shown);
    assert_eq!(doc.history.counts(), counts, "the session's history");
    assert!(doc.persist().is_some() && !doc.is_unsaved(), "nothing changed by reading it back");
    // Read-back tiles are known as saved.
    let snapshot = doc.snapshot().unwrap();
    assert_eq!(lock(&snapshot.file.saved).len(), snapshot.tiles.len());
    let unchanged = snapshot
        .tiles
        .iter()
        .all(|(id, t)| lock(&snapshot.file.saved)[id].as_ptr() == Arc::as_ptr(t));
    assert!(unchanged, "every tile known as saved");
    redo_all(&mut doc);
    let redone = flat(&doc);
    assert_ne!(redone, shown);
    undo_all(&mut doc);
    assert_eq!(flat(&doc), doc.canvas().unwrap().base());
    redo_all(&mut doc);
    assert_eq!(flat(&doc), redone);
    writers.leave_all();
}

/// A released doc that can't be read back stays released, and opening
/// afresh sets the file aside.
#[test]
fn a_layer_that_cannot_be_read_back_is_not_shown_blank() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 3);
    assert!(save(&doc).unwrap());
    doc.close();
    assert!(doc.release());
    sql(dir.path()).execute("UPDATE tiles SET data = x'0102'", []).unwrap();
    assert!(doc.restore().is_err());
    assert!(doc.is_released() && doc.is_painted());
    drop(doc);
    let doc = open(dir.path(), &base);
    assert!(!doc.is_painted());
    assert_eq!(set_aside_in(dir.path()).len(), 1);
}

/// Only changed tiles are written; an unpainted tile loses its row.
#[test]
fn only_changed_tiles_are_written_again() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 2);
    let mut writers = Writers::default();
    assert!(save_in(&mut writers, &mut doc).unwrap());
    let rows = tile_rows(dir.path());
    assert!(rows > 0);
    let changed = |doc: &PaintDoc| {
        let snapshot = doc.snapshot().unwrap();
        let saved = lock(&snapshot.file.saved);
        snapshot.tiles.iter().filter(|(id, t)| saved[id].as_ptr() != Arc::as_ptr(t)).count()
    };
    assert_eq!(changed(&doc), 0);
    erase_all(&mut doc);
    assert!(save_in(&mut writers, &mut doc).unwrap());
    assert_eq!(tile_rows(dir.path()), 0, "every row goes");
    writers.leave_all();
    assert!(!open(dir.path(), &base).is_painted());
}

/// A change after the snapshot is the next commit's.
#[test]
fn a_change_after_the_snapshot_stays_unsaved() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 2);
    let snapshot = doc.snapshot().unwrap();
    paint(&mut doc, 2);
    let mut writers = Writers::default();
    writers.commit(&snapshot).unwrap();
    assert!(doc.is_unsaved(), "painted after the snapshot");
    assert!(save_in(&mut writers, &mut doc).unwrap());
    writers.leave_all();
    let shown = flat(&doc);
    drop(doc);
    assert_eq!(flat(&open(dir.path(), &base)), shown);
}

/// Unreachable paint (access denied) is left where it is and reads again
/// later.
#[cfg(unix)]
#[test]
fn paint_that_cannot_be_reached_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let base = base();
    let dir = render_dir();
    let doc = painted(dir.path(), &base, 2);
    assert!(save(&doc).unwrap());
    drop(doc);
    let path = paint_file(dir.path());
    let mode = |m| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o000);
    if std::fs::read(&path).is_ok() {
        return; // root reads anything: nothing to test
    }
    assert_eq!(load_doc(dir.path(), base.width, base.height).err(), Some(Unread::Unreachable));
    let doc = open(dir.path(), &base);
    assert!(!doc.is_painted());
    assert_eq!(
        doc.unsaveable(),
        Some(Unsaveable::Unreachable),
        "nothing is saved over what couldn't be read, and the panel says so"
    );
    drop(doc);
    assert!(set_aside_in(dir.path()).is_empty(), "and it is not moved aside");
    mode(0o644);
    assert!(open(dir.path(), &base).is_painted(), "it reads again");
}

/// Damaged paint that can't be moved aside opens in memory only.
#[cfg(unix)]
#[test]
fn paint_that_cannot_be_set_aside_opens_unsaveable() {
    use std::os::unix::fs::PermissionsExt;
    let base = base();
    let dir = render_dir();
    std::fs::write(paint_file(dir.path()), vec![0x5a; 40_000]).unwrap();
    let mode = |m| std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(m));
    mode(0o555).unwrap();
    let doc = open(dir.path(), &base);
    let probe = std::fs::write(dir.path().join("probe"), b"");
    mode(0o755).unwrap();
    if probe.is_ok() {
        return; // root writes anywhere: nothing to test
    }
    assert_eq!(doc.unsaveable(), Some(Unsaveable::Unwritable));
    assert!(set_aside_in(dir.path()).is_empty(), "nothing moved");
}

/// Damaged paint is told apart from none, and set aside when opened to
/// paint, never overwritten.
#[test]
fn damaged_paint_is_set_aside_not_overwritten() {
    let base = base();
    type Break<'a> = &'a dyn Fn(&Path);
    let run = |dir: &Path, statements: &str| sql(dir).execute_batch(statements).unwrap();
    let broken: [(&str, Break); 7] = [
        ("another application's", &|d| run(d, "PRAGMA application_id = 7")),
        ("an older schema", &|d| run(d, "PRAGMA user_version = 0")),
        ("a newer schema", &|d| run(d, "PRAGMA user_version = 2")),
        ("another size", &|d| run(d, "UPDATE canvas SET width = 299")),
        ("a tile off the grid", &|d| run(d, "UPDATE tiles SET x = x + 3")),
        ("a tile that doesn't decode", &|d| run(d, "UPDATE tiles SET data = x'01020304'")),
        ("garbage", &|d| std::fs::write(paint_file(d), vec![0x5a; 40_000]).unwrap()),
    ];
    for (what, damage) in broken {
        let dir = render_dir();
        let doc = painted(dir.path(), &base, 2);
        assert!(save(&doc).unwrap());
        drop(doc);
        damage(dir.path());
        let kept = std::fs::read(paint_file(dir.path())).unwrap();
        let unread = load_doc(dir.path(), base.width, base.height).err();
        assert_eq!(unread, Some(Unread::Damaged), "{what}: told apart from no paint");
        assert!(set_aside_in(dir.path()).is_empty(), "{what}: a read moves nothing");

        let mut doc = open(dir.path(), &base);
        assert!(!doc.is_painted(), "{what}");
        assert!(!doc.is_unsaved(), "{what}: opening alone saves nothing");
        assert!(!paint_file(dir.path()).exists(), "{what}: moved");
        paint(&mut doc, 0);
        assert!(save(&doc).unwrap(), "{what}");
        let aside = set_aside_in(dir.path());
        assert_eq!(aside.len(), 1, "{what}: {aside:?}");
        assert!(aside[0].ends_with(".sqlite"), "{what}: {aside:?}");
        assert_eq!(std::fs::read(dir.path().join(&aside[0])).unwrap(), kept, "{what}: as it was");
    }
}

/// A read writes and deletes nothing.
#[test]
fn a_read_writes_nothing() {
    let (dir, base) = (render_dir(), base());
    let doc = painted(dir.path(), &base, 4);
    assert!(save(&doc).unwrap());
    assert!(dir.path().join(UNDO_FILE).exists(), "the open doc's");
    let files = |dir: &Path| -> Vec<(String, Vec<u8>)> {
        let kept = names(dir).into_iter().filter(|n| !n.ends_with("-shm") && !n.ends_with("-wal"));
        kept.map(|n| (n.clone(), std::fs::read(dir.join(n)).unwrap())).collect()
    };
    let before = files(dir.path());
    assert!(load_doc(dir.path(), base.width, base.height).unwrap().unwrap().is_painted());
    assert!(has_paint(dir.path()).unwrap());
    assert!(frame_roots(dir.path()).unwrap().is_empty());
    assert_eq!(files(dir.path()), before);
    drop(doc);
}

#[test]
fn a_fresh_doc_saves_nothing_until_painted() {
    let (dir, base) = (render_dir(), base());
    let doc = open(dir.path(), &base);
    assert!(doc.persist().is_some() && !doc.is_unsaved());
    drop(doc);
    assert_eq!(names(dir.path()), [RENDER_FILE]);
    assert_eq!(has_paint(dir.path()), Ok(false));
}

#[test]
fn a_malformed_source_table_alone_is_replaced() {
    let (dir, base) = (render_dir(), base());
    let doc = painted(dir.path(), &base, 2);
    assert!(save(&doc).unwrap());
    drop(doc);
    sql(dir.path()).execute("DELETE FROM sources WHERE id = 0", []).unwrap();
    let doc = open_doc(dir.path(), base.width, base.height);
    assert!(doc.sources.is_valid() && doc.is_unsaved() && doc.is_painted());
    assert!(set_aside_in(dir.path()).is_empty());
}

#[test]
fn a_reset_is_saved() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 3);
    let shown = flat(&doc);
    assert!(save(&doc).unwrap());
    let canvas = doc.canvas_mut().unwrap();
    let before: Vec<_> = canvas.painted().map(|(id, t)| (id, Some(t.clone()))).collect();
    for (id, _) in &before {
        canvas.replace(*id, None);
    }
    doc.history.push("Reset", before);
    doc.mark_changed();
    assert!(save(&doc).unwrap());
    assert_eq!(tile_rows(dir.path()), 0);
    let (canvas, history) = doc.parts_mut().unwrap();
    history.undo(canvas).unwrap();
    assert_eq!(flat(&doc), shown, "the reset undoes");
    drop(doc);
    assert!(!open(dir.path(), &base).is_painted());
}

/// A commit after its render is deleted fails without making a folder. On
/// Unix only: Windows doesn't move a folder with a file open in it.
#[cfg(unix)]
#[test]
fn a_commit_never_brings_back_a_deleted_render() {
    let (dir, base) = (render_dir(), base());
    let doc = painted(dir.path(), &base, 2);
    let gone = dir.path().with_extension("partial");
    std::fs::rename(dir.path(), &gone).unwrap();
    assert!(save(&doc).is_err(), "it fails");
    assert!(!dir.path().exists(), "no folder made");
    std::fs::rename(&gone, dir.path()).unwrap();
    let no_render = tempfile::tempdir().unwrap();
    let doc = open_doc(no_render.path(), base.width, base.height);
    assert_eq!(doc.unsaveable(), Some(Unsaveable::NoRender));
    assert!(names(no_render.path()).is_empty());
}

/// A full disk rolls a commit back whole; the next commit with room
/// writes it.
#[test]
fn a_full_disk_fails_a_commit_whole() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 2);
    let mut writers = Writers::default();
    assert!(save_in(&mut writers, &mut doc).unwrap());
    let saved = flat(&doc);
    for i in 2..8 {
        paint(&mut doc, i);
    }
    full::set(dir.path(), true);
    let e = save_in(&mut writers, &mut doc).unwrap_err();
    full::set(dir.path(), false);
    let StoreError::Io { source, .. } = &e else { panic!("{e}") };
    assert_eq!(source.kind(), io::ErrorKind::StorageFull, "{e}");
    assert!(doc.is_unsaved());
    assert_eq!(flat(&load(dir.path(), &base)), saved, "the commit before stands");
    assert!(save_in(&mut writers, &mut doc).unwrap());
    writers.leave_all();
    assert_eq!(flat(&load(dir.path(), &base)), flat(&doc));
}

/// Puts every folder on a share.
struct Share;

impl Probe for Share {
    fn stat(&self, path: &Path) -> io::Result<Option<crate::frames::sources::Stat>> {
        Disk.stat(path)
    }
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        Disk.canonical(path)
    }
    fn volume(&self, path: &Path) -> io::Result<crate::frames::sources::Mounted> {
        let mut mounted = Disk.volume(path)?;
        mounted.volume.kind = VolumeKind::Share { server: "nas".into(), share: "photos".into() };
        Ok(mounted)
    }
    fn volumes(&self) -> Vec<crate::frames::sources::Mounted> {
        Disk.volumes()
    }
    fn read(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        Disk.read(path, offset, len)
    }
    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        Disk.list(dir)
    }
}

/// The journal follows the volume at each open, both ways.
#[test]
fn the_journal_follows_the_volume() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 2);
    let mode = |writers: &Writers| -> (String, i64) {
        let conn = &writers.open[dir.path()].conn;
        let mode = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        let synchronous = conn.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
        (mode, synchronous)
    };
    let mut writers = Writers::default();
    writers.commit_on(&doc.snapshot().unwrap(), &Share).unwrap();
    assert_eq!(mode(&writers), ("delete".into(), 3), "made on a share: EXTRA");
    writers.leave_all();
    paint(&mut doc, 2);
    writers.commit_on(&doc.snapshot().unwrap(), &Disk).unwrap();
    assert_eq!(mode(&writers), ("wal".into(), 2), "moved to a local disk: FULL");
    writers.leave_all();
    paint(&mut doc, 3);
    writers.commit_on(&doc.snapshot().unwrap(), &Share).unwrap();
    assert_eq!(mode(&writers), ("delete".into(), 3), "and back");
    writers.leave_all();
    assert!(!doc.is_unsaved());
    let shown = flat(&doc);
    drop(doc);
    assert_eq!(flat(&open(dir.path(), &base)), shown);
}

/// A hot journal is rolled back by the painting open; read-only readers
/// find it unreachable and leave it.
#[test]
fn a_hot_journal_is_rolled_back_by_the_painting_open() {
    let (dir, base) = (render_dir(), base());
    let doc = painted(dir.path(), &base, 3);
    let mut writers = Writers::default();
    writers.commit_on(&doc.snapshot().unwrap(), &Share).unwrap();
    writers.leave_all();
    let committed = flat(&doc);
    drop(doc);

    // A commit under way, copied as a crash would leave it.
    let conn = sql(dir.path());
    conn.execute_batch(
        "PRAGMA cache_size = 1; BEGIN IMMEDIATE;
         UPDATE tiles SET data = data || x'00'; DELETE FROM tiles;",
    )
    .unwrap();
    let crashed = render_dir();
    for name in [PAINT_FILE.to_string(), format!("{PAINT_FILE}-journal")] {
        std::fs::copy(dir.path().join(&name), crashed.path().join(&name)).unwrap();
    }
    drop(conn);
    let crashed = crashed.path();
    let before = names(crashed);

    let unread = load_doc(crashed, base.width, base.height).err();
    assert_eq!(unread, Some(Unread::Unreachable), "a read-only reader can't roll it back");
    assert_eq!(frame_roots(crashed).err(), Some(Unread::Unreachable));
    let doc = open_doc(crashed, base.width, base.height);
    assert_eq!(doc.unsaveable(), Some(Unsaveable::Unreachable), "with nothing to roll it back");
    assert_eq!(names(crashed), before, "left as it is");

    let mut writers = Writers::default();
    let mut doc =
        open_doc_rolling_back(crashed, base.width, base.height, || writers.roll_back(crashed));
    assert!(writers.open.is_empty(), "closed again: Retouch wasn't keeping it");
    doc.open(base.clone());
    assert_eq!(doc.unsaveable(), None, "saved as it is painted");
    assert_eq!(flat(&doc), committed, "the last committed paint");
    assert!(set_aside_in(crashed).is_empty(), "nothing set aside");
    assert!(!crashed.join(format!("{PAINT_FILE}-journal")).exists(), "rolled back");
    paint(&mut doc, 9);
    assert!(save(&doc).unwrap());
    assert_eq!(flat(&load(crashed, &base)), flat(&doc));
}

/// Leaving gives free pages back and empties the WAL.
#[test]
fn leaving_tidies_the_file() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 6);
    let mut writers = Writers::default();
    assert!(save_in(&mut writers, &mut doc).unwrap());
    erase_all(&mut doc);
    assert!(save_in(&mut writers, &mut doc).unwrap());
    let free = |dir: &Path| -> i64 {
        sql(dir).query_row("PRAGMA freelist_count", [], |r| r.get(0)).unwrap()
    };
    writers.leave_all();
    assert_eq!(free(dir.path()), 0);
    let wal = dir.path().join(format!("{PAINT_FILE}-wal"));
    assert!(std::fs::metadata(&wal).map_or(true, |m| m.len() == 0));
}

/// A frame source is saved by root: a relink moves it with nothing
/// written; another Library resolves it to nothing.
#[test]
fn frame_sources_are_saved_by_root() {
    let (dir, base) = (render_dir(), base());
    let (library, frames, paths) = crate::paint::source::tests::library(1);
    let mut doc = open(dir.path(), &base);
    paint(&mut doc, 0);
    doc.sources.id(&SourceKind::Frame { path: paths[0].clone() }, &frames).unwrap();
    assert!(save(&doc).unwrap());
    drop(doc);
    let written = std::fs::read(paint_file(dir.path())).unwrap();
    let root = frames.refs(&paths).unwrap().remove(0).root;
    assert_eq!(frame_roots(dir.path()).unwrap(), [root]);

    let new = library.path().join("new");
    std::fs::rename(library.path().join("Photos"), &new).unwrap();
    frames.relink(&library.path().join("Photos"), &new, None).unwrap();
    let doc = load_doc(dir.path(), base.width, base.height).unwrap().unwrap();
    let moved = SourceKind::Frame { path: new.join("frame-0.tif") };
    assert_eq!(doc.sources.kind(1, &frames), Some(moved));
    assert_eq!(std::fs::read(paint_file(dir.path())).unwrap(), written, "no paint written");

    let (_other, elsewhere, _) = crate::paint::source::tests::library(0);
    assert_eq!(doc.sources.kind(1, &elsewhere), None);
}

std::thread_local! {
    /// The page size for files made on this thread; changed only by the
    /// benchmark.
    pub(crate) static NEW_PAGE_SIZE: std::cell::Cell<u32> = const { std::cell::Cell::new(super::PAGE_SIZE) };
}

/// 16 KB against 8 KB pages for a fully painted 45 MP render (ADR-148).
/// `cargo test --release -p deptheos_app --lib page_sizes -- --ignored --nocapture`
#[test]
#[ignore = "a benchmark: run by hand"]
fn page_sizes() {
    use std::time::Instant;
    const W: u32 = 8192;
    const H: u32 = 5504;
    let tile = |id: TileId, n: u16, grain: u32| {
        let mut tile = TileData::blank();
        let t = TILE as usize;
        for (i, v) in tile.rgb.iter_mut().enumerate() {
            let (px, ch) = (i / 3, i % 3);
            let (x, y) = (id.x as usize * t + px % t, id.y as usize * t + px / t);
            let mut h = ((y * 8192 + x) as u64 * 3 + ch as u64) ^ u64::from(n) << 40;
            h = (h ^ h >> 31).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            h = (h ^ h >> 29).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            let noise = (h >> (64 - grain)) as usize;
            *v = (x * 5 + y * 3 + ch * 4000 + noise) as u16;
        }
        tile.coverage.iter_mut().for_each(|c| *c = 255);
        tile.source.iter_mut().for_each(|s| *s = 1);
        Arc::new(tile)
    };
    // Bits of noise: clean, then grainy high-ISO.
    let runs = [6, 12].into_iter().flat_map(|g| [(16, g), (8, g), (16, g), (8, g)]);
    for (page, grain) in runs.map(|(kb, g)| (kb * 1024, g)) {
        NEW_PAGE_SIZE.set(page);
        let dir = render_dir();
        let mut doc = open_doc(dir.path(), W, H);
        doc.open(stack(W, H, 20000));
        let canvas = doc.canvas_mut().unwrap();
        let [cols, rows] = canvas.grid();
        let ids: Vec<_> = (0..rows).flat_map(|y| (0..cols).map(move |x| TileId { x, y })).collect();
        for &id in &ids {
            canvas.replace(id, Some(tile(id, 0, grain)));
        }
        doc.mark_changed();
        let mut writers = Writers::default();
        let at = Instant::now();
        writers.commit(&doc.snapshot().unwrap()).unwrap();
        let first = at.elapsed();
        let canvas = doc.canvas_mut().unwrap();
        for &id in ids.iter().step_by(10) {
            canvas.replace(id, Some(tile(id, 1, grain)));
        }
        doc.mark_changed();
        let at = Instant::now();
        writers.commit(&doc.snapshot().unwrap()).unwrap();
        let tenth = at.elapsed();
        writers.leave_all();
        let size = std::fs::metadata(paint_file(dir.path())).unwrap().len();
        let at = Instant::now();
        let read = load_doc(dir.path(), W, H).unwrap().unwrap();
        let read_back = at.elapsed();
        assert!(read.is_painted());
        eprintln!(
            "{} KB pages, {grain} bits of noise, {} tiles: first commit {:.0} ms, a tenth again {:.0} ms, \
             {:.1} MB, read back {:.0} ms",
            page / 1024,
            ids.len(),
            first.as_secs_f64() * 1e3,
            tenth.as_secs_f64() * 1e3,
            size as f64 / 1e6,
            read_back.as_secs_f64() * 1e3,
        );
    }
}

/// A commit with a newer one sent behind it is overtaken.
#[test]
fn a_commit_queued_behind_a_newer_one_is_overtaken() {
    let (dir, base) = (render_dir(), base());
    let mut doc = painted(dir.path(), &base, 1);
    let first = doc.snapshot().unwrap().sent();
    assert!(!first.is_overtaken(), "the newest sent");
    paint(&mut doc, 2);
    let second = doc.snapshot().unwrap().sent();
    assert!(first.is_overtaken(), "a newer one sent behind it");
    assert!(!second.is_overtaken());
    let mut writers = Writers::default();
    writers.commit(&second).unwrap();
    writers.leave_all();
    assert!(!doc.is_unsaved(), "the newer commit saved every change");
}

/// The cost of committing each stroke on a 45 MP render, local and as on
/// a share.
/// `cargo test --release -p deptheos_app --lib stroke_commits -- --ignored --nocapture`
#[test]
#[ignore = "a benchmark: run by hand"]
fn stroke_commits() {
    use std::time::Instant;
    const W: u32 = 8192;
    const H: u32 = 5504;
    let tile = |n: u16| {
        let mut tile = TileData::blank();
        for (i, v) in tile.rgb.iter_mut().enumerate() {
            let h = (i as u64 ^ u64::from(n) << 32).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            *v = (i % 4000) as u16 + (h >> 58) as u16 + n;
        }
        tile.coverage.iter_mut().for_each(|c| *c = 255);
        tile.source.iter_mut().for_each(|s| *s = 1);
        Arc::new(tile)
    };
    let shared = tile(0);
    for (name, probe) in [("local disk", &Disk as &dyn Probe), ("as on a share", &Share)] {
        let dir = match std::env::var_os("STROKE_COMMITS_DIR") {
            Some(at) => tempfile::tempdir_in(at).unwrap(),
            None => render_dir(),
        };
        std::fs::write(dir.path().join(RENDER_FILE), "{}").unwrap();
        let mut doc = open_doc(dir.path(), W, H);
        doc.open(stack(W, H, 20000));
        let canvas = doc.canvas_mut().unwrap();
        let [cols, rows] = canvas.grid();
        for y in 0..rows {
            for x in 0..cols {
                canvas.replace(TileId { x, y }, Some(shared.clone()));
            }
        }
        doc.mark_changed();
        let mut writers = Writers::default();
        writers.commit_on(&doc.snapshot().unwrap(), probe).unwrap();
        let mut times = Vec::new();
        for n in 1..=30u16 {
            // A stroke across 6 × 6 tiles.
            let canvas = doc.canvas_mut().unwrap();
            let (x0, y0) = (u32::from(n) * 7 % (cols - 6), u32::from(n) * 5 % (rows - 6));
            for y in y0..y0 + 6 {
                for x in x0..x0 + 6 {
                    canvas.replace(TileId { x, y }, Some(tile(n)));
                }
            }
            doc.mark_changed();
            let at = Instant::now();
            let snapshot = doc.snapshot().unwrap();
            writers.commit_on(&snapshot, probe).unwrap();
            times.push(at.elapsed().as_secs_f64() * 1e3);
        }
        writers.leave_all();
        times.sort_by(f64::total_cmp);
        eprintln!(
            "{name}, {} tiles painted, a stroke of 36 committed: median {:.1} ms, slowest {:.1} ms",
            cols * rows,
            times[times.len() / 2],
            times[times.len() - 1],
        );
    }
}
