#!/bin/bash

# Local release helper: default toolchain / host from the environment (same idea as GitHub Actions).
# Produces bin/organise and copies field_model_mappings.toml alongside it.

set -euo pipefail

BIN_NAME="organise"
RELEASE_BIN="target/release/${BIN_NAME}"

echo "Running clippy..."
cargo clippy --all-targets -- -D warnings

echo "Running tests..."
cargo test

echo "Building release..."
cargo build --release

echo ""
if command -v upx &> /dev/null; then
    echo "Compressing binary with UPX..."
    upx --best --lzma "${RELEASE_BIN}" 2>/dev/null || echo "  WARNING: UPX compression failed"
else
    echo "  UPX not found - skipping compression (install with: sudo apt install upx)"
fi

mkdir -p bin

echo ""
echo "Copying to bin/..."
mv -f "${RELEASE_BIN}" "bin/${BIN_NAME}"
cp -f src/modifiers/field_model_mappings.toml bin/field_model_mappings.toml

echo ""
echo "Done."
ls -lh "bin/${BIN_NAME}" | awk '{printf "  %s  (%s %s)\n", $9, $5, $6}'
