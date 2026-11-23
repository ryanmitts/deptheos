#!/usr/bin/env bash
# Check that the sample stacks the apps carry are here whole before a
# bundle is made (made by script/sample-stack.swift). The bundle scripts run
# this first, so a release never ships without them by accident.
# script/bundle-windows.ps1 checks the same list for itself: keep the two
# in step.
set -euo pipefail

cd "$(dirname "$0")/.."

# Each sample: its folder under samples/, its frames' prefix, how many and
# how many digits their numbers have (hornet-01.jpg … hornet-66.jpg).
samples=(
  "hornet hornet 66 2"
  "pcb pcb 200 3"
)

for sample in "${samples[@]}"; do
  read -r folder prefix count digits <<< "$sample"
  dir="samples/$folder"
  missing=()
  for n in $(seq 1 "$count"); do
    name=$(printf '%s-%0*d.jpg' "$prefix" "$digits" "$n")
    [[ -s "$dir/$name" ]] || missing+=("$name")
  done
  if (( ${#missing[@]} )); then
    echo "error: the sample stack is incomplete: $dir lacks ${missing[*]}" >&2
    exit 1
  fi
  pattern="$prefix-$(printf '[0-9]%.0s' $(seq 1 "$digits")).jpg"
  extra=$(find "$dir" -type f ! -name "$pattern" | head -5)
  if [[ -n "$extra" ]]; then
    echo "error: $dir holds files that aren't its frames: $extra" >&2
    exit 1
  fi
done
