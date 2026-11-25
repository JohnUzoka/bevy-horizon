#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/build_uam.sh
#   Builds the two native libraries the deko3d backend (crates/wgpu_deko3d) links on the console:
#     deko3d/lib/libuam.a         UAM's compiler plus dkp_uam_compile/dkp_uam_free (patches/uam)
#     deko3d/lib/libdeko3d_bh.a   deko3d (patches/deko3d), named apart from devkitPro's libdeko3d.a,
#     deko3d/lib/libdeko3dd_bh.a  and its debug build
#     deko3d/BUILD_INFO           the tags and patches they were built from
#
#   UAM is devkitPro's deko3d shader compiler (https://github.com/devkitPro/uam, fincs; zlib, with
#   Mesa's MIT GLSL frontend and nouveau codegen), built as a static library so the backend can
#   compile the GLSL that naga writes from a game's WGSL. The source is UAM v1.1.0, the version
#   devkitPro ships as its host `uam` tool, cloned into deko3d/src/uam with
#   patches/uam/console-library.patch applied:
#     - meson builds a static library with a C API in a cross build, and DKSH comes back in memory;
#     - Instruction::init initializes its scheduling data, which the GM107 scheduler read before
#       it was set, so stall counts depended on heap contents;
#     - an instruction ending a block that sets a barrier stalls 2, as in Mesa, since the next
#       block's waits aren't known yet;
#     - no dual issue: Mesa never dual-issues on GM107+, and UAM's experimental scheduling of it
#       has been reported to corrupt lighting on hardware.
#
#   deko3d (https://github.com/devkitPro/deko3d, fincs; zlib) is v0.5.0, the source of devkitPro's
#   deko3d 0.5.0-1 package, cloned into deko3d/src/deko3d with patches/deko3d applied:
#     - alpha-dst-blend-factor.patch: blend states write the alpha destination factor from
#       dstAlphaBlendFactor, not dstColorBlendFactor (devkitPro/deko3d#27);
#     - release-reports.patch: the release library hands its fatal errors and warnings (a GPU
#       fault's details, a failed kickoff's result) to dkBhReport, which the backend's bridge
#       writes to sdmc:/switch/deko3d.log; upstream's release library aborts without a word;
#     - multi-draw.patch: dkCmdBufMultiDraw(Indexed)Indirect draw a batch of indirect records
#       with new draw macros that loop over them, two gpfifo entries per batch rather than per
#       draw.
#   Its build generates headers with dekomme/dekodef (https://github.com/fincs/dekotools), which
#   devkitPro doesn't package; they are built for the container from deko3d/src/dekotools.
IMAGE_NAME="bevy-horizon-uam:local"
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"
BASE_IMAGE="bevy-horizon-toolchain:local"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$WORKSPACE_DIR/deko3d"
SRC_DIR="$OUT_DIR/src/uam"
PATCH="$WORKSPACE_DIR/patches/uam/console-library.patch"
UAM_TAG="v1.1.0"
DEKO3D_SRC_DIR="$OUT_DIR/src/deko3d"
DEKO3D_PATCHES=("$WORKSPACE_DIR/patches/deko3d/alpha-dst-blend-factor.patch"
                "$WORKSPACE_DIR/patches/deko3d/release-reports.patch"
                "$WORKSPACE_DIR/patches/deko3d/multi-draw.patch")
DEKO3D_TAG="v0.5.0"
DEKOTOOLS_SRC_DIR="$OUT_DIR/src/dekotools"
# dekotools has no tags; this is its master, whose only change since 2019 is an include newer
# compilers need.
DEKOTOOLS_COMMIT="c15523cea963c04fad77a46f23c3a96d26a86ab9"

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

# clone_patched <dir> <url> <tag or commit> [patch...]: clones the source once and applies the
# patches, accepting a source that already has them.
clone_patched() {
    local dir="$1" url="$2" rev="$3" patch
    shift 3
    if [ ! -d "$dir/.git" ]; then
        echo "==> Cloning $url $rev..."
        mkdir -p "$(dirname "$dir")"
        git clone -q "$url" "$dir"
        git -C "$dir" -c advice.detachedHead=false checkout -q "$rev"
    fi
    for patch in "$@"; do
        if git -C "$dir" apply --check "$patch" 2>/dev/null; then
            git -C "$dir" apply "$patch"
        elif ! git -C "$dir" apply --check --reverse "$patch" 2>/dev/null; then
            echo "error: $patch applies neither forwards nor backwards to $dir" >&2
            exit 1
        fi
    done
}
clone_patched "$SRC_DIR" https://github.com/devkitPro/uam "$UAM_TAG" "$PATCH"
clone_patched "$DEKO3D_SRC_DIR" https://github.com/devkitPro/deko3d "$DEKO3D_TAG" "${DEKO3D_PATCHES[@]}"
clone_patched "$DEKOTOOLS_SRC_DIR" https://github.com/fincs/dekotools "$DEKOTOOLS_COMMIT"

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

echo "==> Building deko3d for the console..."
"$ENGINE" run --rm --network none -v "$OUT_DIR:/deko3d:z" -w /deko3d "$IMAGE_NAME" bash -euo pipefail -c '
    if [[ ! -d build-dekotools ]]; then
        meson setup build-dekotools src/dekotools
    fi
    ninja -C build-dekotools
    export PATH=/deko3d/build-dekotools:/opt/devkitpro/devkitA64/bin:$PATH
    make -C src/deko3d -j"$(nproc)"
    cp src/deko3d/lib/libdeko3d.a lib/libdeko3d_bh.a
    cp src/deko3d/lib/libdeko3dd.a lib/libdeko3dd_bh.a
'
{
    echo "uam $UAM_TAG $(git -C "$SRC_DIR" rev-parse HEAD)"
    echo "patch $(sha256sum "$PATCH" | cut -d' ' -f1) patches/uam/console-library.patch"
    echo "deko3d $DEKO3D_TAG $(git -C "$DEKO3D_SRC_DIR" rev-parse HEAD)"
    for patch in "${DEKO3D_PATCHES[@]}"; do
        echo "patch $(sha256sum "$patch" | cut -d' ' -f1) patches/deko3d/$(basename "$patch")"
    done
    echo "dekotools $(git -C "$DEKOTOOLS_SRC_DIR" rev-parse HEAD)"
} > "$OUT_DIR/BUILD_INFO"
echo "==> Done: $OUT_DIR/lib/{libuam.a,libdeko3d_bh.a,libdeko3dd_bh.a}"
