#!/usr/bin/env bash
# The end-to-end tests' sets (e2e.toml's `set`s), built from a folder of
# sample frames into e2e.toml's folder of sets, each with its manifest
# (`e2e-check manifest`) and made read-only, so a reference is always held
# to the frames it was made from (crates/deptheos_e2e, `Set::open_checked`).
#
# Usage: script/e2e-sets.sh [SAMPLES [PCB]]   (or `just e2e-sets`)
#   SAMPLES  the sample frames (default ~/Desktop/sample_images)
#   PCB      a long sweep of a circuit board (default
#            "~/Pictures/PCB Sweep Raw/s050", 199 frames)
#
# Every set is a whole scene, every frame of it, so each stack is sharp
# throughout: a set differs from another of its scene only in the frames'
# format, color space or size. The scenes: canon1 (a spider, 46 frames),
# hornet (66), the PCB sweep (199) and an OM-1's hornet (63) and
# screwdriver (250). canon1 and hornet each have a 16-bit TIFF master
# (canon1's TIFFs, in sRGB; hornet's linear DNGs rendered by sips, in
# Display P3) from which its JPEG, PNG and gray sets are made, and its
# camera RAW and DNG sets as the camera and converter wrote them.
#
# Environment:
#   DEPTHEOS_E2E_SETS  where the sets go (default ~/DeptheosE2E/sets)
#
# A set already there is kept as it is, never rebuilt: to move the tests
# to another Mac, copy the sets folder with its references. Sets copied
# from the samples are APFS clones (`cp -c`), taking no space until one
# side changes. Derived ones (gray, PNG) are made with sips, whose output
# can differ between macOS versions, so a set rebuilt elsewhere is new
# frames and its cases need signing off again.
set -euo pipefail

cd "$(dirname "$0")/.."

samples="${1:-$HOME/Desktop/sample_images}"
pcb="${2:-$HOME/Pictures/PCB Sweep Raw/s050}"
# A path given as "~/…" (quoted, as the recipe passes it) is the home's.
samples="${samples/#\~/$HOME}"
pcb="${pcb/#\~/$HOME}"
sets="${DEPTHEOS_E2E_SETS:-$HOME/DeptheosE2E/sets}"
[[ -d "$samples" ]] || { echo "no samples at $samples" >&2; exit 2; }
[[ -d "$pcb" ]] || { echo "no PCB sweep at $pcb" >&2; exit 2; }

cargo build -q -p deptheos_e2e
check=target/debug/e2e-check

profiles=/System/Library/ColorSync/Profiles

