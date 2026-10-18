# bevy-horizon

Reusable Nintendo Switch (`target_os = "horizon"`) support layer and crate patches for Bevy Engine.

## Overview

This repository provides two components necessary to run Bevy games (including **IW4L**) on Nintendo Switch homebrew:

1. **`bevy_horizon` Plugin**:
   - Libnx service initialization (`romfsInit`, `socketInitializeDefault`).
   - Applet lifecycle: an exit request becomes `AppExit`; HOME becomes Bevy's `WindowFocused` and `AppLifecycle` messages, then the app is suspended until the player is back ([docs/lifecycle-and-exit.md](docs/lifecycle-and-exit.md#home-and-focus)).
   - Joy-Con and Pro Controller input translated into standard Bevy `GamepadEvent` and `ButtonInput<GamepadButton>`.
   - Handheld (720p) vs Docked (1080p) dynamic resolution switching.

2. **Crate patches (`patches/`)**: Horizon support in the crates a Bevy game pulls in. libc gets devkitA64 newlib's struct layouts; wgpu, wgpu-core, wgpu-hal and raw-window-handle get a Vulkan backend that draws through NVK; bevy_asset reads files synchronously, without `async-io` or blocking's thread pool; bevy_tasks joins its threads at exit; cpal gets an audio host on libnx's audio renderer. getrandom, inventory, lua-src, memmap2, polling, ring and rustix get smaller fixes. The Bevy 0.19 versions are in `patches/`, the Bevy 0.18 ones in `patches/bevy-0.18/`. [docs/toolchain.md](docs/toolchain.md#the-crate-patches) says what each one changes.

## Using with a Bevy game

Check this repository out next to your game, so both folders share a parent. In the game's `Cargo.toml`, add bevy_horizon for the Switch only (the path is relative to that `Cargo.toml`):

```toml
[target.'cfg(target_os = "horizon")'.dependencies]
bevy_horizon = { path = "../bevy-horizon/crates/bevy_horizon", features = ["vulkan"] }
# A Bevy 0.18 game: default-features = false, features = ["bevy_0_18", "vulkan"]
```

Then build the NRO, with Podman or Docker:

```bash
BEVY=0.19 scripts/build_game.sh ../my-game my-game my-game "My Game"
```

The arguments are the game's folder, the package with its binary, the binary, and the title HOME shows. `build_game.sh` applies the patches for that Bevy version from `.cargo/bevy-0.19.toml` or `.cargo/bevy-0.18.toml`, so the game's `Cargo.toml` needs no `[patch]` table. It also moves each patched crate in the game's `Cargo.lock` to the patch's version, builds std and the game for `aarch64-nintendo-horizon`, and writes the NRO to the game's `target/aarch64-nintendo-horizon/release/`. [docs/porting-a-bevy-game.md](docs/porting-a-bevy-game.md) covers the rest: the Bevy features a Switch build needs, input, files and memory.

In your game code:

```rust
use bevy::prelude::*;

fn main() {
    // stdout, stderr and Bevy's log go to sdmc:/switch/<NRO name>/log.txt; this name is the fallback.
    #[cfg(target_os = "horizon")]
    bevy_horizon::init_switch_logging("my-game");
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    // The frame loop, window, controllers, HOME menu and exit, in place of winit and gilrs.
    #[cfg(target_os = "horizon")]
    app.add_plugins(bevy_horizon::HorizonPlugin);
    app.run();
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
  Copy `hello_switch.nro` to `sdmc:/switch/hello_switch/hello_switch.nro` on your SD card. Logs go to that folder, and Bevy's asset root is the NRO's folder, so it needs its own. Launch via **Title Override** (hold `R` while launching any game title) to ensure full memory access.

### Controls

| Action | Switch Controller | Desktop Keyboard |
|---|---|---|
| Adjust Rotation Speed | Left Stick / D-Pad | Arrow Keys |
| Cycle Cube Color | **(A)** Button | Space |
| Exit | **(+)** Button | Escape |


## Credits

bevy-horizon copies and adapts code from other projects, among them kyokoxdev's crate patches, danfromtico's mesa-switch and libnx. [CREDITS.md](CREDITS.md) lists every source with its license and what it was used for.

## License

bevy-horizon is licensed under either of the [MIT license](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE), at your option. The crates under `patches/` keep their own licenses.
