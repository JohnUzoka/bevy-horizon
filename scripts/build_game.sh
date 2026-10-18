#!/usr/bin/env bash
set -euo pipefail

# Builds a Bevy game from its own repository into an NRO.
# Usage: [JOBS=n] [BEVY=0.18|0.19] [CARGO_ARGS="..."] scripts/build_game.sh <game_dir> <package> <bin> [title]
#   game_dir    the game's Cargo workspace, checked out next to this repository (its Cargo.toml
#               reaches bevy_horizon through ../bevy-horizon, which is where the container mounts it)
#   package     the package with the game's binary
#   bin         that binary
#   title       the name the HOME menu and sphaira show (default: bin)
#   BEVY        the game's Bevy version, by default bevy_app's in the game's Cargo.lock;
#               .cargo/bevy-$BEVY.toml has the [patch] table for it
#   CARGO_ARGS  more `cargo build` arguments, e.g. "--no-default-features"
#   JOBS        caps cargo's parallel jobs; each rustc on a Bevy crate can take 1-2 GB of RAM
#   ROMFS       a directory inside game_dir to pack into the NRO as its romfs
#   AUTHOR      the author the HOME menu and sphaira show (default: bevy-horizon)
#   VERSION     the version they show (default: 0.1.0)
#   ICON        the icon they show: a 256x256 JPEG (default: libnx's)
# The NRO lands in <game_dir>/target/aarch64-nintendo-horizon/release/<bin>.nro.
# The game's Cargo.lock is changed in place: each patched crate is moved to the patch's version.
# docs/porting-a-bevy-game.md has what the game itself needs.
IMAGE_NAME="bevy-horizon-toolchain:local"
# Docker or Podman: CONTAINER_ENGINE picks one, otherwise docker if it's installed.
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ $# -lt 3 ]; then
    sed -n '4,21p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 1
fi
GAME_DIR="$(cd "$1" && pwd)"
PACKAGE="$2"
BIN="$3"
TITLE="${4:-$BIN}"
GAME_NAME="$(basename "$GAME_DIR")"
if [ -z "${BEVY:-}" ]; then
    # A wrong table isn't an error to cargo: patches for another Bevy are skipped with a warning,
    # and the game builds against crates.io's wgpu, which can't draw on Horizon.
    if [ -f "$GAME_DIR/Cargo.lock" ]; then
        BEVY="$(sed -n '/^name = "bevy_app"$/{n;s/^version = "\([0-9]*\.[0-9]*\)\..*"$/\1/p;}' "$GAME_DIR/Cargo.lock" | sort -u | paste -sd' ')"
    fi
    case "${BEVY:-}" in
        "" | *" "*)
            echo "error: can't tell $GAME_NAME's Bevy version from bevy_app in its Cargo.lock (found: ${BEVY:-none}); set BEVY=0.18 or BEVY=0.19" >&2
            exit 1 ;;
    esac
    echo "==> $GAME_NAME's Cargo.lock has Bevy $BEVY (bevy_app)"
fi
PATCHES=".cargo/bevy-$BEVY.toml"
if [ ! -f "$WORKSPACE_DIR/$PATCHES" ]; then
    echo "error: no $PATCHES for Bevy $BEVY" >&2
    exit 1
fi
if [ ! -f "$GAME_DIR/Cargo.toml" ]; then
    echo "error: no $GAME_DIR/Cargo.toml" >&2
    exit 1
fi
if [ -n "${ICON:-}" ]; then
    # Checked before the build, which takes a while; the HOME menu wants 256x256.
    python3 - "$ICON" <<'EOF'
import sys
path = sys.argv[1]
try:
    data = open(path, "rb").read()
except OSError as error:
    sys.exit(f"error: ICON: {error}")
size, at = None, 2
while data[:2] == b"\xff\xd8" and at + 9 <= len(data) and data[at] == 0xFF:
    marker = data[at + 1]
    if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):  # a start of frame
        size = (int.from_bytes(data[at + 7:at + 9], "big"), int.from_bytes(data[at + 5:at + 7], "big"))
        break
    at += 2 + int.from_bytes(data[at + 2:at + 4], "big")
