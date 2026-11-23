#!/usr/bin/env bash
# Build the Linux tarball, .deb and .rpm of Deptheos: the desktop app
# (deptheos-gui) and the command line (deptheos), with the menu entry,
# icons, AppStream metadata and licenses from packaging/linux.
#
# Usage: script/bundle-linux.sh [x86_64|arm64]
#
# The architecture defaults to this machine's, and must be this machine's:
# the build is native (C++ libraries and system libraries included), so
# build arm64 on an arm64 host.
#
# Needs:
#   - Ubuntu 22.04, or another distribution with glibc 2.35 or older: a
#     binary runs on the glibc it was built against and every newer one, not
#     older ones, so the oldest build host reaches the most systems (the
#     packages declare glibc >= 2.35, and this script fails if the binaries
#     need a newer one).
#   - The build packages in README.md (build-essential pkg-config cmake clang
#     libfontconfig-dev libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev
#     libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev …).
#   - nfpm, which makes the .deb and .rpm (https://nfpm.goreleaser.com).
#
# Output goes to dist/:
#   Deptheos-<version>-linux-<arch>.tar.gz   bin/ and share/, install.sh
#   deptheos_<version>_<amd64|arm64>.deb
#   deptheos-<version>-1.<x86_64|aarch64>.rpm
set -euo pipefail

cd "$(dirname "$0")/.."

host=$(uname -m)
case "$host" in
  x86_64) host=x86_64 ;;
  aarch64 | arm64) host=arm64 ;;
  *) echo "error: unsupported host architecture $host" >&2; exit 1 ;;
esac
arch="${1:-$host}"
case "$arch" in
  x86_64) deb_arch=amd64 rpm_arch=x86_64 ;;
  arm64) deb_arch=arm64 rpm_arch=aarch64 ;;
  *) echo "usage: $0 [x86_64|arm64]" >&2; exit 2 ;;
esac
if [[ "$arch" != "$host" ]]; then
  echo "error: this is an $host machine; build $arch on an $arch one (no cross-compiling)" >&2
  exit 1
fi

# The tools, found before the long build so a missing one fails at once.
if ! command -v nfpm > /dev/null; then
  cat >&2 << 'EOF'
error: nfpm not found. Install the .deb from its releases,
  https://github.com/goreleaser/nfpm/releases (nfpm_<version>_<amd64|arm64>.deb,
  then sudo apt install ./nfpm_*.deb), or with Go: go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest
EOF
  exit 1
fi
for tool in readelf objdump strip; do
  if ! command -v "$tool" > /dev/null; then
    echo "error: $tool not found (sudo apt install binutils)" >&2
    exit 1
  fi
done

version=$(script/version.sh)
script/sample-check.sh
name="Deptheos-${version}-linux-${arch}"
stage="dist/linux/${name}"
tarball="dist/${name}.tar.gz"
deb="dist/deptheos_${version}_${deb_arch}.deb"
rpm="dist/deptheos-${version}-1.${rpm_arch}.rpm"
# The newest glibc the binaries may need, and the packages' dependency.
max_glibc=2.35

echo "==> Building deptheos-gui and deptheos ${version} (${arch})"
cargo build --release --locked -p deptheos_gpui -p deptheos_cli

echo "==> Assembling ${stage}"
rm -rf dist/linux "$tarball" "$deb" "$rpm"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/metainfo" \
  "$stage/share/doc/deptheos" "$stage/share/deptheos/samples/hornet" \
  "$stage/share/deptheos/samples/pcb"
# The release profile keeps the symbol tables; they are tens of MB.
for bin in deptheos-gui deptheos; do
  strip --strip-unneeded -o "$stage/bin/$bin" "target/release/$bin"
done
cp packaging/linux/com.deptheos.Deptheos.desktop "$stage/share/applications/"
cp packaging/linux/com.deptheos.Deptheos.metainfo.xml "$stage/share/metainfo/"
cp -R packaging/linux/icons "$stage/share/"
# The licenses: the About window opens these (deptheos_gpui::about finds
# them at ../share/doc/deptheos from bin/).
cp LICENSE.md TRADEMARKS.md THIRD_PARTY_NOTICES.txt "$stage/share/doc/deptheos/"
# The sample stacks, which Try a Sample Stack copies into the Library
# folder (deptheos_gpui's app/sample.rs finds them at
# ../share/deptheos/samples from bin/): their frames, not samples' README.
for sample in hornet pcb; do
  install -m 644 "samples/$sample/"*.jpg "$stage/share/deptheos/samples/$sample/"
done
cp packaging/linux/tarball/install.sh packaging/linux/tarball/README.txt "$stage/"
chmod 755 "$stage/install.sh" "$stage/bin/"*

echo "==> Checking the libraries the binaries link"
# The shared libraries packaging/linux/nfpm.yaml depends on, and glibc's own.
# Anything else is a new dependency: add it to nfpm.yaml, README.txt and this
# list.
allowed=(
  libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 librt.so.1
  ld-linux-x86-64.so.2 ld-linux-aarch64.so.1
  libgcc_s.so.1 libstdc++.so.6 libgomp.so.1
  libfontconfig.so.1 libfreetype.so.6
  libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb.so.1
)
for bin in "$stage/bin/"*; do
  while read -r lib; do
    if [[ " ${allowed[*]} " != *" $lib "* ]]; then
      echo "error: $(basename "$bin") links $lib, which the packages don't depend on" >&2
      exit 1
    fi
  done < <(readelf -d "$bin" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
  glibc=$(objdump -T "$bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/GLIBC_//' \
    | sort -uV | tail -1)
  if [[ "$(printf '%s\n%s\n' "$glibc" "$max_glibc" | sort -V | tail -1)" != "$max_glibc" ]]; then
    echo "error: $(basename "$bin") needs glibc $glibc, newer than $max_glibc;" \
      "build on Ubuntu 22.04 (or raise the minimum in packaging/linux/nfpm.yaml)" >&2
    exit 1
  fi
  echo "    $(basename "$bin"): glibc ${glibc}"
done

echo "==> Making ${tarball}"
tar -C dist/linux -czf "$tarball" --owner=0 --group=0 "$name"

echo "==> Making ${deb} and ${rpm}"
export ARCH="$deb_arch" VERSION="$version" STAGE="$PWD/$stage"
nfpm package --config packaging/linux/nfpm.yaml --packager deb --target "$deb"
nfpm package --config packaging/linux/nfpm.yaml --packager rpm --target "$rpm"

echo "==> ${tarball}"
echo "==> ${deb}"
echo "==> ${rpm}"
