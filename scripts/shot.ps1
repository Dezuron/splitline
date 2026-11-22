# Screenshot profile: a dev instance that runs next to the portable app.
#
# The single-instance plugin is keyed by the bundle identifier, so the shot
# profile uses `com.zapret.ui.shot` (src-tauri/tauri.shot.conf.json) and its own
# copy of the data folder. The working app keeps running and its config is
# never touched.
#
#   npm run shot             # reuse the last copy
#   npm run shot -- -Fresh   # refresh the copy from %LOCALAPPDATA%\zapret-ui
#
# Keep this file ASCII-only: Windows PowerShell 5.1 reads a BOM-less script as
# ANSI, and non-ASCII string literals break the parser.
[CmdletBinding()]
param(
    [switch]$Fresh
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$source = Join-Path $env:LOCALAPPDATA "zapret-ui"
$target = Join-Path $env:LOCALAPPDATA "zapret-ui-shot"
$config = Join-Path $root "src-tauri\tauri.shot.conf.json"

if (-not (Test-Path $config)) {
    throw "Shot profile not found: $config"
}

$hasCopy = Test-Path (Join-Path $target "config.json")
if ($Fresh -or -not $hasCopy) {
    New-Item -ItemType Directory -Path $target -Force | Out-Null
    if (Test-Path $source) {
        # Overwrite in place: no remove, so a locked runtime file cannot break
        # the copy halfway through.
        Copy-Item -Path (Join-Path $source "*") -Destination $target -Recurse -Force
        Write-Host "Data copied: $source -> $target"
    } else {
        Write-Host "No working data found; the profile starts with a clean config."
    }
} else {
    Write-Host "Using copy: $target (refresh: npm run shot -- -Fresh)"
}

$env:SPLITLINE_APP_DIR = $target
Set-Location $root

& npm run tauri dev -- --config $config
exit $LASTEXITCODE
