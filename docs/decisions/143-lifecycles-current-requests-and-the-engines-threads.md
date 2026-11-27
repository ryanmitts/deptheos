# ADR-143: Each lifecycle is one enum; a result applies only while its request is current; the engine owns every blocking thread, each request carries a cancel token from one tree, and quit finishes only what must

## Status

Accepted.

## Context

State tracked as separate booleans, `Option`s and counters across files allows combinations that mean nothing, and an async result that lands after the user moved on corrupts what is on screen (an open replacing the frame list under a stack just started, a batch finishing while its exports still write).

Blocking work has the same problem in the other direction. Swift's cooperative pool has a thread per core, doesn't grow when one blocks, and assumes every task makes progress, so a few blocking calls on it stall every `await` in the app, and blocked tasks waiting on another task can deadlock it; its threads, like GCD's and GPUI's background executor's, have 512 KB stacks against the 8 MiB the engine's decoders need, and neither lets a task ask for more. A thread started per request has no bound, and dropping its future drops only the wait: five large renders opened in a row would all load to the end. Codecs make thread teams of their own at the machine's width, outside any budget, and a codec that can't be stopped makes quitting during a conversion hang until it ends. Library writes on threads of their own land in either order (two quick renames), a hand-kept list of what quit waits for misses whatever is added later, and a file check on the UI thread stalls it on a share gone quiet.

## Decision

### State

1. **A lifecycle is one `enum`** (a discriminated union in TypeScript), owned by one module and changed only through its methods. Data that belongs to one state lives inside that variant, so a `match` that misses a state fails to build. Each lifecycle enum documents its transitions and logs each at debug. `CLAUDE.md` lists them; a new mode is a new variant there, never a flag.
2. **An async result applies only while its request is current.** The state holds the request in flight, and every completion checks, after every `await` and before changing anything, that the state still holds it. Web: `latest()` (`web/src/latest.ts`), which also aborts superseded work through `AbortSignal`. Desktop: a task lives inside the state it is for, so leaving the state drops it, which cancels its work (below); an open is the latest-wins `Open` (`crates/deptheos_gpui/src/renders/mod.rs`, changed in `renders/opening.rs`), whose load applies only while the state still holds its `generation`. Adjust: every request `adjust::Machine` asks for (the reduced copy, a preview, the full picture, a write) carries a `RequestId`, and its result counts only while the machine's state still holds it; `AdjustSession` frees a stale upload's GPU copies and drops a stale open's read or eyedropper sample (ADR-153). iOS: `Opening` (`beginOpening`, `expectLanding`, `endOpening` in `Model/AppModel+Opening.swift`) with `viewGeneration`, and the `LatestLoad`s (`Model/LatestLoad.swift`).
3. **Call order is enforced by consuming types** where callers already follow it: the sequence writer's `close(self)`, `ImageUploader::prepare*` returning a `PreparedUpload` that `dispatch_to_rgba` uses up, the CLI's `TempStore::keep(self) -> KeptStore`.

### The model of threads

> The UI thread owns the app's state and never blocks. The engine owns every blocking thread: the stack thread, the paint worker, one serial store thread for every Library write, and a bounded pool for decodes, previews, thumbnails, conversions and exports, in two lanes so background work never holds up what the user waits for. Every thread has an 8 MiB stack. Codecs' own threads come out of one CPU budget shared with the pool. Each app reaches the engine through one doorway. Every request carries the identity of the state it belongs to and a cancel token from one tree (quit, then a run, then its work); dropping the request cancels it. Every blocking wait, the codecs' included, watches its token, and results apply only while the identity still matches: cancel saves work, the identity check keeps results correct. Quit stops new work, cancels what can be abandoned, waits with a message for what must finish, then exits without waiting on anything stuck in a codec.

### The cancel token

