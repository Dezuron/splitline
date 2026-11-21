<#
.SYNOPSIS
    Generates the minisign key pair that signs Splitline releases.

.DESCRIPTION
    Creates the key pair outside the repository (by default in
    %USERPROFILE%\.config\opencode\keys\), prints the public key and its
    fingerprint, and lists the GitHub settings that must be filled in.

    The private key never leaves this machine through this script: it is not
    printed and must not be copied into the repository. The script also does not
    talk to GitHub - secrets and variables are added by hand under
    Settings -> Secrets and variables -> Actions.

    Keep two backups of the private key file and of its password. Without them
    no further release can be signed, and installed apps will stop updating.

.PARAMETER KeyPath
    Path of the private key. The public key is written next to it as
    "<KeyPath>.pub". The path must be outside the repository.

.PARAMETER Force
    Overwrite an existing key pair.

.EXAMPLE
    powershell -File scripts/generate-signing-key.ps1

.EXAMPLE
    powershell -File scripts/generate-signing-key.ps1 -KeyPath D:\keys\splitline.key
#>
[CmdletBinding()]
param(
    [string]$KeyPath = (Join-Path $env:USERPROFILE ".config\opencode\keys\splitline.key"),
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$repoRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$keyFile = [IO.Path]::GetFullPath($KeyPath)
$publicFile = "$keyFile.pub"

if ($keyFile.StartsWith($repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "The private key must not live in the repository ($repoRoot). Pass -KeyPath with a path outside it."
}
if (-not $Force -and ((Test-Path -LiteralPath $keyFile) -or (Test-Path -LiteralPath $publicFile))) {
    throw "A key pair already exists at $keyFile. Add -Force to overwrite it (the old key then stops signing)."
}

# `tauri signer generate` writes the format `tauri signer sign` reads in CI, so
# the helper uses the same CLI instead of a locally installed minisign.
$tauri = Join-Path $repoRoot "node_modules\.bin\tauri.cmd"
if (-not (Test-Path -LiteralPath $tauri)) {
    throw "Tauri CLI not found at $tauri. Run 'npm ci' in $repoRoot first."
}

$directory = Split-Path -Parent $keyFile
New-Item -ItemType Directory -Force -Path $directory | Out-Null

Write-Host "Generating a minisign key pair for Splitline updates."
Write-Host "The CLI asks for a password twice; store it in a password manager."
Write-Host "Private key: $keyFile"
Write-Host ""

$arguments = @("signer", "generate", "-w", $keyFile)
if ($Force) {
    $arguments += "-f"
}
& $tauri @arguments
if ($LASTEXITCODE -ne 0) {
    throw "tauri signer generate failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path -LiteralPath $publicFile)) {
    throw "The public key file was not created: $publicFile"
}

# The .pub file holds base64 of the public key box ("untrusted comment" line
# plus the key line). Both that value and the bare key line are accepted by the
# app; the bare line is what minisign and SECURITY.md show, so recommend it.
$publicBox = (Get-Content -Raw -Path $publicFile).Trim()
$boxText = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($publicBox))
$keyLine = ($boxText -split "`r?`n" |
    Where-Object { $_ -match '^\s*RW' } |
    Select-Object -First 1)
if (-not $keyLine) {
    throw "Could not read the key line from $publicFile."
}
$keyLine = $keyLine.Trim()
$rawKey = [Convert]::FromBase64String($keyLine)
if ($rawKey.Length -ne 42) {
    throw "Unexpected public key length ($($rawKey.Length) bytes) in $publicFile."
}
$fingerprint = (($rawKey[2..9]) | ForEach-Object { $_.ToString("X2") }) -join ""

Write-Host ""
Write-Host "Key pair created."
Write-Host "  private: $keyFile"
Write-Host "  public:  $publicFile"
Write-Host "  key id:  $fingerprint"
Write-Host ""
Write-Host "GitHub -> Settings -> Secrets and variables -> Actions:"
Write-Host "  Variable SPLITLINE_UPDATE_PUBKEY    = $keyLine"
Write-Host "  Secret   MINISIGN_SECRET_KEY        = content of $keyFile (one base64 line)"
Write-Host "  Secret   MINISIGN_SECRET_KEY_PASSWORD = the password you just typed"
Write-Host ""
Write-Host "The whole content of $publicFile is accepted as SPLITLINE_UPDATE_PUBKEY too."
Write-Host "Publish the key id ($fingerprint) in SECURITY.md so users can compare it."
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Back up $keyFile and the password in two places."
Write-Host "  2. Never commit the private key, never paste it into an issue or chat."
Write-Host "  3. Rebuild the app with SPLITLINE_UPDATE_PUBKEY set, otherwise it keeps"
Write-Host "     refusing updates (the key is compiled in at build time)."
Write-Host "  4. Sign a test release before trusting the setup."
