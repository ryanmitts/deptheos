#!/bin/bash
# Builds the engine (crates/deptheos_ffi) for iOS and packages it for the
# app: DeptheosEngine/DeptheosFFI.xcframework (static library + C header +
# module map, device and simulator) and the generated Swift bindings in
# DeptheosEngine/Sources/DeptheosEngine/. Run it after any Rust change;
# the Xcode project does not call cargo itself.
#
#   ios/build-xcframework.sh            # release (what the app should use)
#   PROFILE=dev ios/build-xcframework.sh  # faster to build, no LTO
#
# It also writes ios/Version.xcconfig, the app's version and build number
# (BUILD_NUMBER, as script/build-number.sh reads it).
#
# Needs Xcode (not just the command-line tools) and the Rust targets:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PACKAGE="$ROOT/ios/DeptheosEngine"
PROFILE="${PROFILE:-release}"
TARGET_DIR_NAME=$([ "$PROFILE" = dev ] && echo debug || echo "$PROFILE")
# Must match the app's deployment target, or the linker warns that the
# library was built for a newer iOS. The native libraries (DNG SDK, libjxl)
# are compiled for it too.
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-26.0}"

# A shell whose xcode-select points at the command-line tools has no iOS SDK.
if ! xcrun --sdk iphoneos --show-sdk-path >/dev/null 2>&1; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

cd "$ROOT"

# The app's version and build number, for the Xcode project (its project
# configurations are based on this file): the workspace version every
# platform has (script/version.sh) and the build number the release
# workflow sets (script/build-number.sh; 0 for a build made elsewhere).
version=$(script/version.sh)
build=$(script/build-number.sh)
cat > ios/Version.xcconfig <<XCCONFIG
// Written by ios/build-xcframework.sh; not checked in.
MARKETING_VERSION = ${version}
CURRENT_PROJECT_VERSION = ${build}
XCCONFIG
echo "==> Version ${version} (${build})"

# The app's threads (ADR-143): engine calls are the bridge's async ones,
# and the app's own blocking work goes through Blocking. Neither runs in a
# detached task or on a global queue; only the bookmark resolver does,
# which waits on the file provider with a deadline (Bookmarks.swift), and
# the memory-pressure source, which hears iOS's warnings off the main
# thread and hands them to the engine without waiting
# (MemoryPressure.swift).
if grep -rnE 'Task\.detached|DispatchQueue\.global' ios/DeptheosMobile --include='*.swift' \
    | grep -vE '^ios/DeptheosMobile/(Engine/Blocking|Model/Bookmarks|Model/MemoryPressure)\.swift:' \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//'; then
    echo "error: Task.detached or DispatchQueue.global outside Blocking (ADR-143)" >&2
    exit 1
fi

# Every geometry reading goes through `measure`, which watches for a
# layout loop (ADR-169).
if grep -rn 'onGeometryChange' ios/DeptheosMobile --include='*.swift' \
    | grep -vE '^ios/DeptheosMobile/Views/Measure\.swift:' \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//'; then
    echo "error: onGeometryChange outside Measure.swift; use measure (ADR-169)" >&2
    exit 1
fi

TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim)
for target in "${TARGETS[@]}"; do
    echo "==> cargo build ($PROFILE) for $target"
    cargo build --profile "$PROFILE" -p deptheos_ffi --lib --target "$target"
done

lib() { echo "${CARGO_TARGET_DIR:-$ROOT/target}/$1/$TARGET_DIR_NAME/libdeptheos_ffi.a"; }

GEN="$(mktemp -d)"
trap 'rm -rf "$GEN"' EXIT
mkdir -p "$GEN/include" "$GEN/swift"
bindgen() {
    cargo run -q -p deptheos_ffi --features bindgen --bin uniffi-bindgen-swift -- "$@"
}
echo "==> Swift bindings"
LIB="$(lib aarch64-apple-ios-sim)"
bindgen --swift-sources "$LIB" "$GEN/swift"
bindgen --headers "$LIB" "$GEN/include"
# A library (not framework) XCFramework: a plain module, named as the
# generated Swift imports it.
bindgen --modulemap --module-name deptheos_ffiFFI --modulemap-filename module.modulemap "$LIB" "$GEN/include"

echo "==> XCFramework"
rm -rf "$PACKAGE/DeptheosFFI.xcframework"
args=()
for target in "${TARGETS[@]}"; do
    args+=(-library "$(lib "$target")" -headers "$GEN/include")
done
xcodebuild -create-xcframework "${args[@]}" -output "$PACKAGE/DeptheosFFI.xcframework" >/dev/null

# The folder holds only generated bindings: clear the last run's, so a
# renamed file can't linger beside its replacement.
rm -f "$PACKAGE/Sources/DeptheosEngine/"*.swift
cp "$GEN/swift/"*.swift "$PACKAGE/Sources/DeptheosEngine/"
echo "==> Done: $PACKAGE"
ls -la "$PACKAGE/Sources/DeptheosEngine/"
du -sh "$PACKAGE/DeptheosFFI.xcframework"
