#!/usr/bin/env bash
# Prints the build number, the one rule for both Apple apps: the Mac's
# CFBundleVersion (script/bundle-mac.sh) and the iOS app's
# CURRENT_PROJECT_VERSION (ios/build-xcframework.sh). The release workflow
# sets BUILD_NUMBER to its run number, which only goes up, so every build it
# makes is numbered past the last; App Store Connect refuses a build whose
# number isn't. A build made anywhere else is 0, never newer than a release.
# The version beside it is script/version.sh's, on every platform.
set -euo pipefail
build="${BUILD_NUMBER:-0}"
[[ "$build" =~ ^[0-9]+$ ]] || { echo "BUILD_NUMBER must be a whole number, not \"$build\"" >&2; exit 1; }
echo "$build"
