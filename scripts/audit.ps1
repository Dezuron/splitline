# Runs the same dependency checks as the "Dependency audit" CI job, so a
# contributor can catch advisories before pushing instead of after.
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    Write-Host "npm audit --audit-level=high"
    npm audit --audit-level=high
    if ($LASTEXITCODE -ne 0) {
        throw "npm audit found high or critical advisories"
    }

    # cargo-audit is installed by `cargo install cargo-audit`; ~/.cargo/bin is
    # not always on PATH, so fall back to the default install location.
    $audit = Join-Path $HOME ".cargo\bin\cargo-audit.exe"
    if (-not (Test-Path $audit)) {
        $command = Get-Command cargo-audit -ErrorAction SilentlyContinue
        if (-not $command) {
            throw "cargo-audit not found. Install it with: cargo install --locked cargo-audit"
        }
        $audit = $command.Source
    }

    Write-Host "cargo audit --file src-tauri/Cargo.lock"
    & $audit audit --file "src-tauri/Cargo.lock"
    if ($LASTEXITCODE -ne 0) {
        throw "cargo audit found vulnerabilities"
    }

    Write-Host "No known vulnerabilities." -ForegroundColor Green
} finally {
    Pop-Location
}
