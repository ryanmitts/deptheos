#!/usr/bin/env bash
# Regenerate THIRD_PARTY_NOTICES.txt: the license and copyright notices of
# everything compiled into Deptheos, which the release bundles ship and the
# About window links to.
#
# Usage: script/notices.sh [--check]
#   --check  fail if the committed file is out of date instead of writing it
#
# Part 1 (the C and C++ libraries) is assembled from the license files in
# native/vendor/ and licenses/ (which also holds the licenses of the projects
# our own code is derived from: focus-stack and OpenCV). Part 2 (the Rust crates) comes from
# cargo-about (`cargo install cargo-about --locked --features cli`), configured
# by licenses/about.toml. Rerun after adding a crate or updating a vendored
# library; a crate under a license not yet accepted in about.toml fails here.
set -euo pipefail

cd "$(dirname "$0")/.."

check=0
for arg in "$@"; do
  case "$arg" in
    --check) check=1 ;;
    *) echo "usage: $0 [--check]" >&2; exit 2 ;;
  esac
done

out=THIRD_PARTY_NOTICES.txt
tmp=$(mktemp)
crates=$(mktemp)
trap 'rm -f "$tmp" "$crates"' EXIT

v=native/vendor

rule() {
  printf '\n================================================================================\n'
}

# heading NAME [LINE...]: a library's title and lines that describe it.
heading() {
  rule
  printf '%s\n' "$1"
  shift
  for line in "$@"; do printf '%s\n' "$line"; done
  printf '\n'
}

# between FILE START END: the lines from the one matching START up to,
# not including, the one matching END.
between() {
  awk -v s="$2" -v e="$3" '$0 ~ s {p=1} p && $0 ~ e {exit} p' "$1"
}

cargo about generate --workspace --locked -c licenses/about.toml \
  licenses/about.hbs -o "$crates" 2>/dev/null \
  || { echo "cargo about failed; run it without 2>/dev/null to see why" >&2; exit 1; }

{
  cat <<'EOF'
Deptheos third-party notices

Deptheos is free software, licensed under the GNU General Public License
version 3 with additional permissions for the Adobe DNG SDK and Adobe XMP
Toolkit, for LibRaw and for app stores (see LICENSE.md). It includes the
third-party software below, each under its own license. Part 1 lists the C
and C++ libraries, Part 2 the Rust crates.

The source code of Deptheos and of every library below is available at
https://github.com/ryanmitts/deptheos.

This software is based in part on the work of the Independent JPEG Group.
EOF

  rule
  cat <<'EOF'
PART 1: C AND C++ LIBRARIES
EOF

  heading "Adobe DNG SDK 1.7.1" \
    "Copyright 2006-2023 Adobe Systems Incorporated. All rights reserved." \
    "Used under the DNG SDK License Agreement below."
  cat "$v/dng_sdk/LICENSE.txt"

  heading "Adobe XMP Toolkit (XMPCore), from the DNG SDK 1.7.1 package" \
    "Copyright 2002-2020 Adobe. All rights reserved." \
    "Distributed with the DNG SDK and used under the same DNG SDK License" \
    "Agreement, above."

  heading "RSA Data Security, Inc. MD5 Message-Digest Algorithm (in the XMP Toolkit)"
  between "$v/xmp/toolkit/third-party/zuid/sources/MD5.cpp" 'MD5C.C - RSA' '^[*][/]' \
    | sed -e 's/^\t//' -e 's/[[:space:]]*$//'

  heading "Expat 2.10.0 (in the XMP Toolkit)"
  cat licenses/expat-COPYING

  heading "Boost 1.60 (in the XMP Toolkit)" \
    "Headers only, used on Linux to make UUIDs."
  cat licenses/boost-LICENSE_1_0.txt

  heading "zlib 1.2.11 (in the XMP Toolkit)"
  between "$v/xmp/toolkit/third-party/zlib/README" '^Copyright notice:' '^If you use the zlib library in a product'

  heading "Independent JPEG Group's JPEG software, release 9c" \
    "This software is based in part on the work of the Independent JPEG Group."
  # The section's own heading, not its line in the table of contents.
  between "$v/libjpeg/README" '^LEGAL ISSUES$' '^REFERENCES$'

  heading "libjxl (JPEG XL reference implementation)"
  cat "$v/libjxl/libjxl/LICENSE"
  printf '\n'
  cat licenses/libjxl-PATENTS

  heading "Brotli (in libjxl)"
  cat "$v/libjxl/libjxl/third_party/brotli/LICENSE"

  heading "Highway (in libjxl)" \
    "Used under the BSD 3-Clause option of its Apache-2.0 / BSD-3-Clause license."
  cat "$v/libjxl/libjxl/third_party/highway/LICENSE-BSD3"

  heading "skcms (in libjxl)"
  cat "$v/libjxl/libjxl/third_party/skcms/LICENSE"

  heading "LibRaw" \
    "Used under the Common Development and Distribution License version 1.0," \
    "one of the two licenses LibRaw offers. Its source, with the build Deptheos uses, is" \
    "in the Deptheos source repository under native/vendor/LibRaw, and at" \
    "https://github.com/ryanmitts/LibRaw."
  cat "$v/LibRaw/COPYRIGHT"
  printf '\nDCB demosaic and FBDD denoise:\n\n'
  between "$v/LibRaw/src/demosaic/dcb_demosaic.cpp" 'Copyright [(]C[)] 2010' '^ [*][/]' \
    | sed -e 's/^[[:space:]]*\*[[:space:]]\{0,4\}//' -e 's/[[:space:]]*$//'
  printf '\nX3F library (Foveon files):\n\n'
  between "$v/LibRaw/src/x3f/x3f_utils_patched.cpp" '^[*] Copyright [(]c[)] 2010, Roland Karlsson' '^[*][/]' \
    | sed -e 's/^\*[[:space:]]\{0,1\}//' -e 's/[[:space:]]*$//'
  printf '\nThe pieces of code from Adobe DNG SDK 1.4, under the MIT license:\n\n'
  cat <<'EOF'
Copyright (c) 2005 Adobe Systems Incorporated

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
EOF
  printf '\nCommon Development and Distribution License version 1.0:\n\n'
  cat "$v/LibRaw/LICENSE.CDDL"

  heading "focus-stack" \
    "Deptheos's wavelet stacking method is adapted from focus-stack," \
    "https://github.com/PetteriAimonen/focus-stack."
  cat licenses/focus-stack-LICENSE.md

  heading "OpenCV" \
    "Deptheos's ECC aligner is derived from OpenCV's findTransformECC" \
    "(modules/video/src/ecc.cpp), and its tests follow OpenCV's" \
    "(modules/video/test/test_ecc.cpp), https://github.com/opencv/opencv." \
    "Each file is used under the license in its header, below."
  cat licenses/opencv-LICENSE.txt

  rule
  cat <<'EOF'
PART 2: RUST CRATES

Each license below is followed by the crates that use it.
EOF
  cat "$crates"
} > "$tmp"

if [[ "$check" == 1 ]]; then
  if ! cmp -s "$tmp" "$out"; then
    echo "$out is out of date; run script/notices.sh" >&2
    exit 1
  fi
  echo "$out is up to date"
else
  mv "$tmp" "$out"
  chmod 0644 "$out"
  trap 'rm -f "$crates"' EXIT
  echo "wrote $out ($(wc -c < "$out" | tr -d ' ') bytes)"
fi
