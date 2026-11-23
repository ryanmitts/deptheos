# ADR-141: One persistence layer: the Library's one format checked as it opens, atomic replacing writes, damaged files set aside or replaced, fixtures for every format

## Status

Accepted.

## Context

Deptheos keeps render manifests (`render.json`), the working version on screen (`list.json`), each stack's file (`Stacks/<id>.json`), failed batch stacks (`Failed/<id>.json`), the roots frames are named from (`sources.json`, ADR-145), retouching (`paint.sqlite`, ADR-148) and preferences. Handling errors file by file loses or hides user data:

- An unreadable `list.json` taken for an empty list is overwritten by the next edit.
- A transient I/O error (a drive unmounted, a cloud file not downloaded) taken for damaged paint sets the retouching aside and loses it.
- An older build that moves a newer build's files aside or writes over them destroys the newer build's state, after a downgrade or with a Library shared by two computers; one that reads part of the Library and writes beside what it can't read leaves the newer build a mix of the two.

## Decision

`crates/deptheos_app/src/fsutil.rs` is the one persistence layer, and its module doc states the rule.

- **The Library has one format: the schema of its `sources.json`** (`SOURCES_SCHEMA`), checked once, as the Library opens (`Sources::open`). Any change to the format of any Library file (a field added or taken away, an enum's value, what a field means) bumps it, with each file's own schema if its reader changes. An upgrade is one way: the first build of the new format moves every file to it, then writes the schema. Every later build opens a Library any earlier one made (the `library-<version>` fixtures).
- **A Library of a newer format is refused whole and left untouched** (`StoreError::NewerLibrary`), before anything is written but `library.lock`, the file the open's lock is held on (`History::open_in` reads `sources.json` right after taking it, and makes `Renders/` and writes `library-owner.json` only once it passes), saying which version last used it: every build writes its version into `sources.json` (`written_by`, `Sources::stamp`) as it opens a Library ("last used by Deptheos 0.6.0").
- **Each file has its schema** (`RENDER_SCHEMA`, `LIST_SCHEMA`, `STACK_SCHEMA`, `FAILURE_SCHEMA`, `SOURCES_SCHEMA`; paint's is its SQLite `user_version`, ADR-148), read and edited through `read_versioned` and `edit_versioned` (the `Versioned` trait). Readers ignore unknown fields. A file of this build's schema holds only the fields this build knows, so `edit_versioned` reads it as its type, changes it and writes it back (a render's rename and star).
- **Atomic replacing writes.** Every write goes through `deptheos_core::io::write_replacing`: a sibling temporary file, synced (`F_FULLFSYNC` on Apple systems; a plain `fsync` where the file system refuses that: smbfs, exFAT, NFS; any other failure, a volume gone mid-save among them, is an I/O error), replaced atomically (`ReplaceFileW` on Windows, retried against antivirus and indexer locks, keeping permissions, symlinks and extended attributes), then the parent folder synced. A render's folder is assembled under `<id>.partial` and renamed into place (ADR-140).
- **Inside a Library that passed, a file of another schema is damaged**, like one that doesn't parse. No file is kept aside for a newer build one by one.
- **Damaged files.** A damaged render or failed stack is skipped and left where it is. A damaged `list.json` is logged and replaced by the next write: the working version on screen is the app's to remake from the renders. Damaged `Stacks/<id>.json`, paint and preferences are set aside beside themselves as `<stem>.unreadable-<id>.<ext>` (`fsutil::set_aside`) by the write that replaces them, since a stack's name and kept working version are in no render; a damaged stack file restored from Recently Deleted is set aside in `Stacks/` the same way, and Delete Library removes what is set aside there.
- **A file that couldn't be read is never taken for damaged.** Paint is set aside only when it is structurally damaged; a transient error (permission denied, timeout, a disconnected drive) is `Unread::Unreachable` and leaves `paint.sqlite` as it is, to load next time. Paint that can't be read is never taken for no paint: `load_doc` answers `Err(Unread)` apart from `Ok(None)`, so an export of the render fails with that reason (`deptheos_app::Error::PaintUnread`) rather than leave the retouching out, and a render shown outside Retouch says it is shown without it (ADR-148).
- **A damaged `sources.json` doesn't lock the Library out.** Every write of it writes the same roots to `sources.previous.json` after it. A damaged `sources.json` is set aside and the Library opens from the copy, written back as `sources.json`. The app's writes replace the file whole, so damage comes from outside (a disk, a tool) and reaches one file: the copy holds the last write or, after a crash between the two, the one before, which named every root but the newest. Roots can't be rebuilt from the renders, which name only a root's id, so a render under a root the copy lacks is skipped and left on disk. With no copy that reads, the Library is refused.
- **Checked-in fixtures for every format** (`crates/deptheos_app/tests/fixtures/`): `render-v1` with its stack file, `list-v1`, `failure-v1`, `paint-v1`, and whole Libraries: `library-0.2.0`, which every later build must open; `library-newer`, the same Library of a newer format, refused untouched; `library-damaged` (a render's file cut short, a stack's file not JSON) and `library-unknown-value` (a render's method and the list's order with values no build wrote), each handled as damage. `crates/deptheos_app/tests/integration/formats.rs` reads them through the real readers, so a format changed without its upgrade fails at once.
- **Preferences (`preferences.json`) have no schema number**: every field has a `serde` default and unknown keys are ignored; a file that fails to parse is set aside. Stored enum keys (`libraw`, `adobe_dng`, …) and iOS `@AppStorage` values never change when their labels do.

## Alternatives considered

### Ad hoc readers that move newer files aside

Moving a newer file to `.unreadable-*` hid it from the newer build too.

### Leaving each newer file in place for its build

Each file then needs a rule of its own (renders counted and hidden, a stack refused a number, paint refused in Retouch, a list not saved), and an older build still writes the files it can read beside the ones it can't, so the newer build comes back to a mix. One check, as the Library opens, keeps an older build from touching anything.

## Consequences

- `render.json` is written through mirror types in `crates/deptheos_app/src/library/render_file.rs`, not by deriving serde on the engine's own structs, so a refactor doesn't change the format. The app's leaf enums (`FrameOrder`, `RawBackend`, `Source`, `Split`) derive serde with explicit spellings, so each has one encoding in every file.
- Renaming, adding, removing or changing the meaning of a stored field bumps its schema and `SOURCES_SCHEMA`, adds its upgrade in `fsutil.rs` (beside `Versioned`) and adds a fixture.
- A key renamed in the desktop's `preferences.json` resets that preference for every desktop user (ADR-150).

## Code

- `crates/deptheos_app/src/fsutil.rs` (the rule), `crates/deptheos_app/src/library/render_file.rs`, `crates/deptheos_app/src/library/stacks.rs`, `crates/deptheos_app/src/paint/store.rs`, `crates/deptheos_app/src/frames/sources/mod.rs`.
- `crates/deptheos_core/src/io/replace.rs`.
- `crates/deptheos_app/tests/integration/formats.rs`, `crates/deptheos_app/tests/fixtures/`.
