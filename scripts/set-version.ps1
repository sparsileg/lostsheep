# Sets the app version everywhere it is written: src-tauri/Cargo.toml,
# src-tauri/Cargo.lock, and APP_VERSION in src/js/core.js. Windows
# PowerShell 5.1 or later; the same as set-version.sh.
#
# Usage: scripts/set-version.ps1 X.Y.Z
param([Parameter(Mandatory = $true)][string]$V)
# Not 'Stop': Windows PowerShell 5.1 would stop on any line a program
# writes to stderr. Exit codes are checked instead; anything else
# that fails stops the script.
trap { [Console]::Error.WriteLine($_); exit 1 }
Set-Location (Join-Path $PSScriptRoot '..')

if ($V -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    [Console]::Error.WriteLine("not X.Y.Z: $V")
    exit 1
}

# Files are read and written whole, as UTF-8 without a BOM, so line
# endings are kept.
function Edit-File([string]$Path, [scriptblock]$Change) {
    $full = Join-Path (Get-Location) $Path
    $text = [IO.File]::ReadAllText($full)
    [IO.File]::WriteAllText($full, (& $Change $text), (New-Object Text.UTF8Encoding $false))
}

# The first `version = "..."` only: the package's own.
Edit-File 'src-tauri/Cargo.toml' {
    param($t)
    ([regex]'(?m)^version = ".*"').Replace($t, "version = `"$V`"", 1)
}
Edit-File 'src/js/core.js' {
    param($t)
    ([regex]"APP_VERSION: '[0-9.]*'").Replace($t, "APP_VERSION: '$V'", 1)
}

cargo update --manifest-path src-tauri/Cargo.toml -p lost-sheep --offline *> $null
if ($LASTEXITCODE -ne 0) {
    cargo update --manifest-path src-tauri/Cargo.toml -p lost-sheep *> $null
    if ($LASTEXITCODE -ne 0) { [Console]::Error.WriteLine('cargo update failed'); exit 1 }
}
Write-Output "version set to ${V}:"
git diff --stat -- src-tauri/Cargo.toml src-tauri/Cargo.lock src/js/core.js