if size != (256, 256):
    found = f"{size[0]}x{size[1]}" if size else "not a JPEG"
    sys.exit(f"error: ICON={path} must be a 256x256 JPEG ({found})")
EOF
    ICON="$(cd "$(dirname "$ICON")" && pwd)/$(basename "$ICON")"
fi
REGISTRY_VOLUME="bh-cargo-registry:/usr/local/cargo/registry"
# The game sees bevy-horizon where it expects it, next to itself.
MOUNTS=(-v "$WORKSPACE_DIR:/src/bevy-horizon:z" -v "$GAME_DIR:/src/$GAME_NAME:z")
CONFIG="--config /src/bevy-horizon/.cargo/config.toml --config /src/bevy-horizon/$PATCHES"

echo "==> Ensuring toolchain image exists..."
if ! "$ENGINE" image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
    "$ENGINE" build -t "$IMAGE_NAME" -f "$WORKSPACE_DIR/Containerfile" "$WORKSPACE_DIR"
fi

echo "==> Moving $GAME_NAME's Cargo.lock onto the patches in $PATCHES..."
# A [patch] replaces only crates.io versions it's semver-compatible with, and cargo keeps a locked
# crates.io version even when a compatible patch has another one ("Patch ... was not used in the
# crate graph"). So each patched crate is moved to the patch's version first, then checked.
"$ENGINE" run --rm --log-driver=none -i \
    -e CONFIG="$CONFIG" \
    -v "$REGISTRY_VOLUME" "${MOUNTS[@]}" \
    -w "/src/$GAME_NAME" \
    "$IMAGE_NAME" \
    python3 - <<'EOF'
import os, re, subprocess, sys
config = os.environ["CONFIG"].split()
def family(version):  # the versions one semver-compatible requirement can match
    major, minor, patch = (int(x) for x in re.match(r"(\d+)\.(\d+)\.(\d+)", version).groups())
    return (major,) if major else (0, minor) if minor else (0, 0, patch)
def patches():
    # Config files resolve paths from the parent of their directory, manifests from their own.
    tables = [(f, os.path.dirname(os.path.dirname(f))) for f in config if f.endswith(".toml")]
    tables.append(("Cargo.toml", "."))
    for file, base in tables:
        text = open(file).read()
        if "[patch.crates-io]" not in text:
            continue
        section = text.split("[patch.crates-io]")[1].split("\n[")[0]
        # A renamed key (`getrandom03 = { path = ..., package = "getrandom" }`) patches a second version.
        for key, path, rest in re.findall(r'^([\w-]+) = \{ path = "([^"]+)"(.*)$', section, re.M):
            name = (re.search(r'package = "([^"]+)"', rest) or [None, key])[1]
            path = os.path.normpath(os.path.join(base, path))
            ours = re.search(r'^version = "([^"]+)"', open(f"{path}/Cargo.toml").read(), re.M)[1]
            yield name, ours, path
def locked():
    return [(re.search(r'name = "([^"]+)"', p)[1], re.search(r'version = "([^"]+)"', p)[1],
             "source = " in p) for p in open("Cargo.lock").read().split("[[package]]")[1:]]
def stale():
    return [(name, other, ours, path) for name, ours, path in patches()
            for n, other, registry in locked() if n == name and registry and family(other) == family(ours)]
# A patch with the locked version replaces crates.io's as soon as cargo resolves.
subprocess.run(["cargo", "metadata", *config, "--format-version", "1"], check=True, stdout=subprocess.DEVNULL)
# One at a time: an update can move other crates too (polling's moved rustix).
tried = set()
while todo := [s for s in stale() if s[1] != s[2] and s[:2] not in tried]:
    name, other, ours, path = todo[0]
    tried.add((name, other))
    print(f"    {name} {other} -> {ours} ({path})", flush=True)
    subprocess.run(["cargo", "update", *config, "-p", f"{name}@{other}", "--precise", ours], check=True)
left = stale()
for name, other, ours, path in left:
    print(f"error: {path} is {name} {ours}, but Cargo.lock still has crates.io's {name} {other}, "
          f"so the patch isn't used. Re-make it from {name} {other}.", file=sys.stderr)
sys.exit(1 if left else 0)
EOF

