# Porting a Bevy game to the Switch

This page takes a Bevy game that runs on PC and makes it run on the Switch as [homebrew](glossary.md#homebrew): the Cargo setup, `main`, the build, files, memory, input, audio, exit, the dependency audit and testing. It ends with a reference of every environment variable bevy-horizon reads and a real port as an example. It assumes you have built and started [hello_switch](glossary.md#hello_switch) by following [Getting started](building.md).

## In short

- **Get the game running on PC first,** on Bevy 0.19.1 or 0.18.1, with the Bevy features you'll use on the Switch.
- **Add `bevy_horizon` for Horizon only,** with its `vulkan` feature and `render` (Bevy 0.19) or `render_0_18` (Bevy 0.18), and leave Bevy's `default_platform` features out of the Switch build. Add `top_down_alloc` if the game loads large assets.
- **In `main`,** set any environment variables, call `init_switch_logging`, then add `DefaultPlugins` and `HorizonPlugin`, in that order.
- **Build with `scripts/build_game.sh`** from bevy-horizon. It applies the patch set for your Bevy version, builds the [NRO](glossary.md#nro) and checks it.
- **Copy the NRO and its `assets` folder to `sdmc:/switch/<game>/`,** and start it with a game's memory, about 3.1 GiB, which GPU memory shares.
- **Pace big loads,** and keep CPU copies of meshes and textures only where the game reads them.
- **Quit with `AppExit` and return from `main`.** Save before that, and on `AppLifecycle::WillSuspend`. Join the threads you start.
- **Audit your dependencies** for crates that don't build or don't work on [Horizon](glossary.md#horizon).
- **Test on the console.** Stock [Ryujinx](glossary.md#ryujinx) can't run builds that draw.

## 1. Before you start

- **The game builds and runs on PC with the same Bevy version, features and plugin setup** that the Switch build will use. Fix desktop bugs on the desktop; the console is a slow place to find them.
- **Bevy 0.19.1 or 0.18.1.** bevy-horizon has a patch set for each ([step 4](#4-match-the-patch-set-to-your-bevy-version-and-build)). Another version needs a new one.
- **hello_switch runs.** [Getting started](building.md) builds the toolchain image and [NVK](glossary.md#nvk), copies an NRO to the SD card and starts it with a game's memory.
- **No `rust-toolchain.toml` that pins another toolchain.** The build uses the container's nightly Rust, which [build-std](glossary.md#build-std) needs.

Check bevy-horizon out next to the game:

```text
projects/
├── bevy-horizon/    with NVK built: nvk/lib/libvulkan_bh.a
├── mesa-switch/     only needed to build NVK
└── my_game/         your game's Cargo workspace
```

The build container mounts bevy-horizon as `/src/bevy-horizon` and your game as `/src/<its folder name>`, so a path dependency through `../bevy-horizon` resolves in both places. Keep the folder named `bevy-horizon`: Cargo resolves every path dependency, even one listed only for Horizon, so a wrong path breaks the desktop build too.

Tested on a Switch Lite, in [handheld mode](glossary.md#handheld-mode), under [Atmosphère](glossary.md#atmosphère), with apps started from [sphaira](glossary.md#sphaira). [Docked mode](glossary.md#docked-mode) is untested.

## 2. Add bevy_horizon

Give the Switch build its own `bevy` dependency, at the same version but without `default_platform`, whose [winit](glossary.md#winit) and gilrs have no Horizon backend. `HorizonPlugin` stands in for them. For Bevy 0.19:

```toml
[target.'cfg(not(target_os = "horizon"))'.dependencies]
bevy = "=0.19.1"

[target.'cfg(target_os = "horizon")'.dependencies]
bevy = { version = "=0.19.1", default-features = false, features = [
    # Bevy's default set, without default_platform
    "default_app",
    "2d_bevy_render",
    "3d_bevy_render",
    "ui_bevy_render",
    "bevy_ui_widgets",
    "scene",
    "audio",
    "picking",
    # the parts of default_platform that work on Horizon
    "std",
    "multi_threaded",
    "default_font",
] }
bevy_horizon = { path = "../bevy-horizon/crates/bevy_horizon", features = ["vulkan", "render"] }

[profile.release]
debug = 1
strip = false
```

For Bevy 0.18, turn `bevy_horizon`'s default `bevy_0_19` feature off and use `bevy_0_18` and `render_0_18`. This list is Skate 3 Rust Engine's, which plays on the console:

```toml
[target.'cfg(not(target_os = "horizon"))'.dependencies]
bevy = "=0.18.1"

[target.'cfg(target_os = "horizon")'.dependencies]
bevy = { version = "=0.18.1", default-features = false, features = [
    # default_app, without custom_cursor
    "async_executor", "bevy_asset", "bevy_input_focus", "bevy_log", "bevy_state", "bevy_window",
    "reflect_auto_register",
    # the parts of default_platform that work on Horizon
    "std", "default_font", "multi_threaded",
    "2d_bevy_render", "3d_bevy_render",
    # ui_api and ui_bevy_render, without the default_app they include
    "bevy_ui", "bevy_ui_render",
    "scene", "audio", "picking",
] }
bevy_horizon = { path = "../bevy-horizon/crates/bevy_horizon", default-features = false, features = ["bevy_0_18", "vulkan", "render_0_18"] }
```

- **Pin Bevy with `=`.** Bevy's crates require each other at exact versions, and the patches are made from 0.19.1 and 0.18.1.
- **The path is relative to the `Cargo.toml` it's in.** From `crates/my_game/Cargo.toml` in a workspace, it's `../../../bevy-horizon/crates/bevy_horizon`.
- **In a virtual workspace, set `resolver = "2"` or later.** Resolver 1 merges the desktop table's features into the Switch build, which then pulls in winit. A package with `edition = "2021"` or later already gets resolver 2.
- **Keep symbols in release builds:** `debug = 1` and `strip = false`, in the workspace root's `[profile.release]` (Cargo ignores profiles anywhere else). Crash addresses and CPU profiles are looked up in the ELF; the NRO holds only code and data, so it doesn't grow.

### Choosing Bevy's features

- **Start from Bevy's defaults without `default_platform`,** as above, then drop what the game doesn't use.
- **Of `default_platform`, keep `std`, `multi_threaded` and `default_font`.** `bevy_winit` and `bevy_gilrs` have no Horizon backend, `custom_cursor` turns `bevy_winit` on, and the rest are for desktop and web platforms.
- **On Bevy 0.18, `custom_cursor` is part of `default_app`,** and `ui_api` includes `default_app`, so the 0.18 list names their other features one by one.
- **A 3D game needs `tonemapping_luts`, `ktx2` and `zstd_rust`.** `3d_bevy_render` brings them. Without them, whatever a camera tonemaps comes out magenta ([Bevy features a game needs](rendering.md#bevy-features-a-game-needs)).
- **Leave `dynamic_linking` off in the Switch build.** An NRO is one statically linked program. If the game's default features turn it on, build with `CARGO_ARGS="--no-default-features"`, as Skate 3 Rust Engine does.

The 0.18 list plays on the console in a full 3D game. The 0.19 list builds for Horizon; this repo's Bevy 0.19 examples run on the console with shorter lists (`examples/hello_switch/Cargo.toml`), and a full 3D Bevy 0.19 game hasn't been tried yet.

### bevy_horizon's features

| Feature | What it does | Turn it on when |
|---|---|---|
| `bevy_0_19` | The default: builds against Bevy 0.19's `bevy_app`, `bevy_ecs`, `bevy_input`, `bevy_window` and `bevy_tasks` | The game uses Bevy 0.19 |
| `bevy_0_18` | The same crates at 0.18.1 | The game uses Bevy 0.18; set `default-features = false` |
| `vulkan` | Links NVK (`nvk/lib/libvulkan_bh.a`), so wgpu draws through [Vulkan](glossary.md#vulkan). Without it, Bevy's renderer panics with `Unable to find a GPU! Make sure you have installed required drivers!` | The game draws |
| `render` | Adds `HorizonUploadPlugin`, which [paces GPU uploads](platform-notes.md#pacing-uploads-the-render-feature) | The game draws, with Bevy 0.19 |
| `render_0_18` | The same, for Bevy 0.18 | The game draws, with Bevy 0.18 |
| `top_down_alloc` | Turns on `vulkan` and installs a global allocator: blocks of 256 KiB and up go in the region NVK takes GPU memory from, and freed blocks of up to 8 KiB are cached | The game loads large assets; not with your own `#[global_allocator]` |
| `fps_overlay` | Exports `HorizonFpsOverlayPlugin` ([the FPS overlay](rendering.md#the-fps-overlay)), which you add and toggle with (−)+(+), and `fps_chord_held`, which tells your (+) handler to ignore presses while (−) is held. Needs `bevy_ui_render`, `bevy_text` and `default_font` | You want a frame counter on the console; Bevy 0.19 only |

A wrong combination stops the build with a message that starts `bevy_horizon:`, for example ``bevy_horizon: a Bevy 0.18 game turns on `render_0_18`; `render` is for Bevy 0.19`` ([Troubleshooting](troubleshooting.md)). With `vulkan`, the build also stops if NVK isn't built: ``bevy_horizon's `vulkan` feature links …/nvk/lib/libvulkan_bh.a, which is missing``. Build it with `scripts/build_nvk.sh`, then `scripts/localize_nvk.sh` ([Getting started](building.md)).

## 3. Set up main

```rust
use bevy::prelude::*;

fn main() {
    #[cfg(target_os = "horizon")]
    {
        // SAFETY: no other thread reads the environment yet.
        unsafe {
            std::env::set_var("BH_RT_COMPRESSION", "1");
        }
        bevy_horizon::init_switch_logging("my_game");
    }

    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    #[cfg(target_os = "horizon")]
    app.add_plugins(bevy_horizon::HorizonPlugin);
    app.add_systems(Startup, setup);
    app.run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-2.5, 4.5, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
```

- **Environment variables come first,** before you build the App: `HorizonPlugin`, `DefaultPlugins` and the drivers they start read them ([Reference: environment variables](#reference-environment-variables)). Setting them just after `init_switch_logging` works too.
- **`init_switch_logging` starts the logs.** It starts the writer thread behind `log.txt`, installs the panic hook that writes `panic.txt`, and sets `NO_COLOR=1`. The NRO's startup code normally opens the log and sets `BEVY_ASSET_ROOT` before `main`; the name you pass is the folder it falls back to, `sdmc:/switch/<name>/` ([Logs and crash reports](logging-and-crash-reports.md)).
- **Add `HorizonPlugin` after `DefaultPlugins`.** It stands in for winit and gilrs: it attaches the primary window to the screen, reads the controller into a Bevy gamepad, sends the HOME and focus messages winit sends on other platforms, follows the dock state, paces uploads with the `render` features, mounts [RomFS](glossary.md#romfs), starts sockets, and runs the frames at 60 Hz unless the app already has a runner. Without it, `App::run` runs one update and returns. [Lifecycle and exit](lifecycle-and-exit.md) lists its parts ("What `HorizonPlugin` adds").
- **Keep winit-only code off the Switch,** such as `WinitSettings`, behind `#[cfg(not(target_os = "horizon"))]`.
- **Leave the window at Bevy's default size.** bevy_horizon makes the primary window 1280×720 in handheld mode and 1920×1080 docked; other windows aren't drawn.
- **Bevy's multi-threaded executor and pipelined rendering work.** A full 3D game runs with both; you don't need a single-threaded executor.

### Games without a window or a renderer

A game that doesn't draw, or a test like [captest](glossary.md#captest), leaves out `bevy_render` and `bevy_window`. It needs three things the renderer and window would bring (as in `examples/switch_captest/src/bevy_phase.rs`, `build_app`):

```toml
[target.'cfg(target_os = "horizon")'.dependencies]
bevy = { version = "=0.19.1", default-features = false, features = [
    "std", "async_executor", "multi_threaded",
    "bevy_asset", "bevy_log", "gamepad", "bevy_audio", "wav", "bevy_image", "png",
] }
# Only for the messages HorizonPlugin sends. Bevy's own bevy_window feature stays off,
# so DefaultPlugins keeps its ScheduleRunnerPlugin.
bevy_window = { version = "=0.19.1", default-features = false }
bevy_horizon = { path = "../bevy-horizon/crates/bevy_horizon" }
```

```rust
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::image::{CompressedImageFormats, ImageLoader};
use bevy::prelude::*;

fn main() {
    bevy_horizon::init_switch_logging("my_game");

    let mut app = App::new();
    // Replace DefaultPlugins' runner, which loops with no wait.
    app.add_plugins(
        DefaultPlugins
            .build()
            .set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 60.0))),
    );
    // bevy_render would register this; without it, image loads never finish.
    app.register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()));
    app.add_plugins(bevy_horizon::HorizonPlugin);
    // What WindowPlugin would add: the HOME messages, and a window for WindowFocused to name.
    app.add_message::<bevy_window::AppLifecycle>();
    app.add_message::<bevy_window::WindowFocused>();
    app.world_mut()
        .spawn((bevy_window::Window::default(), bevy_window::PrimaryWindow));
    app.run();
}
```

- **A runner that waits.** Without `bevy_window`, `DefaultPlugins` adds a `ScheduleRunnerPlugin` that never sleeps and keeps a core busy. Add `HorizonPlugin` after `DefaultPlugins`: added before, it adds its own runner, and Bevy panics on the duplicate.
- **The image loader.** `bevy_image` only announces `ImageLoader`, and `bevy_render` registers it, so without the renderer image loads stay `Loading`. This holds in Bevy 0.18 and 0.19.
- **The window messages.** Without them, HOME suspends the game without telling it.

Tested on a Switch Lite: captest, built this way on Bevy 0.19, passes 179 checks, fails none and warns on 2 (at handheld clocks, before the current audio host).

## 4. Match the patch set to your Bevy version, and build

Some crates need changes to build or run on Horizon, so bevy-horizon carries patched copies of them in `patches/`, and one `[patch.crates-io]` table per Bevy version in `.cargo/`. `scripts/build_game.sh` passes the right table to Cargo with `--config`, so your `Cargo.toml` needs no `[patch]` section and its desktop builds don't change.

| File | Used for | Patches |
|---|---|---|
| `.cargo/config.toml` | Every build | `libc` 0.2.190 (`patches/libc`), which std builds against too; and the C compiler and flags for crates that build C code |
| `.cargo/bevy-0.19.toml` | Bevy 0.19.1 | `bevy_asset` and `bevy_tasks` 0.19.1; `cpal` 0.17.3; `wgpu`, `wgpu-core`, `wgpu-core-deps-windows-linux-android` and `wgpu-hal` 29.0.4; `getrandom` 0.4.3 and 0.2.17; `inventory` 0.3.24, `lua-src` 551.0.2, `memmap2` 0.9.11, `polling` 3.10.0, `raw-window-handle` 0.6.2, `ring` 0.17.14, `rustix` 1.1.5 |
| `.cargo/bevy-0.18.toml` | Bevy 0.18.1 | From `patches/bevy-0.18/`: `bevy_asset` and `bevy_tasks` 0.18.1, `cpal` 0.15.3, `wgpu` 27.0.1, `wgpu-core` 27.0.3, `wgpu-core-deps-windows-linux-android` 27.0.0, `wgpu-hal` 27.0.4. Shared with 0.19: `getrandom` 0.4.3, `inventory`, `lua-src`, `memmap2`, `raw-window-handle` |

[The crate patches](toolchain.md#the-crate-patches) says what each patch changes. A patch for a crate your game doesn't build makes Cargo warn that it `was not used in the crate graph`; that's normal. A patch that should apply but doesn't is not an error to Cargo, which then builds the crates.io release, and the game can't draw or play sound. So `build_game.sh` moves your lock file onto the patches and stops if one still isn't used ([How to check that a patch is really applied](toolchain.md#how-to-check-that-a-patch-is-really-applied)).

### Build

Run the script from bevy-horizon, with the game's folder, the package that has its binary, the binary, and the title the HOME menu shows (the binary's name if you leave it out):

```sh
cd bevy-horizon
scripts/build_game.sh ../my_game my_game my_game "My Game"
```

It does this:

1. **Picks the patch set:** `BEVY`, or else the version of `bevy_app` in the game's `Cargo.lock` (`==> my_game's Cargo.lock has Bevy 0.19 (bevy_app)`).
2. **Checks `ICON`,** if set, is a 256×256 JPEG.
3. **Builds the toolchain image** `bevy-horizon-toolchain:local` from the `Containerfile` if it's missing.
4. **Moves the game's `Cargo.lock` onto the patches,** one `cargo update -p <name>@<locked> --precise <patched>` at a time, printing each move as `<name> <old> -> <new> (<path>)`. This covers the patches in your own root `[patch.crates-io]` too. The lock changes in place; the moved versions work on the desktop too. If a patch still isn't used, it stops and names the version to re-make the patch from.
5. **Compiles** with `cargo build --release`, `-Z build-std=core,alloc,std,panic_abort`, the target `aarch64-nintendo-horizon.json`, the two `--config` files, `--package`, `--bin` and `CARGO_ARGS`. It first checks that std builds against `patches/libc`.
6. **Packs the NRO** with nacptool (title, `AUTHOR`, `VERSION`) and elf2nro (`ICON`, and `ROMFS` if set).
7. **Checks the NRO** has the asset section with the icon and [NACP](glossary.md#nacp), which sphaira forwarders need: `ok: image 0x… + ASET section, …` and `NACP: 'My Game' by 'bevy-horizon', version '0.1.0'; icon … bytes`.

The NRO lands in `<game_dir>/target/aarch64-nintendo-horizon/release/<bin>.nro`, with the [ELF](glossary.md#elf) `<bin>` beside it. Keep the ELF of every build you hand out: crash addresses are looked up in it.

| Variable | Default | What it does |
|---|---|---|
| `BEVY` | `bevy_app`'s version in `Cargo.lock` | Picks `.cargo/bevy-$BEVY.toml`: `0.19`, `0.18`, or the name of a set you add |
| `CARGO_ARGS` | none | More `cargo build` arguments, split at spaces, for example `--no-default-features` |
| `JOBS` | Cargo's default | Caps Cargo's parallel jobs. Each `rustc` on a Bevy crate can take 1–2 GB of RAM |
| `ROMFS` | none | A folder inside the game's folder, given relative to it, packed into the NRO as its RomFS |
| `AUTHOR` | `bevy-horizon` | The author the HOME menu and sphaira show |
| `VERSION` | `0.1.0` | The version they show |
| `ICON` | libnx's default icon | The icon they show: a 256×256 JPEG, given relative to the folder you run the script from |
| `CONTAINER_ENGINE` | `docker` if it's installed, else `podman` | The container engine |

The image name and the Cargo registry volume, `bh-cargo-registry` (shared with `build_example.sh`), are fixed. The host needs bash, python3, and Docker or Podman.

Cargo in the container also reads the game's own `.cargo/config.toml`. Settings under `[build]` there, such as `rustflags`, apply to the Switch build too, so put desktop-only settings under `[target.<host triple>]`.

### Adding a patch, or another Bevy version

- **A crate your patch set leaves out.** The 0.18 set has no `getrandom` 0.2, `polling`, `ring` or `rustix`. If the dependency audit shows your game builds one, copy `.cargo/bevy-0.18.toml` to `.cargo/bevy-<name>.toml`, add the entries from `.cargo/bevy-0.19.toml`, and build with `BEVY=<name>`.
- **Your own patch.** Start from the crates.io release your lock has, change it, and list it in your game's root `[patch.crates-io]`; `build_game.sh` moves the lock onto it like the others. `diff -r` against the release shows what any of bevy-horizon's patches change.
- **Another Bevy version** needs `bevy_asset`, `bevy_tasks`, `cpal` and the wgpu crates re-made from that version's releases in `patches/bevy-<version>/`, a `.cargo/bevy-<version>.toml`, and a `bevy_horizon` feature that renames its Bevy dependencies for that version and aliases them in `lib.rs` with `extern crate … as`, as `bevy_0_18` and `render_0_18` do.

## 5. Files and assets on the SD card

```text
sdmc:/switch/my_game/
├── my_game.nro
├── assets/       Bevy's asset folder: copy the game's assets folder here
├── log.txt       written by bevy_horizon
└── panic.txt     written after a panic
```

- **Bevy's asset folder is `assets/` beside the NRO.** bevy_horizon sets `BEVY_ASSET_ROOT` to the NRO's folder before `main`, and logs `[Assets] BEVY_ASSET_ROOT = … (Bevy's default asset folder is …/assets)`.
- **Relative paths start at the NRO's folder,** where libnx sets the working directory. `sdmc:/…` paths work everywhere, but Rust's `Path` doesn't count them as absolute, so `base.join("sdmc:/…")` appends instead of replacing.
- **The SD card is usually [FAT32](glossary.md#fat32):** files under 4 GiB, case-insensitive names (a wrong case that works on the console fails on Linux), ASCII names, and no folders with tens of thousands of files.
- **Saves are files.** There's no system save-data API in bevy_horizon. Write them from a thread or at a pause, because writes cost frames. `fs::rename` replaces a file but not atomically: until it finishes, the old file is kept as `<new>.bh-replaced`, so a save loader can look there. [Platform notes](platform-notes.md) has the details ("Files and the SD card").
- **RomFS is optional:** a read-only file system packed into the NRO. `ROMFS=romfs scripts/build_game.sh …` packs the game's `romfs` folder. `HorizonPlugin` mounts it as `romfs:` when it is added, so `std::fs::read("romfs:/data/level.bin")` works from then on. RomFS files stay on the SD card and take no heap. Without one, `romfsMountSelf: 0x1559 = 2345-0010 (libnx IoError): this NRO has no RomFS, …` at startup is normal.

To load Bevy assets from RomFS, register an asset source before adding `DefaultPlugins`, then load `romfs://textures/logo.png`:

```rust
use bevy::asset::io::file::FileAssetReader;
use bevy::asset::io::{AssetSourceBuilder, AssetSourceId};

app.register_asset_source(
    AssetSourceId::from("romfs"),
    AssetSourceBuilder::new(|| Box::new(FileAssetReader::new("romfs:/"))),
);
```

bevy-horizon's `bevy_asset` patch keeps a device path such as `romfs:/` as it is. captest reads `romfs://captest.png` on the console through a reader with the same root (`examples/switch_captest/src/bevy_phase.rs`, `romfs_reader`).

## 6. Memory

One [heap](glossary.md#heap) holds everything: code, Rust and C allocations, thread stacks and, with NVK, every GPU buffer and texture ([GPU memory comes from the same heap](platform-notes.md#gpu-memory-comes-from-the-same-heap)). Started with a game's memory, the heap is about 3.1 GiB: 3,132.7 MiB, or about 3,114 MiB once the NRO links NVK. Started in [applet mode](glossary.md#applet-mode), from the Album for example, it is far smaller.

- **Start the game with a game's memory:** hold R while starting any game and pick the NRO in the homebrew menu ([title override](glossary.md#title-override)), or make a HOME-menu [forwarder](glossary.md#forwarder) with sphaira ([Getting started](building.md)). The log's first `[Memory]` line says which you got:

  ```text
  [Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
  ```

  In applet mode it reads `applet type 2 = LibraryApplet: applet mode, e.g. started from the Album, so it gets an applet's memory`.
- **Turn on `render` or `render_0_18`.** It paces uploads: at most 32 MiB of new meshes a frame, other render assets at most 64 MiB a frame (Bevy's `RenderAssetBytesPerFrame`, if the game sets none), and after a frame that uploads 8 MiB or more it waits for the GPU to finish, so memory freed by the upload comes back. `HorizonUploadSettings` changes the limits, and `None` turns one off ([Pacing uploads](platform-notes.md#pacing-uploads-the-render-feature)). It logs:

  ```text
  [Upload] GPU uploads a frame: new meshes at most 32.0 MiB, other render assets at most 64.0 MiB (RenderAssetBytesPerFrame); waits for the GPU after frames that upload 8.0 MiB or more
  ```
- **Use smaller mesh slabs.** Bevy packs meshes into GPU buffers of up to 512 MiB, and gives a mesh of 256 MiB or more its own. On the Switch, smaller buffers are easier to place in the shared heap. Skate 3 Rust Engine uses these, after adding `DefaultPlugins` (the sub-app exists only then):

  ```rust
  use bevy::render::RenderApp;
  use bevy::render::mesh::allocator::MeshAllocatorSettings;
  use bevy::render::render_asset::RenderAssetBytesPerFrame;

  app.insert_resource(RenderAssetBytesPerFrame::new(64 << 20));
  if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
      let mut slabs = MeshAllocatorSettings::default();
      slabs.max_slab_size = 64 << 20;
      slabs.large_threshold = 32 << 20;
      render_app.insert_resource(slabs);
  }
  ```
- **Drop CPU copies you don't read.** An asset loaded with `RenderAssetUsages::RENDER_WORLD` is freed from the main world once it is on the GPU. Use it for textures and meshes the game never reads on the CPU. In Bevy 0.19:

  ```rust
  use bevy::asset::RenderAssetUsages;
  use bevy::image::ImageLoaderSettings;

  let ground: Handle<Image> = asset_server
      .load_builder()
      .with_settings(|settings: &mut ImageLoaderSettings| {
          settings.asset_usage = RenderAssetUsages::RENDER_WORLD;
      })
      .load("textures/ground.png");
  ```

  In Bevy 0.18, pass the same closure to `asset_server.load_with_settings`. For glTF files, set `load_meshes` and `load_materials` in `GltfLoaderSettings` the same way.
- **Turn on `top_down_alloc` for large assets.** Blocks of 256 KiB and up then come from the top of the heap, where NVK takes GPU memory, and leave malloc's free space whole; freed blocks of up to 8 KiB are kept for reuse (`BH_ALLOC_CACHE=0` turns that off). [Platform notes](platform-notes.md) explains both ("Large Rust blocks in the region" and "Small blocks: a cache in front of malloc").
- **Measure at load stages.** `bevy_horizon::memory_report()` (Horizon only) returns one line on the heap: what malloc has in use and has taken, and what GPU memory holds. For example: `info!("level loaded: {}", bevy_horizon::memory_report())`. It walks the heap, so call it when a stage finishes, not every frame. The heartbeat also logs memory every 60 s ([Heartbeat lines](logging-and-crash-reports.md#heartbeat-lines)).

## 7. Input, audio, HOME and exit

- **Input.** The controller arrives as one Bevy `Gamepad`. A is `East`, B `South`, X `North`, Y `West`; L and R are `LeftTrigger` and `RightTrigger`, ZL and ZR `LeftTrigger2` and `RightTrigger2`; (+) is `Start` and (−) `Select`. The sticks rest at about 0.081, and `Gamepad::left_stick()` and `right_stick()` return the raw value (Bevy's dead zone applies only to `GamepadAxisChangedEvent`), so apply a dead zone of your own, about 0.1. There is no touch screen, motion, rumble or keyboard ([Input and audio](input-and-audio.md)).
- **Audio.** bevy_audio plays through bevy-horizon's cpal host, which drives the system's audio renderer. Only the patched cpal versions play, 0.17.3 for Bevy 0.19 and 0.15.3 for Bevy 0.18; a crate that brings another cpal version, or SDL2, plays nothing. The log shows `[Audio] audren output: …` when a stream opens, and Bevy's `No audio device found.` means the patch isn't applied. One output stream; no microphone ([The audio host](input-and-audio.md#the-audio-host)). Tested on a Switch Lite with Bevy 0.18 in a full 3D game: no crackle, and sound comes back after HOME and sleep. The Bevy 0.19 host has run only in the emulator so far.
- **HOME and sleep.** bevy_horizon sends `WindowFocused { focused: false }` and `AppLifecycle::WillSuspend`, then `Suspended`, runs those two frames, and lets the system suspend the game; on return it sends `WillResume` and `Running`. Pause on `WillSuspend`, and save there if progress matters ([HOME and focus](lifecycle-and-exit.md#home-and-focus)).
- **Quit with `AppExit`.** `app.run()` returns, `main` returns, and dropping the App closes the GPU and audio before bevy_horizon releases what it took. The log then says `[Exit] PASS` and the console returns to the homebrew menu; `[Exit] WARN` means something still held memory, and it goes to HOME instead ([Quitting: the App drop](lifecycle-and-exit.md#quitting-the-app-drop), [The audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit)). Don't call `std::process::exit` in a game that draws: it skips the drop, so the GPU stays open.

  ```rust
  fn quit(gamepads: Query<&Gamepad>, mut exit: MessageWriter<AppExit>) {
      if gamepads.iter().any(|gamepad| gamepad.just_pressed(GamepadButton::Start)) {
          exit.write(AppExit::Success);
      }
  }
  ```
- **Save before you send `AppExit`,** and on `WillSuspend`. A player who closes the game from the HOME menu runs no more of its code.
- **Stop the threads you start.** Join them before `main` returns. A thread you can't join from the App, such as a logger, can stop in a hook: `bevy_horizon::register_release_hook(stop_my_thread)` runs it at exit, newest first, up to 8 hooks; a hook must not allocate or panic ([Rules for services you add later](lifecycle-and-exit.md#rules-for-services-you-add-later), [The `userAppExit` hook](lifecycle-and-exit.md#the-userappexit-hook)).
- **Without `bevy_render`, register `ImageLoader` yourself** ([Games without a window or a renderer](#games-without-a-window-or-a-renderer)).

## 8. Audit your dependencies

Every crate in the Switch build must compile for Horizon and work there. std works, through [newlib](glossary.md#newlib) and [libnx](glossary.md#libnx), but crates that call an OS directly may not. List what the Switch build pulls in, from bevy-horizon, with the game next to it:

```sh
docker run --rm -v bh-cargo-registry:/usr/local/cargo/registry \
    -v "$PWD:/src/bevy-horizon" -v "$(cd ../my_game && pwd):/src/my_game" -w /src/my_game \
    bevy-horizon-toolchain:local \
    cargo tree -Z json-target-spec --target ../bevy-horizon/aarch64-nintendo-horizon.json \
        --config ../bevy-horizon/.cargo/config.toml --config ../bevy-horizon/.cargo/bevy-0.19.toml \
        -e normal,build
```

Use `bevy-0.18.toml` for Bevy 0.18, and add `-i <crate>` to see what pulls a crate in. Then check each area:

| Area | On Horizon | What to do |
|---|---|---|
| Rendering | wgpu's Vulkan backend through NVK, with the patched wgpu | Turn on `vulkan` and `render` or `render_0_18`. Crates that need OpenGL or another graphics API won't draw |
| Windows and input | winit and gilrs have no Horizon backend | Leave `default_platform` out; `HorizonPlugin` replaces them. A crate that needs a winit window or event loop won't work |
| Audio | The patched cpal 0.17.3 and 0.15.3 | Use bevy_audio or another crate on the same cpal version. SDL2 and other cpal versions play nothing |
| Threads | std threads work; the game gets three cores | See [Platform rules that bite any game](#9-platform-rules-that-bite-any-game) |
| Files | std's file functions, on FAT32; `memmap2` is patched | See [Files and assets on the SD card](#5-files-and-assets-on-the-sd-card) |
| Networking | std TCP and UDP sockets work on the console; libnx's default buffers allow about 42 UDP sockets before `ENOBUFS`. tokio's multi-threaded runtime, timers and sync pass captest; tokio's `net` ([mio](glossary.md#mio)), quinn and rustls build but haven't run on the console | No game has used networking on the console yet; test what you use |
| Randomness | [getrandom](glossary.md#getrandom) 0.4 is patched in both sets, 0.2 in the 0.19 set. 0.3 has no backend for the Switch and stops the build with `target is not supported` | Move to a patched version, or add the 0.2 patch to a 0.18 set |
| Crypto | [ring](glossary.md#ring) is patched in the 0.19 set | Add it to a 0.18 set if you need it |
| Lua | `lua-src` is patched in both sets ([Lua](glossary.md#lua)) | — |
| Loading code | No dynamic loading, no `std::env::current_exe` | Link everything statically; find files through `BEVY_ASSET_ROOT` or relative paths |
| Bevy plugins | Work if their own dependencies pass this audit | Check the ones that touch windows, input devices, files or threads |

## 9. Platform rules that bite any game

- **Plan on three cores.** The Switch has four; the game gets cores 0–2, and core 3 is shared with the system. bevy-horizon's `bevy_tasks` patch sizes Bevy's task pools from the cores the game may use.
- **Never spin-wait.** Threads have fixed priorities (the main thread 0x2C, std threads 0x3B, the audio thread 0x2B), and a busy thread starves the others on its core. To choose which cores std threads start on, call `bevy_horizon::set_thread_core_policy` before `App::new`: `ThreadCorePolicy::AnyCore` (cores 0–2), `Spread` (starts them on 1, 2, 0 in turn) or `Pinned` (keeps them there); the default leaves libnx's behaviour ([Platform notes](platform-notes.md), "Cores and priorities").
- **Join your threads,** as in [step 7](#7-input-audio-home-and-exit). Bevy's task pools stop at exit within 500 ms.
- **A panic aborts the game.** The target builds with `panic = "abort"`, so `catch_unwind` catches nothing; the panic hook writes `panic.txt` beside the NRO first ([Logs and crash reports](logging-and-crash-reports.md)).
- **Crash addresses need the ELF.** A CPU crash leaves an [Atmosphère crash report](glossary.md#crash-report); subtract the `nro base` that `log.txt` records and look the address up in the ELF of the same build ([Logs and crash reports](logging-and-crash-reports.md)).
- **The first launch compiles every shader.** NVK caches them on the SD card under `sdmc:/.mesa/`, so later launches start faster.
- **Docked mode is untested.** The code sets the window to 1920×1080 when docked, but a Switch Lite can't dock.

## 10. Testing on the console

Test each platform feature the game relies on with a small check that logs PASS or FAIL, before building on it. This repo's examples show the pattern:

- **hello_switch's `ChecksPlugin`** (`examples/hello_switch/src/checks.rs`) checks an asset load, file metadata, a thread with its own thread-locals, and a RomFS read, each logged as a line such as `[Check] asset: PASS, …`.
- **captest** (`examples/switch_captest`) runs one check per feature, in sections. `report.rs` has `begin` and `end` around each check, `check` and `check_warn`, and a [watchdog](glossary.md#watchdog). It writes `captest_results.txt`, and a [crash marker](glossary.md#crash-marker) so a check that crashed is skipped the next time ([Testing](testing.md)).

A port order that finds problems one at a time:

1. Build and start hello_switch.
2. Add `bevy_horizon` and the Switch feature list, and build the game with `build_game.sh` until it links.
3. Start it with a game's memory. In `log.txt`, look for `applet type 0`, the `[Assets]` and `[Upload]` lines, and `[Audio] audren output` if it plays sound.
4. Check the first scene draws and the controls map as you expect.
5. Load the largest level, with `memory_report()` at each load stage.
6. Press HOME and come back, and put the console to sleep and wake it: the game should pause and resume, with sound.
7. Quit from the game (`[Exit] PASS`), and close it from the HOME menu: the saves should be there both times.
8. Play for a while and watch the heartbeat lines stay steady.

The [emulator](glossary.md#emulator) doesn't replace the console: stock Ryujinx can't run builds that draw, and its speed says nothing about the console's ([What the emulator gets wrong](rendering.md#what-the-emulator-gets-wrong)).

## Reference: environment variables

Horizon has no shell and no launch arguments, so the game sets these itself, with `std::env::set_var` at the start of `main`, before it builds the App, as in [step 3](#3-set-up-main). `set_var` is `unsafe` in Rust's 2024 edition, because another thread reading the environment at the same time is undefined behaviour; at the start of `main` none does. To change settings without a rebuild, read them from a file, as the [example below](#a-real-example-skate-3-rust-engine) does.

| Variable | Values | Default | Read | Documented in |
|---|---|---|---|---|
| `BH_ROBUST_BUFFER_ACCESS` | `clamp` (or empty), `driver` (or `1`), `off` (or `0`) | `clamp` | When Bevy sets up the GPU, as `DefaultPlugins` is added | [Buffer robustness](rendering.md#buffer-robustness) |
| `BH_RT_COMPRESSION` | `1` turns render-target compression on; recommended for 3D | Off | When wgpu creates its first texture | [Render-target compression](rendering.md#render-target-compression) |
| `BH_AUDIO_FRAMES` | 256 to 8192, decimal or `0x` hex | 2048 with Bevy 0.19, 1024 with 0.18 | When the audio stream opens, as `DefaultPlugins` is added | [The audio host](input-and-audio.md#the-audio-host) |
| `BH_AUDIO_STATS` | `1` logs an `[Audio]` timing line every 10 s | Off | When the audio stream opens | [The audio host](input-and-audio.md#the-audio-host) |
| `BH_AUDIO_PRIORITY` | A thread priority, or `default` | 0x2B | When the audio stream opens | [The audio host](input-and-audio.md#the-audio-host) |
| `BH_ALLOC_CACHE` | `0` turns `top_down_alloc`'s small-block cache off | On | When `HorizonPlugin` is added | [Platform notes](platform-notes.md), "Small blocks" |
| `BEVY_HORIZON_MEMORY_EVERY` | Seconds between `[Memory]` heartbeat lines; `0` turns them off | 60 | When `HorizonPlugin` is added | [Heartbeat lines](logging-and-crash-reports.md#heartbeat-lines) |
| `BEVY_HORIZON_CPU_PROFILE` | `1` samples every 2000 µs, or a period in µs from 500; writes `cpu-profile.txt` | Off | When `HorizonPlugin` is added | [Logs and crash reports](logging-and-crash-reports.md), Part 3 |
| `BEVY_HORIZON_CPU_PROFILE_EVERY` | Seconds between writes of `cpu-profile.txt`; `0` turns them off | 30 | When `HorizonPlugin` is added | [Logs and crash reports](logging-and-crash-reports.md), Part 3 |
| `BEVY_HORIZON_ZCULL_DIR` | `greater`, for Bevy's reversed depth, or `less` | `greater` | When `HorizonPlugin` is added, with `vulkan` | [On the Switch](rendering.md#on-the-switch) |
| `NAK_DEBUG` | Mesa's shader-compiler flags, comma-separated, for example `stats`; `-<flag>` turns a Switch default off | The Switch defaults | Once, when NVK first compiles a shader; part of the shader cache's key | [Logs and crash reports](logging-and-crash-reports.md), Part 4; [Linking NVK](toolchain.md#linking-nvk-the-vulkan-feature) |
| `BEVY_HORIZON_NVK_LIB` | The folder that has `libvulkan_bh.a` | `nvk/lib` in bevy-horizon | At build time, by `bevy_horizon`'s build script; the build scripts don't pass it into the container | [Linking NVK](toolchain.md#linking-nvk-the-vulkan-feature) |

Other variables a port may set:

| Variable | What it does | Documented in |
|---|---|---|
| `NAK_GPR_LIMIT` | Caps the registers a shader may use: a multiple of 8 from 24 to 248 | [Linking NVK](toolchain.md#linking-nvk-the-vulkan-feature) |
| `NOUVEAU_HORIZON_TOP_DOWN` | `false` stops NVK taking GPU memory from the top of the heap | [NVK takes GPU memory from the top of the heap](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap) |
| `NOUVEAU_HORIZON_BO_CACHE_MB` | MiB of freed GPU memory NVK keeps for reuse; default 128, `0` turns it off | [NVK takes GPU memory from the top of the heap](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap) |
| `MESA_SHADER_CACHE_DISABLE` | `true` turns off NVK's shader cache on the SD card | [On the Switch](rendering.md#on-the-switch) |
| `RUST_LOG` | Bevy's log filter, read when `DefaultPlugins` is added; Bevy's default is `info` with `wgpu=error,naga=warn` | — |
| `BEVY_ASSET_ROOT` | The folder whose `assets/` Bevy reads, set to the NRO's folder before `main`; change it before adding `DefaultPlugins` | [Files and assets on the SD card](#5-files-and-assets-on-the-sd-card) |

## A real example: Skate 3 Rust Engine

Skate 3 Rust Engine is a full 3D Bevy 0.18 game that ships for the Switch with bevy-horizon; its latest release is 0.1.1. Its Switch setup is on the `horizon-port` branch of https://github.com/JohnUzoka/skate-3-rust-engine, almost all of it in `crates/skate-game/src/horizon.rs`.

- **Cargo.** Bevy `=0.18.1` with the Switch feature list from [step 2](#2-add-bevy_horizon); `bevy_horizon` with `bevy_0_18`, `vulkan`, `render_0_18` and `top_down_alloc`; `debug = 1` and `strip = false` in the release profile. Its default features turn on Bevy's `dynamic_linking` for desktop development, so the Switch build passes `--no-default-features`.
- **Before the App** (`init`): `init_switch_logging("skate3rust")`, the working folder set to `BEVY_ASSET_ROOT`, `RUST_LOG`, and release defaults such as `BH_RT_COMPRESSION=1`. Then it applies the `env NAME=VALUE` lines of an `options.txt` beside the NRO, so a setting can change without a rebuild. It turns the `[Memory]` heartbeat lines off by default.
- **After `DefaultPlugins`** (`install`): `HorizonPlugin`, `RenderAssetBytesPerFrame` at 64 MiB, `MeshAllocatorSettings` with 64 MiB slabs and a 32 MiB large threshold, a 1024 `DirectionalLightShadowMap`, a release hook that stops its own logging thread, and pipelined rendering. It logs `memory_report()` at each load stage.

In its heaviest view it holds about 30 fps, limited by the GPU, and its large world takes about 40–70 s to load. To build it, clone it next to bevy-horizon and run, from bevy-horizon:

```sh
git clone -b horizon-port https://github.com/JohnUzoka/skate-3-rust-engine ../skate-3-rust-engine
BEVY=0.18 JOBS=16 CARGO_ARGS="--no-default-features" AUTHOR="Skate 3 Rust Engine contributors" VERSION=0.1.1 \
    ICON=../skate-3-rust-engine/docs/images/skating-crab-switch.jpg \
    scripts/build_game.sh ../skate-3-rust-engine skate-game skate3rust "Skate 3 Rust Engine"
```

`ICON` is relative to the folder you run the script from. Lower `JOBS` if the build runs out of memory. The NRO goes in `sdmc:/switch/skate3rust/`, beside the game's files (`assets/`, `maps/`, `maps.json` and `settings/`), which the PC version sets up from your own Skate 3 disc; the game's [Switch guide](https://github.com/JohnUzoka/skate-3-rust-engine/blob/horizon-port/docs/switch/README.md) has the steps.

## What bevy_horizon provides, and what it doesn't

It provides:

- **The platform layer Bevy expects:** the run loop, the window on the screen, one controller as a Bevy gamepad, HOME, sleep and focus as Bevy's messages, the dock state, RomFS and sockets.
- **Drawing,** through wgpu's Vulkan backend and NVK linked into the NRO (`vulkan`), with upload pacing (`render`, `render_0_18`) and an allocator for large assets (`top_down_alloc`).
- **Sound,** through the patched cpal hosts.
- **Logs:** `log.txt`, `panic.txt`, the heartbeat, `memory_report()`, a CPU profiler, and an [FPS overlay](rendering.md#the-fps-overlay) (`fps_overlay`, Bevy 0.19).
- **A clean exit** back to the homebrew menu, with release hooks for your own services.
- **std fixes** for `fs::copy`, `fs::rename` and `fs::remove_dir`, and thread core policies.
- **Patch sets** for Bevy 0.19.1 and 0.18.1, and `scripts/build_game.sh`.

It doesn't provide, yet:

- touch, motion controls, rumble, a keyboard, or a second controller;
- a microphone, or more than one audio output;
- system save data; saves are files on the SD card;
- tested docked mode;
- tested networking in a game; only captest's socket checks have run on the console;
- other Bevy versions, which need a new patch set;
- a way to run builds that draw in stock Ryujinx.
