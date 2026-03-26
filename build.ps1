#!/usr/bin/env pwsh

# Build script for workbench-preprocessor
# Builds the Windows release binary and copies field_model_mappings.toml into bin/

param(
    [ValidateSet("msvc", "gnu")]
    [string]$Toolchain = "msvc"
)

$ErrorActionPreference = "Stop"

$BIN_NAME = "organise"
$TARGET_TRIPLE = if ($Toolchain -eq "gnu") { "x86_64-pc-windows-gnu" } else { "x86_64-pc-windows-msvc" }
$RUST_TOOLCHAIN = if ($Toolchain -eq "gnu") { "stable-x86_64-pc-windows-gnu" } else { "stable-x86_64-pc-windows-msvc" }

# Prefer rustup-managed cargo (do not hardcode ~/.rustup/toolchains/.../cargo.exe).
$prevRustupToolchain = $env:RUSTUP_TOOLCHAIN
$env:RUSTUP_TOOLCHAIN = $RUST_TOOLCHAIN
try {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo not found on PATH. Install Rust from https://rustup.rs and ensure ~/.cargo/bin is on PATH."
    }

    Write-Host "Building $BIN_NAME for Windows target: $TARGET_TRIPLE (toolchain: $RUST_TOOLCHAIN)..."
    Write-Host ""

    Write-Host "Cleaning previous builds..."
    cargo clean

    Write-Host "Running clippy..."
    cargo clippy --target $TARGET_TRIPLE --all-targets -- -D warnings

    Write-Host "Running tests..."
    cargo test --target $TARGET_TRIPLE

    Write-Host "Building Windows ($TARGET_TRIPLE) release..."
    cargo build --target $TARGET_TRIPLE --release

    Write-Host ""
    if (Get-Command upx -ErrorAction SilentlyContinue) {
        Write-Host "Compressing binaries with UPX..."
        try {
            & upx --best --lzma "target/$TARGET_TRIPLE/release/$BIN_NAME.exe" | Out-Null
        }
        catch {
            Write-Host "  WARNING: UPX compression failed for Windows binary"
        }
    }
    else {
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
}
finally {
    $env:RUSTUP_TOOLCHAIN = $prevRustupToolchain
}
