#!/usr/bin/env bash
set -euo pipefail

# Usage: [JOBS=n] scripts/build_nvk.sh [mesa-switch checkout]
#   Builds NVK, Mesa's Vulkan driver, for the Switch's GM20B GPU from danfromtico/mesa-switch
#   (https://github.com/danfromtico/mesa-switch, by danfromtico and NaGaa95) and stages it in nvk/:
#     nvk/lib/libvulkan.a      loaderless: it exports vkGetInstanceProcAddr and every vk* itself
#     nvk/include/vulkan       the Khronos headers it was built against (has vulkan_vi.h)
#     nvk/BUILD_INFO           the Mesa commit and toolchain versions
#   Linking it also needs -lexpat -lzstd -lz -lstdc++ -lm -lnx; the toolchain image already has
#   those portlibs (switch-libexpat, switch-libzstd and switch-zlib come with switch-mesa/sdl2).
#   The checkout defaults to ../mesa-switch. Build directories stay in the checkout, so a rerun
#   after editing Mesa only recompiles what changed.
#   bevy-horizon's own changes to mesa-switch are patches/mesa-switch/*.patch, applied in order to
#   the checkout unless they are there already, and BUILD_INFO lists them.
#
# This follows mesa-switch's own build-switch.sh and Docker.rust, with four differences:
#   - The image is bevy-horizon-toolchain plus Mesa's build needs, so Mesa is compiled by the same
#     devkitA64 (r30) and libnx that link our NROs. Docker.rust starts from devkita64:latest (r29.2).
#   - NAK, the shader compiler, is Rust. build-switch.sh leaves rustc-switch-wrapper.sh without a
#     target on Linux, which builds it for the host; the README asks for aarch64-unknown-linux-gnu,
#     so this sets MESA_SWITCH_RUST_TARGET (as build-unified.sh does).
#   - The wrappers are pointed at /usr/local/cargo/bin, because this image's rustup lives there and
#     not in /root/.cargo; otherwise they find themselves on PATH and recurse.
#   - Only the Vulkan driver is built (no Gallium), at -O2 as build-unified.sh's release SDK is.
IMAGE_NAME="bevy-horizon-nvk:local"
# Docker or Podman: CONTAINER_ENGINE picks one, otherwise docker if it's installed.
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"
BASE_IMAGE="bevy-horizon-toolchain:local"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MESA_DIR="$(cd "${1:-$WORKSPACE_DIR/../mesa-switch}" && pwd)"
OUT_DIR="$WORKSPACE_DIR/nvk"
JOBS="${JOBS:-4}"

if [ ! -f "$MESA_DIR/src/nouveau/vulkan/nvk_archive_merge.py" ]; then
    echo "error: $MESA_DIR is not a mesa-switch checkout (no src/nouveau/vulkan/nvk_archive_merge.py)" >&2
    exit 1
