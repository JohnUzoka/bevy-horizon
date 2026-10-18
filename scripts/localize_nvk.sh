#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/localize_nvk.sh
#   Turns nvk/lib/libvulkan.a (from scripts/build_nvk.sh) into nvk/lib/libvulkan_bh.a: one
#   relocatable object whose only global definitions are the vk* entry points, the top-down
#   region's nouveau_horizon_region_{alloc,free,stats} (patches/mesa-switch/), which bevy_horizon's
#   `top_down_alloc` allocator calls, and nouveau_horizon_tune (patch 0008), bevy_horizon's
#   `nvk_tune`.
#
# Why: libvulkan.a carries two Rust staticlibs, NAK (the shader compiler) and NIL (image layout),
#   and each bundles its own copy of Rust's std, core, alloc and compiler_builtins for
#   aarch64-unknown-linux-gnu. A Rust game brings its own std, built for Horizon. Linking both
#   clashes on every unhashed Rust symbol (rust_eh_personality, compiler_builtins' intrinsics) and,
#   because NAK was built by the same nightly as our toolchain, on the hashed allocator and panic
#   shims too (_RNvCs..._7___rustc12___rust_alloc). mesa-switch's rust_switch_stubs.c also defines
#   libc names (posix_memalign, writev, __errno_location...) for the Linux std, and the rest of
#   Mesa's helpers (ralloc, hash tables, NIR) would clash with devkitPro's Mesa (libEGL) in apps
#   that link both. A C app linking libvulkan.a never sees this: it has no std of its own.
#
# How: GNU ld -r, seeded with -u for every symbol that stays global, pulls exactly the archive
#   members a C link would (one copy of each std object) and binds their references to each other.
#   objcopy then makes every other definition local. References to libnx, newlib, libstdc++, expat, zstd
#   and zlib stay undefined and are resolved by the final link as before.
#   --force-group-allocation turns COMDAT group members into plain sections. A group survives ld -r
#   otherwise, and the final link keeps only the first copy of each group it sees: NAK's
#   DW.ref.rust_eh_personality lost to the one in any game crate with extern "C-unwind" calls
#   (mlua, for Skate 3), and the object's now-local references to it were left pointing into the
#   discarded copy ("relocation refers to a symbol in a discarded section").
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="bevy-horizon-toolchain:local"
# Docker or Podman: CONTAINER_ENGINE picks one, otherwise docker if it's installed.
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"

if [ ! -f "$WORKSPACE_DIR/nvk/lib/libvulkan.a" ]; then
    echo "error: nvk/lib/libvulkan.a is missing; run scripts/build_nvk.sh first" >&2
    exit 1
fi

"$ENGINE" run --rm --log-driver=none -v "$WORKSPACE_DIR/nvk:/nvk:z" "$IMAGE" bash -euo pipefail -c '
    BIN=/opt/devkitpro/devkitA64/bin/aarch64-none-elf
    cd /nvk/lib
    work=$(mktemp -d)

    # Only the Vulkan API (vkCreateInstance...), all defined in vulkan-dispatch.o, which is libnvk
    # whole-archived by nvk_archive_merge.py. Mesa'\''s own vk_* helpers are internal: seeding with
    # them pulls other members first, and some (the nv_push_cl*.c.o class headers) are already
    # inside vulkan-dispatch.o, so both copies would be linked.
    # The top-down region'\''s API (patches/mesa-switch/) stays global too, for bevy_horizon'\''s
    # `top_down_alloc` allocator, and so does the measurement switches'\'' setter (patch 0008).
    KEEP_RE="^(vk[A-Z]|nouveau_horizon_region_(alloc|free|stats)\$|nouveau_horizon_tune\$)"
    $BIN-nm -g --defined-only libvulkan.a 2>/dev/null \
        | awk -v re="$KEEP_RE" "NF == 3 && \$2 ~ /^[TDRBVW]\$/ && \$3 ~ re { print \$3 }" \
        | sort -u > "$work/keep.txt"
    echo "==> $(grep -c "^vk" "$work/keep.txt") vk* entry points and $(grep -vc "^vk" "$work/keep.txt") bevy-horizon API functions stay global"

    $BIN-ld -r --force-group-allocation -o "$work/vulkan_bh.o" $(sed "s/^/-u /" "$work/keep.txt") libvulkan.a
    $BIN-objcopy --keep-global-symbols="$work/keep.txt" "$work/vulkan_bh.o"

    # Anything else still global and defined would clash in the final link.
    $BIN-nm -g --defined-only "$work/vulkan_bh.o" | awk -v re="$KEEP_RE" "\$3 !~ re" > "$work/leftover.txt"
    if [ -s "$work/leftover.txt" ]; then
        echo "error: globals other than the API survived:" >&2
        head -20 "$work/leftover.txt" >&2
        exit 1
    fi
    if $BIN-readelf -g "$work/vulkan_bh.o" | grep -q "COMDAT group"; then
        echo "error: COMDAT groups survived --force-group-allocation; a game with its own copy of" >&2
        echo "       one would discard ours and leave our now-local references to it dangling:" >&2
        $BIN-readelf -g "$work/vulkan_bh.o" | grep "COMDAT group" | head -20 >&2
        exit 1
    fi
    $BIN-nm -u "$work/vulkan_bh.o" | awk "{ print \$NF }" | sort -u > undefined_bh.txt
    echo "==> $(wc -l < undefined_bh.txt) undefined symbols for the final link (nvk/lib/undefined_bh.txt)"

    rm -f libvulkan_bh.a
    $BIN-ar rcs libvulkan_bh.a "$work/vulkan_bh.o"
    rm -rf "$work"
'
ls -la "$WORKSPACE_DIR/nvk/lib"
