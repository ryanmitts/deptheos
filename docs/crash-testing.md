# Testing crash handling

What a crash leaves, and how to cause one on purpose to check it. The
handling itself is ADR-166: the panic hook writes the panic to the log
with its backtrace and frame addresses, and the OS writes its own crash
report (on Windows the app writes a minidump). Save Diagnostics… zips
them for the user to send; nothing is shown at the next launch. Reading a crash once you
have one is in `docs/releasing.md` (Reading a crash).

The crashes on purpose are
`deptheos_app::engine::logs::on_purpose` (`crates/deptheos_app/src/engine/logs/on_purpose.rs`):

| Kind | What it does | The app | Leaves |
|---|---|---|---|
| `panic` | a Rust panic on the UI thread | ends (SIGABRT: a panic can't unwind out of the run loop) | the log's `PANIC` with backtrace, an OS crash report |
| `thread-panic` | a Rust panic on an engine thread of its own | goes on | the log's `PANIC` |
| `abort` | `abort()`, no panic | ends (SIGABRT) | an OS crash report (on Windows no minidump: Rust's abort fails fast past the filter) |
| `fault` | a read of an address never mapped | ends (SIGSEGV, `EXC_BAD_ACCESS` at `0x10`; an access violation on Windows) | an OS crash report, the Windows minidump |

## Desktop

Launch with `DEPTHEOS_CRASH` set to a kind: the app crashes that way two
seconds after its main window opens. It works in a release build too,
which is the one to check symbolication with.

```sh
just crash panic                  # a debug build, on a scratch home (below)
just crash fault release          # the release build
```

`just crash` runs the app with `HOME` set to a scratch folder
(`target/crash-home`, kept between runs), so your own Library and
settings are untouched; the log folder is then
`target/crash-home/Library/Logs/Deptheos`. The OS's crash reports still go
to your own `~/Library/Logs/DiagnosticReports`. To crash the app you use
day to day instead, set the variable yourself:
`DEPTHEOS_CRASH=panic cargo run -p deptheos_gpui --bin deptheos-gui`.

Then check, for each kind that ends the app:

1. **The log.** The newest `deptheos-*.log` in
   `target/crash-home/Library/Logs/Deptheos` ends with
   `crashing on purpose: <kind>` and, for `panic`, a `PANIC thread 'main'`
   block with the backtrace and the frames' addresses.
2. **Save Diagnostics….** `just crash-home` launches the app on the same
   scratch home without crashing, and shows nothing about the crash.
   Help ▸ Save Diagnostics… writes a zip whose `crash-reports/` holds the
   `.ips` (macOS) or `.dmp` (Windows), the machine ids blanked in an
   `.ips`, and whose logs write the home as `~`.

For `thread-panic`, the app stays open: the log has a `PANIC thread
'crash-on-purpose'` block.

Per platform:

- **macOS**: the crash report is
  `~/Library/Logs/DiagnosticReports/deptheos-gui-<date>.ips` (a bundled
  app's is named after its executable too). `panic` and `abort` show
  `EXC_CRASH (SIGABRT)`, `fault` `EXC_BAD_ACCESS (SIGSEGV)`. To check
  symbols, crash a `script/bundle-mac.sh` build and symbolize as Reading
  a crash says, with the dSYM from `dist/symbols`.
- **Windows**: after `fault`, `%LOCALAPPDATA%\Deptheos\crashes` holds
  `deptheos-gui.exe.<launched>.<pid>.dmp` (`abort` leaves none, as the
  table says). Set the variable in PowerShell with
  `$env:DEPTHEOS_CRASH = "fault"` before launching.
- **Linux**: no crash report of the app's own; the log is the check.

## iOS

The crash comes from Settings ▸ Diagnostics ▸ Crash on Purpose, a menu
of the four kinds, shown in Debug builds only, never in a Release or App
Store build. In a Debug build in the Simulator, the launch argument does
the same without a tap:

```sh
xcrun simctl launch <device> com.deptheos.DeptheosApp -CrashOnPurpose panic
```

Under the bridge a `panic` ends the app too: the bridge hands the panic
to Swift as an error a non-throwing call can't return, so the crash
report says `EXC_BREAKPOINT`, after the log has the panic.

**Run it without Xcode's debugger.** Attached, the debugger stops at the
crash and no crash report is written: launch from the Home Screen (or
`xcrun simctl launch`), or stop the run in Xcode and open the app again.

Then check:

1. **The log.** In the Simulator,
   `xcrun simctl get_app_container <device> com.deptheos.DeptheosApp data`
   names the container; the newest `deptheos-*.log` in
   `Library/Caches/Logs` has `crashing on purpose:` and, for `panic`, the
   `PANIC` block. On a device, Send Diagnostics… after the next launch
   has the log.
2. **The crash report.** Simulator: `~/Library/Logs/DiagnosticReports/DeptheosMobile-<date>.ips`
   on the Mac. Device: Settings ▸ Privacy & Security ▸ Analytics &
   Improvements ▸ Analytics Data, `DeptheosMobile-<date>.ips`, or Xcode ▸
   Window ▸ Devices and Simulators ▸ View Device Logs. A TestFlight
   build's crashes also reach App Store Connect (TestFlight ▸ Crashes)
   and Xcode's Organizer ▸ Crashes, symbolicated with the archive's dSYM,
   a day or so later when the tester shares them.
