#!/usr/bin/env pwsh

# Local release helper: same idea as GitHub Actions — default toolchain / host from the environment.
# Produces bin/organise.exe and copies field_model_mappings.toml alongside it.

$ErrorActionPreference = "Stop"

$BIN_NAME = "organise"
$RELEASE_EXE = "target/release/${BIN_NAME}.exe"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo not found on PATH. Install Rust from https://rustup.rs and ensure ~/.cargo/bin is on PATH."
}

Write-Host "Running clippy..."
cargo clippy --all-targets -- -D warnings

Write-Host "Running tests..."
cargo test

Write-Host "Building release..."
cargo build --release

Write-Host ""
if (Get-Command upx -ErrorAction SilentlyContinue) {
    Write-Host "Compressing binary with UPX..."
    try {
        & upx --best --lzma $RELEASE_EXE | Out-Null
    }
    catch {
        Write-Host "  WARNING: UPX compression failed"
    }
}
else {
    Write-Host "  UPX not found - skipping compression (install with: choco install upx)"
}

New-Item -ItemType Directory -Force -Path "bin" | Out-Null

Write-Host ""
Write-Host "Copying to bin/..."
Move-Item -Force $RELEASE_EXE "bin/${BIN_NAME}.exe"
Copy-Item -Force "src/modifiers/field_model_mappings.toml" "bin/field_model_mappings.toml"

Write-Host ""
Write-Host "Done."
Get-Item "bin/${BIN_NAME}.exe" | ForEach-Object {
    Write-Host ("  {0}  ({1:N0} bytes)" -f $_.FullName, $_.Length)
}
