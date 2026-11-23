#!/usr/bin/env bash
# The performance baselines (docs/releasing.md): three reference stacks
# through the release CLI, each timed with its peak memory, as a table to
# compare with the one in docs/releasing.md. The stacks are sets the
# end-to-end cases use (e2e.toml), under the owner's sample folder:
#
#   canon1 JPEG      jpeg/canon1, Pyramid, to a TIFF          (case canon1)
#   canon1 CR3       canon, LibRaw, Pyramid, to a linear DNG  (case canon1-raw)
#   canon1 DNG       canon_dng, Wavelet, to a 16-bit TIFF     (case canon1-wavelet)
#
# Usage: script/baselines.sh [sets folder]   (default ~/Desktop/sample_images)
# The results go to a temporary folder, deleted at the end.
set -euo pipefail

cd "$(dirname "$0")/.."
sets="${1:-$HOME/Desktop/sample_images}"

cargo build --release --locked -q -p deptheos_cli
cli="$PWD/target/release/deptheos"
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

# name | folder under $sets | arguments
stacks=(
  "canon1 JPEG, Pyramid|jpeg/canon1|pyramid -o $out/canon1.tif"
  "canon1 CR3, LibRaw, Pyramid|canon|pyramid --raw-backend libraw -o $out/canon1-raw.dng"
  "canon1 DNG, Wavelet|canon_dng|wavelet -o $out/canon1-wavelet.tif"
)

printf '| %-28s | %6s | %8s | %12s |\n' "Stack" "Frames" "Seconds" "Peak memory"
printf '|%s|%s|%s|%s|\n' "------------------------------" "--------" "----------" "--------------"
for stack in "${stacks[@]}"; do
  IFS='|' read -r name folder args <<<"$stack"
  dir="$sets/$folder"
  [[ -d "$dir" ]] || { echo "error: no $dir" >&2; exit 1; }
  frames=$(find "$dir" -maxdepth 1 -type f ! -name '.*' | wc -l | tr -d ' ')
  log="$out/time.txt"
  # shellcheck disable=SC2086
  if [[ "$(uname)" == Darwin ]]; then
    /usr/bin/time -l "$cli" $args "$dir" >/dev/null 2>"$log"
    seconds=$(awk '/ real /{print $1}' "$log")
    peak=$(awk '/peak memory footprint/{print $1}' "$log")
    [[ -n "$peak" ]] || peak=$(awk '/maximum resident set size/{print $1}' "$log")
  else
    /usr/bin/time -v "$cli" $args "$dir" >/dev/null 2>"$log"
    seconds=$(awk -F': ' '/Elapsed \(wall clock\)/{n=split($2,t,":"); s=0; for(i=1;i<=n;i++) s=s*60+t[i]; print s}' "$log")
    peak=$(( $(awk -F': ' '/Maximum resident set size/{print $2}' "$log") * 1024 ))
  fi
  printf '| %-28s | %6s | %8.1f | %9.1f GB |\n' "$name" "$frames" "$seconds" "$(echo "$peak / 1073741824" | bc -l)"
done
