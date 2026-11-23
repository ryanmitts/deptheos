# Releasing

A version tag builds the macOS DMG, the Windows installer and the Linux
packages (x86_64 and arm64) in GitHub Actions
(`.github/workflows/release.yml`) and drafts a GitHub release with them,
the full source with the LibRaw submodule
(`Deptheos-<version>-source.tar.gz`, `script/source-tarball.sh`; GitHub's
own source downloads leave submodules out, and the GPL and the CDDL ask for
all of it) and `SHA256SUMS.txt`, a checksum for each file.
The arm64 Linux job runs on GitHub's `ubuntu-22.04-arm` runners. If the
repository's plan has none, that job waits for one, and so does the draft:
take arm64 out of the matrix, build those files in an ARM VM (below) and
add them to the draft by hand.

Publishing the draft makes it the release the app's update check offers
(`crates/deptheos_gpui/src/updates.rs`, which asks `ryanmitts/deptheos`).

## Release checklist

One pass, top to bottom, for every release. The tag builds a draft that
nobody sees until it is published, so the files tested below are the
ones that ship. Tick each line as it passes.

### 1. Before the tag

- [ ] `just check`, `just lint`, `just test-words`, `just test-examples`.
- [ ] `just e2e` (every desktop case) and `just e2e-ios <case>` for each
      iOS case. A case without references yet is judged only by its other
      checks: look at its pictures, then `just bless <case>`.
- [ ] Performance: `just baselines` on the Mac the baselines below were
      made on; each stack's time and peak memory within about 10% of its
      row, or the reason known.
- [ ] The Tests workflow's macOS job passed on `main`: its
      `select-xcode` step (`.github/actions/select-xcode`) fails when the
      runner image has no Xcode 27, whose SDK the macOS 27 floor needs
      (ADR-102), and the release's Mac jobs use the same step.
- [ ] Set `version` in the root `Cargo.toml` (`[workspace.package]`),
      newer than the last release's tag, add its `## <version>` section to
      `CHANGELOG.md` (the release's notes; the workflow refuses a tag
      without one), and commit.

### 2. Tag

- [ ] `git tag v0.2.0 && git push origin v0.2.0` to the public repo. The
      workflow refuses a tag that doesn't match the version.
- [ ] The run's draft holds the DMG, the Windows installer and portable
      zip, the Linux `.deb`, `.rpm` and tarballs for both architectures,
      `Deptheos-<version>-source.tar.gz` and `SHA256SUMS.txt` naming each.
- [ ] The `ios` job uploaded the build to TestFlight, numbered with the
      run (Versions and build numbers, below).
- [ ] Keep the run's `symbols-macos`, `symbols-windows` and `symbols-ios`
      artifacts with the tag: GitHub deletes them after 90 days.

### 3. The device matrix

Each row installs the draft's file (TestFlight for iOS), then runs the
checks under it. The oldest systems are the floors ADR-102 sets.

| Device | System | Why |
|---|---|---|
| Mac with Apple silicon, 16 GB or more | macOS 27 (the oldest supported) | the main platform |
| Mac with 8 GB | macOS 27 | the memory check refuses, never a kill |
| iPhone of the smallest the App Store tier admits (iPhone 15 Pro, 8 GB) | iOS 26 (the oldest supported) | memory, camera (ADR-164) |
| iPad with an M-series chip | iPadOS 26 or later | the regular-width layout, Pencil |
| PC with NVIDIA | Windows 11 | DirectX 12 |
| PC with AMD | Windows 10 (the oldest supported) | DirectX 12, another driver |
| PC with Intel graphics | Windows 11 | integrated graphics, low memory |
| One Linux (a preview) | Ubuntu 22.04 or 24.04, with a Vulkan driver | glibc floor, the .deb |

On every row:

- [ ] **Signing.** macOS: the DMG and app are notarized and stapled
      (`spctl -a -vv -t install` on the DMG, `spctl -a -vv` on the app),
      and it opens with no warning. Windows: the installer and exe are
      signed, or, unsigned, show SmartScreen's warning as Installing on
      the website says. iOS: TestFlight installs it.
- [ ] **First launch offline.** With the network off: it starts, the
      update check fails quietly, and Try a Sample Stack stacks.
- [ ] **Upgrade over the last release.** With it installed and a full
      Library (several stacks with renders, one retouched so it has a
      `paint.sqlite`, a stack whose frames' folder was renamed so its
      bookmark is broken), install over it and open it: every stack and
      render opens, the retouched one shows its paint, the broken one
      offers to find its folder, and nothing is set aside (no
      `*.unreadable-*` file in the Library, no "kept unreadable" line in
      the log).
