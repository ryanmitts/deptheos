# Working in Deptheos

A GPU focus-stacking engine (Rust, `crates/`) with three front ends: the
desktop app (`crates/deptheos_gpui`), the iOS app (`ios/`, over
`crates/deptheos_ffi`) and the website (`web/`), plus the CLI
(`crates/deptheos_cli`) and the camera rig tool (`tools/stackrig`).
`docs/architecture.md` has the crate graph, the stack pipeline across its
threads, the main code paths and the Library on disk.

## System map

The crates:

- `deptheos_gpu`: the one wgpu device, `GpuContext`, headless (ADR-100).
- `deptheos_core`: the three stackers, their GPU pipelines, image loading
  and saving (`io`).
- `deptheos_render`: the color render, from stored pixels to the colors
  shown and exported (`dng_render`, ADR-130), kept out of the stackers'
  reach.
- `deptheos_aligner`: alignment on the CPU, ECC and grid search, behind
  `Aligner` (ADR-112).
- `deptheos_dng`: the Adobe DNG SDK, to read, render and write DNG.
- `deptheos_raw`: LibRaw, for camera RAW. `native_build`: the build-script
  helpers both native crates share.
- `deptheos_app`: what every front end shares: `Engine`, `StackingSession`,
  the Library (`History`), paint, the batch machine, the view reducer, the
  work pool and store thread, logs and the user's sentences (`Words`).
- `deptheos_gpui`: the desktop app, on GPUI (ADR-101).
- `deptheos_ffi`: the iOS bridge (UniFFI), every call `async` (ADR-143).
- `deptheos_cli`: the `deptheos` command line.
- `deptheos_e2e`: the end-to-end checker of exports (`e2e.toml`).
- `deptheos_webexport`: the website's `manifest.json`, from the engine's
  own parameters (ADR-160).

The apps: `ios/DeptheosMobile` (SwiftUI over the bridge), `web/` (an Astro
site whose docs figures are in `web/src/labs/`), `tools/stackrig` (a macOS
tool that drives a camera and turntable).

Who owns what:

- The GPU: one `GpuContext`, made at launch and held by `Engine`. Stacks,
  renders and paint previews run on it; the apps draw CPU images read back
  from it, on their own devices (ADR-100).
- Threads (ADR-143): each app's UI thread owns its state and never
  blocks. The engine owns the rest, each made by `work::thread`: a `stack`
  thread per run, the pool (`work::pool`: an interactive and a background
  lane), the store thread (`store::store`: every Library write, in order),
  the `paint` worker and `paint-timers`, and the prefetch and conversion
  threads a run starts.
- The Library: `History` (`crates/deptheos_app/src/library/history.rs`),
  opened by `History::open`, which takes `library.lock`: one app at a time
  (ADR-140). Its writes go through the store thread.

## Decisions

Why things are built as they are is in `docs/decisions/` (its `README.md`
lists the records by area). Read the record before changing something it
covers, and write a new one for a choice that would be expensive to reverse.

Comments and docs are plain sentences about the code as it is: they cite
only files that exist (`just check-paths` checks) and ADR numbers, never
history, plans or reviews.

A commit that removes a feature or branch also removes what only it used
(variants, messages, Options, tests, docs), found by grep before
committing.

## State: one enum per lifecycle

Anything with a lifecycle (a run, an open, a session, a gesture) is one
`enum` (a discriminated union in TypeScript), owned by one place and changed
only through its methods. Data that belongs to one state lives inside that
variant, not in an `Option` or a flag beside the enum. Then a `match` that
misses a state fails to build, and one file shows the whole lifecycle.

Don't add a bool or an `Option` that together with others encodes a mode.
When you work in code that does, convert it. Two states where one holds
nothing is an `Option`, not an enum. Log each transition at debug level.

A lifecycle's doc comment has one paragraph starting `Lifecycle:` that
names its states and what changes it, followed by its transitions.
`just lifecycles` lists every one. The main machines:

- A batch run, every front end: `Machine` in
  `crates/deptheos_app/src/runs/batch/machine.rs`, events in, actions out,
  no I/O (ADR-144).
- A stack run's progress, every front end: `Progress` in
  `crates/deptheos_app/src/runs/session/progress.rs`.
- The stackers: `State` in `crates/deptheos_core/src/pyramid/mod.rs`,
  `crates/deptheos_core/src/depth/mod.rs` and
  `crates/deptheos_core/src/wavelet/mod.rs`.
- Paint, both apps: `Machine` in `crates/deptheos_app/src/paint/machine.rs`,
  driven by `PaintSession` (`paint/session.rs`).
- Adjust, both apps: `Machine` in `crates/deptheos_app/src/adjust/machine.rs`,
  events in, actions out, no I/O (ADR-153).
