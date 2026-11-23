# ADR-145: A frame is a root and a path relative to it, the roots one Library file, so a relink rewrites one entry; iOS reads folders in place through leased scopes and copies only where a reference would break

## Status

Accepted.

## Context

A focus stack is dozens to hundreds of full-size frames, often 25 to 100 MB each for RAW and DNG, and the app reads them again later: to stack again, to paint from them, to Revert, to run a batch. Frames come from anywhere: the computer's disks, external drives and cards, network shares, cloud folders, and on iOS the app's own container (the camera, Photos imports, copies) or folders chosen in Files, reachable only through security-scoped bookmarks.

A folder moves, a drive is mounted elsewhere, a share is mounted at a new path each time, a copy is made into the app. If every Library file that names a frame (`render.json`, failed runs, the kept working versions, `list.json`, the paint store) held its absolute path, each of these would rewrite every one of those files: a crash could half-relink, a reconnect would write the whole Library, and every open thing (the stack on screen, Retouch) would have to be patched by hand.

On iOS, opening every bookmark at launch hits the per-process limit on open security scopes (Apple doesn't document it; it is a few hundred on an iPhone), after which every later one fails until the app restarts. And a bookmark to another volume (an external drive, an SMB share) resolves at first and stops resolving once that volume has been unmounted, so finding a drive's folder again is the normal case, not the exception.

## Decision

### Roots

- **`sources.json`**, one Library file written atomically (ADR-141) and owned by `deptheos_app::frames::sources` (`Sources`), lists the roots. A root has an id (`RootId`) and is one of:
  - **on a volume:** the volume (its id where it has one, its kind, its name; a share by server and share name, never by mount point or drive letter), the root's path within the volume, the mount path it was last seen at, and its bookmark (iOS always; the desktop on macOS too, which follows a rename);
  - **inside the Library folder or the app's storage** (Imports, the camera's captures, the samples, any folder the user keeps under the Library), stored relative to it, since iOS moves the app's container between installs and a user can move the Library;
  - **inside another root**, stored as that root and a relative path, with no bookmark of its own: its parent's covers it, as one security scope covers a folder and everything under it. A root that becomes nested drops its bookmark; one split off to another place gets a new one from the folder picked. There is one bookmark per root added at the top, never one per subfolder. A root may also hold the frames named from another root under a subfolder split off from it (`split_from`).
