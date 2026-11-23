# ADR-170: iOS backs up the Library, all but Recently Deleted

## Status

Accepted.

## Context

iOS backs up an app's Application Support and Documents to iCloud or a computer unless a file or folder is marked `isExcludedFromBackup`. The Library is in Application Support (`AppFiles.historyDirectory` in `ios/DeptheosMobile/Model/AppFiles.swift`). Its renders are the user's only copy of each result and its retouching: nothing can make `result.jxl`, `frame_map.jxl` or `paint.sqlite` again without the frames, which may be gone. The Library's layout is in `crates/deptheos_app/src/library/history.rs` (ADR-140).

## Decision

- **The Library is backed up**: every render whole (`result.jxl`, `frame_map.jxl`, `thumb.jpg`, `render.json`, color metadata, `paint.sqlite`), the stacks, the failed batch stacks, the working version and `sources.json`.
- **Recently Deleted (`Deleted/`) is left out.** It holds what the user chose to delete, kept 30 days for Undo, and can be as large as the Library. The app marks the folder once the engine has opened the Library (`AppFiles.leaveRecentlyDeletedOutOfBackup`), so what a delete moves into it later is left out too, and a render restored from it is backed up again.
- **Thumbnails and frame maps are backed up.** Neither is made again when missing: the Library's tiles read `thumb.jpg` as it is, and the frame map comes only from the stack. A thumbnail is at most 512 px, small beside its result.
- **Frames are as ADR-145 has them**: copies of frames from Photos or Files are left out, as those apps keep the originals; a camera burst, the only copy, is backed up.

## Alternatives considered

### Leave the whole Library out

It would make backups small, but a restored device would lose every render and its retouching.

### Leave thumbnails out, marked as each is written

It saves little, needs a hook in the engine's render writer, and a restored Library would show blank tiles until something made thumbnails again, which nothing does.

## Consequences

- A restored device has the Library as it was, with Recently Deleted empty.
- A file added to the Library that can be made again from what is backed up should be left out of backups where it is written.

## Code

- `ios/DeptheosMobile/Model/AppFiles.swift`, `ios/DeptheosMobile/Model/AppModel.swift`.
- `crates/deptheos_app/src/library/history.rs`, `crates/deptheos_app/src/library/history/deleted.rs`.