- [ ] **A stack of your own** with each method, exported, retouched and
      exported again, with no `PANIC` in the log.
- [ ] **Uninstall.** macOS: the app dragged to the Trash; Windows:
      Settings ▸ Apps removes it and its PATH entry, the Library stays;
      Linux: `apt remove`; iOS: deleting the app. The Library and logs
      are where Installing on the website says, and only Delete Library
      or removing that folder deletes them.

### 4. By hand, once per release

- [ ] **Soak.** In the desktop app's release build, stack 200 frames (the
      `pcb1_dng` sample set) with each method, export,
      retouch and export again: no crash, no notice it shouldn't give,
      memory flat between stacks, times within the baselines' range.
- [ ] **RAW to DNG.** A DNG export of a camera RAW stack opens in
      Lightroom or Camera Raw and in Photos or Preview: color matches the
      frames, upright, metadata shown.
- [ ] **Color.** A Display P3 JPEG and a 16-bit TIFF export look the same
      in Photos and in a browser as in the app.
- [ ] **`dng_validate`** (the DNG SDK's, `native/vendor/dng_sdk/source/dng_validate.cpp`)
      passes on a handful of DNG exports: a camera RAW stack, a linear DNG
      stack, one retouched.
- [ ] **Diagnostics.** Help ▸ Save Diagnostics… on each desktop system and
      Send Diagnostics… on iOS: the zip's logs write the home as `~`
      (on Windows also a home in its short folder name, as `~\…`, and in
      a path logged with doubled backslashes), a Mac's crash reports have
      no machine ids, and a Linux card's mount (`/media/<user>`,
      `/run/media/<user>`) shows no account name.
- [ ] **Crashes.** On each system, `docs/crash-testing.md`'s checks
      with the release build: each kind leaves what its table says, the
      next launch shows nothing about it (ADR-166), and a crash report or
      dump in the diagnostics zip symbolizes with the tag's symbols.

### 5. By hand on each system

Desktop, on a scratch home:

- [ ] With one frame included, and with a RAW and a JPEG included, the
      inspector says why Stack is off.
- [ ] While an export is written, Export… and Export Again are off, both
      bars say Exporting, and the Library's Export… does nothing.
- [ ] Switching between Recently Deleted and the Library's filters redraws
      cleanly, and leaving Recently Deleted comes back to All.
- [ ] In the grid, the frame list and Recently Deleted, ⇧-click selects a
      range, ⌘-click toggles, and ⌘-click on the only selected item leaves
      none.
- [ ] Settings' Choose… opens one dialog; a second click while it is up
      does nothing.
- [ ] A turntable grow sequence on a disk without room for it is refused
      with the no-room message.
- [ ] With Contribution on, picking a frame left out shows its toggle off
      and the frame plain while Contribution stays on, and picking a
      stacked frame shows its contribution again.
- [ ] Try a Sample Stack twice in a row: both stack.
- [ ] Recently Deleted sits in the Library's left bar, its tiles and header
      look right in light and dark, the notice's Undo brings a stack back,
      and a stack deleted, restored and opened again shows its renders and
      frames.

macOS:

- [ ] Filling the Library's disk while retouching, then quitting, asks
      first, and Cancel keeps the app open.
- [ ] Retouch on a Library on a network share feels as it does on a local
      disk.

Windows and Linux:

- [ ] Opening a file from the file manager while the app runs opens it in
      the running app and makes no new log file; a second launch hands its
      files to the first.
- [ ] A launch started while another app holds the Library is told it is
      in use (ADR-140).
- [ ] Try a Sample Stack from the installed bundle stacks (ADR-151).

Windows:

- [ ] An MSVC build (`/EHs /w /utf-8 /bigobj`) and the release workflow's
      LibRaw step pass, and `cargo test -p deptheos_dng --lib corrupt`
      passes.
- [ ] A panic's log shows the exe's load address.
- [ ] A `fault` crash's `.dmp` opens in WinDbg at the faulting thread with
      the tag's PDBs, and with Windows Error Reporting's LocalDumps on, its
      dump is in the diagnostics zip too.

iOS, on a device (the iPhone 15 Pro row for the memory and timing checks):

- [ ] With Contribution on, frames left out and stacked show as on the
      desktop.
