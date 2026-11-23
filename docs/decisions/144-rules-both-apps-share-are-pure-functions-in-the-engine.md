# ADR-144: The rules both apps share are pure functions and reducers in the engine: the batch run, the Library's tiles, render thumbnails and adding frames

## Status

Accepted.

## Context

ADR-101 puts what every front end shares in `deptheos_app`. Four rules every front end needs (the batch run, the Library's tiles, render thumbnails and adding frames) are easy to write once per front end, and copies written apart disagree: on canceling and waiting for exports, on counts and order, on when a failed read is tried again, on an add that lands after the user has moved on. Each disagreement is a bug one app has and the other doesn't.

## Decision

Each rule is one pure function or reducer in `deptheos_app`, with table tests, run on the UI thread, holding no I/O, no pixels and no time of its own. Its sentences are the engine's messages in `deptheos_app.ftl`. Each front end feeds it and carries out what it says; what stays per app is listed with it.

### The batch run: `deptheos_app::runs::batch::Machine`

- **Events in, actions out, no I/O**, each stack tracked by its index (`crates/deptheos_app/src/runs/batch/machine.rs`). The desktop (`app/batch_run.rs`), the CLI's `batch` and `sequence turntable`, and iOS (through `BatchQueue`, `crates/deptheos_ffi/src/batch.rs`) all drive it, and fast GPU-free transition tables test it. Each front end translates its callbacks into the machine's events and carries out the actions returned.
- **A render made is reported only through `Machine::stacked(paths, shape)`**, which returns a `Stacked`: `save: Option<Save>` (the stack to keep it in the history as, and the files it records) and `then: Vec<Action>`, the rest. A save can only be of the stack just reported, so a front end saves what it holds, and nothing pairs a number with a stack.
- **The files a render records are the machine's to say**: its job's frames, or a restack's render's list. `Machine::listed` is private, read inside `stacked` before the run moves on, so a save never takes them from the `Stack` action (whose `listed` the desktop stacks with); iOS's `BatchAction::Stack` carries no list.
- **iOS's `BatchQueue::stacked` returns a `BatchStacked`** (`save: BatchSave?`, `then`), as the machine's.
- **Kept per front end:** when the save runs (the desktop's lands later, iOS's is waited for before the next stack), the desktop's sentence for a render whose thumbnail couldn't be made, and the CLI, which keeps no history and gets no save.

### The Library's tiles: `deptheos_app::library::tiles`

- **One pure function** takes the history as borrowed entries (each render's id, stack, its stack's name as shown, its own name, star, time, local day and batch; each failure's; the stacks not stacked yet, the one on screen among them), the filter, the search and today. It returns `Tiles`: the unstacked stacks shown, the days with their runs (loose stacks side by side, or a batch with when it started and how many failed) and their tiles (renders with the cover, those let through and the total, or a failure), Jump To's four spans (Today, Yesterday, the seven days before, Earlier) with their counts and first day, and the whole Library's counts. It is built on `history::strip` (a tile per stack at its newest entry, each batch whole under the day it started) and called each time the Library is drawn. It keeps the rules both apps already shared: what All, Starred and Failed let through, a failure hidden once its stack has a render, a search by the stack's name, the render's own and the batch's label, and the stacks not stacked yet only under All. The desktop calls it directly; iOS through `library_tiles` with light records (`LibraryRender`, `LibraryFailure`, no frame lists), keeping the answer until its inputs change (`AppModel.libraryMemo`).
- **Its words**: the count lines (`Counts::line`), Jump To's names (`Span::label`) and the badge (`Tile::badge`). The apps keep their own filter names, headings, day titles and empty states.
- **The count line follows the filter** ("5 starred renders in 3 stacks", "2 failed stacks"): it says what the user is looking at. The whole Library's count is All's line, and each filter row carries its count.
- **The badge says "2 of 4" when a filter or search hides some of a stack's renders.** A total alone hides that the view leaves renders out.
- **The stacks not stacked yet are by name, the unnamed last, the one on screen among them**, so the order doesn't change as a stack comes on or goes off screen, and an untitled stack sorts by having no name, not by the word "Untitled".
- **Delete Failed Stacks… deletes the failed stacks shown** ("Delete 3 Failed Stacks…" while a search hides some): a command on a filtered view acts on what is in view.
- **A stack is searched by the name its tile shows**, "Untitled stack" included: what can be read can be found.
- **A stack is one tile.** A failed stack with a working version kept is an unstacked stack under All, and a failure under Failed until it has a render. The desktop's Jump To counts the row of unstacked stacks.
- **Kept per app:** the desktop's Jump To lists all four spans and dims an empty one (a sidebar that doesn't reflow); iOS lists only the spans with stacks.

### Render thumbnails: `deptheos_app::library::thumbnails::Thumbnails`

- **One pure reducer, pulled:** `on(event) -> actions`, nothing it does blocks. Its events carry the time they happened (`Instant`), so tests pass plain instants: `Wanted { ids, now }` (what is in view now, the most wanted first, replacing what was wanted: the desktop says so each frame, iOS as rows appear and disappear, and the desktop at launch the newest); `Read { token, id, ok, now }` and `Finished { token, now }`, counted only while `token` is the read in flight; `Saved`, `Deleted`, `Shed`, `Cleared`. Its actions are `Read { token, ids }`, `Keep(id)` and `Drop(id)`. Each render's state is a `Thumb` (reading with its tries, shown, failed with its tries and when), its transitions in the enum's doc comment, each logged at debug.
- **The pixels stay the app's.** Each app keeps its images in a map by render (the desktop's `RenderImage`s, iOS's `CGImage`s) that only `Keep` adds to and `Drop` takes from. iOS reaches the reducer through the bridge's `ThumbnailPolicy`, whose calls read the clock and return the actions.
- **A failed read is tried again only when next wanted, `RETRY_AFTER` (5 s) after it failed, `TRIES` (3) times in all.** A drive asleep gets another go; a file that isn't there isn't read on every scroll.
- **One read at a time, of up to `PER_READ` (24), the latest wanted first**; when it lands, the next reads what is wanted then. A fast scroll doesn't queue reads for rows long gone, and the background lane isn't flooded.
- **Past the most kept, the one wanted longest ago goes, never one wanted now**, so a thumbnail on screen is never dropped and read again.
- **Shedding keeps exactly what is in view**, not a count. iOS sheds on a memory warning and on going to the background; the desktop doesn't shed.
- **Kept per app:** the most kept, which is what the app's pixels cost (400 on the desktop, `THUMBS_KEPT`; 96 on iOS, `thumbnailsKept`); the desktop shows a saved render's thumbnail from its save and reads the newest at launch, iOS reads one when its row shows.

### Adding frames: `deptheos_app::frames::list::add::plan`

- **One pure function, run on the UI thread when a listing lands.** It takes what was listed (`Expanded`: the images, and the folders that couldn't be read), the stack it was listed for, the stack on screen (`OnScreen`: its id, whether a stack, batch or sequence runs, its list, whether it has renders) and where the images came from (`AddedFrom`: the window, or the system), and returns a `Plan`: its `Outcome` (refused, with why; nothing new; or the files to add and whether to offer New Stack Instead) and the folders that couldn't be read. It is derived each time, never stored. Its sentences are the `add-*` messages: the refusals, "No frames were added…", "Added 12 frames to canon1." and the lead naming a folder that couldn't be read, which each app follows with why.
- **A folder that can't be read is always said**, naming it, whatever else came of the add. A folder that passes for empty sends the user looking for frames that aren't there.
- **Images listed while another stack came on screen, or while a stack started, are not added, and that is said.** Frames go only into the working version of the stack they were added to, never into a list being stacked, and an add the user asked for is never dropped without a word.
- **New Stack Instead** is offered when the list had frames, or the system handed the images over into a stack with renders. iOS takes no files from outside the app, so there it comes to the first.
- **A listing is added to the list as it is when it lands**: changes made meanwhile stay, and nothing is read from disk twice. iOS lists in name order only, so adding listed files reads nothing more and is done on the main thread; the bridge has an async `list_expand` and the plan's `add_plan`.
- **Kept per app:** what each says before the add (the desktop refuses files handed over while something runs; iOS holds the stack on screen while files picked in Files are planned and copied, `Adding`), the name the notice gives the stack, and how the notice and its New Stack Instead are shown.

## Alternatives considered

### Keep `Action::Save`, carrying the files recorded

The files would be the machine's, but each front end would still pair a number with what it holds and need a sentence for a mismatch that can't happen.

### Hold the stacks in the batch machine

The machine would pair them itself, but it would hold pixels, which it is made never to do, and each front end's stack is a different type.

### A stored Library model, updated as renders are saved and deleted

Each change would have to reach it in order, and a missed one would show a stale tile until restart. The history is already in memory on both apps; deriving the tiles is a pass over it.

### Share only the easy half (the counts; the retry rule; New Stack Instead)

The copies are where the differences and the bugs came from. Sharing the half both apps already wrote alike would leave the half that disagrees.

### The engine reads its own history for iOS's tiles

The bridge could list the renders itself with nothing sent across. But iOS changes its history in place as a star or delete lands, before the engine's list is read again, and a view's synchronous call must not wait on the engine's threads (ADR-143). The light records cost a copy of a few fields a render, made once per change.

### A thumbnail cache that holds the pixels

One cache for both apps, but the pixels would cross the bridge as one type, and the desktop draws `RenderImage`s that must leave GPUI's atlas through `drop_image`. A policy that only names what to keep leaves each app's images as its drawing needs them.

### An observer the thumbnail policy calls when a read lands

Reads finish off the UI thread, but each app already brings the result back to it; a pulled reducer needs no observer and no lock beyond the bridge's.

### Plan inside the bridge's listing call

One call would list, check and add, but the checks need the stack on screen when the listing lands, which only the app knows after its `await`; checked before it, they would be stale.

## Consequences

- A rule changes in one place, with its table tests, and every front end follows. A front end can't save one stack's render under another's number, and the machine's tests check what each save records, a restack's included.
- The desktop's title bar count reads "5 starred renders in 3 stacks" under Starred; iOS's badge reads "1 of 3" there.
- iOS sends each render's few Library fields across on every change of the history or the search, not on every redraw, and reads thumbnails in batches of up to 24, so the first screen fills in the order its rows appeared.
- A render whose thumbnail can't be read shows its placeholder after three tries until the app is opened again. The desktop's tests age failures by moving its clock (`age_failures` in the desktop's `renders/thumbnails.rs`).

## Code

- The batch run: `crates/deptheos_app/src/runs/batch/machine.rs`; desktop `crates/deptheos_gpui/src/app/batch_run.rs`; CLI `crates/deptheos_cli/src/drive.rs`, `crates/deptheos_cli/src/batch.rs`, `crates/deptheos_cli/src/sequence/turntable.rs`; iOS `crates/deptheos_ffi/src/batch.rs`, `ios/DeptheosMobile/Model/AppModel+Batch.swift`.
- The Library's tiles: `crates/deptheos_app/src/library.rs`, `library/tests.rs`, `library/history/strip.rs`; desktop `crates/deptheos_gpui/src/renders/library_layout.rs`, `renders/library.rs`; iOS `crates/deptheos_ffi/src/library.rs`, `ios/DeptheosMobile/Model/AppModel+Library.swift`, `Views/LibraryView.swift`.
- Render thumbnails: `crates/deptheos_app/src/library/thumbnails.rs`, `library/thumbnails/tests.rs`; desktop `crates/deptheos_gpui/src/renders/thumbnails.rs`, `renders/mod.rs`; iOS `crates/deptheos_ffi/src/library.rs`, `ios/DeptheosMobile/Model/HistoryModel.swift`, `Views/LibraryTiles.swift`.
- Adding frames: `crates/deptheos_app/src/frames/list/add.rs`, `list/add/tests.rs`; desktop `crates/deptheos_gpui/src/app/list.rs`; iOS `crates/deptheos_ffi/src/list.rs`, `ios/DeptheosMobile/Model/AppModel+Adding.swift`.
- The messages: `crates/deptheos_app/i18n/en-US/deptheos_app.ftl`.
