#!/bin/bash

# Build script for workbench-preprocessor
# Builds for both Linux and Windows targets

set -e  # Exit on any error

BIN_NAME="organise"

echo "Building ${BIN_NAME} for multiple targets..."
echo ""

echo "Cleaning previous builds..."
cargo clean

echo "Running clippy..."
cargo clippy -- -D warnings

echo "Running tests..."
cargo test

echo "Building Linux (x86_64-unknown-linux-gnu) release..."
cargo build --release

echo "Building Windows (x86_64-pc-windows-gnu) release..."
cargo build --target x86_64-pc-windows-gnu --release

# Optional: Further compress with UPX if available
echo ""
if command -v upx &> /dev/null; then
    echo "Compressing binaries with UPX..."
    upx --best --lzma target/release/${BIN_NAME} 2>/dev/null || echo "  ⚠️  UPX compression failed for Linux binary"
    upx --best --lzma target/x86_64-pc-windows-gnu/release/${BIN_NAME}.exe 2>/dev/null || echo "  ⚠️  UPX compression failed for Windows binary"
else
    echo "  UPX not found - skipping compression (install with: sudo apt install upx)"
fi

mkdir -p bin

echo ""
echo "Moving binaries to bin/ folder..."
mv target/release/${BIN_NAME} bin/${BIN_NAME}-linux
mv target/x86_64-pc-windows-gnu/release/${BIN_NAME}.exe bin/${BIN_NAME}.exe
cp src/modifiers/field_model_mappings.toml bin/field_model_mappings.toml

echo ""
echo "All builds completed successfully!"
echo ""
echo "Build artifacts and sizes:"
ls -lh bin/${BIN_NAME}-linux bin/${BIN_NAME}.exe | awk '{printf "  %-40s %s\n", $9, $5}'
