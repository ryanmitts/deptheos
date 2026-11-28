# ADR-150: Preferences both apps offer are defined once, in deptheos_app; each front end keeps its own storage

## Status

Accepted.

## Context

Several preferences mean the same on the desktop and on iOS: how an export is written (JPEG quality, TIFF compression), what it is called (the file name pattern), and how far Grid search looks (its four limits and exposure compensation). Defined in each app, they would have two sets of defaults, ranges and sentences that drift apart, which the terms page (ADR-161) forbids for anything both apps show.

The two apps store settings differently: the desktop in one JSON file, written on the store thread as it changes (ADR-143), iOS in its own settings storage. The desktop's file must keep loading.

## Decision

- **The definitions are shared, in `deptheos_app::preferences`.** `ExportPreferences` (`file_name_pattern`, `jpeg_quality`, `sixteen_bit`, `tiff_compression`, `space`) and `AlignmentPreferences` (`grid_horizontal_pct`, `grid_vertical_pct`, `grid_rotation_deg`, `grid_magnification_pct`, `exposure_compensation`) hold the fields with their defaults; `sanitized()` clamps stored values into their ranges (a value that isn't a number becomes 0, clamped); and `Preference` gives each one's label, help and range. The words are messages in `deptheos_app.ftl` (`pref-…`), worded through `Words` (ADR-161). How they reach the engine is shared too: `AlignmentPreferences::apply` sets a stack's Grid search, `ExportPreferences::save_options` an export's options, and `export::file_stem` makes a file name from the pattern (`DEFAULT_PATTERN`, `NameParts`).
- **16 bits and the space are not settings.** `sixteen_bit` and `space` (an adjusted camera RAW render's, kept as `export_space`) have no `Preference`: each is the last choice made in an Export sheet (and for the bits on the desktop the Batch dialog's Start), which the next one starts from. Each is kept under its key, so an older `preferences.json` or iOS's stored value loads unchanged.
- **Storage stays with each front end.** The desktop flattens both structs into its own `Preferences` (`#[serde(flatten)]`), so `preferences.json` keeps its keys and values (tested); the export pattern's key is `file_name_pattern`. iOS keeps the values in its own storage (`UserDefaults`, `Model/AppPreferences.swift`) and hands them to the engine with each stack and export as the shared structs themselves, exported as they are (`uniffi::remote`, `crates/deptheos_ffi/src/preferences.rs`), clamping what it reads through `sanitized_preferences`.
- **Not shared, on purpose:** the theme, the GPU, the DNG converter's setup, the read-ahead memory, update checks, and the camera profile (it applies only to a RAW decoder iOS doesn't have). The Export sheet's remembered format stays each app's (iOS remembers one for each kind of stack), and its remembered folder the desktop's. The reverse-order check is no preference at all: every app and the CLI always run it when aligning.
- **Preferences never make a stack edited.** The alignment preferences are recorded with each render's options, but `stacks::Compared` doesn't count them (ADR-142): they are the app's setup, not the stack's settings.

## Alternatives considered

### One shared preferences file

Both apps reading one file in the engine would need a location iOS can share with nothing, and would make the desktop's GPU, theme and converter settings part of the shared format. Each app already has the settings store its platform expects.

### Copy the definitions into the iOS app

The cheapest start, and the drift the terms page exists to prevent: two defaults for JPEG quality, two ranges for the limits, two help texts translated separately.

### Put the preferences into `StackSettings` and the stack's options

Grid search's limits would then be per stack and count as edits, so changing a preference would mark every working version edited. A preference is a default for new work; the stack's settings are what the user chose for that stack.

## Consequences

- A new preference both apps offer goes in `deptheos_app::preferences` with its message, its range and a field in the bridge's remote records; a desktop-only one stays in `crates/deptheos_gpui/src/prefs.rs`.
- The desktop's Settings window reads the shared labels and ranges.
- A key renamed in the shared structs resets that preference for every desktop user, as any stored name would (ADR-141).
- iOS's stack and export calls take the preferences (`Engine::stack`, `StackResult::export`, `PaintSession::export`); there are no calls with the defaults beside them.

## Code

- `crates/deptheos_app/src/preferences.rs`, `crates/deptheos_app/src/library/export.rs`, `crates/deptheos_app/i18n/en-US/deptheos_app.ftl`.
- `crates/deptheos_gpui/src/prefs.rs`, `crates/deptheos_gpui/src/prefs_window.rs`.
- `crates/deptheos_ffi/src/preferences.rs`, `crates/deptheos_ffi/src/engine.rs`.
- iOS: `ios/DeptheosMobile/Model/AppPreferences.swift`, shown in `ios/DeptheosMobile/Views/SettingsSheet.swift`.
