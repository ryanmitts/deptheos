# ADR-102: Releases are native packages for the platforms the owner tests on; the app points to updates and carries the CLI

## Status

Accepted.

## Context

Deptheos ships the desktop app for macOS, Windows and Linux, and the iOS app, from one repository. Users need the app, the `deptheos` command-line tool (ADR-103) and a way to learn of a new version. Focus stacking holds many full-resolution frames in system and GPU memory (ADR-111), and every platform build is checked on real hardware before a release (ADR-113, the device table in `docs/releasing.md`).

## Decision

### Platforms and floors

- **macOS:** Apple silicon (`aarch64-apple-darwin`) only, macOS 27.0 or later. `min_macos` in `script/bundle-mac.sh` sets the build's `MACOSX_DEPLOYMENT_TARGET`, the icon's and `LSMinimumSystemVersion`. 27 is the macOS the owner can test on; the floor follows the test Mac rather than the oldest the code might build for.
- **iPhone and iPad:** iOS and iPadOS 26.0 or later (`IPHONEOS_DEPLOYMENT_TARGET` in `ios/DeptheosMobile.xcodeproj` and `ios/build-xcframework.sh`).
- **Windows:** `x86_64`, built with MSVC, Windows 10 or later (`MinVersion` in `packaging/windows/deptheos.iss`).
- **Linux, as a preview:** `x86_64` and `arm64`, each built natively on Ubuntu 22.04, so the packages run on any distribution with its `glibc` (2.35) or newer.

### One tag, every platform

- A `v*` tag pushed to the public repository runs `.github/workflows/release.yml`, which builds each platform on its own runner and drafts a GitHub release; the owner edits the notes and publishes it (`docs/releasing.md`). Run by hand, the workflow is a test build whose files stay workflow artifacts.
- The workflow refuses a tag that doesn't match `[workspace.package] version` (`script/version.sh`), and a tag build refuses a Mac build without the signing secrets. Every action is pinned to a commit.
- The draft holds the packages, the full source with LibRaw for the GPL and the CDDL (`script/source-tarball.sh`, since GitHub's own source downloads leave the submodule out) and `SHA256SUMS.txt`. The symbols (dSYMs, PDBs) stay workflow artifacts, out of the release.
- The iOS app is built in the same workflow with the same version; on a tag, with the App Store Connect key, it is signed and uploaded to TestFlight (`ios/ExportOptions.plist`). The desktop release doesn't wait for it.

### Native packages

- **macOS:** a signed, notarized DMG of `Deptheos.app` (`script/bundle-mac.sh`). Its bundle ID is `com.deptheos.Deptheos`.
- **Windows:** a per-user Inno Setup installer and a portable zip, with the C++ runtime DLLs beside the exe (`script/bundle-windows.ps1`, `packaging/windows/deptheos.iss`). Both are unsigned.
- **Linux:** `.deb`, `.rpm` and a tarball for each architecture (`script/bundle-linux.sh`).

### The update check only points

At startup (a preference turns it off) and from Check for Updates…, the app asks GitHub for the latest published release (drafts are left out), compares versions with semver and, for a newer one, opens its release page. The URL is built from the repository constant `REPO` and the validated tag, so a response can't send Download elsewhere. The request carries only the app's version, in the User-Agent GitHub requires, and gives up after 15 s. Nothing is downloaded or installed (`crates/deptheos_gpui/src/updates.rs`).

### The CLI ships inside the app

`deptheos` is in `Deptheos.app/Contents/MacOS`, and Install Command Line Tool… links `/usr/local/bin/deptheos` to it (`crates/deptheos_gpui/src/app/command_line_tool.rs`). On Windows it is beside the exe, and the installer can add it to the user's `PATH`. The Linux packages put it in `/usr/bin`.

## Alternatives considered

### Intel Macs, older iOS versions, or Windows on ARM

Without test hardware, GPU driver and memory limits would ship untested. On iOS, 26 also gives `BGContinuedProcessingTask` for stacking in the background (ADR-165) and devices with enough memory for multi-frame stacks.

### Ship only the app, and leave the CLI to build from source

Shipping `deptheos` in the packages gives every user the CLI without a Rust and C++ toolchain, and the macOS link into `Deptheos.app` keeps it updated with the app.

### A self-updater

Opening the release page avoids signing an update feed and writing self-replacement for macOS, Windows and the Linux package managers.

### Windows code signing

Not done: there is no certificate. `docs/releasing.md` ("Windows signing") has the steps, and the workflow marks where signing goes.

## Consequences

- Releases are tagged on the public repository (`ryanmitts/deptheos`), which `updates.rs` queries.
- The Windows installer's `AppId` never changes: Windows matches upgrades and uninstalls by it (`packaging/windows/deptheos.iss`). The macOS bundle ID stays constant for the same reason.
- Every package carries a matching `deptheos-gui` and `deptheos`.
- Building for macOS 27 needs the macOS 27 SDK (Xcode 27), on the release workflow's runner too.
- The website's download page and the platform table in `README.md` state these floors and change with the build scripts when a floor does.

## Code

- `.github/workflows/release.yml`, `script/version.sh`, `script/source-tarball.sh`.
- `script/bundle-mac.sh`, `script/bundle-windows.ps1`, `script/bundle-linux.sh`, `packaging/windows/deptheos.iss`.
- `ios/build-xcframework.sh`, `ios/DeptheosMobile.xcodeproj`, `ios/ExportOptions.plist`.
- `crates/deptheos_gpui/src/updates.rs`, `crates/deptheos_gpui/src/app/command_line_tool.rs`.
- `docs/releasing.md`, `docs/windows.md`, `README.md`.
