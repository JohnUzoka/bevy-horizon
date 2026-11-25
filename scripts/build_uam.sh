#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/build_uam.sh
#   Builds UAM, devkitPro's deko3d shader compiler (https://github.com/devkitPro/uam, fincs; zlib,
#   with Mesa's MIT GLSL frontend and nouveau codegen), as a static library for the console, so the
#   deko3d backend (crates/wgpu_deko3d) can compile the GLSL that naga writes from a game's WGSL:
#     deko3d/lib/libuam.a     UAM's compiler plus dkp_uam_compile/dkp_uam_free (patches/uam)
#     deko3d/BUILD_INFO       the UAM tag and patch
#   The source is UAM v1.1.0, the version devkitPro ships as its host `uam` tool, cloned into
#   deko3d/src/uam with patches/uam/console-library.patch applied:
#     - meson builds a static library with a C API in a cross build, and DKSH comes back in memory;
#     - Instruction::init initializes its scheduling data, which the GM107 scheduler read before
#       it was set, so stall counts depended on heap contents;
#     - an instruction ending a block that sets a barrier stalls 2, as in Mesa, since the next
#       block's waits aren't known yet;
#     - no dual issue: Mesa never dual-issues on GM107+, and UAM's experimental scheduling of it
#       has been reported to corrupt lighting on hardware.
IMAGE_NAME="bevy-horizon-uam:local"
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"
BASE_IMAGE="bevy-horizon-toolchain:local"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$WORKSPACE_DIR/deko3d"
SRC_DIR="$OUT_DIR/src/uam"
PATCH="$WORKSPACE_DIR/patches/uam/console-library.patch"
UAM_TAG="v1.1.0"

if ! "$ENGINE" image inspect "$BASE_IMAGE" >/dev/null 2>&1; then
    "$ENGINE" build -t "$BASE_IMAGE" -f "$WORKSPACE_DIR/Containerfile" "$WORKSPACE_DIR"
fi
if ! "$ENGINE" image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
    echo "==> Building $IMAGE_NAME (UAM's generators: bison, flex, meson, mako)..."
    "$ENGINE" build -t "$IMAGE_NAME" - <<EOF
FROM $BASE_IMAGE
RUN apt-get update && apt-get install -y --no-install-recommends bison flex meson python3-mako && rm -rf /var/lib/apt/lists/*
EOF
fi

if [ ! -d "$SRC_DIR/.git" ]; then
    echo "==> Cloning UAM $UAM_TAG..."
    mkdir -p "$OUT_DIR/src"
    git clone -q --branch "$UAM_TAG" https://github.com/devkitPro/uam "$SRC_DIR"
fi
if git -C "$SRC_DIR" apply --check "$PATCH" 2>/dev/null; then
    git -C "$SRC_DIR" apply "$PATCH"
elif ! git -C "$SRC_DIR" apply --check --reverse "$PATCH" 2>/dev/null; then
    echo "error: $PATCH applies neither forwards nor backwards to $SRC_DIR" >&2
    exit 1
fi

echo "==> Cross-compiling UAM for the console..."
mkdir -p "$OUT_DIR/lib"
"$ENGINE" run --rm --network none -v "$OUT_DIR:/deko3d:z" -w /deko3d "$IMAGE_NAME" bash -euo pipefail -c '
    export PATH=/opt/devkitpro/devkitA64/bin:$PATH
    flags="'\''-march=armv8-a+crc+crypto'\'', '\''-mtune=cortex-a57'\'', '\''-mtp=soft'\'', '\''-fPIE'\'', '\''-D__SWITCH__'\'', '\''-isystem'\'', '\''/opt/devkitpro/libnx/include'\'', '\''-ffunction-sections'\'', '\''-fdata-sections'\''"
    link="'\''-specs=/opt/devkitpro/libnx/switch.specs'\'', '\''-L/opt/devkitpro/libnx/lib'\''"
    cat > /tmp/cross.ini <<EOF
[binaries]
c = '\''aarch64-none-elf-gcc'\''
cpp = '\''aarch64-none-elf-g++'\''
ar = '\''aarch64-none-elf-gcc-ar'\''
strip = '\''aarch64-none-elf-strip'\''

[host_machine]
system = '\''horizon'\''
cpu_family = '\''aarch64'\''
cpu = '\''cortex-a57'\''
endian = '\''little'\''

[properties]
needs_exe_wrapper = true

[built-in options]
c_args = [$flags]
cpp_args = [$flags]
c_link_args = [$link]
cpp_link_args = [$link]
EOF
    if [[ -d build ]]; then
        meson setup --reconfigure --cross-file /tmp/cross.ini build src/uam >/dev/null
    else
        meson setup --cross-file /tmp/cross.ini --buildtype release -Db_ndebug=true -Dstrip=false build src/uam
    fi
    ninja -C build
    # Meson makes a thin archive for an uninstalled library; the linker needs the objects themselves.
    rm -f lib/libuam.a
    aarch64-none-elf-gcc-ar crs lib/libuam.a build/libuam.a.p/*.o
'
{
    echo "uam $UAM_TAG $(git -C "$SRC_DIR" rev-parse HEAD)"
    echo "patch $(sha256sum "$PATCH" | cut -d' ' -f1) patches/uam/console-library.patch"
} > "$OUT_DIR/BUILD_INFO"
echo "==> Done: $OUT_DIR/lib/libuam.a"