fi
MESA_PATCHES=()
# The patches stack: a later one may change lines that an earlier one added, after which the
# earlier one no longer reverse-applies on its own. So the longest series 0001..k that
# reverse-applies, last patch first, to a scratch copy of the files it touches counts as in the
# checkout already, and the rest are applied in order.
PATCH_FILES=()
for PATCH in "$WORKSPACE_DIR"/patches/mesa-switch/*.patch; do
    [ -e "$PATCH" ] && PATCH_FILES+=("$PATCH")
done
patches_in_checkout() {
    local count="$1" scratch path i ok=0
    scratch="$(mktemp -d)"
    git -C "$scratch" init -q
    for path in $(sed -n 's|^+++ b/||p' "${PATCH_FILES[@]:0:count}" | sort -u); do
        if [ -f "$MESA_DIR/$path" ]; then
            mkdir -p "$scratch/$(dirname "$path")"
            cp "$MESA_DIR/$path" "$scratch/$path"
        fi
    done
    for ((i = count - 1; i >= 0; i--)); do
        git -C "$scratch" apply --reverse "${PATCH_FILES[i]}" 2>/dev/null || { ok=1; break; }
    done
    rm -rf "$scratch"
    return "$ok"
}
PATCHES_IN=0
for ((k = ${#PATCH_FILES[@]}; k > 0; k--)); do
    if patches_in_checkout "$k"; then
        PATCHES_IN="$k"
        break
    fi
done
for ((i = 0; i < ${#PATCH_FILES[@]}; i++)); do
    PATCH="${PATCH_FILES[i]}"
    if [ "$i" -lt "$PATCHES_IN" ]; then
        echo "==> $(basename "$PATCH"): already in $MESA_DIR"
    else
        echo "==> Applying $(basename "$PATCH") to $MESA_DIR..."
        git -C "$MESA_DIR" apply "$PATCH"
    fi
    MESA_PATCHES+=("$(basename "$PATCH") $(sha256sum "$PATCH" | cut -c1-16)")
done
MESA_COMMIT="$(git -C "$MESA_DIR" rev-parse HEAD)"
MESA_DIRTY="$(git -C "$MESA_DIR" status --porcelain --untracked-files=no | wc -l)"

if ! "$ENGINE" image inspect "$BASE_IMAGE" >/dev/null 2>&1; then
    echo "error: build $BASE_IMAGE first (scripts/build_example.sh does)" >&2
    exit 1
fi

echo "==> Ensuring $IMAGE_NAME exists..."
if ! "$ENGINE" image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
    # SPIRV-Tools comes from source because Mesa needs 2024.1 or later and bookworm has 2023.1;
    # the tag is the one Docker.rust uses. cargo's registry is dropped from the image because
    # build_example.sh mounts its own over that path.
    "$ENGINE" build -t "$IMAGE_NAME" -f - "$WORKSPACE_DIR" <<EOF
FROM $BASE_IMAGE
RUN apt-get update && apt-get install -y --no-install-recommends \\
        python3-pip python3-setuptools python3-wheel python3-mako python3-yaml python3-packaging \\
        pkg-config flex bison clang libclang-dev \\
        llvm-15-dev libllvmspirvlib-15-dev libclc-15-dev libclang-15-dev \\
        zlib1g-dev libexpat1-dev libzstd-dev \\
    && rm -rf /var/lib/apt/lists/*
RUN pip3 install --break-system-packages meson
RUN git clone --depth 1 --branch vulkan-sdk-1.3.290.0 https://github.com/KhronosGroup/SPIRV-Tools.git /tmp/spirv-tools \\
    && git clone --depth 1 --branch vulkan-sdk-1.3.290.0 https://github.com/KhronosGroup/SPIRV-Headers.git /tmp/spirv-tools/external/spirv-headers \\
    && cmake -S /tmp/spirv-tools -B /tmp/spirv-tools/build -G Ninja -DCMAKE_BUILD_TYPE=Release \\
        -DCMAKE_INSTALL_PREFIX=/usr/local -DSPIRV_SKIP_TESTS=ON \\
    && cmake --build /tmp/spirv-tools/build -j $JOBS && cmake --install /tmp/spirv-tools/build \\
    && rm -rf /tmp/spirv-tools
RUN rustup target add aarch64-unknown-linux-gnu \\
    && cargo install --locked bindgen-cli cbindgen \\
    && rm -rf /usr/local/cargo/registry
EOF
fi

echo "==> Building NVK from $MESA_DIR ($MESA_COMMIT)..."
"$ENGINE" run --rm --log-driver=none \
    -e JOBS="$JOBS" -e MESA_COMMIT="$MESA_COMMIT" \
    -v "$MESA_DIR:/mesa:z" \
    -w /mesa \
    "$IMAGE_NAME" \
    bash -c 'set -euo pipefail
        # The steps below up to the native build are build-switch.sh'\''s container setup.
        mkdir -p /usr/local/libexec
        cp bindgen-switch-wrapper.sh /usr/local/libexec/bindgen
        cp rustc-switch-wrapper.sh /usr/local/libexec/rustc
        cp bindgen-atomic-shim.h /usr/local/libexec/bindgen-atomic-shim.h
        chmod +x /usr/local/libexec/bindgen /usr/local/libexec/rustc
        export MESA_SWITCH_RUSTC=/usr/local/cargo/bin/rustc
        export MESA_SWITCH_BINDGEN=/usr/local/cargo/bin/bindgen
        export MESA_SWITCH_RUST_TARGET=aarch64-unknown-linux-gnu

        cp src/gallium/winsys/nouveau/drm/nouveau.h src/nouveau/headers/nv_device_info.h \
            /opt/devkitpro/portlibs/switch/include/
        if [ ! -d /usr/lib/llvm-15/lib/clang/15/include ]; then
            ln -sf /usr/lib/llvm-15/lib/clang/15.0.6/include /usr/lib/llvm-15/lib/clang/15/include
        fi
        # Rust std asks for -ldl -lrt -lutil; src/nouveau/horizon/rust_switch_stubs.c supplies
        # the symbols, so empty archives satisfy the linker.
        echo "void __mesa_switch_stub_lib(void) {}" > /tmp/stub_posix.c
        aarch64-none-elf-gcc -c /tmp/stub_posix.c -o /tmp/stub_posix.o
        for L in dl rt util; do
            [ -f /opt/devkitpro/portlibs/switch/lib/lib$L.a ] ||
                aarch64-none-elf-ar rcs /opt/devkitpro/portlibs/switch/lib/lib$L.a /tmp/stub_posix.o
        done

        # setup reuses an existing build directory so reruns are incremental.
        setup() { local dir=$1; shift
            if [ -f "$dir/meson-private/coredata.dat" ]; then
                meson setup "$dir" --reconfigure "$@"
            else
                meson setup "$dir" "$@"
            fi
        }

        echo "=== Native host tools (mesa_clc, vtn_bindgen2) ==="
        setup builddir-native \
            -Dvulkan-drivers= -Dgallium-drivers= -Dshader-cache=enabled -Dplatforms= \
            -Dglx=disabled -Degl=disabled -Dopengl=false -Dgles1=disabled -Dgles2=disabled \
            -Dtools=[] -Dllvm=enabled -Dmesa-clc=enabled -Dprecomp-compiler=enabled \
            -Dinstall-mesa-clc=true
        ninja -C builddir-native -j "$JOBS" src/compiler/clc/mesa_clc src/compiler/spirv/vtn_bindgen2

        echo "=== NVK for Horizon ==="
        export PATH="/usr/local/libexec:/mesa/builddir-native/src/compiler/clc:/mesa/builddir-native/src/compiler/spirv:$PATH"
        # Proc-macro crates (paste, NAK'\''s nak_ir_proc) are loaded by rustc itself, so meson builds
        # them with the build machine'\''s Rust compiler. Without a native file meson finds "rustc"
        # on PATH, which is the wrapper above, and the wrapper adds the Switch target.
        # build-unified.sh gives meson a native rustc the same way (switch_native_tools_msys2.txt).
        printf "[binaries]\nrust = '\''%s'\''\n" "$MESA_SWITCH_RUSTC" > /usr/local/libexec/nvk-native.txt
        setup builddir-switch \
            --cross-file switch_cross_file.txt \
            --native-file /usr/local/libexec/nvk-native.txt \
            --default-library=static --buildtype=release -Doptimization=2 \
            -Db_lto=false -Db_ndebug=true \
            -Dvulkan-drivers=nouveau -Dgallium-drivers= -Dgallium-rusticl=false \
            -Dplatforms=switch -Dglx=disabled -Degl=disabled -Dopengl=false \
            -Dgles1=disabled -Dgles2=disabled -Dvideo-codecs= \
            -Dshader-cache=enabled -Dxmlconfig=enabled -Dexpat=enabled -Dtools=[] \
            -Dllvm=disabled -Dshared-glapi=disabled -Dshared-llvm=disabled \
            -Dmesa-clc=system -Dprecomp-compiler=system -Dcpp_rtti=false \
            -Dbuild-tests=false -Dnvk-build-id="$MESA_COMMIT"
        ninja -C builddir-switch -j "$JOBS" src/nouveau/vulkan/libvulkan.a

        # nvk_archive_merge.py writes the archive with the host ar, which cannot index AArch64
        # objects; without an index the linker finds nothing in it.
        aarch64-none-elf-ranlib builddir-switch/src/nouveau/vulkan/libvulkan.a

        {
            dkp-pacman -Q devkitA64 libnx switch-libexpat switch-libzstd switch-zlib
            echo "rustc (NAK): $(/usr/local/cargo/bin/rustc -V)"
            echo "meson: $(meson --version)"
        } > /mesa/builddir-switch/toolchain.txt
    '

echo "==> Staging into $OUT_DIR..."
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR/lib" "$OUT_DIR/include"
cp "$MESA_DIR/builddir-switch/src/nouveau/vulkan/libvulkan.a" "$OUT_DIR/lib/"
cp -r "$MESA_DIR/include/vulkan" "$MESA_DIR/include/vk_video" "$OUT_DIR/include/"
{
    echo "mesa-switch: $MESA_COMMIT ($(git -C "$MESA_DIR" remote get-url origin))"
    for PATCH in ${MESA_PATCHES[@]+"${MESA_PATCHES[@]}"}; do
        echo "  patched: $PATCH (patches/mesa-switch/, sha256 prefix)"
    done
    if [ "$MESA_DIRTY" != "0" ]; then
        echo "  with uncommitted changes to $MESA_DIRTY tracked file(s):"
        git -C "$MESA_DIR" status --porcelain --untracked-files=no | sed 's/^/    /'
    fi
    echo "built: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    cat "$MESA_DIR/builddir-switch/toolchain.txt"
} > "$OUT_DIR/BUILD_INFO"
cat "$OUT_DIR/BUILD_INFO"
ls -la "$OUT_DIR/lib"
echo "==> Done!"
