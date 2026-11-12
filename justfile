# Lost Sheep task runner. `just` lists recipes.
#
# Adapted from kansha's justfile. Lost Sheep is a single crate in
# src-tauri/ with a plain-JS frontend (no npm), so cargo recipes pass
# --manifest-path and there are no npm/Svelte/bindings recipes.

set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

manifest := "--manifest-path src-tauri/Cargo.toml"

default:
    @just --list

# Run the full test suite: Rust tests, then a syntax check of the frontend JS
test: test-rust js-check

# Rust tests only
test-rust:
    cargo test {{manifest}}

# Syntax-check every frontend JS file (src/js; vendored src/include skipped)
js-check:
    node scripts/js-check.mjs

# Test coverage: summary here, detail in src-tauri/target/llvm-cov/html/index.html
cov:
    cargo llvm-cov {{manifest}} --html
    cargo llvm-cov {{manifest}} report --summary-only

# Every automated release check, with a PASS/FAIL/NOTE summary; BASE = last release (default: latest tag)
[unix]
release-check BASE="":
    scripts/release-check.sh {{BASE}}

[windows]
release-check BASE="":
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/release-check.ps1 {{BASE}}

# Set the app version in all the places it is written, e.g. `just version 1.4.0`
[unix]
version V:
    scripts/set-version.sh {{V}}

[windows]
version V:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/set-version.ps1 {{V}}

# Formatting, lints, and tests
check: fmt-check clippy test

# Format all Rust code
fmt:
    cargo fmt {{manifest}}

# Fail if Rust code is not formatted
fmt-check:
    cargo fmt {{manifest}} -- --check

# Lint; warnings are errors
clippy:
    cargo clippy {{manifest}} --all-targets -- -D warnings

# Run the desktop app in dev mode
dev:
    cargo tauri dev

# Production build (installers for the current platform; needs the pdftotext sidecar in src-tauri/binaries)
build:
    cargo tauri build

# Release exe only, no installers (src-tauri/target/release/)
build-exe:
    cargo tauri build --no-bundle

# Touches every .rs file and rebuilds (build, test, clippy), so the crate
# recompiles once; any incremental dir those builds don't write is dead.
# First, `cargo sweep --installed` drops artifacts from toolchains no
# longer installed (needs cargo-sweep).
#
# Delete stale incremental dirs from src-tauri/target/debug (full rebuild)
[unix]
sweep:
    #!/usr/bin/env bash
    set -euo pipefail
    cd src-tauri
    mkdir -p target
    marker=$(mktemp target/sweep-marker.XXXXXX)
    trap 'rm -f "$marker"' EXIT
    if command -v cargo-sweep >/dev/null; then
        cargo sweep --installed
    else
        echo "sweep: cargo-sweep missing; install with: cargo install cargo-sweep"
    fi
    sleep 1
    find src build.rs -name '*.rs' -exec touch -c {} +
    cargo build
    cargo test --no-run
    cargo clippy --all-targets -- -D warnings || echo "sweep: clippy reported problems (build artifacts still written)"
    before=$(du -sh target | cut -f1)
    n=0
    for d in target/debug/incremental/*/; do
        [ -d "$d" ] || continue
        if [ -z "$(find "$d" -newer "$marker" -print -quit)" ]; then
            rm -rf -- "$d"
            n=$((n + 1))
        fi
    done
    echo "sweep: removed $n incremental dirs; target $before -> $(du -sh target | cut -f1)"
