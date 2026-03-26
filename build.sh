#!/bin/bash

# Build script for workbench-preprocessor
# Builds Linux (host) and Windows (x86_64-pc-windows-gnu) release binaries.
#
# Prerequisites for the Windows cross-target on Linux:
#   rustup target add x86_64-pc-windows-gnu
#   (often also: a MinGW linker — e.g. mingw-w64 packages on Debian/Ubuntu)

set -euo pipefail

BIN_NAME="organise"
WIN_TARGET="x86_64-pc-windows-gnu"

echo "Building ${BIN_NAME} for multiple targets..."
echo ""

echo "Cleaning previous builds..."
cargo clean

echo "Running clippy..."
cargo clippy --all-targets -- -D warnings

echo "Running tests..."
cargo test

echo "Ensuring Windows GNU target is installed..."
rustup target add "${WIN_TARGET}"

echo "Building Linux (host) release..."
cargo build --release

echo "Building Windows (${WIN_TARGET}) release..."
cargo build --target "${WIN_TARGET}" --release

# Optional: Further compress with UPX if available
echo ""
if command -v upx &> /dev/null; then
    echo "Compressing binaries with UPX..."
    upx --best --lzma "target/release/${BIN_NAME}" 2>/dev/null || echo "  WARNING: UPX compression failed for Linux binary"
    upx --best --lzma "target/${WIN_TARGET}/release/${BIN_NAME}.exe" 2>/dev/null || echo "  WARNING: UPX compression failed for Windows binary"
else
    echo "  UPX not found - skipping compression (install with: sudo apt install upx)"
fi

mkdir -p bin

echo ""
echo "Moving binaries to bin/ folder..."
mv "target/release/${BIN_NAME}" "bin/${BIN_NAME}-linux"
mv "target/${WIN_TARGET}/release/${BIN_NAME}.exe" "bin/${BIN_NAME}.exe"
cp src/modifiers/field_model_mappings.toml bin/field_model_mappings.toml

echo ""
echo "All builds completed successfully!"
echo ""
echo "Build artifacts and sizes:"
ls -lh "bin/${BIN_NAME}-linux" "bin/${BIN_NAME}.exe" | awk '{printf "  %-40s %s\n", $9, $5}'
