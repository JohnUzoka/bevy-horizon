#!/usr/bin/env bash
set -euo pipefail

# Usage: [JOBS=n] scripts/build_example.sh [example] [title]
#   example  a package under examples/ (default hello_switch)
#   title    the name the HOME menu and sphaira show (default: from the example's name)
#   JOBS     caps cargo's parallel jobs; each rustc on a Bevy crate can take 1-2 GB of RAM
# examples/<example>/romfs, if it exists, is packed into the NRO as its romfs.
IMAGE_NAME="bevy-horizon-toolchain:local"
# Docker or Podman: CONTAINER_ENGINE picks one, otherwise docker if it's installed.
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="${1:-hello_switch}"
TITLE="${2:-$(echo "$EXAMPLE" | sed -E 's/_/ /g; s/\b(.)/\u\1/g')}"
if [ ! -f "$WORKSPACE_DIR/examples/$EXAMPLE/Cargo.toml" ]; then
    echo "error: no examples/$EXAMPLE/Cargo.toml" >&2
    exit 1
fi
ROMFS=""
if [ -d "$WORKSPACE_DIR/examples/$EXAMPLE/romfs" ]; then
    ROMFS="examples/$EXAMPLE/romfs"
fi
# Crates downloaded once stay in this volume between builds.
REGISTRY_VOLUME="bh-cargo-registry:/usr/local/cargo/registry"

echo "==> Ensuring toolchain image exists..."
if ! "$ENGINE" image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
    echo "Building $IMAGE_NAME from $WORKSPACE_DIR/Containerfile..."
    "$ENGINE" build -t "$IMAGE_NAME" -f "$WORKSPACE_DIR/Containerfile" "$WORKSPACE_DIR"
fi

echo "==> Compiling $EXAMPLE for Nintendo Switch (Horizon OS)..."
# std is built from source (-Z build-std) as its own workspace, locked by the Cargo.lock in the
# toolchain's rust-src. .cargo/config.toml patches libc for that workspace too, but cargo keeps the
# crates.io libc that lock pins, so first move the lock onto our libc. That lock is the container's
# copy, thrown away with it (--rm). The grep stops the build if std would still use crates.io's.
"$ENGINE" run --rm --log-driver=none \
    -e EXAMPLE="$EXAMPLE" -e JOBS="${JOBS:-}" \
    -v "$REGISTRY_VOLUME" \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    sh -c 'set -e
        STD_LIB="$(rustc --print sysroot)/lib/rustlib/src/rust/library"
        cargo update -p libc --manifest-path "$STD_LIB/Cargo.toml"
        if grep -A2 "^name = \"libc\"" "$STD_LIB/Cargo.lock" | grep -q "^source = "; then
            echo "error: std would build against crates.io libc, not patches/libc" >&2
            exit 1
        fi
        cargo build --release ${JOBS:+-j "$JOBS"} \
            -Z build-std=core,alloc,std,panic_abort -Z json-target-spec \
            --target /workspace/aarch64-nintendo-horizon.json \
            --package "$EXAMPLE"'

echo "==> Checking every crate patch replaced its crates.io version..."
# A [patch] only replaces crates.io versions it's semver-compatible with, and cargo doesn't warn
# when a patch that is also a workspace member goes unused. patches/bevy_asset was 0.19.0 while Bevy
# locked 0.19.1, so every build used crates.io's bevy_asset without saying so.
"$ENGINE" run --rm --log-driver=none -i \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    python3 - <<'EOF'
import re, sys
def family(version):  # the versions one semver-compatible requirement can match
    major, minor, patch = (int(x) for x in re.match(r"(\d+)\.(\d+)\.(\d+)", version).groups())
    return (major,) if major else (0, minor) if minor else (0, 0, patch)
section = open("Cargo.toml").read().split("[patch.crates-io]")[1].split("\n[")[0]
locked = [(re.search(r'name = "([^"]+)"', p)[1], re.search(r'version = "([^"]+)"', p)[1],
           "source = " in p) for p in open("Cargo.lock").read().split("[[package]]")[1:]]
failed = False
# A renamed key (`getrandom02 = { path = ..., package = "getrandom" }`) patches a second version.
for key, path, rest in re.findall(r'^([\w-]+) = \{ path = "([^"]+)"(.*)$', section, re.M):
    name = (re.search(r'package = "([^"]+)"', rest) or [None, key])[1]
    ours = re.search(r'^version = "([^"]+)"', open(f"{path}/Cargo.toml").read(), re.M)[1]
    for other in [v for n, v, registry in locked if n == name and registry]:
        if family(other) == family(ours):
            print(f"error: {path} is {name} {ours}, but Cargo.lock still has crates.io's {name} "
                  f"{other}, so the patch isn't used. Re-make it from {name} {other}.", file=sys.stderr)
            failed = True
sys.exit(1 if failed else 0)
EOF

# elf2nro has no --name/--author/--version flags (those belong to nacptool) and
# silently ignores them. Without --nacp/--icon the NRO has no asset section, and
# sphaira forwarders reject it (they require file size >= image size + 0x80) by
# calling diagAbortWithResult(0) before any of our code runs.
echo "==> Packaging into $EXAMPLE.nro Homebrew binary${ROMFS:+ with romfs from $ROMFS}..."
"$ENGINE" run --rm --log-driver=none \
    -e EXAMPLE="$EXAMPLE" -e TITLE="$TITLE" -e ROMFS="$ROMFS" \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    sh -c 'set -e
        OUT=target/aarch64-nintendo-horizon/release
        nacptool --create "$TITLE" "bevy-horizon" "0.1.0" "$OUT/$EXAMPLE.nacp"
        elf2nro "$OUT/$EXAMPLE" "$EXAMPLE.nro" \
            --nacp="$OUT/$EXAMPLE.nacp" \
            --icon=/opt/devkitpro/libnx/default_icon.jpg \
            ${ROMFS:+--romfsdir="$ROMFS"}'

echo "==> Verifying NRO asset section..."
# -i is required: without it the engine doesn't forward the heredoc, and python3 runs an empty script.
"$ENGINE" run --rm --log-driver=none -i \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    python3 - "$EXAMPLE.nro" "$ROMFS" <<'EOF'
import os, struct, sys
path, romfs_dir = sys.argv[1], sys.argv[2]
with open(path, "rb") as f:
    head = f.read(0x80)
    image_size = struct.unpack_from("<I", head, 0x18)[0]
    f.seek(image_size)
    asset = f.read(0x38)
file_size = os.path.getsize(path)
if head[0x10:0x14] != b"NRO0" or asset[:4] != b"ASET" or file_size < image_size + 0x80:
    sys.exit(f"error: {path} has no asset section (image 0x{image_size:x}, file 0x{file_size:x}); "
             "sphaira forwarders will refuse to load it")
# ASET header: magic, version, then (offset, size) pairs for the icon, the NACP and the romfs.
romfs_size = struct.unpack_from("<Q", asset, 0x30)[0]
if romfs_dir and romfs_size == 0:
    sys.exit(f"error: {path} should carry {romfs_dir} as its romfs, but its romfs section is empty")
print(f"    ok: image 0x{image_size:x} + ASET section, romfs {romfs_size} bytes, file 0x{file_size:x} bytes")
EOF

echo "==> Done! Output: $WORKSPACE_DIR/$EXAMPLE.nro"