- The viewer's view, both apps: `View` in `crates/deptheos_app/src/view.rs`,
  a pure reducer whose table tests are the behavior (ADR-149).
- Desktop: `Screen` and `Running` in `crates/deptheos_gpui/src/app.rs`,
  changed only by `set_screen` (`app/navigate.rs`) and `set_running`
  (`app/run.rs`).
- iOS: `Running` in `ios/DeptheosMobile/Model/AppModel.swift` and `Panel`
  in `ios/DeptheosMobile/Model/Panel.swift`, changed only by `setRunning`
  and `setPanel` and their kin in `AppModel.swift` (ADR-163).

Call order is enforced by types where callers already follow it: the
sequence writer's `close(self)`, `ImageUploader::prepare*` returning a
`PreparedUpload` that `dispatch_to_rgba` uses up.

## Async results apply only while their request is current

A slow task can finish after the user has moved on. Keep the request in
flight in the state, and apply a result only if the state still holds that
request: check after every `await`, before changing anything.

- Web: `latest()` (`web/src/latest.ts`), rather than a hand-written token.
- Desktop: blocking work goes through `crate::work` (ADR-143):
  `interactive`, `background`, `write` (an export, which quit waits for)
  and `store` (a Library write, on the store thread). Never a thread of its
  own or GPUI's background executor, but for the update check and the
  command-line tool's install. The task awaiting it lives in the state it
  is for, so leaving the state drops it, which cancels the work. A codec is
  called through `deptheos_core::cancel::codec`.
- iOS: every engine call is the bridge's `async` one (ADR-143), through
  `AppModel.ask` or `Request().run`, canceled with its task. A Library
  write is never canceled. The app's own blocking work goes through
  `Blocking` (`Engine/Blocking.swift`); no `Task.detached` or
  `DispatchQueue.global` but the bookmark resolver and
  `Model/MemoryPressure.swift`, which `ios/build-xcframework.sh` checks.
  Observers are one `AsyncStream` each, read on the main actor. Every load
  of the viewer goes through a `LatestLoad` (`Model/LatestLoad.swift`).

## Logs

Every front end logs through `tracing`. The desktop and iOS apps set it up
with `deptheos_app::engine::logs::install` (`crates/deptheos_app/src/engine/logs.rs`):
stderr, plus a file per launch that users send with bug reports (the CLI
logs to stderr only, `install_logs` in `deptheos_cli/src/main.rs`). `info`
and up is what a user's file holds; `debug` only with Verbose Logging on.
The file writes the home folder as `~`, but don't log photo metadata (GPS,
serial numbers, owner names) at any level.

## The engine's sentences

What the shared layer says to a user (errors, notices, plan and batch
lines, method labels) is a message in
`crates/deptheos_app/i18n/en-US/deptheos_app.ftl`, worded through a
`Words` (`deptheos_app::words`, ADR-161) that each front end makes once
and passes down. The desktop's own strings are messages in
`crates/deptheos_gpui/i18n/en-US/deptheos_gpui.ftl`, worded with that
crate's `say!(cx, "id", …)` (`deptheos_gpui/src/words.rs`). In either
file, write a label or sentence whole as one message with a comment naming
its arguments; a count that picks a plural is a number argument,
everything else finished text. `just test-words` checks the ids and
renders every message against each crate's `src/words/english.snap`. The
CLI's and stackrig's own strings stay in code. The words to use are in
`docs/terms-and-usage.md`.

## Quick checks

Every command is a recipe in the `justfile` at the root: `just` lists
them, `just -n <recipe>` prints a recipe's command without running it. Add
a recipe there for a new command rather than writing it out here.

```sh
just check          # the batch machine, stackers, native codecs, iOS bridge, desktop, CLI, e2e checker, website
just test-app       # the shared app layer, all of it (paint, view, Library, …)
just test retouch   # one test or group by name
just lint           # the files docs and comments name, clippy with the tests, and the formatting
just test-words     # the user-facing sentences, both apps
just test-examples  # the benches, probes and iOS smoke run still build
just web-smoke      # the pages in a real browser, with `just web` running
just e2e            # e2e.toml's cases through the desktop app, each export checked
just e2e canon1     # one case
just e2e-ios canon1 # one case in the Simulator (minutes)
just bless canon1   # accept a case's current pictures as its references
just lifecycles     # every lifecycle's doc comment
```

The end-to-end cases are in `e2e.toml` at the root, listed by
`just e2e-cases`; `crates/deptheos_e2e` reads it and checks every export.
The references are in `~/DeptheosE2E/references/<case>`
(`DEPTHEOS_E2E_REFERENCES` names another folder).

iOS has a UI test target (`ios/DeptheosMobileUITests`, the end-to-end
test) and no unit tests. After a Rust change, `just ios-engine`, then
`just ios-build`, which builds by scheme: building by target doesn't
resolve the local engine package.