- **A frame is a `FrameRef`: root, relative path, size and fingerprint**, in every file. The relative path is stored exactly as the file system listed it. The fingerprint is a hash of the file's first and last 64 KiB, read when it is added. Size and fingerprint are evidence for matching, never a gate on reading.
- **Identity** is a frame's path from its deepest covering root, compared exactly as stored. **Matching** (Locate…, adding a frame already in the Library, opening a path whose exact form fails) compares a key: Unicode NFC, case folded (`key`). So identity doesn't change when a root moves to a volume with other case rules, and a file added again under another case or normalization finds the one there.
- **Canonical paths, for roots only.** A root's path is resolved once (symlinks, `/private/var` on iOS, Windows' `\\?\` prefix stripped). A frame's relative path is not resolved: a link inside a root stays inside it. A root added is also compared by file-system identity (volume and file id), so one folder reached by two paths (a share mounted twice, a mapped drive and its server path) is one root.
- **A root is the folder its frames were added from.** A frame added alone makes its folder a root. A root added inside another becomes a root inside it; one that contains roots takes them in. After roots are resolved, any root found inside another is nested again, so two roots never cover one folder. Each change writes only `sources.json`.
- **A frame's path is its deepest root's current path plus the rest**, worked out when it is read (`Sources::resolve`), never stored.
- **`sources.json` is read before anything else in the Library**, and holds the Library's format (ADR-141).

### Finding a root again, and relinking

- **In order:** its bookmark, unless it resolves into a trash folder; its volume by id; by name and then by last path, only as candidates. A candidate is taken silently only when a sample of the root's frames matches by name, size and fingerprint; otherwise it is neither taken nor offered, and the root stays missing or not connected until Locate…. A volume whose id is known and differs is never the root's, whatever its name: cards from one camera share their name, their `DCIM` layout and their file names, and a card's serial is 32 bits and changes when it is formatted, so a name alone proves nothing.
- **Roots are grouped by volume.** When one root is found or located at a new mount path, the volume's new mount point follows from it and every other root on that volume is tried there. On iOS, picking a share's or drive's top folder brings back every root on it. A stale bookmark that resolved is made again from the URL it resolved to, and that root's entry is written; nothing else is.
- **A relink rewrites one entry of `sources.json`.** Locate…, a folder found moved and Copy into Deptheos all point a root elsewhere; a root moves whole. A subfolder moved out of its root becomes a root of its own at its new place. A file moved within its root is a per-frame Locate….
- **Locate…** matches the frames under the folder picked by name (iCloud placeholders read as the file they stand for), size and fingerprint, says "found N of M", and looks for the root's other frames under the same relative layout.
- **Copy into Deptheos is all or nothing.** The frames are copied into a staging folder and checked by size and fingerprint; only then does one write point the root at the copy. A frame that can't be copied (in iCloud, not connected) leaves the root where it was, and the frames are named.
- **What a frame resolves to:** there; **changed** (there, but its size or fingerprint differs: it is used, and the renders made from it may not match it); **missing** (its volume is there but the file or folder isn't, or it resolved into a trash folder: Locate…); **not connected** (its volume isn't there). Nothing is pruned or deleted for a missing or not connected frame. A root goes when no file names it, and only once the whole Library has been read.
- **The front ends make and resolve bookmarks; the engine keeps them as bytes.** Swift on iOS with default options, the macOS desktop with plain bookmarks, off the main thread within a timeout, never mounting or prompting while resolving for status; each hands back the path it resolved to (`Sources::find_again`). `deptheos_core` keeps taking paths.
- **Open things hold resolved paths** (a stack read, Retouch) and are opened again after a relink, through one entry point per app: the desktop's `AppView::relinked` (`crates/deptheos_gpui/src/app/list.rs`), reached by `RendersEvent::Relinked` from the store queue's `SaveRequest::Relink` and `FindAgain`; iOS's `AppModel.relinked` (`Model/AppModel+Files.swift`), whichever made the relink. A relink changes no paint (ADR-148).

### iOS: scopes on demand, in place or copied

- **Scopes are opened on demand, held by leases.** `await access(frames) -> Lease` (`AppModel.access`) opens the scopes of the frames' roots off the main actor and holds them until the `Lease` is dropped, stopping only a scope it started. Every reader takes one: stacking, batch planning, copying in, Retouch, export and drag-export. A stack's and an export's is asked under their lasting request (ADR-143), so one going on in the background holds its scopes; a stack's lives in its `StackingState` until it is saved or ends. A scope stays open after its lease so going back to a stack costs nothing; above `Bookmarks.openLimit` (32) open scopes, the least recently used that no lease holds is closed. Resolving a lease's bookmarks may take `resolveWithin` (5 s); one not back by then is given up, so a share that stopped answering doesn't hold up the read.
- **A bookmark is made only of a `Picked`**, the URL a picker in Files handed over, since only that URL carries the scope, and kept with its root through `Engine::keep_folder` or `Engine::locate`.
- **A folder is used where it is; frames picked one by one are copied** into `Documents/Imports`, since reading them in place would take an open scope per frame while a stack runs, **unless Deptheos already reads their folder**, in which case they are used where they are (`AddPlan.covered`, `AppModel.readable`), so a frame added back to a stack from its own folder doesn't leave the stack's frames in two places.
- **Add Folder… is its own command beside Add from Files…** (`FramePicker` in `Views/SourcesSidebar.swift`): Add from Files… picks any number of images; Add Folder… picks one folder, since the frames go into the stack on screen (several folders as several stacks is New Batch). One picker for images and folders together only opens a folder that is tapped and doesn't offer to pick it, so a folder's frames could only be added by selecting them all, which copies them.
- **What a folder picked in Files does depends on where it is** (`FileSource` in `Model/FileSource.swift`, `AddPlan.asks`):
  - in the app's own Documents (`owned`, such as Imports and Captures): added where it is, without asking;
  - on the device (`device`: On My iPhone, another app's folder on it): used in place, without asking;
  - off the device: the Add Frames sheet asks first, its Copy into Deptheos switch on by default for iCloud Drive (`iCloud`), network shares (`network`) and other providers (`provider`, also whatever the system's description of the volume can't place), since two-pass stacking reads every frame twice, which a download on demand or a network suits badly; off by default for USB drives and SD cards (`removable`).
  - The switch covers every folder in the sheet (on when any of them defaults to on), and Add remembers it for each folder's kind of place (`FileSource.defaultsKey` in `UserDefaults`), the next default for that kind.
- **Photos and the camera copy by nature:** Add from Photos copies the originals into a folder of their own in `Documents/Imports` (`AppModel.addPhotos`); a camera burst is saved in `Documents/Captures`. Both are then read where they are. New Batch's folders are kept and read in place, never offered a copy.
- **Ten or more single frames from one folder are said to be copied before they are** (`manyLoose`), in the Add Frames sheet, with **Use Them Where They Are…**: Files opens at their folder, and once it is picked the same frames are added in place through its bookmark (`AppModel.useFolder`). The sheet can't know whether they are the whole folder (only the picked files can be read), so the test is the count. Fewer are copied without asking, since on the device a copy takes no space, unless the single frames from off the device come to more than 1 GiB (`AddPlan.largeCopy`), which the sheet also says first.
- **Before a stack starts**, every included frame is checked; an iCloud or provider file not downloaded is downloaded through a coordinated read, and a frame that can't be had stops the stack with a message saying why (frames that can't be reached, naming the drive or server to connect, or Locate…; else the system's reason a file can't be read), never silently left out (`AppModel.preflight`).
- **Storage:** copies in `Documents/Imports` are excluded from device backups; unused copies can be deleted in Settings ▸ Storage; an in-place stack's folders can be copied in later with Copy Frames into Deptheos, in the menu of the Frames head's badge that names where the frames are ("On T7 SSD"), or in the Frames list's commands when the head names no place (ADR-163).
- **Settings ▸ Storage ▸ Folders lists the folders read in place** (`KeptFolder`, `Views/FoldersView.swift`): each with what it is on, Not Connected when its bookmark doesn't open, and how many stacks use it, with Copy into Deptheos, or Locate… while it is away. There is no Remove: a folder leaves the list when the last stack or render using it is deleted, and removing one by hand would only break those stacks.

## Alternatives considered

### Absolute paths, rewriting every file on a relink

Each relink touches every file naming the folder, can stop halfway, and must reach open state by hand. A journal of relinks under way would make it crash-safe but leave the rest.

### Frames named by a hash of their whole content

A move would need no relink, but adding a stack would read every frame in full first, gigabytes for a RAW stack. A hash of each file's ends costs 128 KiB a frame and tells two cards' `IMG_0001` apart, which is what matching needs. It hashes bytes, not metadata, so two files with the same tags don't collide.

### A size or fingerprint change makes a frame missing

Tools that write tags into a DNG, TIFF or JPEG rewrite it in place, so a stack would show its frames missing after an ordinary edit elsewhere. Changed is shown, and the frame is read.

### Case-folded identity everywhere

One rule for every volume, but two files differing only by case on a case-sensitive volume would be one frame, and one would be hidden. Identity is exact; only matching folds.

### Copy every folder into the app on iOS, or work inside one folder

Copying doubles storage (gigabytes per RAW stack) and makes batch-stacking from an external SSD or a local folder impractical. One working folder granted once would make users move their shoots under it, need a file browser inside the app, and still take a pick for each shoot; the few open scopes it buys the limit already gives, and a parent folder picked once (New Batch's Add Subfolders of…) already covers every folder in it with one bookmark.

## Consequences

- A Library without `sources.json` has no frames to show. Adding a frame reads 128 KiB of it for the fingerprint, over a share too.
- On iOS a drive or share comes back through one pick of its top folder after it has been unmounted, not by itself; the Folders list says so. Renders stay viewable and exportable from the Library while their frames are away.
- Windows has no bookmarks and no single persistent volume id: the format-time serial is stored, the volume's GUID path only as a hint on that computer. Linux uses the file system's UUID (the serial on FAT and exFAT).
- Use Them Where They Are… is offered by count and adds the frames picked, not the folder's others. It and frames kept in place rely on a picked file's path lying under its folder's path as the bookmark resolves it; where they differ, the frames are copied.

## Code

- `crates/deptheos_app/src/frames/sources/`, with its table tests; `crates/deptheos_app/src/library/history/frames.rs`.
- Desktop: `crates/deptheos_gpui/src/app/list.rs`, `crates/deptheos_gpui/src/bookmarks.rs`, `crates/deptheos_gpui/src/renders/queue.rs`.
- iOS: `crates/deptheos_ffi/src/roots.rs`; `ios/DeptheosMobile/Model/Bookmarks.swift`, `Model/AppModel+Files.swift`, `Model/AppModel+Adding.swift`, `Model/AppModel+Imports.swift`, `Model/AppModel+Folders.swift`, `Model/FileSource.swift`, `Model/AppFiles.swift`; `Views/SourcesSidebar.swift`, `Views/AddSheet.swift`, `Views/FoldersView.swift`, `Views/StorageSection.swift`.
