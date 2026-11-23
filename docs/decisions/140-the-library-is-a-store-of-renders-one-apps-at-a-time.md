# ADR-140: The Library is a store of immutable renders in the app's data folder, one app's at a time, and a delete goes to Recently Deleted

## Status

Accepted.

## Context

Photographers stack the same bracket several times, with different methods, settings or frames left out, to compare the results. Each render has to reopen quickly with everything that made it (its 16-bit result, its frame map, its alignment, its color metadata), so that comparing, painting and exporting work the same later. At 45 MP, uncompressed 16-bit RGB and its maps take hundreds of megabytes a render.

Every Library file is written by reading it, changing it and writing it back, and opening a Library sweeps what a crashed save left. Two writers on one Library (a second app with another home folder, another computer sharing the drive, a command-line tool given a render's folder) would undo each other's edits, and one's sweep could delete the other's save in progress.

A delete that removes at once has to ask first, and still can't be undone. A question before every delete is friction a user learns to click through, and it still can't bring back the wrong stack. The system's trash is not on every volume (a network share, an exFAT card, iOS's container), so a delete that leaned on it would differ by where the Library is.

## Decision

### Where and how renders are kept

- **Every finished stack is an immutable render in the Library folder** (`dirs::data_local_dir()/Deptheos`, or the desktop's `history_dir` setting), one folder per render: `Renders/<uuidv7>/render.json` and its pictures (`crates/deptheos_app/src/library/history.rs`, whose module doc lays the Library out):
  - **`result.jxl`**: the result, lossless 16-bit RGB JPEG XL at effort 1.
  - **`frame_map.jxl`**: the frame map, lossless 16-bit gray JPEG XL at effort 1. Contribution, paint's alignment, the depth map and each frame's share of the result read it (ADR-149).
  - **`thumb.jpg`**: at most `THUMBNAIL_SIDE` pixels, sRGB, for the Library's tiles.
  - **Color metadata**: a linear stack (of camera RAW or DNG frames decoded linear) keeps a 16×16 `color.dng` carrying the reference frame's DNG metadata (ADR-130); a stack of standard images not in sRGB keeps `profile.icc` (ADR-132).
  - **`paint.sqlite`**, and `paint-undo.sqlite` while Retouch has painted it this session (ADR-148).
- **The codec** is the vendored libjxl wrapped by `deptheos_dng::jxl` (`crates/deptheos_dng/native/jxl_c_api.cpp`).
- **A render is made crash-safe**: it is built under `Renders/<id>.partial` with `render.json` written last, then renamed into place, so an interrupted save leaves a `.partial` folder the next open sweeps, never a half-written render.
- **No depth-map picture is saved.** The depth map is drawn from the frame map when shown (ADR-149); both apps show the frame map as the depth map, so nothing showed a saved 8-bit picture, which cost about 7 MB and a GPU render a stack and a decode on every open. The command line has no `--map`.
- **Source photo folders are never written to.** Only exports the user asks for are written outside the Library folder (ADR-142).
- **Frames are named by root** in every file (ADR-145), so a folder that moves is followed by one entry of `sources.json`.

### One app at a time

- **`History::open` takes an OS lock on `<Library>/library.lock` and holds it until the last clone of the History goes** (`crates/deptheos_app/src/library/history/lock.rs`). A second opener, in this process or another, on this computer or another, is refused with `StoreError::InUse`. A volume that can't lock a file is refused too (`StoreError::NoLocks`), never opened unguarded.
- **Who holds it is said.** The holder writes `library-owner.json` beside the lock (the computer's name and the app's version) once it has the lock and the Library's format has passed. A refused open reads it and says "The Library is open in Deptheos 0.3.0 on Studio Mac" or "in another Deptheos on this computer" (`crates/deptheos_app/src/words/explain.rs`). It is read only while someone holds the lock, so one left from an earlier session never misleads.
- **Maintenance runs as the lock's holder.** The open's sweep (a crashed save's `.partial` folders and temporaries, the undo spills of a session that ended) runs after the lock is taken, as the only writer, so it takes whatever is there however new.
- **A Library on a drive not connected isn't made in its place.** `History::open` makes a Library's folder only in a folder that is there (the default Library's folders are made as needed); otherwise it is `StoreError::NotConnected`, said as a drive to connect, not a permissions problem.
- **A Library in a synced folder is refused**, when chosen in Settings and when opened (`StoreError::Synced`): iCloud Drive, Desktop and Documents when iCloud Drive holds them, any File Provider's folder in `~/Library/CloudStorage` (Dropbox, OneDrive, Google Drive), a Dropbox folder by its `.dropbox` mark, OneDrive's and Google Drive's own folders (`history/synced.rs`). A service that syncs files while the app writes them, and brings another computer's copy back, can damage a Library, and the lock doesn't reach across its copies. It is told by the path, so a folder a service keeps somewhere else isn't caught.
- **The Library's format is checked as it opens** (ADR-141): a Library of a newer format is refused whole and left untouched but for the lock's own file.
- **The apps don't run without their Library**, so nothing in them has a state without one. The desktop opens it, and reads the working version left on screen, before its main window; when either fails, a start window says why, with Try Again, Choose Another Library… and Quit, and Get the Latest Version first for a newer Library (`crates/deptheos_gpui/src/start.rs`). iOS starts no engine without its Library, and says whether its GPU or its Library didn't start, and why (`Engine::new` in `crates/deptheos_ffi/src/engine.rs`).
- **The command line never opens a Library.** `sequence grow` takes frames only (ADR-103).

### A delete goes to Recently Deleted

- **A delete moves, never removes.** Deleting renders, failed batch stacks or whole stacks renames them into `<Library>/Deleted/<id>/`, laid out as the Library is (`Renders/`, `Failed/`, `Stacks/`), with a small record, `deleted.json`: when, the stack, its name as it was called, whether the whole stack went, the renders' ids and numbers, the failed runs and the size (`crates/deptheos_app/src/library/history/deleted.rs`). It is a rename within the Library's folder, so it is instant and the same on every volume and both apps.
- **One record per stack a delete reaches, restored whole.** A whole stack's record holds its renders, failed runs and its file (name, numbering, kept working version); a delete of renders holds those renders and failed runs, and the stack stays, named. A delete reaching several stacks makes a record for each, and its Undo restores them all. A restore renames everything back; a whole stack's file is merged into one the stack has made since (its name where it has none, the higher number, the working version kept now first; `restore_stack_file`).
- **Deleting asks nothing; Undo is in the notice.** Each app says what went ("Bee #2 moved to Recently Deleted.") with Undo beside it, and keeps a Recently Deleted list with each record's thumbnail, name, days left and size, Restore and Empty. Of deletes, only Empty Recently Deleted and Delete Library ask. Delete Library stays permanent, empties Recently Deleted too and says what it frees.
- **Kept 30 days (`KEPT_DAYS`), removed by the lock's holder.** `History::open` removes older records after its sweep, as the Library's only writer. A stack then left with nothing (no render, failed run, kept working version or record, and not on screen) is forgotten, its name and numbering with it (`forget_if_unused`). This is done here, not when a stack's last render is deleted, so a restore within the 30 days finds its stack named.
- **What is deleted still counts.** The listing reads `Renders/`, `Failed/` and `Stacks/` only, so nothing deleted lists or opens. The roots a record's files name are kept (`roots_named`), so a restored render finds its frames, and a save's number passes the numbers a record holds of its stack (`claim_number`), so a restored render never shares one.
- **The record first, the moves after.** The record is written under `<id>.partial` and renamed into place before anything moves into it, so a crash part way leaves a record of what moved, restored like any other, or a `.partial` the next open sweeps.
- **Recently Deleted is the Library's format 2** (`SOURCES_SCHEMA`, ADR-141).
- **Each app's Library has a Recently Deleted entry**, with its size; the notice after a delete, with Undo, shows at the foot of the Library and of the Renders list.

## Alternatives considered

### Keep state and renders in a `.deptheos` folder beside the photos

SD cards and network shares can be read-only; photo managers and sync folders import or sync files written beside originals; iOS's security-scoped folder bookmarks are not reliably writable; and a stack of frames from several folders has no one parent folder.

### TIFF and PNG instead of JPEG XL

On a 44 MP stack on an Apple M5, lossless JPEG XL at effort 1 encoded in 0.06 s and decoded in 0.17 s at 46% of the raw size; 16-bit PNG took 0.40 s at 75%. Parallel JPEG XL decoding opens a 36 MP render in about 280 ms, against about 1.1 s for deflate TIFF and PNG on Windows.

### A sweep that leaves recent files alone, without a lock

A grace period (an hour) would let a second writer's save in progress survive the sweep, but nothing would stop the two writers undoing each other's edits. With the lock the sweep is the only writer and takes everything.

### A question before each delete, or the system's trash

A question is clicked through and can't undo; the trash is missing on shares, cards and iOS's container, and would make a delete depend on the volume. A rename into the Library's own `Deleted/` works everywhere.

## Consequences

- Any render can be reopened, compared, retouched or exported at any time with its exact settings and geometry. The Library shows its disk use and offers Delete Other Renders….
- A deleted render's folder stays on disk for 30 days and counts in the Library's size; Empty frees it at once. Delete Other Renders and Cmd-Backspace delete straight away, with Undo.
- Tests that relaunch the app on one Library close the first window and wait for its lock (`test_support::quit` in the desktop crate), and read the Library back through the app's own History while it runs.
- The lock files stay in the Library folder; Delete Library leaves them.

## Code

- `crates/deptheos_app/src/library/history.rs`, `history/lock.rs`, `history/synced.rs`, `history/deleted.rs`, `history/words.rs`, `crates/deptheos_app/src/library/render_file.rs`, `crates/deptheos_app/src/library/stacks.rs`, `crates/deptheos_app/src/words/explain.rs`, `crates/deptheos_app/src/frames/sources/mod.rs`.
- `crates/deptheos_dng/native/jxl_c_api.cpp`.
- Desktop: `crates/deptheos_gpui/src/start.rs`, `crates/deptheos_gpui/src/renders/library_deleted.rs`, `crates/deptheos_gpui/src/renders/queue.rs`, `crates/deptheos_gpui/src/app.rs`.
- iOS: `crates/deptheos_ffi/src/engine.rs`, `ios/DeptheosMobile/Model/AppModel+Deleted.swift`, `ios/DeptheosMobile/Views/RecentlyDeletedView.swift`.
