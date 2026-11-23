#!/usr/bin/env bash
# Prints the workspace version, from [workspace.package] in Cargo.toml: the version the release
# scripts put in the bundles' names and Info.plist, and the one a release tag must match.
set -euo pipefail
awk -F'"' '/^\[workspace.package\]/{p=1} p && /^version/{print $2; exit}' "$(dirname "$0")/../Cargo.toml"
