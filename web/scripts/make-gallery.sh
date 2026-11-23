#!/bin/zsh
setopt extended_glob
# A gallery picture's files, from its frames (JPEG or camera RAW): the stack (Pyramid, the defaults), as the page's
# stills and the 100% viewer's tiles, and the stack building up from the back as a video.
#
#   web/scripts/make-gallery.sh <name> <folder of frames>
#
# Writes public/media/gallery/<name>/:
#   2400.jpg, 1200.jpg, 900.jpg   the stack, by long edge (the page, the cards, the home page's hero)
#   frame-2400.jpg                the grow sequence's first frame: one frame alone, aligned and
#                                 cropped like the stack (the before of the before/after)
#   tiles/<row>-<col>.jpg         the stack at full size in 1024 px tiles (src/viewer.ts)
#   grow.av1.mp4, grow.h264.mp4, grow-poster.jpg
# Needs ffmpeg, ffprobe and macOS's sips; cargo builds deptheos. Add the gallery entry in src/data/gallery.ts.
set -euo pipefail

if (( $# < 2 )); then
  echo "usage: make-gallery.sh <name> <folder of frames>" >&2
  exit 2
fi

here=${0:A:h}
web=${here:h}
repo=${web:h}
name=$1
src=${2:A}
out=$web/public/media/gallery/$name
work=$(mktemp -d)
trap 'rm -rf $work' EXIT
mkdir -p $out/tiles

(cd $repo && cargo build --release -q --bin deptheos)
bin=$repo/target/release/deptheos
# JPEG frames, or camera RAW and DNG, decoded by the command line's default (Automatic).
frames=($src/*.(#i)(jpg|jpeg|cr2|cr3|nef|arw|orf|raf|rw2|dng)(N))
(( ${#frames} )) || { echo "no JPEG or RAW frames in $src" >&2; exit 1; }

$bin pyramid $frames -o $work/full.jpg --jpeg-quality 95 -q
size=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=s=x:p=0 $work/full.jpg)
w=${size%x*}; h=${size#*x}
# ffmpeg resizes the stills and sips encodes them at "high", finer than ffmpeg's JPEG at its best -q:v (2).
for edge in 2400 1200 900; do
  ffmpeg -v error -y -i $work/full.jpg -vf "scale='if(gte(iw,ih),$edge,-1)':'if(gte(iw,ih),-1,$edge)':flags=lanczos" $work/$edge.png
  sips -s format jpeg -s formatOptions high $work/$edge.png --out $out/$edge.jpg >/dev/null
done
rm -f $out/tiles/*.jpg(N)
for (( r = 0; r * 1024 < h; r++ )); do
  for (( c = 0; c * 1024 < w; c++ )); do
    tw=$(( w - c * 1024 < 1024 ? w - c * 1024 : 1024 )); th=$(( h - r * 1024 < 1024 ? h - r * 1024 : 1024 ))
    ffmpeg -v error -y -i $work/full.jpg -vf "crop=$tw:$th:$(( c * 1024 )):$(( r * 1024 ))" -q:v 3 $out/tiles/$r-$c.jpg
  done
done

$bin sequence grow $frames --name $name -o $work --format jpg --jpeg-quality 92 --long-edge 2400 -q
seq=$work/"$name grow"
grown=($seq/*_<->.jpg(N)); grown=(${(o)grown})
(( ${#grown} )) || { echo "no frames in $seq" >&2; exit 1; }
cp $grown[1] $out/frame-2400.jpg
vf="scale=1920:-2:flags=lanczos,format=yuv420p"
common=(-framerate 24 -i "$seq/$name grow_%04d.jpg" -an -map_metadata -1 -vf $vf -color_primaries bt709 -color_trc bt709 -colorspace bt709 -movflags +faststart)
SVT_LOG=1 ffmpeg -v error -y $common -c:v libsvtav1 -preset 4 -crf 40 -svtav1-params tune=0 $out/grow.av1.mp4
ffmpeg -v error -y $common -c:v libx264 -preset veryslow -crf 25 -profile:v high -tag:v avc1 $out/grow.h264.mp4
ffmpeg -v error -y -i $grown[1] -vf "scale=1920:-2:flags=lanczos" -q:v 4 $out/grow-poster.jpg
echo "$name: ${#frames} frames, $size"
du -sh $out