echo "==> Compiling $PACKAGE ($BIN) for Nintendo Switch (Horizon OS)..."
# std's own workspace is moved onto patches/libc first, as in build_example.sh.
"$ENGINE" run --rm --log-driver=none \
    -e PACKAGE="$PACKAGE" -e BIN="$BIN" -e JOBS="${JOBS:-}" -e CONFIG="$CONFIG" \
    -e CARGO_ARGS="${CARGO_ARGS:-}" \
    -v "$REGISTRY_VOLUME" "${MOUNTS[@]}" \
    -w "/src/$GAME_NAME" \
    "$IMAGE_NAME" \
    sh -c 'set -e
        STD_LIB="$(rustc --print sysroot)/lib/rustlib/src/rust/library"
        cargo update $CONFIG -p libc --manifest-path "$STD_LIB/Cargo.toml"
        if grep -A2 "^name = \"libc\"" "$STD_LIB/Cargo.lock" | grep -q "^source = "; then
            echo "error: std would build against crates.io libc, not patches/libc" >&2
            exit 1
        fi
        cargo build --release ${JOBS:+-j "$JOBS"} $CONFIG \
            -Z build-std=core,alloc,std,panic_abort -Z json-target-spec \
            --target /src/bevy-horizon/aarch64-nintendo-horizon.json \
            --package "$PACKAGE" --bin "$BIN" $CARGO_ARGS'

echo "==> Packaging into $BIN.nro${ROMFS:+ with romfs from $ROMFS}..."
# The NACP and icon are what sphaira forwarders need (scripts/build_example.sh).
PACK_MOUNTS=("${MOUNTS[@]}")
if [ -n "${ICON:-}" ]; then
    PACK_MOUNTS+=(-v "$ICON:/src/icon.jpg:ro,z")
fi
"$ENGINE" run --rm --log-driver=none \
    -e BIN="$BIN" -e TITLE="$TITLE" -e ROMFS="${ROMFS:-}" \
    -e AUTHOR="${AUTHOR:-bevy-horizon}" -e VERSION="${VERSION:-0.1.0}" -e ICON="${ICON:+/src/icon.jpg}" \
    "${PACK_MOUNTS[@]}" \
    -w "/src/$GAME_NAME" \
    "$IMAGE_NAME" \
    sh -c 'set -e
        OUT=target/aarch64-nintendo-horizon/release
        nacptool --create "$TITLE" "$AUTHOR" "$VERSION" "$OUT/$BIN.nacp"
        elf2nro "$OUT/$BIN" "$OUT/$BIN.nro" \
            --nacp="$OUT/$BIN.nacp" \
            --icon="${ICON:-/opt/devkitpro/libnx/default_icon.jpg}" \
            ${ROMFS:+--romfsdir="$ROMFS"}'

echo "==> Verifying NRO asset section..."
NRO="$GAME_DIR/target/aarch64-nintendo-horizon/release/$BIN.nro"
python3 - "$NRO" <<'EOF'
import os, struct, sys
path = sys.argv[1]
with open(path, "rb") as f:
    head = f.read(0x80)
    image_size = struct.unpack_from("<I", head, 0x18)[0]
    f.seek(image_size)
    asset = f.read(0x38)
    nacp = b""
    if asset[:4] == b"ASET":
        icon_size, nacp_offset, nacp_size = struct.unpack_from("<QQQ", asset, 0x10)
        f.seek(image_size + nacp_offset)
        nacp = f.read(nacp_size)
file_size = os.path.getsize(path)
if head[0x10:0x14] != b"NRO0" or asset[:4] != b"ASET" or file_size < image_size + 0x80:
    sys.exit(f"error: {path} has no asset section (image 0x{image_size:x}, file 0x{file_size:x}); "
             "sphaira forwarders will refuse to load it")
print(f"    ok: image 0x{image_size:x} + ASET section, file 0x{file_size:x} bytes")
text = lambda start, end: nacp[start:end].split(b"\0")[0].decode("utf-8", "replace")
print(f"    NACP: {text(0, 0x200)!r} by {text(0x200, 0x300)!r}, version {text(0x3060, 0x3070)!r}; "
      f"icon {icon_size} bytes")
EOF

echo "==> Done! Output: $NRO"
