#!/usr/bin/env bash
set -euo pipefail

# Usage: probes/nvk_probe/build.sh
#   Builds target/nvk_probe/nvk_probe.nro: a plain C program that drives NVK (mesa-switch's
#   Vulkan driver) directly and logs what it finds to sdmc:/switch/nvk_probe/log.txt, then draws
#   a spinning triangle. It answers "does NVK work on this console, and what does it support?"
#   without wgpu or Bevy in the way.
#
#   Needs nvk/lib/libvulkan_bh.a: scripts/build_nvk.sh, then scripts/localize_nvk.sh.
#   The shaders are prebuilt into tri_vert.h and tri_frag.h; to rebuild them after editing
#   tri.vert or tri.frag (glslang-tools, Khronos' reference compiler):
#     glslangValidator -V --vn tri_vert_spv -o tri_vert.h tri.vert
#     glslangValidator -V --vn tri_frag_spv -o tri_frag.h tri.frag
PROBE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$PROBE_DIR/../.." && pwd)"
IMAGE="bevy-horizon-toolchain:local"
# Docker or Podman: CONTAINER_ENGINE picks one, otherwise docker if it's installed.
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"
OUT="$WORKSPACE_DIR/target/nvk_probe"

if [ ! -f "$WORKSPACE_DIR/nvk/lib/libvulkan_bh.a" ]; then
    echo "error: nvk/lib/libvulkan_bh.a is missing; run scripts/build_nvk.sh and scripts/localize_nvk.sh" >&2
    exit 1
fi
mkdir -p "$OUT"

"$ENGINE" run --rm \
    -v "$PROBE_DIR:/probe:ro,z" \
    -v "$WORKSPACE_DIR/nvk:/nvk:ro,z" \
    -v "$OUT:/out:z" \
    "$IMAGE" bash -euo pipefail -c '
    export PATH=/opt/devkitpro/devkitA64/bin:/opt/devkitpro/tools/bin:$PATH
    # The flags libnx'\''s template Makefile uses.
    ARCH="-march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE"
    aarch64-none-elf-gcc $ARCH -D__SWITCH__ -O2 -g -Wall -Wextra -Wno-unused-parameter \
        -ffunction-sections -fdata-sections \
        -I/opt/devkitpro/libnx/include -I/nvk/include \
        -c /probe/main.c -o /out/main.o
    # --wrap=threadCreate: NAK is Rust and finds its thread-locals through TPIDR_EL0, which libnx
    # never sets; main.c sets it on every thread, as bevy_horizon does for Rust apps.
    aarch64-none-elf-gcc $ARCH -specs=/opt/devkitpro/libnx/switch.specs -g \
        -Wl,--gc-sections -Wl,--wrap=threadCreate -Wl,-Map,/out/nvk_probe.map \
        /out/main.o \
        -L/nvk/lib -lvulkan_bh \
        -L/opt/devkitpro/portlibs/switch/lib -lexpat -lzstd -lz -lstdc++ \
        -L/opt/devkitpro/libnx/lib -lnx -lm \
        -o /out/nvk_probe.elf
    nacptool --create "nvk_probe" "bevy-horizon" "0.1.0" /out/nvk_probe.nacp
    elf2nro /out/nvk_probe.elf /out/nvk_probe.nro --nacp=/out/nvk_probe.nacp
'
ls -la "$OUT/nvk_probe.nro"
