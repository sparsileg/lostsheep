# Release checks. Runs every check, then prints one line per check:
# PASS, FAIL, or NOTE (look at it; not a failure by itself). Exit
# status is 1 if any check failed. Windows PowerShell 5.1 or later;
# the same as release-check.sh. Adapted from kansha's.
#
# Usage: scripts/release-check.ps1 [BASE]
#   BASE: the commit or tag of the last release. Defaults to the latest
#   tag; with no tag, the checks that compare with it are skipped.
param([string]$Base = '')
# Not 'Stop': Windows PowerShell 5.1 would stop on any line a program
# writes to stderr. Each check looks at exit codes instead.
# Keep this file ASCII: Windows PowerShell 5.1 reads a file with no
# byte order mark as ANSI, so a non-ASCII character comes out garbled.
$ErrorActionPreference = 'Continue'
Set-Location (Join-Path $PSScriptRoot '..')

$log = 'src-tauri/target/release-check'
New-Item -ItemType Directory -Force -Path $log | Out-Null
$results = New-Object System.Collections.Generic.List[string]
$script:failed = 0
function Pass([string]$m) { $results.Add("PASS  $m") }
function Fail([string]$m) { $results.Add("FAIL  $m"); $script:failed = 1 }
function Note([string]$m) { $results.Add("NOTE  $m") }
function Step([string]$m) { Write-Output ''; Write-Output "=== $m" }

# Runs a program, its output (stdout and stderr) to the file `$To`
# (UTF-8) and, with -Show, to the screen too as it comes. Returns true
# when it exits 0. cmd joins stderr to stdout, so PowerShell sees only
# plain lines, not error records.
function Run([string]$To, [switch]$Show, [string]$Exe) {
    if ($Show) {
        & cmd /c $Exe @args '2>&1' |
            ForEach-Object { Out-Host -InputObject $_; $_ } |
            Set-Content -Encoding UTF8 -Path $To
    } else {
        & cmd /c $Exe @args '2>&1' | Set-Content -Encoding UTF8 -Path $To
    }
    return $LASTEXITCODE -eq 0
}

# Lines a program prints, or none when it fails.
function Lines([string]$Exe) {
    $out = & $Exe @args 2>$null
    if ($LASTEXITCODE -ne 0) { return @() }
    return @($out | Where-Object { $_ -ne '' })
}

if (-not $Base) {
    $Base = (Lines git describe --tags --abbrev=0) | Select-Object -First 1
    if (-not $Base) { $Base = '' }
}

# 1. Format, lints, every test.
Step 'just check'
if (Run "$log/check.txt" -Show just check) {
    Pass 'just check'
} else {
    Fail "just check (see $log/check.txt)"
}

# 2. The version is the same in every place it is written.
Step 'version'
$vCargo = ((Get-Content src-tauri/Cargo.toml) -match '^version = "' | Select-Object -First 1) -replace '^version = "(.*)"', '$1'
$lock = Get-Content src-tauri/Cargo.lock
$vLock = ''
for ($i = 0; $i -lt $lock.Count - 1; $i++) {
    if ($lock[$i] -eq 'name = "lost-sheep"' -and $lock[$i + 1] -match '^version = "(.*)"') {
        $vLock = $Matches[1]
    }
}
$vJs = ((Get-Content src/js/core.js) | Select-String "APP_VERSION: '(.*)'" |
    ForEach-Object { $_.Matches[0].Groups[1].Value }) -join ' '
if ($vCargo -and $vCargo -eq $vLock -and $vCargo -eq $vJs) {
    Pass "version $vCargo in all places"
} else {
    Fail "versions differ: Cargo.toml '$vCargo', Cargo.lock '$vLock', core.js '$vJs' (fix with: just version X.Y.Z)"
}
if ($Base -and ($Base -eq $vCargo -or $Base -eq "v$vCargo")) {
    Note "version $vCargo is the same as the last release ${Base}; bump it with: just version X.Y.Z"
}

# 3. Schema changes need a migration for existing databases. schema.sql
# only creates what is missing; a new column on an existing table needs
# a migrate_* function in db/mod.rs and a test.
Step 'schema'
if ($Base) {
    $changed = (Lines git diff --name-only $Base -- src-tauri/src/db/schema.sql src-tauri/src/db/roads_schema.sql src-tauri/src/db/mod.rs) -join ' '
    if ($changed) {
        Note "schema code changed since ${Base}; confirm existing databases are migrated and tested: $changed"
    } else {
        Pass "schema unchanged since $Base"
    }
} else {
    Note "schema not compared: no base (pass the last release's commit)"
}

# 4. Coverage: record it beside the last release's.
Step 'just cov'
if (Run "$log/cov.txt" just cov) {
    $total = Get-Content "$log/cov.txt" | Where-Object { $_ -match '^TOTAL' } | Select-Object -First 1
    $w = "$total" -split '\s+'
    Note "coverage regions $($w[3]), functions $($w[6]), lines $($w[9]); record it in the release notes (detail: src-tauri/target/llvm-cov/html/index.html)"
} else {
    Fail "just cov (see $log/cov.txt)"
}

# 5. The pdftotext sidecar for this platform is present; installers
# can't be built without it (tauri.conf.json bundle.externalBin).
Step 'sidecar'
$hostTriple = ((Lines rustc -vV) | Where-Object { $_ -match '^host: ' } | Select-Object -First 1) -replace '^host: ', ''
if (Test-Path "src-tauri/binaries/pdftotext-$hostTriple.exe") {
    Pass "pdftotext sidecar present for $hostTriple"
} else {
    Fail "src-tauri/binaries/pdftotext-$hostTriple.exe missing; see docs/Architecture_Build.md"
}

# 6. Everything is committed.
Step 'git status'
if (-not (Lines git status --porcelain)) {
    Pass 'working tree clean'
} else {
    Note 'uncommitted changes; commit them before tagging'
}

Write-Output ''
if ($Base) { Write-Output "=== Release check against $Base" } else { Write-Output '=== Release check' }
$results | Set-Content -Encoding UTF8 -Path "$log/summary.txt"
$results | Write-Output
exit $script:failed
