#!/bin/sh
# Checks the iOS app's decoder for standard images (BitmapDecode) against
# Core Image's render, on this Mac: the same pixels, as stored, for JPEG in
# all eight EXIF orientations, HEIC and 16-bit PNG and TIFF, then each one's
# peak memory on a sample frame. See script/check-bitmap-decode.swift.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
frame=${1:-$root/samples/hornet/hornet-01.jpg}
# Top-level code beside another file has to be main.swift.
cp "$root/script/check-bitmap-decode.swift" "$work/main.swift"
swiftc -O -o "$work/check" "$work/main.swift" "$root/ios/DeptheosMobile/Engine/BitmapDecode.swift"
"$work/check" compare "$frame" "$work"
for decoder in ci cg; do "$work/check" peak "$decoder" "$frame"; done
for decoder in ci cg; do "$work/check" peak "$decoder" "$work/sixteen.tif"; done