- [ ] Try a Sample Stack twice: the menu shows Hornet, then Circuit Board,
      and each stacks (ADR-151); with the device nearly full it says why
      it can't.
- [ ] An edit to the frame list survives backgrounding the app at once and
      relaunching it.
- [ ] Batch's Add Subfolders of… with every found folder removed, and
      Include deeper folders flipped while a large parent is still being
      listed, each end with the list matching the choice.
- [ ] Retouch: Paint From's menu, ← and → stepping the source and the brush
      sliders work, and so do the viewer's layouts and the depth map.
- [ ] Add from Photos…, Add from Files… and Add Folder…, from the Frames
      list's +, the empty Frames list and the empty Library (a new stack
      there), each add their frames, and Cancel closes each.
- [ ] Select (from ••• and the iPhone sheet's head), Remove from List,
      Stack These and Done work; closing the sheet while selecting, opening
      another stack and reopening Frames shows nothing selected.
- [ ] Render Info, the Library grid and the Stack hint update when what
      they show changes, and a thumbnail that failed is tried again once
      its file changes.
- [ ] Tapping quickly through several renders in the strip lands on the
      last one tapped.
- [ ] A delete from a tile's menu, a batch card and the Renders list (low
      and tall sheet, and the iPad column) shows the notice, Undo brings it
      back, and the notice goes by itself.
- [ ] Recently Deleted opens from the iPhone's last row and the iPad
      sidebar with its thumbnails; Restore by button, swipe and Select
      works; Empty and Delete Library ask first, Delete Library's question
      counting Recently Deleted.
- [ ] Backgrounding mid-stack and mid-batch pauses or stops the work, and
      it comes back on return.
- [ ] A stack running in the background within its time shows its result
      on return; one whose time iOS takes back stops and says so, with no
      GPU error in the log.
- [ ] A lit depth map, a compare and a frame alone, left for the background
      and back, each load again with no alert.
- [ ] A large TIFF export started just before leaving the app, and a render
      dragged out of the Library, are written and offered on return.
- [ ] A sweep interrupted by a call or Control Center after a few frames
      asks Stack Them or Discard, and Stack Them stacks; one interrupted
      after a single frame says why and keeps nothing.
- [ ] A canceled bracket discards its frames, and a sweep on a nearly full
      device stops with its frames kept and asked about.
- [ ] A bracket from 90% with a 5% step at 20 frames shows and takes 3
      frames, each at its own focus.
- [ ] The camera's lens change, wait for exposure, a step's retake and the
      log's comparison of the two clocks behave as ADR-162 says, and so do
      the book and table layouts, the mirroring, and the Duo's flat fold.
- [ ] The memory floor and table in the iPhone 15 Pro's log match ADR-164.
- [ ] Retouch's memory footprint on the iPhone 15 Pro, paint's display
      levels included, stays under 100 MB.
- [ ] In Instruments, `stopPainting`
      (`ios/DeptheosMobile/Model/AppModel+Retouch.swift`) around
      `paint.paintedImage()` takes under 16 ms; over that, `canvas_image`
      (`crates/deptheos_ffi/src/paint.rs`) moves off the main thread.
- [ ] An Xcode archive's dSYM symbolizes an engine frame: `dwarfdump
      --uuid` lists it and `atos` names a `deptheos_ffi` function.

Tools:

- [ ] `cd web && npm run lab-assets -- radius depthmap wavelet` makes the
      docs figures.
- [ ] A tethered `tools/stackrig` run drives the camera and turntable
      through a sequence.

### 6. Publish

- [ ] Edit the draft's notes, which are `CHANGELOG.md`'s section for the
      version (what to try, how to send diagnostics:
      `https://deptheos.com/support/`), title it "Deptheos 0.x.y Preview"
      while at 0.x, and publish it as a normal release, not a pre-release:
      the update check asks for `/releases/latest`, which skips
      pre-releases. Running copies offer the update on their next start.
- [ ] Open the TestFlight build to the public link's group.
- [ ] Deploy the website if its pages changed (`just web-deploy`).

## Performance baselines

`just baselines` (`script/baselines.sh`) times three reference stacks with
the release CLI, sets the end-to-end cases use, read from the sample
frames' folder (`just baselines <folder>`, the folder `just e2e-sets` also
reads by default): canon1's 48 JPEGs with Pyramid to a TIFF (case
`canon1`), its 47 CR3s through LibRaw with Pyramid to a linear DNG
(`canon1-raw`), and its 46 linear DNGs (Adobe DNG Converter's) with
Wavelet to a 16-bit TIFF (`canon1-wavelet`), all at 8192 × 5464. Seconds
are wall time for the whole command, reading included; peak memory is
`/usr/bin/time -l`'s peak memory footprint on a Mac (the GPU's buffers
count, since its memory is shared), its maximum resident set elsewhere.

