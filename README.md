# bevy-horizon

Run [Bevy](https://bevyengine.org) games on a Nintendo Switch as homebrew.

bevy-horizon has two parts. `bevy_horizon` is a Bevy plugin that takes the place of winit and gilrs on the Switch: it runs the frame loop, opens the window, reads the controllers, handles HOME, sleep and exit, and writes a log to the SD card. The crate patches in `patches/` add Horizon, the Switch's operating system (`target_os = "horizon"`), to the crates a Bevy game pulls in, so the game builds for it and draws through Vulkan with NVK, Mesa's open-source driver for NVIDIA GPUs, as ported to the Switch by [mesa-switch](https://github.com/danfromtico/mesa-switch).

The NROs it builds are homebrew. They run on a Switch with Atmosphère custom firmware, not on an unmodified console.

## Status

bevy-horizon is young. All of its testing on hardware has been on one Switch Lite, in handheld mode.

| Area | Status |
|---|---|
| Bevy versions | 0.19 (the default) and 0.18. |
| Rendering | Bevy draws through wgpu's Vulkan backend and NVK. hello_switch runs at 60 fps, Breakout plays to the end at about 59 fps, and render_check draws all 24 of its cubes. |
| A full game | [Skate 3 Rust Engine](https://github.com/JohnUzoka/skate-3-rust-engine/tree/horizon-port), a Bevy 0.18 game, plays at about 30 fps with sound. It takes 40–70 s to start. |
| Input | The console's controllers arrive as Bevy gamepads. |
| Audio | `bevy_audio` plays through cpal, patched to use libnx's audio renderer. With Bevy 0.18 this is tested on the console; the Bevy 0.19 version has only run in an emulator. |
| HOME, sleep and exit | Bevy gets `WindowFocused` and `AppLifecycle` messages, the game is suspended while the player is away, and it exits cleanly to the homebrew menu ([details](docs/lifecycle-and-exit.md#home-and-focus)). |
| Files and network | romfs, the SD card and std's sockets work. |
| Docked mode | Untested. The window switches to 1920×1080 when docked, but nothing has been run docked yet. |
| Emulators | Stock emulators can't run the Vulkan builds yet ([testing.md](docs/testing.md#testing-in-an-emulator)). |

One known problem: a lit Bevy 0.18 scene has hung the GPU about 10 seconds in, and the cause isn't known yet ([rendering.md](docs/rendering.md#known-issues)).

## Requirements

- A Switch running [Atmosphère](https://github.com/Atmosphere-NX/Atmosphere), with a homebrew launcher. Testing used [sphaira](https://github.com/NaGaa95/sphaira).
- A Linux PC (WSL2 works) with Docker or Podman. The build runs in a container that has devkitA64, libnx and a nightly Rust, so nothing else needs installing.
- Git, and about 20 GB of free disk space for the container images and builds.

## Quick start

Build NVK and the `hello_switch` example:

```bash
git clone https://github.com/JohnUzoka/bevy-horizon
git clone https://github.com/danfromtico/mesa-switch
git -C mesa-switch checkout 606441c
cd bevy-horizon
docker build -t bevy-horizon-toolchain:local -f Containerfile .
scripts/build_nvk.sh
scripts/localize_nvk.sh
scripts/build_example.sh hello_switch
```

- mesa-switch must sit next to bevy-horizon, at the commit bevy-horizon's NVK patches are made for.
- With Podman, build the image with `podman build`. The scripts use Docker if it's installed and Podman if not; `CONTAINER_ENGINE=podman` picks Podman.
- `build_nvk.sh` applies the patches in `patches/mesa-switch/` to the checkout and builds NVK into `nvk/`. `localize_nvk.sh` then makes it safe to link into a Rust game. Both are needed once, and again only when NVK changes.
- `build_example.sh` writes `hello_switch.nro` to the repository's root. Each Bevy crate can take 1–2 GB of memory to compile; if the machine runs short, cap the jobs with `JOBS=4`.

Then, on the Switch:

1. Copy the NRO to `sdmc:/switch/hello_switch/hello_switch.nro`. Give each NRO a folder of its own: its log goes there, and Bevy loads assets from an `assets` folder beside it.
2. Start it with a game's memory: hold R while starting any game (Atmosphère's title override) and choose it in the homebrew menu, or give it a HOME-menu forwarder with sphaira. Started from the album, an NRO runs as an applet, with far less memory.
3. The left stick spins the cube, (A) changes its colour, holding (−) and pressing (+) shows the FPS counter, and (+) exits.

The log is `sdmc:/switch/hello_switch/log.txt`, and a clean exit ends it with `[Exit] PASS`. [Getting started](docs/building.md) goes through all of this in more detail.

## Using it in a game

Check bevy-horizon out next to the game, so both folders share a parent. Add bevy_horizon for the Switch only (the path is relative to the game's `Cargo.toml`):

```toml
[target.'cfg(target_os = "horizon")'.dependencies]
bevy_horizon = { path = "../bevy-horizon/crates/bevy_horizon", features = ["vulkan"] }
# On Bevy 0.18: default-features = false, features = ["bevy_0_18", "vulkan"]
```

Add its plugin after Bevy's:

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

Then build the NRO from bevy-horizon's folder:

```bash
scripts/build_game.sh ../my-game my-game my-game "My Game"
```

The arguments are the game's folder, the package with its binary, the binary, and the title the HOME menu shows. The script applies the patches for the game's Bevy version, so the game needs no `[patch]` table, and it writes the NRO to the game's `target/aarch64-nintendo-horizon/release/`. It also moves each patched crate in the game's `Cargo.lock` to the patch's version. [Porting a Bevy game](docs/porting-a-bevy-game.md) covers the rest: which Bevy features to use, memory, files, and the settings that tune a port.

## Examples

| Example | What it is | On a Switch Lite |
|---|---|---|
| `hello_switch` | A lit, spinning cube with a UI overlay. | 60 fps |
| `breakout` | Bevy's Breakout showcase, played with the D-pad or the left stick. | Plays to the end at about 59 fps |
| `render_check` | Six cameras with different render settings look at four cubes. It screenshots itself and checks that each cube was drawn. | All 24 cubes drawn, about 20 fps |
| `switch_captest` | No graphics: it tests what a Bevy game needs from the console (threads, files, sockets, assets, audio, input, HOME and exit) and writes the results to `captest_results.txt`. | 179 checks pass, 2 warn |

Build any of them with `scripts/build_example.sh <name>`. In the three with graphics, holding (−) and pressing (+) shows the FPS counter, and (+) exits.

## Documentation

In reading order:

1. [Getting started](docs/building.md): set up the build, build NVK and the examples, and run them on a Switch.
2. [Porting a Bevy game](docs/porting-a-bevy-game.md): Cargo features, `build_game.sh`, environment variables, and a real port as an example.
3. [Rendering](docs/rendering.md): the Vulkan path, its settings, and the FPS overlay.
4. [Input and audio](docs/input-and-audio.md): controllers and the audio host.
5. [Lifecycle and exit](docs/lifecycle-and-exit.md): HOME, sleep, focus and quitting.
6. [Logging and crash reports](docs/logging-and-crash-reports.md): `log.txt`, its heartbeat lines, and reading a crash report.
7. [Platform notes](docs/platform-notes.md): memory, CPU cores, files and sockets.
8. [Testing](docs/testing.md): captest, render_check, and testing in an emulator.
9. [Troubleshooting](docs/troubleshooting.md): problems that have come up, and what fixed them.
10. [Toolchain](docs/toolchain.md): the container, the target, NVK's build, and what each crate patch changes.
11. [Glossary](docs/glossary.md): Switch and graphics terms used in these pages.

## Credits

bevy-horizon copies and adapts code from other projects, among them kyokoxdev's crate patches, danfromtico's mesa-switch, libnx and SDL. [CREDITS.md](CREDITS.md) lists every source with its license and what it was used for.

## License

bevy-horizon is licensed under either of the [MIT license](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE), at your option. The crates under `patches/` keep their own licenses, and [CREDITS.md](CREDITS.md#license-notices) has the notices for code adapted from libnx (ISC) and SDL (zlib).
