# ADR-166: Every front end logs through tracing; the desktop and iOS apps also keep a file per launch that users send with bug reports

## Status

Accepted

## Context

Deptheos runs across macOS, Windows, Linux, and iOS on a wide variety of GPUs and drivers. When a user hits a bug, logs written only to `stderr` or `os.Logger` are not available to attach to a bug report. At the same time, Deptheos makes no background telemetry or crash-reporting network requests.

## Decision

`deptheos_app::engine::logs::install` (`crates/deptheos_app/src/engine/logs.rs`) initializes `tracing` for the desktop and iOS apps, writing to both `stderr` and a per-launch log file in the platform's log directory (`~/Library/Logs/Deptheos`, `%LOCALAPPDATA%\Deptheos\logs`, `$XDG_STATE_HOME/Deptheos`, and `Library/Caches/Logs` on iOS), rolled over at 10 MiB and pruned to the newest 10 files. The CLI initializes `tracing` on `stderr` only, controlled by `--quiet`, `--verbose`, and `RUST_LOG`.

- **Header and panic hook:** Each launch log opens with a header recording the app version, build, OS, hardware model, CPU count, RAM, and locale, followed by the GPU adapter and driver logged by `GpuContext`. A panic hook writes the panic message and backtrace into the log file, with each frame's address in the app's loaded images and their load addresses (`logs/images.rs`), since release builds keep their line tables only in the dSYMs and PDBs archived per release (`docs/releasing.md`), before delegating to the default hook so the OS crash reporter still runs.
- **Default level `info`, session-only Verbose Logging:** `info` and above are written by default. Enabling Verbose Logging turns on `debug` for Deptheos crates (which logs all lifecycle enum transitions; ADR-143) for the current session only, resetting on quit so verbose logging is never left on accidentally.
- **No notice at the next launch:** neither app keeps a record of whether the last launch quit cleanly or says anything about it at the next launch. What a crash leaves is the log's panic, the OS crash report and, on Windows, the minidump, all of which Save Diagnostics… gathers; Help ▸ Report a Problem… on the desktop opens the bug form (`.github/ISSUE_TEMPLATE/bug.yml`). GPU device loss stays the fault notice it is.
- **Crashes on purpose:** `logs/on_purpose.rs` causes each kind of crash the above records (a panic on the UI thread or an engine thread, `abort()`, a bad access), so each can be checked by hand: the desktop when launched with `DEPTHEOS_CRASH` set (`just crash <kind>`), the iOS app from Settings ▸ Diagnostics ▸ Crash on Purpose in its Debug and TestFlight builds. `docs/crash-testing.md` says what each should leave.
- **User-initiated diagnostic bundle:** Save Diagnostics… on the desktop (which also includes Deptheos's crash reports from the last 30 days: on macOS its `.ips` reports, on Windows the three newest minidumps of at most 256 MB in `%LOCALAPPDATA%\Deptheos\crashes`, which the desktop writes itself: an unhandled-exception filter calls `MiniDumpWriteDump` (`MiniDumpNormal` with thread info) from a thread of its own and hands the exception on, and each launch leaves room for its own beside the two newest, `crates/deptheos_app/src/engine/logs/dumps.rs`) and Send Diagnostics… on iOS bundle the logs into a `.zip` file that the user can inspect. On the desktop it is emailed to the support address (`SUPPORT_ADDRESS` in `crates/deptheos_gpui/src/app/diagnostics.rs`), never attached to a public issue, since crash reports say much about the machine; the fault notices and the bug form say so.
- **Privacy rules (`CLAUDE.md`):** the home folder is written `~` in the log files and the bundle however it is spelled, a Linux card's mount loses the account name, and a crash report's machine ids are blanked while the build UUIDs symbolication needs stay (`crates/deptheos_app/src/engine/logs/redact.rs`). Paths and file names are logged at `info`, so a log sent without Verbose Logging still says what was read; on iOS a Swift line's string reaches the file only when marked `privacy: .public` (`AppLog`). Photo metadata (GPS coordinates, camera and lens serial numbers, owner names) is never logged at any level.

## Alternatives considered

### Automatic crash-reporting or telemetry services

Deptheos sends no telemetry or background crash reports; users choose when to export and share a diagnostic zip.

### Windows Error Reporting's LocalDumps for the minidumps

It would write the dumps without code of ours, but Windows reads that setting only from `HKEY_LOCAL_MACHINE`, which the per-user installer can't write.

## Consequences

- All `info`, `warn`, and `error` log lines appear in user diagnostic bundles and must be informative and free of personal metadata; lifecycle transitions are logged at `debug`.

## Code

- `crates/deptheos_app/src/engine/logs.rs`, `crates/deptheos_app/src/engine/logs/bundle.rs`, `crates/deptheos_app/src/engine/logs/redact.rs`.
- `crates/deptheos_app/src/engine/logs/dumps.rs`, `crates/deptheos_app/src/engine/logs/on_purpose.rs`, `docs/crash-testing.md`, `.github/ISSUE_TEMPLATE/bug.yml`.
- `crates/deptheos_cli/src/main.rs`: `install_logs`.
- `ios/DeptheosMobile/Model/AppLog.swift`.
