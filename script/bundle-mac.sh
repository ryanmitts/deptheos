#!/usr/bin/env bash
# Build Deptheos.app and its installer DMG: release build, bundle, sign,
# then (with a Developer ID) notarize and staple the app, and the DMG. The
# app carries the command line tool too (Contents/MacOS/deptheos), which the app's
# Install Command Line Tool… links onto the PATH.
#
# Usage: script/bundle-mac.sh [--no-notarize]
#
# Environment (all optional):
#   MACOS_SIGN_IDENTITY  codesign identity. Default: the first
#                        "Developer ID Application" in the keychain, else
#                        ad-hoc ("-"), which runs only on this Mac.
#   NOTARY_PROFILE       notarytool keychain profile (default:
#                        notarytool-password), stored once with
#                        xcrun notarytool store-credentials <profile> …
#   BUILD_NUMBER         CFBundleVersion, set by the release workflow
#                        (default 0: script/build-number.sh).
#
# Needs Xcode 27 or newer (the macOS 27 SDK, ADR-102) and create-dmg
# (`brew install create-dmg`). Output goes to dist/: the DMG, and the
# binaries' dSYMs in dist/symbols.
set -euo pipefail

cd "$(dirname "$0")/.."

notarize=1
for arg in "$@"; do
  case "$arg" in
    --no-notarize) notarize=0 ;;
    *) echo "usage: $0 [--no-notarize]" >&2; exit 2 ;;
  esac
done

version=$(script/version.sh)
script/sample-check.sh
# The oldest macOS the app runs on: the build's deployment target, the icon's and Info.plist's.
min_macos=27.0
build=$(script/build-number.sh)
identity="${MACOS_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  identity=$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)
fi
if [[ -z "$identity" ]]; then
  echo "warning: no Developer ID Application identity; signing ad-hoc, skipping notarization" >&2
  identity="-"
  notarize=0
fi

app_dir=dist/dmg/Deptheos.app
dmg="dist/Deptheos-${version}-macos-arm64.dmg"

echo "==> Building deptheos-gui and deptheos ${version} (${build})"
# The C/C++ libraries (cc) and Rust both read this.
export MACOSX_DEPLOYMENT_TARGET=$min_macos
cargo build --release --locked -p deptheos_gpui -p deptheos_cli --target aarch64-apple-darwin

# The release profile's line tables stay in the build's object files: a
# dSYM of each binary keeps them, for symbolizing a crash (the release
# workflow keeps dist/symbols per tag). Made before signing, which leaves
# the binaries' UUIDs as they are.
echo "==> Symbols"
rm -rf dist/symbols
mkdir -p dist/symbols
for bin in deptheos-gui deptheos; do
  dsymutil "target/aarch64-apple-darwin/release/$bin" -o "dist/symbols/$bin.dSYM"
done

echo "==> Assembling ${app_dir}"
rm -rf dist/dmg "$dmg"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
cp target/aarch64-apple-darwin/release/deptheos-gui target/aarch64-apple-darwin/release/deptheos \
  "$app_dir/Contents/MacOS/"
sed -e "s/@VERSION@/${version}/" -e "s/@BUILD@/${build}/" -e "s/@MIN_MACOS@/${min_macos}/" \
  packaging/macos/Info.plist > "$app_dir/Contents/Info.plist"
printf 'APPL????' > "$app_dir/Contents/PkgInfo"
# The licenses: the About window opens these (deptheos_gpui::about).
cp LICENSE.md TRADEMARKS.md THIRD_PARTY_NOTICES.txt "$app_dir/Contents/Resources/"
# The sample stacks, which Try a Sample Stack copies into the Library
# folder (deptheos_gpui's app/sample.rs): their folders, not samples'
# README. Before signing, which seals everything in Resources; without
# extended attributes, which codesign refuses there.
mkdir -p "$app_dir/Contents/Resources/samples"
for sample in hornet pcb; do
  ditto --norsrc --noextattr --noqtn "samples/$sample" "$app_dir/Contents/Resources/samples/$sample"
done
# Assets.car for macOS 26's layered icon, and DeptheosAppIcon.icns for
# earlier macOS.
xcrun actool DeptheosAppIcon.icon --compile "$app_dir/Contents/Resources" \
  --app-icon DeptheosAppIcon --include-all-app-icons --platform macosx \
  --target-device mac --minimum-deployment-target "$min_macos" \
  --output-partial-info-plist dist/icon-partial.plist > /dev/null
rm -f dist/icon-partial.plist

echo "==> Signing with ${identity}"
# Hardened runtime, no entitlements: Metal compiles the run-time shaders in
# its own process, so no JIT exception is needed. The command line tool
# first: signing the bundle signs only its main executable, and
# notarization refuses an unsigned one beside it.
sign_args=(--force --options runtime --sign "$identity")
if [[ "$identity" != "-" ]]; then
  sign_args+=(--timestamp)
fi
codesign "${sign_args[@]}" --identifier com.deptheos.Deptheos.cli "$app_dir/Contents/MacOS/deptheos"
codesign "${sign_args[@]}" "$app_dir"
codesign --verify --strict --verbose=2 "$app_dir"
codesign --verify --strict --verbose=2 "$app_dir/Contents/MacOS/deptheos"

if [[ "$notarize" == 1 ]]; then
  # The app is notarized and stapled by itself before it goes into the DMG:
  # the DMG's own ticket does not follow an app copied out of it, and a
  # first launch offline has no way to ask for one.
  echo "==> Notarizing the app"
  app_zip=$(mktemp -d)/Deptheos.zip
  ditto -c -k --keepParent "$app_dir" "$app_zip"
  xcrun notarytool submit "$app_zip" \
    --keychain-profile "${NOTARY_PROFILE:-notarytool-password}" --wait
  rm -f "$app_zip"
  xcrun stapler staple "$app_dir"
  xcrun stapler validate "$app_dir"
fi

echo "==> Making ${dmg}"
dmg_args=(
  --volname "Deptheos"
  --window-pos 200 120
  --window-size 800 400
  --icon-size 100
  --icon "Deptheos.app" 200 190
  --hide-extension "Deptheos.app"
  --app-drop-link 600 185
)
if [[ "$identity" != "-" ]]; then
  dmg_args+=(--codesign "$identity")
fi
if [[ "$notarize" == 1 ]]; then
  # create-dmg submits, waits, then staples the ticket to the DMG.
  dmg_args+=(--notarize "${NOTARY_PROFILE:-notarytool-password}")
fi
create-dmg "${dmg_args[@]}" "$dmg" dist/dmg

if [[ "$notarize" == 1 ]]; then
  xcrun stapler validate "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose "$dmg"
fi
echo "==> ${dmg}"
