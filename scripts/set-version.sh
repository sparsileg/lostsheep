#!/usr/bin/env bash
# Sets the app version everywhere it is written: src-tauri/Cargo.toml,
# src-tauri/Cargo.lock, and APP_VERSION in src/js/core.js.
# (tauri.conf.json has no version of its own; Tauri reads Cargo.toml.)
#
# Usage: scripts/set-version.sh X.Y.Z
set -euo pipefail
cd "$(dirname "$0")/.."

v=${1:?usage: set-version.sh X.Y.Z}
[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not X.Y.Z: $v" >&2; exit 1; }

sed -i "0,/^version = \".*\"/s//version = \"$v\"/" src-tauri/Cargo.toml
sed -i "s/APP_VERSION: '[0-9.]*'/APP_VERSION: '$v'/" src/js/core.js
cargo update --manifest-path src-tauri/Cargo.toml -p lost-sheep --offline >/dev/null 2>&1 \
    || cargo update --manifest-path src-tauri/Cargo.toml -p lost-sheep >/dev/null
echo "version set to $v:"
git diff --stat -- src-tauri/Cargo.toml src-tauri/Cargo.lock src/js/core.js
