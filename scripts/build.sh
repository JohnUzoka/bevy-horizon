#!/usr/bin/env bash
set -euo pipefail

IMAGE_NAME="bevy-horizon-toolchain:local"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "==> Building container image $IMAGE_NAME..."
podman build -t "$IMAGE_NAME" -f "$WORKSPACE_DIR/Containerfile" "$WORKSPACE_DIR"

echo "==> Running build in container..."
podman run --rm -it \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    cargo build -Z build-std=core,alloc,std,panic_abort \
        --target /workspace/aarch64-nintendo-horizon.json \
        --package bevy_horizon
