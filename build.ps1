#!/usr/bin/env pwsh

# Build script for workbench-preprocessor
# Builds for Windows targets only

param(
    [ValidateSet("msvc","gnu")]
    [string]$Toolchain = "msvc"
)

$ErrorActionPreference = "Stop"

$BIN_NAME = "organise"
$TARGET_TRIPLE = if ($Toolchain -eq "gnu") { "x86_64-pc-windows-gnu" } else { "x86_64-pc-windows-msvc" }
$RUST_TOOLCHAIN = if ($Toolchain -eq "gnu") { "stable-x86_64-pc-windows-gnu" } else { "stable-x86_64-pc-windows-msvc" }
$TOOLCHAIN_BIN = Join-Path $env:USERPROFILE ".rustup\\toolchains\\$RUST_TOOLCHAIN\\bin"
$CARGO = Join-Path $TOOLCHAIN_BIN "cargo.exe"
$env:PATH = "$TOOLCHAIN_BIN;$env:PATH"

Write-Host "Building $BIN_NAME for Windows target: $TARGET_TRIPLE..."
Write-Host ""

Write-Host "Cleaning previous builds..."
& $CARGO clean

Write-Host "Running clippy..."
& $CARGO clippy --target $TARGET_TRIPLE -- -D warnings

Write-Host "Running tests..."
& $CARGO test --target $TARGET_TRIPLE

Write-Host "Building Windows ($TARGET_TRIPLE) release..."
& $CARGO build --target $TARGET_TRIPLE --release

Write-Host ""
if (Get-Command upx -ErrorAction SilentlyContinue) {
    Write-Host "Compressing binaries with UPX..."
    try { & upx --best --lzma "target/$TARGET_TRIPLE/release/$BIN_NAME.exe" | Out-Null } catch { Write-Host "  ⚠️  UPX compression failed for Windows binary" }
} else {
    Write-Host "  UPX not found - skipping compression (install with: choco install upx)"
}

New-Item -ItemType Directory -Force -Path "bin" | Out-Null

Write-Host ""
Write-Host "Moving binaries to bin/ folder..."
Move-Item -Force "target/$TARGET_TRIPLE/release/$BIN_NAME.exe" "bin/${BIN_NAME}.exe"
Copy-Item -Force "src/modifiers/field_model_mappings.toml" "bin/field_model_mappings.toml"

Write-Host ""
Write-Host "All builds completed successfully!"
Write-Host ""
Write-Host "Build artifacts and sizes:"

Get-Item "bin/${BIN_NAME}.exe" | ForEach-Object {
    $size = "{0:N0} bytes" -f $_.Length
    Write-Host ("  {0,-40} {1}" -f $_.FullName, $size)
}
