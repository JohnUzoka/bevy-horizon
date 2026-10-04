#!/usr/bin/env bash
set -euo pipefail

IMAGE_NAME="bevy-horizon-toolchain:local"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "==> Ensuring toolchain image exists..."
if ! podman image exists "$IMAGE_NAME"; then
    echo "Building $IMAGE_NAME from $WORKSPACE_DIR/Containerfile..."
    podman build -t "$IMAGE_NAME" -f "$WORKSPACE_DIR/Containerfile" "$WORKSPACE_DIR"
fi

echo "==> Compiling hello_switch for Nintendo Switch (Horizon OS)..."
podman run --rm -it \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    cargo build --release -Z build-std=core,alloc,std,panic_abort \
        --target /workspace/aarch64-nintendo-horizon.json \
        --package hello_switch

echo "==> Packaging into hello_switch.nro Homebrew binary..."
podman run --rm -it \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    elf2nro target/aarch64-nintendo-horizon/release/hello_switch hello_switch.nro \
        --name="Hello Switch" \
        --author="bevy-horizon" \
        --version="0.1.0"

echo "==> Done! Output: $WORKSPACE_DIR/hello_switch.nro"
