#!/usr/bin/env bash
# Release checks. Runs every check, then prints one line per check:
# PASS, FAIL, or NOTE (look at it; not a failure by itself). Exit
# status is 1 if any check failed. Adapted from kansha's.
#
# Usage: scripts/release-check.sh [BASE]
#   BASE: the commit or tag of the last release. Defaults to the latest
#   tag; with no tag, the checks that compare with it are skipped.
set -uo pipefail
cd "$(dirname "$0")/.."

log=src-tauri/target/release-check
mkdir -p "$log"
results=()
failed=0
pass() { results+=("PASS  $1"); }
fail() { results+=("FAIL  $1"); failed=1; }
note() { results+=("NOTE  $1"); }
step() { printf '\n=== %s\n' "$1"; }

base=${1:-$(git describe --tags --abbrev=0 2>/dev/null || true)}

# 1. Format, lints, every test.
step "just check"
if just check 2>&1 | tee "$log/check.txt"; then
    pass "just check"
else
    fail "just check (see $log/check.txt)"
fi

# 2. The version is the same in every place it is written.
step "version"
v_cargo=$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)
v_lock=$(grep -A1 -E '^name = "lost-sheep"$' src-tauri/Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')
v_js=$(sed -n "s/.*APP_VERSION: '\(.*\)'.*/\1/p" src/js/core.js)
all="$v_cargo $v_lock $v_js"
if [ -n "$v_cargo" ] && [ -z "$(echo $all | tr ' ' '\n' | grep -vx "$v_cargo")" ] && [ "$(echo $all | wc -w)" -eq 3 ]; then
    pass "version $v_cargo in all places"
else
    fail "versions differ: Cargo.toml '$v_cargo', Cargo.lock '$v_lock', core.js '$v_js' (fix with: just version X.Y.Z)"
fi
if [ -n "$base" ] && [ "v$v_cargo" = "$base" -o "$v_cargo" = "$base" ]; then
    note "version $v_cargo is the same as the last release $base; bump it with: just version X.Y.Z"
fi

# 3. Schema changes need a migration for existing databases. schema.sql
# only creates what is missing; a new column on an existing table needs
# a migrate_* function in db/mod.rs and a test.
step "schema"
if [ -n "$base" ]; then
    changed=$(git diff --name-only "$base" -- src-tauri/src/db/schema.sql src-tauri/src/db/roads_schema.sql src-tauri/src/db/mod.rs | tr '\n' ' ')
    if [ -n "$changed" ]; then
        note "schema code changed since $base; confirm existing databases are migrated and tested: $changed"
    else
        pass "schema unchanged since $base"
    fi
else
    note "schema not compared: no base (pass the last release's commit)"
fi

# 4. Coverage: record it beside the last release's.
step "just cov"
if just cov >"$log/cov.txt" 2>&1; then
    total=$(grep '^TOTAL' "$log/cov.txt" | awk '{print "regions " $4 ", functions " $7 ", lines " $10}')
    note "coverage $total; record it in the release notes (detail: src-tauri/target/llvm-cov/html/index.html)"
else
    fail "just cov (see $log/cov.txt)"
fi

# 5. The pdftotext sidecar for this platform is present; installers
# can't be built without it (tauri.conf.json bundle.externalBin).
step "sidecar"
host=$(rustc -vV | sed -n 's/^host: //p')
if [ -f "src-tauri/binaries/pdftotext-$host" ]; then
    pass "pdftotext sidecar present for $host"
else
    fail "src-tauri/binaries/pdftotext-$host missing; see docs/Architecture_Build.md"
fi

# 6. Everything is committed.
step "git status"
if [ -z "$(git status --porcelain)" ]; then
    pass "working tree clean"
else
    note "uncommitted changes; commit them before tagging"
fi

printf '\n=== Release check%s\n' "${base:+ against $base}"
printf '%s\n' "${results[@]}" | tee "$log/summary.txt"
exit $failed
