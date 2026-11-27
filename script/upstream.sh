#!/usr/bin/env bash
# What lightcraft has changed since the revision the adjustment sliders
# take from (ADR-137), in the files they take from. craft-libs is followed
# at the revision lightcraft pins, not at its own tip.
#
# Usage: script/upstream.sh [lightcraft revision, default origin/main]
# Reads local clones: LIGHTCRAFT (~/lightcraft), CRAFT_LIBS (~/craft-libs).
# Every revision a comment cites must be the one named here.
set -euo pipefail

LIGHTCRAFT_REV=5bdb4e7a6955fde3cb9ebda36576bbea6a0c6d7d
CRAFT_LIBS_REV=b2d2a1e948c43d95aa769d4f0a06fe6b869edc39
LIGHTCRAFT_FILES=(
  crates/develop/src/controls.rs
  crates/ui-egui/src/panels/edit/sliders.rs
  crates/pipeline/src/finish.rs
  crates/gpu/src/ctx.rs
)
CRAFT_LIBS_FILES=(
  crates/develop/src/tone.rs
  crates/develop/src/local.rs
  crates/develop/src/raster/resample.rs
  crates/develop/src/raster/blur.rs
  crates/develop/src/finish.rs
  crates/develop-model/src/settings.rs
  crates/develop/src/colorops.rs
  crates/develop/src/light_v3.rs
  crates/develop/src/light_v3_params.rs
  crates/color-transform/src/cct.rs
  crates/color-transform/src/oklab.rs
  crates/cms/src/math.rs
)

lightcraft=${LIGHTCRAFT:-$HOME/lightcraft}
craft_libs=${CRAFT_LIBS:-$HOME/craft-libs}
target=${1:-origin/main}

# The citations name these revisions and no other.
stale=$(git grep -n -E '(lightcraft|craft-libs) [0-9a-f]{9}' -- crates docs |
  grep -v -e "lightcraft ${LIGHTCRAFT_REV:0:9}" -e "craft-libs ${CRAFT_LIBS_REV:0:9}" || true)
if [[ -n $stale ]]; then
  echo "Citations of another revision than this script's:" >&2
  echo "$stale" >&2
  exit 1
fi

for repo in "$lightcraft" "$craft_libs"; do
  git -C "$repo" rev-parse --git-dir >/dev/null || { echo "No clone at $repo" >&2; exit 1; }
  git -C "$repo" fetch --quiet origin
done

to=$(git -C "$lightcraft" rev-parse "$target")
pin=$(git -C "$lightcraft" show "$to:Cargo.toml" |
  sed -n -E 's/^craft-develop = .*rev = "([0-9a-f]+)".*/\1/p')
[[ -n $pin ]] || { echo "lightcraft $to pins no craft-develop revision" >&2; exit 1; }

show() { # repo name from to files...
  local repo=$1 name=$2 from=$3 to=$4
  shift 4
  echo "== $name ${from:0:9}..${to:0:9}"
  if [[ $from == "$to" ]]; then
    echo "(the same revision)"
    return
  fi
  if git -C "$repo" diff --quiet "$from" "$to" -- "$@"; then
    echo "(no change to these files)"
    return
  fi
  git -C "$repo" --no-pager log --oneline "$from..$to" -- "$@"
  git -C "$repo" --no-pager diff --stat "$from" "$to" -- "$@"
  git -C "$repo" --no-pager diff "$from" "$to" -- "$@"
}

show "$lightcraft" lightcraft "$LIGHTCRAFT_REV" "$to" "${LIGHTCRAFT_FILES[@]}"
echo
show "$craft_libs" "craft-libs (lightcraft's pin)" "$CRAFT_LIBS_REV" "$pin" "${CRAFT_LIBS_FILES[@]}"
