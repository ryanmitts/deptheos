# ADR-146: A folder is read as shots in name order; capture time only splits it into stacks; the frame list's thumbnails come from embedded previews and stay in memory

## Status

Accepted.

## Context

A folder from a memory card or a field shoot often holds many brackets, often with both a camera RAW and a JPEG of each shot, and adding one can add hundreds of 45 MP files at once.

Plain lexicographic sorting puts `frame-100` before `frame-95`. Sorting by EXIF capture time by default fails on cameras that record only whole seconds (OM System bodies among them) and on the occasional mis-ordered burst timestamp. And splitting a folder of several subjects into stacks by a fixed gap (2 seconds, say) breaks rail, microscope and flash-recycled brackets, whose frames are 2 to 10 seconds apart.

The Frames list shows a thumbnail beside every frame. Decoding a RAW file's sensor data takes 250 ms to 2 s even at half size, and a 45 MP JPEG about 130 ms, which would leave a card's thumbnails blank for minutes.

## Decision

### Shots and their order

- **A shot pairs the same-stem RAW and JPEG files of one folder** (`crates/deptheos_app/src/frames/folder.rs`): the two appear as one row in the Frames list. Which file is stacked is a setting of the list and of a batch, `Source::Raw` by default. Pairing is by extension, so it works the same on iOS without LibRaw. Hidden dotfiles, macOS's `._` AppleDouble files on exFAT and SMB volumes among them, are ignored.
- **Frames are in natural name order by default** (`names.rs`, `order.rs`): by folder, then by file name with numbers compared as numbers (`95` before `100`), so a bracket across `100CANON` and `101CANON` stays in shooting order. The app never leaves name order by itself.
- **Capture-time order is chosen, never assumed** (`FrameOrder::CaptureTime`): in the Sort menu, the Batch dialog and `--order time`, and recorded in `render.json`. It is for capture tools whose file names wrap (a camera's buffer handles, say). A frame's time is `DateTimeOriginal` with `SubSecTimeOriginal`; a file with no original time (or a blank one) gives `DateTimeDigitized` with `SubSecTimeDigitized` from the same IFD. `DateTime` is never used: it is the time the file was last changed, rewritten on edit or export.
- **A stack falls back to name order, with a notice, when its times can't order it unambiguously:** a missing time, mixed camera models, whole-second times with more than one frame in some second, or, when other frames record sub-seconds, a frame without them sharing its second with another (a frame without sub-seconds alone in its second is placed by the second).
- **Ties are a total order.** Frames of the same time form a group, ordered by the camera's burst shot number when every frame of the group has one, otherwise by file name alone. Mixing the two in one group (shot number where both frames have one, name otherwise) can contradict itself across three frames, so it is never done.
- **Name order warns only when it is wrong for the stacker.** In name order, a stack whose capture order differs from its names gets the "shot in a different order than their names" notice with a Sort by Capture Time button. A capture order that is the name order exactly reversed (a bracket shot far to near, any two-frame stack shot backwards among them) isn't warned about, since the stacker's reverse check (ADR-112) aligns from the most magnified frame either way; Sort by Capture Time still reverses such a list when picked.

### Splitting a folder into stacks

- **Auto sort splits by the local pace of capture** (`autosort.rs`, `SplitRule`): shots in capture-time order are cut where the gap between two exceeds **both** 2 seconds and 4× the median of up to 5 gaps on each side, or where the camera model changes. Runs of fewer than 5 shots are left out (a batch counts them as skipped). Each shot's time is read from the file of the pair that will be stacked (`Shot::file(source)`), the file the frame order reads. A file with no EXIF capture time falls back to its modification time.
- **A small header reader of our own** (`capture.rs`) reads only the 4 KB blocks the container headers and TIFF and EXIF IFDs point to, at most 512 KB a file, across TIFF, DNG, ORF, RW2, CR3, JPEG, RAF and HEIC: about 16 KB read per file. Its results are checked against ExifTool's.

### The frame list's thumbnails

- **The smallest embedded preview that fits**, full pixels decoded only as a fallback (`crates/deptheos_app/src/frames/source_thumbnail.rs`):
  - **RAW and DNG:** LibRaw lists every embedded preview (DNG preview IFDs included), and only the smallest at least the target size with the frame's aspect ratio is read (a letterboxed 4:3 thumbnail inside a 3:2 CR3 comes last). On folders of 60 files that is about 1 ms a DNG, 2 ms a CR3 and 28 ms an ORF. With no embedded preview, a half-size decode by LibRaw, or by the DNG SDK's CPU renderer (`sdk_render`, ADR-130) for lossy JPEG XL DNGs LibRaw can't read. Adobe DNG Converter is never run for a thumbnail.
  - **JPEG** (`source_thumbnail/exif.rs`): the EXIF IFD1 thumbnail, read from the first 1 MB with `kamadak-exif`, and the main image's size from its header, without decoding the main scan (a 45 MP JPEG's thumbnail goes from about 130 ms to under 1 ms). With no thumbnail of the right aspect, the JPEG is decoded reduced through the vendored libjpeg, at the smallest eighth of its size that still fits (`deptheos_dng::jpeg::decode_scaled`; at 45 MP, 140 MB and 77 ms become 6 MB and 27 ms). PNG and TIFF, and a JPEG libjpeg can't read, are decoded whole.
