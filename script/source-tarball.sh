#!/usr/bin/env bash
# The release's full source: dist/Deptheos-<version>-source.tar.gz, the
# commit checked out with every submodule (LibRaw) at the commit it is
# pinned to, under one Deptheos-<version>/ folder. `git archive` alone
# leaves submodules out, and GitHub's own source downloads are made that
# way, so this is the archive that carries all of the GPL and CDDL source
# the binaries are built from (docs/releasing.md).
#
# Usage: script/source-tarball.sh. The submodules must be checked out
# (git submodule update --init); it refuses otherwise. Only committed files
# are archived, so changes not committed are left out.
set -euo pipefail

cd "$(dirname "$0")/.."

version=$(script/version.sh)
name="Deptheos-${version}-source"
out="dist/${name}.tar.gz"

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root="$stage/Deptheos-${version}"
mkdir -p "$root" dist

echo "==> ${out}"
git archive --format=tar HEAD | tar -x -C "$root"

# A submodule not checked out ("-") or at another commit than the one
# pinned ("+") would put the wrong source in the archive.
if git submodule status --recursive | grep -q '^[-+U]'; then
  git submodule status --recursive >&2
  echo "error: a submodule isn't at its pinned commit (git submodule update --init --recursive)" >&2
  exit 1
fi

# Each submodule (and theirs, recursively) at the commit the tree pins.
while read -r path; do
  [[ -n "$path" ]] || continue
  echo "    + $path"
  mkdir -p "$root/$path"
  git -C "$path" archive --format=tar HEAD | tar -x -C "$root/$path"
done < <(git submodule foreach --quiet --recursive 'echo "$displaypath"')

# The same bytes for the same commit wherever it is made: names sorted, no
# owner or times from this machine, and no time in the gzip header.
stamp=$(git log -1 --format=%ct HEAD)
find "$root" -exec touch -h -d "@${stamp}" {} + 2>/dev/null \
  || find "$root" -exec touch -h -t "$(date -r "$stamp" +%Y%m%d%H%M.%S)" {} +
if tar --version 2>/dev/null | grep -q GNU; then
  tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner -cf - "Deptheos-${version}" | gzip -n -9 > "$out"
else
  tar -C "$stage" --uid 0 --gid 0 -cf - "Deptheos-${version}" | gzip -n -9 > "$out"
fi
ls -lh "$out"
