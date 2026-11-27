# ADR-148: Paint is a CPU canvas of 128 px tiles in the result's own color space, on one worker, with undo that lasts the session; a render's paint is one SQLite file, written in one transaction per commit, its spilled undo a file of its own

## Status

Accepted.

## Context

Retouching paints pixels from a source frame or another render into a stacked result, to fix halos or bring back detail from one focus plane. Results are 16 bits a channel and up to 45 MP and more; RAW stacks are in linear camera RGB and shown through the GPU's DNG color render (ADR-130).

Paint is also the only part of the Library written many times a session, in place. Keeping its tiles crash-safe in files of our own (a manifest, append-only logs, generations, compaction, sweeps of logs the manifest doesn't name) is a database written by hand.

## Decision

### The canvas, the worker and undo

- **A CPU canvas of copy-on-write tiles**, 128×128 RGB `u16` (`TILE` in `crates/deptheos_app/src/paint/tile.rs`), over the render's original pixels, in the result's own color space (linear camera RGB for a RAW stack, its stored RGB for a standard one). An unpainted tile references the original at no extra memory.
- **A stroke composites from its stroke-start snapshot through an `f32` coverage mask**, so overlapping dabs within a stroke never build past the brush's opacity, and each pixel rounds once to `u16`.
- **One paint worker** (`worker.rs`) processes commands in order and owns the open paint documents, the warped source cache and the display updates. Its `Mode` (idle, open, stroking) is its lifecycle (ADR-143). Both apps drive it through one `PaintSession` (`session.rs`) over a pure `paint::Machine` (`machine.rs`).
- **Display updates:** a stroke's changed tiles are a set, and each display tile they touch is updated on its own (`display.rs`): an sRGB stack's converted to 8 bits on the CPU, a linear RAW stack's rendered through `dng_render.wgsl` on the GPU (`DngRenderPipeline::draw`, ADR-135) and read back.
- **On a turned or flipped render the display is the canvas turned** (ADR-149): the worker reads the render's reframe as it opens, and each display tile is rendered from the stored rect under it and turned (`engine::orient::Orient`), so patches land on the turned picture's grid while the canvas, its strokes and its undo stay in stored pixels. A source frame is handed over turned the same way.
- **Undo and redo swap `Arc` tile snapshots**, so they copy no pixels. A stroke that changes no pixel records no step and leaves redo as it was. Undo lasts until the app quits; the painted layer is kept with the render.
- **Budgets** (`deptheos_app::engine::memory::Budget`, read each time they are asked, whose doc gives each one): undo snapshots stay in RAM up to `PaintHistory` (RAM / 16 on the desktop), the sources painted from up to `PaintSources` (RAM / 4); on iOS each is a share of what the app has left. Older steps spill to the undo file (below), capped at 2 GB a render (`HISTORY_DISK_CAP`).
- **The newest steps are always kept: three, or one on iOS** (`KEPT_STEPS`). When the undo file can't be written (a read-only or full disk), `UndoStack::shed` drops older steps to stay within the RAM budget, keeping the painted pixels, but never the newest `KEPT_STEPS`, even over budget, so a huge image still has some undo. On iOS that is the newest alone, under its memory limit: a stroke over a whole 45 MP render holds 360 MB of snapshots, so three would be over a gigabyte.
- **Memory pressure** (`deptheos_app::engine::memory::shed`, which iOS's memory warnings reach through the bridge's `shed_memory`): a moderate shed drops the previous and prefetched sources (`SourceCache::shed`) and every undo snapshot the spill holds; a critical one also the oldest steps that can't be spilled, never the newest `KEPT_STEPS`.
- **Only the open render's layer stays in RAM.** Each render painted this session keeps its undo history, but once a closed render's paint is committed, its tiles leave RAM (`PaintDoc::release`, `doc.rs`) and are read back from its file when it opens again; undo and redo swap with the read-back tiles as before. Unsaved paint is never let go of. A read-back that fails opens the render's paint afresh from disk, which sets damaged paint aside or leaves unreachable paint in place, and that render's session history goes.
- **Paint is a layer on a saved render.** Retouching changes the render's paint layer, never makes a duplicate render, and never changes the frames and settings the render records (ADR-142). Paint mode opens only on saved renders, so the spill and the store always have a place.
- **Sources are `FrameRef`s** (ADR-145), in memory and in the file, resolved through `Frames` only to load one, so a relink changes no paint and can't race a commit. A relink drops the source loaders of frames that moved.

### The session: scrubs, opens and presents

- **A scrub sends the worker one frame at a time.** The worker skips a frame scrubbed past when a later pick overtakes it in its queue, but can't stop one it has begun building, so the session's `Pace` (idle, scrubbing, settling, prefetching, in `machine.rs`) sends the frame a scrub reaches, then only the newest reached meanwhile once that one lands, and picks the frame a scrub rests on in full `SCRUB_SETTLE` (300 ms) after; `PREFETCH_AFTER` (500 ms) after a frame is picked in full, the next one in the direction stepped is fetched ahead. A pick shows at once, ahead of the worker, and a state from before a pick never moves it back.
- **A failure belongs to its open.** The worker reports an open's failure apart from any other (`Output::OpenFailed`, with the render), so a late failure of a stroke or a save never marks a new open failed, and one for a render no longer painted is dropped.
- **An open sends one source pick**: the frame painted from, else the render's first stacked frame, else the original, never the original's full-size picture and then the frame.
- **A queued present is skipped** when a later present or open of the same render makes it pointless, so showing renders quickly, or leaving and entering Retouch again, doesn't queue full-size flattens ahead of the open.

### One SQLite file per render

- **A render's paint is `Renders/<id>/paint.sqlite`**: the painted layer, the source table and the canvas it was painted on. Its first commit makes it, directly in the render's folder; the store never makes a folder. A render opened in Retouch and never painted gets no file and no connection.
- **SQLite 3.51.3 or later, bundled** (`rusqlite` with `bundled`), never the system's, its `sqlite3_libversion_number()` checked as the Library opens (`check_sqlite` from `History::open_in`). Versions 3.7.0 to 3.51.2 can corrupt a WAL database when a checkpoint races another connection's commit that resets the WAL, and here a writer and short-lived readers share each file.
- **Schema**, `user_version` 1, `application_id` `0x44505448` (`DPTH`):

  ```sql
  CREATE TABLE canvas (
    only   INTEGER PRIMARY KEY CHECK (only = 1),
    width  INTEGER NOT NULL,     -- the render's size, in pixels
    height INTEGER NOT NULL,
    tile   INTEGER NOT NULL      -- the side of the tiles the grid counts in
  );
  CREATE TABLE sources (
    id          INTEGER PRIMARY KEY CHECK (id BETWEEN 0 AND 255),  -- what a tile's source plane holds
    kind        TEXT NOT NULL CHECK (kind IN ('original', 'frame', 'render')),
    root        TEXT,     -- a frame (ADR-145): its root in sources.json,
    path        TEXT,     --   its path from the root,
    size        INTEGER,  --   and its size and fingerprint when it was added
    fingerprint BLOB,
    render      TEXT,     -- a render: its id
    CHECK ((kind = 'frame') = (root IS NOT NULL AND path IS NOT NULL)),
    CHECK ((kind = 'render') = (render IS NOT NULL))
  );
  CREATE TABLE tiles (
    y    INTEGER NOT NULL,       -- the tile's place in the grid, in tiles
    x    INTEGER NOT NULL,
    data BLOB NOT NULL,          -- the tile codec's bytes, codec byte first
    PRIMARY KEY (y, x)
  );
  ```

  A tile is one blob in its row, in the codec of `log.rs` (a codec byte, then byte planes of left deltas compressed by zstd at level 1). It is decoded whole, so incremental blob I/O buys nothing. The table keeps its row ids: a table without them holds its rows in its key's b-tree, which suits small rows, not blobs of tens of KB.
- **Settings made with the file**, before its tables: 16 KB pages, `auto_vacuum = INCREMENTAL`, the `application_id` and `user_version`, then the journal mode. Neither the page size nor auto-vacuum can change later without a `VACUUM`, and in WAL the page size can't change at all. Every connection sets `synchronous = FULL`, `fullfsync` and `checkpoint_fullfsync` on, and a 5 s busy timeout: in WAL, FULL syncs at every commit, and on Apple platforms `fullfsync` makes that sync an `F_FULLFSYNC`, which flushes the drive's cache, so a commit that has returned survives power loss. `fullfsync` already covers checkpoints; `checkpoint_fullfsync` is set to say so.
- **The journal follows the volume, at every open.** The render folder's volume is probed as roots are (`sources::Disk`: `statfs` on Apple platforms, `/proc/self/mountinfo` on Linux, `GetDriveTypeW` and UNC paths on Windows). On a local volume the journal is WAL. On a share (`VolumeKind::Share`) it is DELETE with `synchronous = EXTRA`: WAL needs memory shared by every connection, which a network file system can't give, and FULL with a rollback journal can still lose the last commit to a power loss. A Library moved to or from a share switches the file at its next open.
- **Reading** is one read transaction on a read-only connection, and writes or deletes nothing, with one exception: opening a render to paint rolls a hot journal back first. On `SQLITE_READONLY_ROLLBACK` the worker asks the store thread to open the file through its writer (`StoreJob::RollBack`, `Writers::roll_back`), which rolls the journal back and closes again unless Retouch is keeping it, and reads once more (`open_doc_rolling_back`). A read-only connection can't roll back the hot journal a commit cut off on a share leaves, so without the writer that paint would stay unreachable for good; the other readers (the Library's scan for roots in use, its Export…) take it as unreachable and leave it for the next painting open.
- **What a read finds** (`Unread`, `paint/store.rs`): a file that isn't what this build wrote (another `application_id` or `user_version`, a canvas of another size, a tile off the grid or one that doesn't decode, a corrupt database) is damage (`Unread::Damaged`), refused, never read wrong, and set aside by the painting open; any other error is `Unread::Unreachable` and leaves the file as it is. A malformed source table alone is replaced. Paint that can't be read is never taken for no paint: an export of the render fails with `Error::PaintUnread`, and a render shown outside Retouch says it is shown without its retouching.
- **Paint that can't be saved is said.** A render opened in Retouch whose paint is unreachable, or whose folder can't be written or damaged paint set aside, opens with a doc kept in memory only (`Storage::Memory`, `paint/doc.rs`), and both apps show a notice that stays while it is open: what is painted there can't be saved, and why (`PaintState::unsaveable`).
- **A commit is one transaction.** It takes `Arc` snapshots of the layer's tiles and the source table, and the doc's change count, so painting goes on meanwhile, and encodes the tiles changed since the last commit (by `Arc` identity) before it begins. Then `BEGIN IMMEDIATE`: it writes those tiles, deletes the rows of tiles no longer painted, replaces `sources`, and commits. The doc counts as saved only if nothing changed after its snapshot. A failed commit is rolled back with an explicit `ROLLBACK`, whose error is ignored if SQLite already rolled back; the doc stays unsaved and the next commit tries again. `SQLITE_FULL` is said as a full disk. The first commit syncs the render's folder once after making the file (`deptheos_core::io::sync_folder`), so a power loss can't take the new entry.
- **Commits run on the store thread** (ADR-143), one as each change ends (a stroke, an undo, a redo, a reset; there is no save timer) and one when Retouch leaves a render, so a stroke never waits for one. A commit still queued when a newer one of the same render is sent is skipped (`Snapshot::is_overtaken`), so commits slower than strokes don't pile up. Commits are sent, not waited for, but by `Command::Save` and a delete; a closed doc's tiles go once its commit has landed (`Worker::flush`).
- **One connection writes each file.** The store thread owns it from the render's first commit until Retouch leaves the render. Leaving commits, runs `PRAGMA incremental_vacuum` when free pages are over a quarter of the file, runs `PRAGMA wal_checkpoint(TRUNCATE)`, then closes (`Writers::leave`), so outside Retouch a render's `-wal` is gone or empty.
- **The undo spill is its own file, on its own connection**: `Renders/<id>/paint-undo.sqlite`, one table `snapshots (id INTEGER PRIMARY KEY, data BLOB NOT NULL)` in the same codec, 16 KB pages, `journal_mode = MEMORY` and `synchronous = OFF`, since nothing in it outlives a crash and no write of it should wait on a sync (`Spill`). The first snapshot spilled makes it. It is deleted when its doc leaves the worker (`Remove`, quit); one a crash left is deleted by the Library's sweep at launch, before anything is open (ADR-140), never by a read. The store thread writes it in transactions of its own (`Blob::write_all`); the worker reads an evicted snapshot through the connection's lock, so an undo never waits behind a commit. A snapshot is a row (`BlobState::Stored { row, .. }`, among `Unwritten`, `Stuck`, `Stored` and `Evicted` in `undo.rs`), freed as its blob drops.
- **Rules around the file:**
  - its `-wal` (or `-journal`) and `-shm` are part of it;
  - it is never opened through plain file calls while SQLite has it open (closing such a handle drops SQLite's locks on it); a size or existence check is a `stat`;
  - its folder is never renamed or deleted while it is open: a delete of renders, or of the Library, first has the paint store close their files (`History::close_paint`), then renames;
  - a damaged file is set aside with its `-wal` or `-journal` as `paint.unreadable-<id>.sqlite`, only while no connection has it open.

## Alternatives considered

### A full-frame GPU canvas texture

It needs 350 to 700 MB of dedicated video memory for a 44 MP image, and readbacks, while the CPU already holds the result's pixels, and copy-on-write tiles take memory only where the user paints.

### Undo kept across restarts

Multi-gigabyte undo logs kept in every retouched render's folder would grow the Library without bound. Keeping the final layer and dropping the undo on quit keeps disk use small, and **Reset…** still restores the original result at any time.

### Command-log undo, replaying strokes from scratch

Undo's cost would grow with the stroke count, and every source frame of earlier strokes would have to be kept or decoded again. Swapping tile `Arc`s makes undo and redo immediate.

### Our own files: a manifest and append-only tile logs

Generations, compaction and references into files are a database written by hand, and each fix is another rule in the sweep or the commit, with the next bug in the same code. SQLite's transactions replace them, and its crash safety is far more tested than ours can be.

### Tiles as files, with an index beside them

A tile's write would be a file made, synced and named in a folder synced after, and a commit would still need its own way to make many such writes atomic. SQLite's measurements put blobs under about 100 KB in the database as fast as or faster than separate files at 8 or 16 KB pages ([Internal Versus External BLOBs](https://www.sqlite.org/intern-v-extern-blob.html)); a tile is 128 KB raw and smaller compressed.

### 8 KB pages, or the default 4 KB

A compressed tile spans several pages, so larger pages mean shorter overflow chains, at the cost of space left unused at the end of each blob's last page. On a fully painted 45 MP render (`store_tests::page_sizes`), 16 KB pages committed 15–20% faster than 8 KB, at about the same size and read speed; 8 KB doesn't clearly win, and the page size can't change later.

### The undo spill attached to the layer's connection

One connection runs one transaction at a time, so a spill would either join a commit's open transaction or wait for it. In WAL a transaction across attached files is atomic only file by file ([ATTACH](https://www.sqlite.org/lang_attach.html)), and nothing needs the two together: undo is the session's, and a crash discards it. A file of its own keeps undo's churn out of the layer's file and its free pages, and the 2 GB cap apart.

### The undo spill as SQLite's temporary database

It is deleted when it closes and, on Unix, unlinked as soon as it is made, so a crash leaves nothing to sweep. But it lands in the system's temporary folder, apart from the Library's volume and its free space, and keeping undo across sessions later would mean moving it. A file beside the paint keeps that open with no format change.

### WAL everywhere

The WAL index is memory shared through the `-shm` file, which a network file system doesn't share reliably, and SQLite advises against WAL there ([SQLite Over a Network](https://www.sqlite.org/useovernet.html)). A rollback journal is the safer of the two, though over a network nothing is as safe as on a local disk.

## Consequences

- Painting needs no GPU work on standard stacks and only sub-rectangle DNG renders on RAW ones.
- Restarting the app clears undo and keeps every painted pixel; **Reset…** restores the original result after a confirmation.
- SQLite's C source is built into every app by `rusqlite`'s build script, as the other native libraries are (ADR-134).
- On Apple platforms every commit waits for an `F_FULLFSYNC`, tens of milliseconds on an internal SSD and more on an external drive, on the store thread. A stroke's commit (`store_tests::stroke_commits`) takes a median 5 ms with WAL and 15 ms with the share's rollback journal on an internal SSD; a real share adds its round trips to each sync.
- On a share a commit is slower, and SQLite's locks over a network can't be relied on; the Library's own lock keeps a second app out (ADR-140).
- On iOS the Library is in Application Support, which has the default protection class (complete until the first unlock), and SQLite's files inherit it, so a commit in the background works after the first unlock; the Library must never get a stricter class. It isn't in a shared App Group container, where the system ends a suspended app that holds a SQLite lock. Going to the background commits and closes Retouch's file inside the app's background time.
- Crash safety is tested by a child process that commits in a loop and is killed at random while the parent reopens and checks (`store_kill.rs`), and a full disk by holding `PRAGMA max_page_count` to the file's size (`store_tests::full`). The fixture `crates/deptheos_app/tests/fixtures/paint-v1` is read by `formats.rs` through the real reader.

## Code

- `crates/deptheos_app/src/paint/`: the canvas (`tile.rs`, `doc.rs`, `display.rs`), undo (`undo.rs`), sources (`source.rs`), the worker, session and machine (`worker.rs`, `session.rs`, `machine.rs`), the file (`store.rs`, `log.rs`), and the tests (`store_tests.rs`, `store_kill.rs`, `machine_tests.rs`).
- `crates/deptheos_app/src/engine/memory.rs`; `crates/deptheos_app/src/library/history.rs`; `crates/deptheos_app/src/fsutil.rs`.
- `crates/deptheos_ffi/src/paint.rs` (the bridge).
