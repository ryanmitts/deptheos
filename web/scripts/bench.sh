#!/usr/bin/env bash
# Rerun the site's stacking times on this machine, into
# web/measured/benchmarks/<machine>-<stack>-<format>.json:
#
#   web/scripts/bench.sh <machine> <pcb1 JPEGs> <pcb1 CR3s> <hornet JPEGs> [bench flags]
#
# e.g. web/scripts/bench.sh m5-max ~/stacks/jpeg/pcb1 ~/stacks/PCB/115EOSR5 ~/stacks/jpeg/hornet
#
# Flags after the folders go to every bench, e.g. --machine "Ryzen 9 9950X3D + RTX 5090".
# Every time is the app's path (--live) and the median of five rounds, each
# round timing every configuration once, with 30 s idle before each run so a
# laptop is as cool as between a photographer's stacks (without it, an M5
# Max's 200-frame JPEG stack slows from 5.2 s to 9 s over five minutes).
# The CR3s are timed through LibRaw, decoding included, with the Adobe
# Standard profile. pcb1-dng (the RAW docs' conversion cost) needs Adobe DNG
# Converter.
# Run plugged in, with nothing else busy. Takes about 35 minutes.
set -euo pipefail

if [ $# -lt 4 ]; then
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi
machine=$1 pcb1_jpeg=$2 pcb1_cr3=$3 hornet_jpeg=$4
shift 4
extra=("$@")
repo=$(cd "$(dirname "$0")/../.." && pwd)
out="$repo/web/measured/benchmarks"

cargo build --release -p deptheos_cli --manifest-path "$repo/Cargo.toml"
bin="$repo/target/release/deptheos"

run() {
  local name=$1 frames=$2
  shift 2
  echo "== $machine-$name"
  "$bin" bench "$frames" --live --repeat 5 --cool 30 --json "$out/$machine-$name.json" "$@" ${extra[@]+"${extra[@]}"}
}

run pcb1-jpeg "$pcb1_jpeg" --stacker pyramid,depth,wavelet --align ecc,grid
run pcb1-cr3 "$pcb1_cr3" --stacker pyramid --align ecc --raw-backend libraw --camera-profile adobe
run hornet-jpeg "$hornet_jpeg" --stacker pyramid --align ecc
run pcb1-dng "$pcb1_cr3" --stacker pyramid --align ecc --raw-backend adobe-dng --dng-uncompressed ||
  echo "== $machine-pcb1-dng failed (is Adobe DNG Converter installed?); the other records are written"
