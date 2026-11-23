#!/usr/bin/env bash
# The iOS end-to-end test: a set of real frames through the app in the
# Simulator (ios/DeptheosMobileUITests: Stack, Export as JPEG and TIFF,
# Retouch, Export), then every file it exported checked by e2e-check, the
# same checks the desktop's test makes (crates/deptheos_e2e).
#
# Usage: script/e2e-ios.sh CASE [--bless]   (or `just e2e-ios CASE`)
#   CASE     a case of e2e.toml that runs on iOS: its set (its `ios_set`,
#            shrunk to what the Simulator's GPU takes, when it has one),
#            method, settings, exports and whether it paints
#   --bless  sign this run's pictures off as the case's references
#
# Environment:
#   DEPTHEOS_E2E_DEVICE      the Simulator (default "iPhone 18 Pro")
#   DEPTHEOS_E2E_OUT         where the exports are kept (default: a temporary folder)
#   DEPTHEOS_E2E_REFERENCES  the pictures signed off (default ~/DeptheosE2E/references)
#   DEPTHEOS_E2E_SETS        overrides e2e.toml's folder of sets
#
# The app is installed fresh, so its Library holds only this run. After a
# change to the Rust engine, run `just ios-engine` first. The frames are
# copied out of protected folders (Desktop, Documents) first, where the
# Simulator's app can't read them.
set -euo pipefail

cd "$(dirname "$0")/.."

case_name=""
bless=()
while (($#)); do
    case "$1" in
        --bless) bless=(--bless) ;;
        -*) echo "unknown option $1" >&2; exit 2 ;;
        *) case_name="$1" ;;
    esac
    shift
done
[[ -n "$case_name" ]] || { sed -n '2,25p' "$0" >&2; exit 2; }

echo "== checker"
cargo build -q -p deptheos_e2e
json=$(target/debug/e2e-check case "$case_name")
field() { python3 -c "import json,sys; v=json.loads(sys.argv[1])[sys.argv[2]]; print(' '.join(v) if isinstance(v,list) else v)" "$json" "$1"; }
[[ " $(field on) " == *" ios "* ]] || { echo "case $case_name doesn't run on iOS (its \`on\` in e2e.toml)" >&2; exit 2; }
set_dir=$(field setDir)
[[ -d "$set_dir" ]] || { echo "no frames at $set_dir" >&2; exit 2; }
name="$case_name"

device="${DEPTHEOS_E2E_DEVICE:-iPhone 18 Pro}"
out="${DEPTHEOS_E2E_OUT:-$(mktemp -d -t deptheos-e2e)}/$name/ios"
references="${DEPTHEOS_E2E_REFERENCES:-$HOME/DeptheosE2E/references}/$case_name"
derived="${TMPDIR:-/tmp}/deptheos-e2e-build"

rm -rf "$out"
frames="$out/frames/$(basename "$set_dir")"
mkdir -p "$frames" "$out/exports"
# The frames the checker reads (not a folder's Thumbs.db or a zip), as
# e2e-check lists them.
python3 -c "import json,sys; print('\0'.join(json.loads(sys.argv[1])['frames']), end='')" "$json" |
    xargs -0 -I{} cp -p {} "$frames/"

echo "== app and test, for $device"
xcodebuild build-for-testing -quiet \
    -project ios/DeptheosMobile.xcodeproj -scheme DeptheosMobile \
    -destination "platform=iOS Simulator,name=$device" \
    -derivedDataPath "$derived" CODE_SIGNING_ALLOWED=NO

# A fresh install: the Library holds only this run.
udid=$(xcrun simctl list devices available -j | python3 -c "
import json, sys
for runtime in json.load(sys.stdin)['devices'].values():
    for d in runtime:
        if d['name'] == sys.argv[1]:
            print(d['udid']); sys.exit()
" "$device")
[[ -n "$udid" ]] || { echo "no Simulator named $device" >&2; exit 2; }
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl uninstall "$udid" com.deptheos.DeptheosApp 2>/dev/null || true

echo "== the test, on $name"
TEST_RUNNER_DEPTHEOS_E2E_CASE="$json" \
TEST_RUNNER_DEPTHEOS_E2E_FRAMES="$frames" \
TEST_RUNNER_DEPTHEOS_E2E_EXPORTS="$out/exports" \
xcodebuild test-without-building -quiet \
    -project ios/DeptheosMobile.xcodeproj -scheme DeptheosMobile \
    -destination "id=$udid" \
    -derivedDataPath "$derived" \
    -resultBundlePath "$out/result.xcresult" \
    -only-testing:DeptheosMobileUITests/EndToEndTests

echo "== the exports"
target/debug/e2e-check notes "$frames" "$out/exports" --case "$case_name" --references "$references" ${bless[@]+"${bless[@]}"}
echo "exports in $out/exports"
