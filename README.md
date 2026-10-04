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

## Testing with `hello_switch`

A minimal 3D spinning cube demo with Joy-Con input is included under `examples/hello_switch`.

### 1. Compile to `.nro`

```bash
./scripts/build_example.sh
```

This produces `hello_switch.nro`.

### 2. Test in Emulator or Hardware

- **PC Emulator (Ryujinx / Sudachi)**:
  Drag and drop `hello_switch.nro` directly into the emulator window. Live logs will stream to your terminal.
- **Nintendo Switch (Atmosphere CFW)**:
  Copy `hello_switch.nro` to `sdmc:/switch/hello_switch.nro` on your SD card. Launch via **Title Override** (hold `R` while launching any game title) to ensure full memory access.

### Controls

| Action | Switch Controller | Desktop Keyboard |
|---|---|---|
| Adjust Rotation Speed | Left Stick / D-Pad | Arrow Keys |
| Cycle Cube Color | **(A)** Button | Space |
| Exit | **(+)** Button | Escape |

