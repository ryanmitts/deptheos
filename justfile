# Deptheos's commands, in one place. `just` lists them; `just <name>` runs
# one; `just -n <name>` prints what it would run without running it.
# Every recipe is a command you can also run by hand.

# List the commands
[private]
default:
    @just --list --unsorted

# ---------------------------------------------------------------- run

# Run the desktop app (release build)
[group('run')]
desktop:
    cargo run --release -p deptheos_gpui --bin deptheos-gui

# Run the desktop app, debug build with Verbose Logging's debug lines
[group('run')]
desktop-debug:
    RUST_LOG=debug cargo run -p deptheos_gpui --bin deptheos-gui

# Crash the desktop app on purpose, on a scratch home: `just crash panic`, `just crash fault release` (docs/crash-testing.md)
[group('run')]
crash kind profile="debug": (desktop-build profile)
    mkdir -p target/crash-home
    HOME="$PWD/target/crash-home" DEPTHEOS_CRASH={{quote(kind)}} target/{{profile}}/deptheos-gui

# Run the desktop app on `just crash`'s scratch home, to save what a crash left (docs/crash-testing.md)
[group('run')]
crash-home profile="debug": (desktop-build profile)
    mkdir -p target/crash-home
    HOME="$PWD/target/crash-home" target/{{profile}}/deptheos-gui

[private]
desktop-build profile:
    cargo build {{ if profile == "release" { "--release" } else if profile == "debug" { "" } else { error("profile is debug or release") } }} -p deptheos_gpui --bin deptheos-gui

# Run the command line tool: `just cli pyramid ~/shots/bee -o bee.tif`
[group('run')]
cli *args:
    cargo run --release -q -p deptheos_cli -- {{args}}

# Run the website at localhost:5173
[group('run')]
web:
    cd web && npm run dev

# Run the website reachable from a phone on this network
[group('run')]
web-lan:
    cd web && npm run dev:lan

# ---------------------------------------------------------------- test

# The fast tests: the batch machine, stackers, native codecs, iOS bridge, desktop, command line, end-to-end checker, website
[group('test')]
check: test-batch test-core test-native test-ffi test-desktop test-tools web-check

# The batch machine's table tests (milliseconds)
[group('test')]
test-batch:
    cargo test -p deptheos_app --lib batch

# The shared app layer, all of it
[group('test')]
test-app:
    cargo test -p deptheos_app --lib

# The stackers, image loading and saving, and the color render
[group('test')]
test-core:
    cargo test -p deptheos_core -p deptheos_render

# The native codecs: the DNG SDK and LibRaw
[group('test')]
test-native:
    cargo test -p deptheos_dng -p deptheos_raw

# The iOS engine's API
[group('test')]
test-ffi:
    cargo test -p deptheos_ffi

# The desktop app, on the GPU (seconds)
[group('test')]
test-desktop:
    cargo test -p deptheos_gpui

# The command line tool and the end-to-end checker
[group('test')]
test-tools:
    cargo test -p deptheos_cli -p deptheos_e2e

# One test or group by name, in any crate: `just test retouch`
[group('test')]
test name:
    cargo test --workspace -- {{name}}

# Accept the stackers' current golden outputs as their references (crates/deptheos_core/tests/goldens)
[group('test')]
bless-goldens:
    DEPTHEOS_BLESS_GOLDENS=1 cargo test -p deptheos_core --test integration output_is_pinned

# Write the pixels every draw site hands over, on screen and in exports, to a folder (crates/deptheos_app/src/engine/handoff_tests.rs)
[group('test')]
handoff-capture dir:
    DEPTHEOS_HANDOFF_WRITE={{quote(dir)}} cargo test --release -p deptheos_app --lib handoff_pixels -- --ignored --nocapture

# Check every draw site still hands over the pixels `just handoff-capture` wrote, bit for bit, on the machine that wrote them
[group('test')]
handoff-check dir:
    DEPTHEOS_HANDOFF_CHECK={{quote(dir)}} cargo test --release -p deptheos_app --lib handoff_pixels -- --ignored --nocapture

# The user-facing sentences: every message's id and wording, both apps
[group('test')]
test-words:
    cargo test -p deptheos_app --lib words
    cargo test -p deptheos_gpui --bin deptheos-gui words

# The benches, probes and iOS smoke run still build
[group('test')]
test-examples:
    cargo check -p deptheos_aligner -p deptheos_core -p deptheos_dng -p deptheos_gpu -p deptheos_raw --features examples --examples
    cargo check -p deptheos_app --features examples --examples
    cargo check -p deptheos_ffi --features examples --examples

# The website: types and tests
[group('test')]
web-check:
    cd web && npm run typecheck && npm test

# The website in a real browser (start `just web` first)
[group('test')]
web-smoke url="http://localhost:5173":
    cd web && npx tsx scripts/smoke.ts {{url}} && npx tsx scripts/touch-check.ts {{url}}

# The camera rig tool
[group('test')]
test-stackrig:
    cd tools/stackrig && swift test

# ---------------------------------------------------------------- end to end

# Real frames through the desktop app, each export checked: the smoke and full cases of e2e.toml, or those named (comma-separated)
[group('end to end')]
e2e case="":
    DEPTHEOS_E2E_CASE={{quote(case)}} cargo test -p deptheos_gpui --bin deptheos-gui e2e -- --ignored --nocapture

# The desktop's smoke cases: a few small ones, in a couple of minutes
[group('end to end')]
e2e-smoke:
    DEPTHEOS_E2E_TIER=smoke cargo test -p deptheos_gpui --bin deptheos-gui e2e -- --ignored --nocapture

