#!/usr/bin/env bash
# The exports of an end-to-end run, kept by DEPTHEOS_E2E_OUT, gathered into
# one folder to look through, each named for what it is:
#
#   canon1 - desktop - stack - jpg 8-bit.jpg
#   tiff16 - ios - retouched - tif 16-bit.tif
#
# Usage: script/e2e-review.sh [EXPORTS [REVIEW]]   (or `just e2e-review`)
#   EXPORTS  the run's folder (default ~/DeptheosE2E/exports)
#   REVIEW   where they go (default ~/DeptheosE2E/review), made anew
#
# Only the exports are taken: a case's desktop/stack and desktop/retouched
# files, and its ios/exports files by the note the iOS test wrote beside
# each (a retouched one names the frame it was painted from); never the
# copies of the frames an iOS run keeps beside them. The files are APFS
# clones, taking no space. Two exports that would get one name stop it.
set -euo pipefail

exports="${1:-$HOME/DeptheosE2E/exports}"
review="${2:-$HOME/DeptheosE2E/review}"
[[ -d "$exports" ]] || { echo "no exports at $exports" >&2; exit 2; }
rm -rf "$review"
mkdir -p "$review"

# Copy FILE in as CASE - PLATFORM - KIND - EXT BITS-bit.EXT
take() {
    local file="$1" case_name="$2" platform="$3" kind="$4" ext bits name
    ext="${file##*.}"
    bits=$(sips -g bitsPerSample "$file" 2>/dev/null | awk '/bitsPerSample/ {print $2}')
    name="$case_name - $platform - $kind - $ext ${bits:-?}-bit.$ext"
    [[ -e "$review/$name" ]] && { echo "two exports would be \"$name\"" >&2; exit 1; }
    cp -c "$file" "$review/$name"
}

count=0
for case_dir in "$exports"/*/; do
    case_name=$(basename "$case_dir")
    for kind in stack retouched; do
        for file in "$case_dir"desktop/"$kind"/*; do
            [[ -f "$file" ]] || continue
            take "$file" "$case_name" desktop "$kind"
            count=$((count + 1))
        done
    done
    for note in "$case_dir"ios/exports/*.json; do
        [[ -f "$note" ]] || continue
        file="${note%.json}"
        kind=$(python3 -I -c 'import json,sys; print("retouched" if json.load(open(sys.argv[1]))["paintedFrom"] else "stack")' "$note")
        take "$file" "$case_name" ios "$kind"
        count=$((count + 1))
    done
done
echo "$count exports in $review"