Made 2026-11-20 at `ad266dfd`, on a MacBook Pro with an Apple M5 Max (32
GPU cores, 36 GB), macOS 27.2:

| Stack | Frames | Seconds | Peak memory |
|---|---|---|---|
| canon1 JPEG, Pyramid | 48 | 2.9 | 6.6 GB |
| canon1 CR3, LibRaw, Pyramid | 47 | 8.5 | 6.7 GB |
| canon1 DNG, Wavelet | 46 | 4.6 | 4.6 GB |

A row more than about 10% slower or larger on the same Mac is a
regression to explain before the tag. On another machine, run it once on
the last release's tag first and compare with that. Replace the table
when a change makes a row better on purpose, with the date and commit.

## Versions and build numbers

One version for every platform: `version` in the root `Cargo.toml`
(`[workspace.package]`, read by `script/version.sh`). The desktop builds
and the command line take it from Cargo; the iOS app's `MARKETING_VERSION`
comes from it too, through `ios/Version.xcconfig`, which
`ios/build-xcframework.sh` writes and the Xcode project's configurations
are based on (it isn't checked in, and the project sets no version of its
own).

The build number only goes up: it is the release workflow's run number
(`BUILD_NUMBER`, read by `script/build-number.sh`), the Mac's
`CFBundleVersion` and the iOS app's `CURRENT_PROJECT_VERSION` alike. A
build made anywhere else is 0, so App Store Connect never takes a local
archive over a workflow's. The workflow's `ios` job archives the app; on a
tag, with the `ASC_API_KEY` secrets (below), it uploads it to TestFlight.
To upload by hand instead, `BUILD_NUMBER=<a number past the last upload>
ios/build-xcframework.sh`, then Archive in Xcode.

## Reading a crash

Release builds keep line tables (`[profile.release]` in `Cargo.toml`),
but not in what ships. A panic's log lists each frame's address with the
image and offset it falls in, and each image's load address. On macOS,
`atos -o deptheos-gui.dSYM/Contents/Resources/DWARF/deptheos-gui -l <load
address> <addresses>`; a `.ips` crash report symbolizes in Console with
the dSYM beside it. On Windows, open the dump in WinDbg or Visual Studio
with the tag's PDBs on the symbol path. Elsewhere, `llvm-symbolizer
--obj=<binary> <offsets>`. To make a crash to read, `docs/crash-testing.md`.

Actions → Release → Run workflow builds the same files without a release
(they are kept as workflow artifacts), for testing.

## Building locally

- macOS: `script/bundle-mac.sh` (or `--no-notarize`). Needs Xcode 27+ (ADR-102) and
  `brew install create-dmg`. It signs with the first "Developer ID
  Application" identity in the keychain (`MACOS_SIGN_IDENTITY` overrides),
  or ad-hoc when there is none, and notarizes with the notarytool profile
  `notarytool-password` (`NOTARY_PROFILE` overrides; making it is in
  One-time setup). If `xcodebuild` or the tests fail from the command
  line, point it at the full Xcode rather than the Command Line Tools:
  `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`.
- Windows: `script\bundle-windows.ps1`. Needs Visual Studio's C++ tools
  and Inno Setup 6 ([windows.md](windows.md)). It ships the C++ runtime
  DLLs next to the exe.
- Linux: `script/bundle-linux.sh [x86_64|arm64]` makes the `.deb`, the
  `.rpm` and the tarball for the machine's own architecture (no
  cross-compiling: the arm64 ones are built on arm64, such as an ARM
  Ubuntu VM in UTM). Needs the build packages in the README and
  [nfpm](https://nfpm.goreleaser.com/install/). Build on Ubuntu 22.04 for
  a release: the binaries need the build machine's glibc or newer.

Every one ships the command line tool (`deptheos`) with the app: inside
Deptheos.app on macOS (Deptheos ▸ Install Command Line Tool… links it into
`/usr/local/bin`), beside the exe on Windows (the installer can add it to
the user's PATH), and in `/usr/bin` from the Linux packages.

All of them write to `dist/`. Before a release, test the Windows installer in a
clean Hyper-V VM ([windows.md](windows.md)).

## One-time setup

### Developer ID certificate

1. developer.apple.com → Certificates → + → **Developer ID Application**
   (G2 Sub-CA). Make the CSR with Keychain Access → Certificate Assistant →
   Request a Certificate From a Certificate Authority.
2. Install the downloaded certificate (double-click). `security
   find-identity -v -p codesigning` should list "Developer ID Application:
   …". `script/bundle-mac.sh` signs with it.
3. For CI, export it with its private key from Keychain Access as a `.p12`
   with a password, then `base64 -i cert.p12 | pbcopy`.

### App Store Connect API key (notarization in CI)

App Store Connect → Users and Access → Integrations → App Store Connect
API → Team Keys → + with the **Developer** role. Note the key id and the
issuer id, and download the `.p8` (only once).

### Notarizing from this Mac

`script/bundle-mac.sh` notarizes with a keychain profile named
`notarytool-password`, made once with an app-specific password from
account.apple.com:

```sh
xcrun notarytool store-credentials "notarytool-password" \
  --apple-id <Apple ID> --team-id <team id> --password <app-specific password>
