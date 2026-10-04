# bevy-horizon

Reusable Nintendo Switch (`target_os = "horizon"`) support layer and crate patches for Bevy Engine.

## Overview

This repository provides two components necessary to run Bevy games (including **IW4L**) on Nintendo Switch homebrew:

1. **`bevy_horizon` Plugin**:
   - Libnx service initialization (`romfsInit`, `socketInitializeDefault`).
   - Applet lifecycle integration (`appletMainLoop` graceful shutdown on Home button).
   - Joy-Con and Pro Controller input translated into standard Bevy `GamepadEvent` and `ButtonInput<GamepadButton>`.
   - Handheld (720p) vs Docked (1080p) dynamic resolution switching.

2. **Crate Patches (`patches/`)**:
   - `bevy_asset`: Routes Horizon file access through synchronous worker threads (bypassing missing `async-io`/`rustix`).
   - `memmap2`: Stubs out memory mapping on Horizon, falling back to safe heap reads.
   - `cpal`: Implements a `horizon` audio host driving devkitPro SDL2's audio callback.

## Using with Any Bevy Game

In your Bevy game's `Cargo.toml`:

```toml
[dependencies]
bevy = "0.19"
bevy_horizon = { git = "https://github.com/JohnUzoka/bevy-horizon", branch = "main" }

[patch.crates-io]
bevy_asset = { git = "https://github.com/JohnUzoka/bevy-horizon", branch = "main" }
memmap2 = { git = "https://github.com/JohnUzoka/bevy-horizon", branch = "main" }
cpal = { git = "https://github.com/JohnUzoka/bevy-horizon", branch = "main" }
```

In your game code:

```rust
use bevy::prelude::*;
use bevy_horizon::HorizonPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(HorizonPlugin)
        .run();
}
```

## Cross-Compiling with Podman

```bash
./scripts/build.sh
```