# The desktop's cases of a tier or several (comma-separated): smoke, full, stress, converter
[group('end to end')]
e2e-tier tier:
    DEPTHEOS_E2E_TIER={{quote(tier)}} cargo test -p deptheos_gpui --bin deptheos-gui e2e -- --ignored --nocapture

# Build the end-to-end sets from the sample frames, each with its manifest; a set already there is kept
[group('end to end')]
e2e-sets samples="~/Desktop/sample_images" pcb="~/Pictures/PCB Sweep Raw/s050":
    script/e2e-sets.sh {{samples}} {{quote(pcb)}}

# A run's exports (kept by DEPTHEOS_E2E_OUT) in one folder to look through, each named for its case, platform and format
[group('end to end')]
e2e-review exports="~/DeptheosE2E/exports" review="~/DeptheosE2E/review":
    script/e2e-review.sh {{exports}} {{review}}

# Remove a run's kept exports and their review folder (never the sets or references), before a run and after signing off
[group('end to end')]
e2e-clean exports="~/DeptheosE2E/exports" review="~/DeptheosE2E/review":
    rm -rf {{quote(replace_regex(exports, "^~", home_directory()))}} {{quote(replace_regex(review, "^~", home_directory()))}}

# Real frames through the iOS app in the Simulator: one case of e2e.toml
[group('end to end')]
e2e-ios case="canon1":
    script/e2e-ios.sh {{quote(case)}}

# Accept the desktop's current pictures as the references: every desktop case, or those named (comma-separated)
[group('end to end')]
bless case="":
    DEPTHEOS_E2E_BLESS=1 DEPTHEOS_E2E_CASE={{quote(case)}} cargo test -p deptheos_gpui --bin deptheos-gui e2e -- --ignored --nocapture

# Accept the iOS app's current pictures as the references for a case
[group('end to end')]
bless-ios case="canon1":
    script/e2e-ios.sh {{quote(case)}} --bless

# List the end-to-end cases
[group('end to end')]
e2e-cases:
    @cargo run -q -p deptheos_e2e --bin e2e-check -- cases

# ---------------------------------------------------------------- tidy

# Format all the Rust
[group('tidy')]
fmt:
    cargo fmt --all

# Lint all the Rust, tests included, check its formatting, and check that no doc or comment names a file that doesn't exist
[group('tidy')]
lint: check-paths
    cargo clippy --workspace --tests
    cargo fmt --all --check

# Fail on a doc or comment naming a file that isn't in the repository (exceptions in script/check-paths.allow)
[group('tidy')]
check-paths:
    python3 script/check-paths.py

# Every lifecycle in the code, by its `Lifecycle:` doc line
[group('tidy')]
lifecycles:
    git grep -n 'Lifecycle:' -- crates ios web/src tools

# ---------------------------------------------------------------- iOS

# Rebuild the Rust engine for iOS (after any engine change)
[group('ios')]
ios-engine:
    PROFILE=dev ios/build-xcframework.sh

# Build the iOS app for the Simulator
[group('ios')]
ios-build:
    xcodebuild -project ios/DeptheosMobile.xcodeproj -scheme DeptheosMobile \
        -destination 'generic/platform=iOS Simulator' CODE_SIGNING_ALLOWED=NO build -quiet

# The viewer's controls hide, zoom and come back with a camera faked at the top and either side (LayoutTests)
[group('ios')]
ios-layout device="iPad Pro 13-inch (M5)":
    xcodebuild -project ios/DeptheosMobile.xcodeproj -scheme DeptheosMobile \
        -destination 'platform=iOS Simulator,name={{device}}' \
        -only-testing:DeptheosMobileUITests/LayoutTests test -quiet

# ---------------------------------------------------------------- release

# The workspace version
[group('release')]
version:
    @script/version.sh

# Build Deptheos.app and its DMG, signed and notarized: `just bundle-mac --no-notarize`
[group('release')]
bundle-mac *args:
    script/bundle-mac.sh {{args}}

# Build the Linux tarball, .deb and .rpm
[group('release')]
bundle-linux arch="":
    script/bundle-linux.sh {{arch}}

# Time three reference stacks with the release CLI, with their peak memory (docs/releasing.md)
[group('release')]
baselines sets="~/Desktop/sample_images":
    script/baselines.sh {{sets}}

# The full source with the LibRaw submodule, as a release attaches it: dist/Deptheos-<version>-source.tar.gz
[group('release')]
source-tarball:
    script/source-tarball.sh

# What lightcraft changed in the files the sliders take from (ADR-137)
[group('tidy')]
upstream *rev:
    script/upstream.sh {{rev}}

# Regenerate THIRD_PARTY_NOTICES.txt
[group('release')]
notices:
    script/notices.sh

# Check THIRD_PARTY_NOTICES.txt is up to date
[group('release')]
notices-check:
    script/notices.sh --check

# Check the bundled sample stacks are here whole
[group('release')]
samples-check:
    script/sample-check.sh

# Build the website
[group('release')]
web-build:
    cd web && npm run build

# Deploy the website
[group('release')]
web-deploy:
    cd web && npm run deploy

# Apply .github/labels.tsv to the public repository's issue labels (makes or updates; removes none)
[group('release')]
labels repo="ryanmitts/deptheos":
    #!/usr/bin/env bash
    set -euo pipefail
    grep -v '^#' .github/labels.tsv | while IFS=$'\t' read -r name color description; do
        [[ -n "$name" ]] || continue
        gh label create "$name" --repo {{quote(repo)}} --color "$color" --description "$description" --force </dev/null
    done