```

Apple's [Customizing the notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
explains the rest.

### GitHub secrets

In the public repo, Settings → Secrets and variables → Actions:

| Secret | Value |
|---|---|
| `MACOS_CERTIFICATE` | the base64 `.p12` |
| `MACOS_CERTIFICATE_PASSWORD` | its export password |
| `APPLE_API_KEY` | the `.p8` file's contents |
| `APPLE_API_KEY_ID` | the key id |
| `APPLE_API_ISSUER` | the issuer id |
| `ASC_API_KEY` | for TestFlight uploads: a team key with the **Admin** role (or App Manager with access to cloud-managed distribution certificates), the `.p8` file's contents |
| `ASC_API_KEY_ID` | its key id |

## Windows signing

Unsigned, the installer shows "Windows protected your PC" until the user
clicks More info → Run anyway. To sign with Azure Artifact Signing
($9.99/month; individuals in the US and Canada):

1. Make an Azure subscription with an *Individual* billing account in your
   legal name, register the `Microsoft.CodeSigning` provider and create a
   Basic Artifact Signing account.
2. Complete the individual identity validation and create a Public Trust
   certificate profile.
3. Add an app registration with a federated credential for the repo's
   tags, and give it the Certificate Profile Signer role.
4. In the workflow's Windows job (the comment marks the place): log in with
   `azure/login`, sign `target\release\deptheos-gui.exe` with
   `azure/artifact-signing-action`, and pass `/DSign` and
   `/Sdeptheos="<sign command> $f"` to iscc so the installer and its
   uninstaller are signed too (`packaging/windows/deptheos.iss`).

Signing does not silence SmartScreen at once: the warning goes after a few
weeks and some hundreds of installs build the certificate's reputation.

## App Store privacy answers

What App Store Connect's App Privacy asks, answered from what the iOS
app does; the website's privacy page (`web/src/pages/privacy/index.astro`) says
the same, and the two change together.

- **Data collection: Data Not Collected.** The app makes no network
  requests of its own (no URLSession in Swift, no HTTP crate in the
  engine's iOS build), has no analytics, advertising or crash-reporting
  SDK, and no account. Photos, the Library and the log stay on the
  device.
- **Tracking: No.**
- **Diagnostics.** Send Diagnostics… writes a zip on the device and
  opens the share sheet; the user chooses where it goes. Apple counts
  only data sent off the device for the developer to keep, so a file the
  user shares themselves is not collection. If support starts asking for
  the file as a matter of course, the careful answer becomes Diagnostics
  ▸ Other Diagnostic Data, not linked to the user, not used for tracking,
  for app functionality, and the privacy page changes with it.
- **Privacy Policy URL:** `https://deptheos.com/privacy/`. **Support
  URL:** `https://deptheos.com/support/`.
- **Privacy manifest** (`PrivacyInfo.xcprivacy`): the required-reason
  APIs only; no tracking domains.

## Identifiers

- macOS: `com.deptheos.Deptheos` (never change it: an update replaces the installed app by it).
- iOS: `com.deptheos.DeptheosApp` (`PRODUCT_BUNDLE_IDENTIFIER` in the Xcode project).
- Windows installer AppId: `CFDC2FBF-1867-4001-86A4-09C2B657C283` (never
  change it: Windows matches upgrades by it).