# Build set $1 with the function $2 (and its arguments), which fills the
# folder it is given last. Built beside the set, then moved there whole,
# so a failed build leaves nothing behind.
build() {
    local name="$1" dir="$sets/$1"
    shift
    if [[ -e "$dir" ]]; then
        echo "kept   $name"
        return
    fi
    local partial="$dir.partial"
    rm -rf "$partial"
    mkdir -p "$partial"
    "$@" "$partial"
    mv "$partial" "$dir"
    "$check" manifest "$dir" >/dev/null
    chmod a-w "$dir"/* "$dir"
    echo "built  $name ($(find "$dir" -type f ! -name manifest.json | wc -l | tr -d ' ') frames)"
}

# The files of a folder (a sample folder's name, or a path) whose names
# match, cloned.
clone() {
    local from="$1" into="${*: -1}"
    [[ "$from" == /* ]] || from="$samples/$from"
    for pattern in "${@:2:$#-2}"; do
        # shellcheck disable=SC2086
        cp -c "$from"/$pattern "$into/"
    done
}

# A scene's master: 16-bit TIFFs. hornet's are its DNGs as sips renders
# them, in Display P3: converting them to sRGB too (`--matchTo`) would
# make them 8-bit.
hornet_master() {
    for dng in "$samples"/hornet_dng/*.dng; do
        sips -s format tiff "$dng" --out "$1/$(basename "$dng" .dng).tif" >/dev/null
    done
}

# Each of a master's frames made into another format: from_master SCENE KIND INTO
from_master() {
    local master="$sets/tiff16/$1" kind="$2" into="$3"
    for tif in "$master"/*.tif; do
        local stem
        stem=$(basename "$tif" .tif)
        case "$kind" in
            jpeg) sips -s format jpeg -s formatOptions 95 "$tif" --out "$into/$stem.jpg" ;;
            gray) sips --matchTo "$profiles/Generic Gray Gamma 2.2 Profile.icc" \
                      -s format jpeg -s formatOptions 95 "$tif" --out "$into/$stem.jpg" ;;
            png8) "$check" eight "$tif" "$into/$stem.png" ;;
            png16) sips -s format png "$tif" --out "$into/$stem.png" ;;
        esac >/dev/null
    done
}

# A master's frames alternately as 8-bit JPEGs and its 16-bit TIFFs: the
# stack takes the deepest frame's bits. A JPEG first, in the app's order
# and the checker's.
mixed_depth() {
    local master="$sets/tiff16/$1" into="$2" i=0
    for tif in "$master"/*.tif; do
        local stem
        stem=$(basename "$tif" .tif)
        if ((i % 2 == 0)); then
            sips -s format jpeg -s formatOptions 95 "$tif" --out "$into/$stem.jpg" >/dev/null
        else
            cp -c "$tif" "$into/"
        fi
        i=$((i + 1))
    done
}

# A sample folder's camera RAW as mosaic DNGs, by Adobe DNG Converter
# without -l, which keeps the mosaic (the SDK reads their metadata, LibRaw
# demosaics them, ADR-131): mosaic_dngs FOLDER INTO
mosaic_dngs() {
    local converter="/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"
    [[ -x "$converter" ]] || { echo "mosaic DNGs need Adobe DNG Converter" >&2; return 1; }
    "$converter" -c -d "$2" "$samples/$1"/*.ORF >/dev/null 2>&1 || true
    local made
    made=$(find "$2" -name '*.dng' | wc -l)
    ((made > 0)) || { echo "Adobe DNG Converter made no DNGs of $1" >&2; return 1; }
}

# canon1: the 46 frames as the camera and its editor made them (JPEG,
# CR3, 16-bit TIFF, linear DNG), and from the TIFFs in gray and mixed
# with JPEGs.
build jpeg/canon1 clone jpeg/canon1 '*.jpg'
build cr3/canon1 clone canon '*.CR3'
build tiff16/canon1 clone tiff/canon1 '*.tif'
build dng-linear/canon1 clone canon_dng '*.dng'
build gray/canon1 from_master canon1 gray
build mixed-depth/canon1 mixed_depth canon1

# hornet: the 66 frames as a master, in each format, and as the camera
# wrote them.
build tiff16/hornet hornet_master
build p3/hornet from_master hornet jpeg
build png8/hornet from_master hornet png8
build png16/hornet from_master hornet png16
build cr3/hornet clone hornet2 '*.CR3'

# The PCB sweep, all 199 frames: a long stack at full size.
build jpeg/pcb-s050 clone "$pcb" '*.JPG'

# Another maker's camera RAW (OM System OM-1): a hornet, 63 frames, and
# the same frames as mosaic DNGs; a screwdriver, 250 frames, the long RAW
# stack.
build orf/om-hornet clone om_hornet_2 '*.ORF'
build dng-mosaic/om-hornet mosaic_dngs om_hornet_2
build orf/om-screwdriver clone om_screwdriver '*.ORF'

# Handheld frames the aligner moves (canon1's, 2048 a side).
build jpeg/unaligned clone jpeg/spider_unaligned '*.jpg'

# A set shrunk for the Simulator, whose GPU takes at most about 16.7 MP
# (ADR-110): each frame 4096 on its long side, a quarter of a 45 MP
# frame. Shrunk, not cropped, so a handheld bracket's drift stays the same
# share of the frame. 16-bit TIFFs by e2e-check (sips writes 8 bits when
# it resamples), JPEGs by sips, which keeps their tags: sim SET INTO
sim() {
    local from="$sets/$1" into="$2"
    for frame in "$from"/*; do
        case "$frame" in
            *.tif) "$check" fit "$frame" "$into/$(basename "$frame")" 4096 ;;
            *.jpg | *.JPG) sips -Z 4096 "$frame" --out "$into/$(basename "$frame")" >/dev/null ;;
            *manifest.json) ;;
            *) echo "sim: $frame isn't a JPEG or TIFF" >&2; return 1 ;;
        esac
    done
}
build sim/jpeg/canon1 sim jpeg/canon1
build sim/p3/hornet sim p3/hornet
build sim/tiff16/canon1 sim tiff16/canon1

# A set's camera RAW for the Simulator: linear DNGs by Adobe DNG Converter,
# `-lossy -side 4128` (half a 45 MP frame's long side, JPEG XL at 16
# bits), still camera RAW so a kelvin white applies: sim_dngs SET INTO
sim_dngs() {
    local converter="/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"
    [[ -x "$converter" ]] || { echo "the Simulator's DNGs need Adobe DNG Converter" >&2; return 1; }
    "$converter" -lossy -side 4128 -d "$2" "$sets/$1"/*.CR3 >/dev/null 2>&1 || true
    local made wanted
    made=$(find "$2" -name '*.dng' | wc -l)
    wanted=$(find "$sets/$1" -name '*.CR3' | wc -l)
    ((made == wanted)) || { echo "Adobe DNG Converter made $made of $1's $wanted DNGs" >&2; return 1; }
}
build sim/dng/canon1 sim_dngs cr3/canon1
