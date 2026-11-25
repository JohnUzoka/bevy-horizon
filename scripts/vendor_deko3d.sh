#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/vendor_deko3d.sh <game checkout>
#   Copies the deko3d backend (crates/wgpu_deko3d) into <game checkout>/third_party/wgpu_deko3d for
#   a game on wgpu 30: its src/, native/ and build.rs, Cargo.wgpu-30.toml as its Cargo.toml (a
#   standalone package with the `wgpu-30` feature on by default), and this repository's licence
#   texts. The folder is replaced whole, so rerun this after changing the backend here rather than
#   editing the copy. Its build.rs links libuam.a and the patched deko3d (libdeko3d_bh.a) from
#   $BEVY_HORIZON_UAM_LIB (point it at this repository's deko3d/lib).
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ $# -ne 1 ] || [ ! -d "$1" ]; then
    echo "usage: $0 <game checkout>" >&2
    exit 1
fi
SOURCE="$WORKSPACE_DIR/crates/wgpu_deko3d"
DEST="$(cd "$1" && pwd)/third_party/wgpu_deko3d"
COMMIT="$(git -C "$WORKSPACE_DIR" rev-parse HEAD)"
if [ -n "$(git -C "$WORKSPACE_DIR" status --porcelain -- crates/wgpu_deko3d)" ]; then
    COMMIT="$COMMIT, with uncommitted changes"
fi

rm -rf "$DEST"
mkdir -p "$DEST"
cp -R "$SOURCE/src" "$SOURCE/native" "$SOURCE/build.rs" "$DEST/"
cp "$SOURCE/Cargo.wgpu-30.toml" "$DEST/Cargo.toml"
cp "$WORKSPACE_DIR/LICENSE-MIT" "$WORKSPACE_DIR/LICENSE-APACHE" "$DEST/"
cat > "$DEST/README.md" <<EOF
# wgpu_deko3d

Generated from bevy-horizon's \`crates/wgpu_deko3d\` (https://github.com/JohnUzoka/bevy-horizon)
by its \`scripts/vendor_deko3d.sh\`, at commit
$COMMIT.
Do not edit these files by hand: change the backend in bevy-horizon and run the script again.

A wgpu-hal backend on deko3d, the Nintendo Switch's own GPU API; \`wgpu_deko3d::instance()\` makes a
\`wgpu::Instance\` on it. Building it for the Switch needs \`BEVY_HORIZON_UAM_LIB\` set to a directory
holding \`libuam.a\` and \`libdeko3d_bh.a\` (bevy-horizon's \`deko3d/lib\`, from its
\`scripts/build_uam.sh\`). An instance made with \`wgpu::Instance::from_hal\` takes wgpu-core's default
flags, so the game's wgpu-core patch should drop \`VALIDATION_INDIRECT_CALL\` there as bevy-horizon's
does. Licensed MIT OR Apache-2.0 (\`LICENSE-MIT\`, \`LICENSE-APACHE\`). bevy-horizon's
\`docs/deko3d.md\` has the design and limits.
EOF
echo "vendored crates/wgpu_deko3d ($COMMIT) into $DEST"