- **Orientation and color go through the frame's own input stage** (ADR-132): the EXIF thumbnail is read with its image's header (color tag and orientation), so it is turned once, by the frame's rule, and shown converted to sRGB from the frame's own space.
- **In memory only.** The desktop reads them on the pool's background lane (`crates/deptheos_gpui/src/thumbs.rs`, ADR-143) and keeps them while their files are in the list; no thumbnail cache is written to disk.
- **iOS** reads them with ImageIO (`ImagePreview` in `ios/DeptheosMobile/Engine/EngineBridge.swift`: a RAW's stored preview when it is large enough and shows the whole image, else ImageIO's decode) through `Blocking`'s interactive queue, three at a time, and never of a frame in iCloud not downloaded, so listing a folder never starts a download (`loadThumbnails` in `Model/AppModel+Files.swift`).
- The Library's tiles use the render's own `thumb.jpg` (ADR-140), not these.

## Alternatives considered

### Capture-time order by default

Cameras without sub-second times, or with clock jitter, can mis-order neighboring frames of a fast burst, which costs a stack detail. Natural name order is reliable for cameras' own numbering, with capture time there when asked for.

### A fixed gap to split stacks

A 2-second threshold cuts rail, microscope and flash brackets shot 2 to 10 seconds apart; a gap measured against the pace around it doesn't.

### An existing Rust EXIF crate for capture times

On a 250 MB TIFF, `kamadak-exif` 0.6 read the whole file (814 ms) and didn't support CR3 or ORF containers; `nom-exif` 3.8 failed on DNG and TIFF and rejected ORF. The IFD reader reads about 16 KB a file.

### A disk cache for the list's thumbnails

Embedded previews take 1 to 2 ms a file, so reading them again takes well under a second for a typical bracket, with no cache to invalidate or clean up.

## Consequences

- A new RAW container needs its header and IFD rules in `capture.rs`, checked against ExifTool with the `capture_check` example.
- An embedded preview is the camera's own JPEG rendering, so a frame's thumbnail can differ slightly in tone or color from the viewer's DNG rendering.

## Code

- `crates/deptheos_app/src/frames/folder.rs`, `names.rs`, `order.rs`, `autosort.rs`, `capture.rs`.
- `crates/deptheos_app/src/frames/source_thumbnail.rs`, `crates/deptheos_app/src/frames/source_thumbnail/exif.rs`.
- `crates/deptheos_gpui/src/thumbs.rs`.
- `ios/DeptheosMobile/Engine/EngineBridge.swift`, `ios/DeptheosMobile/Engine/Blocking.swift`, `ios/DeptheosMobile/Model/AppModel+Files.swift`.