- **`Cancel`, in `deptheos_core::cancel`**, our own: `Cancel::root()`, `child()`, `counted()`, `cancel()`, `check()`, `flag() -> &AtomicBool`, `on_cancel(f) -> Hook`, `guard() -> CancelOnDrop`. Canceling a token cancels its children; a child holds its parent only weakly, and a parent prunes children that have gone.
- **Canceling pushes down**, so each token is one `AtomicBool` and checking it is one relaxed load, cheap enough for an inner loop. `deptheos_raw` and `deptheos_dng` sit below `deptheos_core` and take a plain `&AtomicBool`, never our type.
- **`on_cancel` wakes what can't poll** (a condition variable's waiters, LibRaw's cancel flag, a converter's child process). The hook runs once, on the canceling thread; dropping the `Hook` removes it.
- **The tree.** The engine's root (`work::root`) is canceled only by quit. Under it, one token per run: a stack, a batch, a sequence, an export, a Retouch session, the Library's read. Under a run, its work: a decode, a conversion, a preview. A request that belongs to no run (a thumbnail, a render open) hangs from the root. Work quit must let finish (an export writing, a render's pictures encoding) takes a `must_finish` token, with no ancestor, which only dropping its request cancels; quit waits while one lives (`writing`).

### The engine's threads

- **Every thread the engine starts is made by one builder**, `deptheos_core::threads::thread` (re-exported by `deptheos_app::engine::work`), with an 8 MiB stack (`STACK_SIZE`) and a name; among them the stack thread (`stack`, one stack or batch stack at a time), the paint worker (`paint`, ADR-148), the store thread (`store`), `paint-timers`, the pool's workers (`work-0`…), the prefetch thread (`frame-prefetch`) and the conversion threads (`dng-convert`). rayon's global pool and the aligner's (`align-0`…) are given the same `STACK_SIZE`. The DNG host's threads get 8 MiB too (`pthread_attr_setstacksize` in `crates/deptheos_dng/native/dng_common.cpp`, since `std::thread` can't set it).
- **The store thread** (`deptheos_app::engine::store`: `store().write`, `keep`, `forget`, `busy`, `flush`) takes every Library write in the order sent: render saves, renames, picks, failures, deletes (each with its paint and Adjust closed on the pool first, since each close waits for this thread) and restores, relinks, `sources.json`, the kept working version, paint's commits, a render's adjustments (one write as each change ends, an overtaken one skipped, ADR-152 and ADR-153) and the desktop's preferences. `keep(key, f)` writes a key's newest value once it has stood `KEEP_AFTER` (250 ms): the desktop's preferences (`prefs.rs`) and its window layout (`ui_state.rs`), so a window drag writes once, and on both apps the working version on screen (`History::keep_working`, keyed by its Library), so a run of edits writes `list.json` once. A write that supersedes a kept value drops it as it runs (`forget`): `History::switch_stack` drops the working version kept for the stack it leaves, which it keeps itself, so that never lands over the stack switched to. The store thread writes; it never encodes: a render's JPEG XL is encoded on the pool straight into its `.partial` folder (`History::encode_render`), and the store thread writes the color carrier and manifest and renames the folder (`History::save_encoded`), so a rename never waits behind an encode. Both apps save through `History::save`.
- **The pool** (`Pool` in `crates/deptheos_app/src/engine/work.rs`) has `W = clamp(cores / 2, 2, 8)` workers and two lanes. **Interactive:** a render opening, the frame or render on screen, a compare's or lit map's load, a full-size frame, a tile, a preflight, a batch plan, Adjust's GPU copies and draws. **Background:** thumbnails, conversions, exports, sequences, Library measuring. A free worker takes interactive work first; background work never takes the last free worker, so what the user asks for starts at once. An interactive request of a kind (`Key::kind`), for its owner (`Key::owned_by`: the desktop's UI thread, iOS's engine), is a latest-wins slot: a new one cancels the one before, and at most one runs, so they can't pile up and starve the background lane. A request for a particular thing (`Key::of`) takes no slot, and its asker drops it; two equal requests run twice, since both apps name a thing once.
- **Quality of service** on Apple platforms follows the lane: user-initiated, or utility for background.
- **A thread of its own, outside the pool, only for a call that can hang in the operating system with no way to stop it:** resolving a bookmark (ADR-145) keeps a thread per call (`bookmark`, made with `std::thread::Builder`, not the engine's builder) and a timeout, and is abandoned when it expires, so a share gone quiet never holds a pool worker.

### One CPU budget

- `cores` is `available_parallelism()`. rayon's global pool has `cores` threads, for the data-parallel loops inside one job. A job that runs codecs side by side (a render's result, frame map and color reference) runs them on scoped threads within its own work, its token and codec threads split between them, never on rayon's.
- **Codecs' teams are sized by how many decodes run at once**: a decode on the pool gets `max(1, cores / W)` codec threads; a stack's frame decode gets `max(1, (cores − aligner threads) / read-ahead window)`, the window being ADR-111's `ReadAhead`. The DNG SDK's host (`deptheos_host`) takes that count in its constructor, which also bounds the fast read's threads (at most 8 there). LibRaw with OpenMP (Windows, Linux) gets `omp_set_num_threads` on the decoding thread before `unpack`; with GCD (Apple) `dispatch_apply` has no width to set, so it is left at the machine's and counted as one decode. JPEG XL gets `JxlThreadParallelRunnerCreate(nullptr, n)`.
- **A codec gets its token and its thread count from the thread's current work** (`cancel::within`, which the pool and the prefetch threads set for each job), handed to it by `cancel::codec`, the one way a codec is called; not from an argument passed down every engine call.

### Codecs and waits watch their token

- **DNG SDK:** `deptheos_sniffer`, a `dng_abort_sniffer` whose `Sniff()` throws `dng_error_user_canceled` once the flag is set, with `ThreadSafe()` true so area tasks call it from every thread; every host gets one. Our fast read and lossless JPEG paths (`dng_fast_read.cpp`), which bypass the SDK's tasks, check the flag between runs and between tiles.
- **LibRaw:** a progress handler that returns nonzero once the flag is set, so LibRaw throws `LIBRAW_EXCEPTION_CANCELLED_BY_CALLBACK` at its next stage, and an `on_cancel` hook that calls `setCancelFlag()`, which the decoders' `checkCancel()` calls inside their `load_raw` loops see, since a long `load_raw` reports progress only at its start and end.
- **JPEG XL:** our parallel runner wraps `JxlThreadParallelRunner` and returns an error without running anything once the flag is set. The decoder calls the runner for each stage of each group pass, so a decode ends within a stage. libjxl turns a runner's error into `JXL_DEC_ERROR` in its source, not its documented API, so a test pins it. Input stays one buffer.
- **A codec's cancel is `CoreError::Canceled`**, never a frame left out (ADR-121).
- **Every blocking wait** in the engine is on a condition variable an `on_cancel` hook notifies, or a channel the token closes, and returns `Canceled` once its token is. The DNG cache's wait for another thread's conversion is one (`DngCache::acquire`). `DngCache::clear` cancels the cache's conversions through their tokens and deletes on the store thread without waiting for a reader still holding a file; what is held stays for the next launch's sweep (`sweep_default_dir`).

### The desktop's doorway

- **`crate::work`** (`crates/deptheos_gpui/src/work.rs`): `interactive`, `background`, `write` (an export, which quit waits for) and `store` (a Library write). Each puts its work on the engine's threads and returns a future whose drop cancels the work's token; a canceled request never resolves. GPUI drops a `Task`'s future with the `Task`, so a task living in the state it is for stops its work when the state is left, not only its wait.
- **Render opens** are one latest-wins slot: a new open cancels the one running, and the open checks its token between loading the stack and presenting it.
- **Off the UI thread:** the preferences' save, onto the store thread; Export's and a sequence's preflights, on the interactive lane. GPUI's background executor runs nothing that blocks but two that can wait long and would hold a pool worker meanwhile: the update check (`app/update_check.rs`, the network) and the command-line tool's install (`app/command_line_tool.rs`, an administrator prompt).

### The iOS doorway

- **The bridge's calls are UniFFI `async` functions** whose Rust side hands the work to an engine thread at once (the pool, the stack thread or the store thread: `on_pool`, `on_thread`, `on_store` in `crates/deptheos_ffi/src/request.rs`) and awaits a one-shot channel. UniFFI polls the future on whichever thread resumes the Swift side, so a future that blocked would block it; these never do. No async runtime is needed.
- **Cancel is explicit.** UniFFI's Swift side doesn't pass a task's cancellation to the Rust future (it frees the future only after it completes). So every call takes a `Request`, a UniFFI object around a `Cancel`, and one Swift helper (`Request.run`, `AppModel.ask`) wraps each call in `withTaskCancellationHandler { request.cancel() }`. A Swift task canceled, or dropped by the state that held it, cancels its engine work, and a canceled call is no failure (`AppModel.report`).
- **A Library write takes no `Request`:** once sent it is never canceled, and the app awaits it (`Engine::written` awaits the store's queue). Pasting adjustments onto a render and drawing its thumbnail again are such writes.
- **The paint and Adjust sessions' calls are plain**, not `async`: each only queues work on the engine's threads and returns, and what follows comes through the session's observer (`PaintObserver`, `AdjustObserver`), whose results the session applies only while current.
- **Only a render opening is latest-wins in the pool** (`Ask::Latest`; the rest `Own`): the app's `LatestLoad`s already run one load at a time, and a replaced request would end as `Canceled` where the app expects the newest load's own result.
- **Swift never blocks on Swift's pool.** Engine calls are `await engine.…`; the app's own blocking calls (ImageIO previews, file copies, iCloud downloads, a folder's listing, the diagnostics) go through `Blocking` (`Engine/Blocking.swift`), two `OperationQueue`s, interactive and background, which run Apple's frameworks, made for GCD's stacks, and never the engine. `Task.sleep`, never `Thread.sleep`. Retouch's and a picture's tiles are fetched through the engine's async calls in batches (`Picture::tiles`, `PaintSession::tiles`, a `TileAsk` each). `ios/build-xcframework.sh` fails on `Task.detached` or `DispatchQueue.global` in any Swift file of the app but `Engine/Blocking.swift`, `Model/Bookmarks.swift` (the bookmark resolver) and `Model/MemoryPressure.swift` (whose handler only asks the engine's holders to let go), comment lines aside.
- **Observers deliver through one `AsyncStream` each**, read on the main actor, so callbacks are handled in the order sent. The stack's progress and live map keep the newest only (`.bufferingNewest(1)`; a reversal, once heard, is kept); paint's reports, which change state, are never dropped (`.unbounded`); a run's own end is the `async` call's return or throw. A stream's `onTermination` cancels its request, so a view that stops listening stops the work; paint's report stream cancels nothing as it ends, since the paint session lives as long as the engine.
- **Debug builds run with `LIBDISPATCH_COOPERATIVE_POOL_STRICT=1`** in the shared scheme, which narrows Swift's pool to one thread so most blocking calls on it hang at once rather than in the field. It isn't a proof: a framework's own queue can still hide one.

### Quit and the background

- **Must finish:** the store thread's queue (a render being saved, renames, picks, relinks, a paint commit, the kept working version, the preferences), an export being written, a sequence's frame being written, and a stack that has finished but isn't saved yet, which is saved first.
- **Abandoned:** everything else, canceled through the root: opens, previews, thumbnails, conversions, a stack or batch still running (after the question it asks, keeping what is finished, as Cancel does), the Library's Export… still reading, plans and preflights.
- **The desktop** cancels the root, posts the pinned "quit-waiting" notice, and waits for the store thread's queue to drain and the writers to end, at most `QUIT_WAIT` (60 s); Quit again quits at once (`app/quit.rs`). Then it flushes the log file and exits. If a pool or prefetch thread is still inside a codec call, or a stack thread is still winding down after its cancel (a GPU pass or a read-back finishing), it ends the process with `_exit` once the store thread has flushed (`work::exit`), so no static destructor runs under work still using what it frees; otherwise it exits normally.
- **iOS** has no quit. On entering the background (`enteredBackground`), a background task awaits the store thread's queue (the list's write, paint's save, a render being saved). Every request but those made with `Request::lasting` (a stack's, an export's, the engine's own starts and writes) is canceled together, and refused until the app is back, through one foreground token (`leave_foreground`, `enter_foreground` in `crates/deptheos_ffi/src/request.rs`); a stack or batch runs on under the time `BGContinuedProcessingTask` gives it (ADR-165), and every full-size picture is let go of.

## Alternatives considered

### Separate flags and booleans for a lifecycle

Invalid combinations become possible and every reader has to rule them out; a `match` over one enum rules them out at build time.

### Threads of our own on the Swift side

Engine-owned Swift `Thread`s with 8 MiB stacks, fed by a locked queue, or a task executor preference (SE-0417) on such threads, would keep the bridge synchronous. But the lanes, slots and CPU budget would be written twice, in Swift and in Rust, the Swift threads couldn't count the codecs' teams, and each call would hold a Swift thread blocked on a Rust pool that already queues it. Async calls that hop to the engine's own threads keep one pool for both apps (ADR-101) and hold no thread while they wait.

### `Task.detached`

It runs blocking work on Swift's cooperative pool, which assumes tasks never block, with 512 KB stacks. It works until enough calls block at once.

### An async runtime's cancellation token

It has the same tree, but brings the runtime for one type. The engine's work is CPU and GPU work on our own threads, not I/O on a reactor, and the codecs need a plain `AtomicBool`.

### rayon, or GPUI's background executor, as the pool

rayon has no priorities and no cancellation, and its `join` steals work across jobs; it stays for the data-parallel loops inside one job. GPUI's executor is GCD on macOS, with 512 KB stacks, and exists only on the desktop.

### Feeding JPEG XL in chunks

A check between chunks comes only as input arrives, and most of a frame's decode runs after its last byte, inside one call. The runner is called for every stage, so a check there ends a decode sooner and changes nothing about how input is given.

## Consequences

- A process ended by `_exit` skips `atexit` handlers: the log file is flushed before it, and a DNG cache left behind is swept at the next launch, as after a crash.
- Every iOS engine call is `async` with a `Request`; a stack's job is a `Request` like any other.
- Each codec canceled mid-decode returns `Canceled` within 100 ms on a 45 MP frame, a test for each; tests hold a kind of work back with `work::gate` (`test-support`).

## Code

- State: `CLAUDE.md`'s lifecycle table; `web/src/latest.ts`; `crates/deptheos_app/src/adjust/machine.rs`.
- Engine: `crates/deptheos_core/src/cancel.rs`, `crates/deptheos_core/src/threads.rs`, `crates/deptheos_app/src/engine/work.rs`, `crates/deptheos_app/src/engine/store.rs`, `crates/deptheos_app/src/library/history.rs`, `crates/deptheos_app/src/frames/conversions/mod.rs`.
- Codecs: `crates/deptheos_raw/src/lib.rs`, `crates/deptheos_raw/native/raw_c_api.cpp`, `crates/deptheos_dng/native/dng_bridge_internal.h`, `dng_fast_read.cpp`, `jxl_c_api.cpp`.
- Desktop: `crates/deptheos_gpui/src/work.rs`, `renders/opening.rs`, `app/quit.rs`, `bookmarks.rs`.
- iOS: `crates/deptheos_ffi/src/request.rs`; `ios/DeptheosMobile/Model/AppModel.swift`, `Engine/Blocking.swift`, `Model/LatestLoad.swift`, `Model/AppModel+Lifecycle.swift`, `ios/build-xcframework.sh`, the shared scheme.
