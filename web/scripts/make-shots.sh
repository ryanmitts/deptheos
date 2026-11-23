#!/bin/zsh
# The screenshots for src/components/Shot.astro, from the PNGs in $SHOTS_SRC (default ~/DeptheosShots,
# outside git). For each <name>.png it writes public/media/shots/<name>.avif at full size, <name>-half.avif
# at half its width, and <name>.jpg at half its width, the fallback for browsers without AVIF.
# Both sides are rounded down to even: Chrome paints an AVIF of odd width from sips as transparent.
# Rewrites a shot only when its PNG is newer. Needs macOS's sips.
#
#   web/scripts/make-shots.sh
set -euo pipefail

src=${SHOTS_SRC:-$HOME/DeptheosShots}
shots=${0:A:h:h}/public/media/shots
[[ -d $src ]] || { echo "no screenshots folder at $src (set SHOTS_SRC)" >&2; exit 1; }
mkdir -p $shots
even() { print $(( $1 / 2 * 2 )) }
for png in $src/*.png(N); do
  name=${png:t:r}
  full=$shots/$name.avif half=$shots/$name-half.avif jpg=$shots/$name.jpg
  [[ $full -nt $png && $half -nt $png && $jpg -nt $png ]] && continue
  w=$(sips -g pixelWidth $png | awk '/pixelWidth/ { print $2 }')
  h=$(sips -g pixelHeight $png | awk '/pixelHeight/ { print $2 }')
  hw=$(( w / 4 * 2 )) hh=$(even $(( (h * (w / 4 * 2) + w / 2) / w )))
  sips -s format avif -s formatOptions 85 -z $(even $h) $(even $w) $png --out $full >/dev/null
  sips -s format avif -s formatOptions 85 -z $hh $hw $png --out $half >/dev/null
  sips -s format jpeg -s formatOptions 82 -z $hh $hw $png --out $jpg >/dev/null
  echo "$name: $(du -h $full | cut -f1), $(du -h $half | cut -f1) and $(du -h $jpg | cut -f1)"
done
