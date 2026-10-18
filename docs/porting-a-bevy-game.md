# Porting a Bevy game to the Switch

This page gives a work order for taking an existing [Bevy](glossary.md#bevy) game toward Nintendo Switch [homebrew](glossary.md#homebrew) with bevy_horizon. It is written for a game owner who knows their game, but does not know this layer yet.

> **Out of date in places (October 2026).** This page was written before Bevy could draw on the
> Switch. What has changed since:
>
> - **Rendering.** wgpu now has a Vulkan backend for Horizon, through the `wgpu`, `wgpu-core`,
>   `wgpu-hal` and `raw-window-handle` patches, and bevy_horizon's `vulkan` feature links the NVK
>   driver from mesa-switch. In the Ryujinx test harness Bevy draws 2D and 3D, and
>   `examples/breakout` plays to the end. It hasn't run on a Switch yet. So starting headless is
>   optional now, and the GL and switch-mesa plan in §5 and "a working wgpu backend" under "It does
>   not yet provide" are superseded. [rendering.md](rendering.md) has what works and the Bevy
>   features a game needs.
> - **Audio.** captest's bevy_audio checks passed on hardware: a chime was heard, looping and
>   pausing worked, and audio came back after HOME. Dropping the App with its audio device took
>   131 ms there, so the hang in §6 has only been seen in the emulator.
> - **Patches.** §4's patch table is incomplete. The current list is `[patch.crates-io]` in
>   `Cargo.toml` (lines 25-50).
> - **Skate 3.** The checkout this page cited is gone. Its paths now point into the repository, at
>   the same commit.
>
> The rest hasn't been re-checked, and its line numbers may have moved.

## In short

- **Start on PC.** First make a PC build use the same Bevy version, features, plugin order, and asset root that the Switch build will use. PC panics are cheaper than console crash reports.
- **Match versions.** The current patches target Bevy 0.19.x. If your game uses Bevy 0.18, 0.20, or a vendored Bevy crate, make a matching patch set. Cargo can silently skip a [crate patch](glossary.md#crate-patch) when the version does not match; `scripts/build_example.sh:33-58` catches that for this repo.
- **Go headless first.** Today wgpu has no [Horizon](glossary.md#horizon) backend. Use `RenderPlugin { backends: None }`, prove schedules, files, assets, input, threads, and exit, then work on rendering.
- **Then add systems one by one.** Recommended order: PC parity, headless NRO build, console checks, rendering, audio, networking.
- **Switch facts:** expect 3 usable application CPU cores, a measured 3,189.0 MiB process in the current HOME-forwarder path, a 3,132.7 MiB malloc [heap](glossary.md#heap), SD-card paths under [sdmc](glossary.md#sdmc), and loader-sensitive cleanup at exit.
- **What works today:** the `hello_switch` path has proved a 60 fps Bevy frame loop, controller input through [HID](glossary.md#hid), SD-card file I/O, Bevy `AssetServer` loading, `std::thread` with [thread-local storage](glossary.md#thread-local-storage), and a clean app return. Rendering, audio, Rust networking, and docked mode are not proven.

## 1. Get the same game running on PC first

**Why:** PC failures give you Rust backtraces and fast rebuilds. Console failures may give only `log.txt`, `panic.txt`, or an Atmosphère [crash report](glossary.md#crash-report).

Make the PC build look like the future Switch build before you change every backend. Pin the same Bevy version and feature list, use the same plugin shape, set `BEVY_ASSET_ROOT` as the Switch build will (`crates/bevy_horizon/src/logging.rs:171-210`), and catch plugin panics early. The example's startup order is the minimal comparison point: app, `DefaultPlugins`, headless renderer on Horizon, `HorizonPlugin`, single-threaded schedules, systems, then `run` (`examples/hello_switch/src/main.rs:18-101`).

## 2. Build without a renderer

**Why:** rendering is the largest missing platform piece. Headless mode lets you prove the rest of the game before you fight [wgpu](glossary.md#wgpu), [Mesa](glossary.md#mesa), [EGL](glossary.md#egl), or [OpenGL ES](glossary.md#opengl-es).

On Horizon, the example sets `RenderPlugin` to `backends: None` (`examples/hello_switch/src/main.rs:40-52`). Bevy still runs schedules, app state, input, assets, and non-render resources. Build the NRO, launch from sphaira or a HOME-menu [forwarder](glossary.md#forwarder) in [handheld mode](glossary.md#handheld-mode), and stop when startup, schedules, file reads, and input-to-exit work. Do not start rendering work before this log is clean.

## 3. Run console checks before adding features

**Why:** a game can hide platform problems by failing earlier. Use small checks that prove one platform surface at a time.

`examples/hello_switch/src/checks.rs` is the current model.

| Check | What it proves | Where it is |
|---|---|---|
| Asset write and load | The asset folder is beside the NRO, the SD card is writable, and Bevy's `AssetServer` can load a text file. | `examples/hello_switch/src/checks.rs:1-85`, `:166-234` |
| `std::fs` metadata | Rust `libc` and devkitA64 `newlib` agree on `stat`, `fstat`, and `read_dir` layouts. | `examples/hello_switch/src/checks.rs:86-164` |
| Thread-local thread | `std::thread::spawn` reaches `--wrap=threadCreate`, sets [TPIDR_EL0](glossary.md#tpidr_el0), and gives a new thread its own `thread_local!`. | `examples/hello_switch/src/checks.rs:236-270` |
| Exit heap check | Open services have released transfer memory before the loader reuses the heap. | `crates/bevy_horizon/src/lifecycle.rs:77-132`, `crates/bevy_horizon/src/memory.rs:180-252` |

Copy the pattern, not necessarily the exact code. A large game should add one tiny check for each platform feature it depends on: save files, shader cache, networking, audio device open, mod folder scan, or background worker shutdown.

## 4. Match the patch set to your Bevy version, and build

**Why:** this is the easiest way to get a false build. Cargo uses a `[patch.crates-io]` entry only when the patch's version is semver-compatible with the version the game's lock file resolves, and when it isn't, Cargo only warns: "Patch ... was not used in the crate graph".

bevy-horizon has one patch set per Bevy version. A game in its own repository gets it from a config file, so its `Cargo.toml` needs no `[patch]` table and its desktop builds don't change:

| Game's Bevy | Patch table | Bevy and wgpu patches | bevy_horizon dependency |
|---|---|---|---|
| 0.19 | `.cargo/bevy-0.19.toml` | `patches/`: bevy_asset and bevy_tasks 0.19.1; wgpu, wgpu-core and wgpu-hal 29.0.4; cpal 0.17.3 | `features = ["vulkan"]` (`bevy_0_19` is the default) |
| 0.18 | `.cargo/bevy-0.18.toml` | `patches/bevy-0.18/`: bevy_asset and bevy_tasks 0.18.1; wgpu 27.0.1, wgpu-core 27.0.3, wgpu-hal 27.0.4; cpal 0.15.3 | `default-features = false, features = ["bevy_0_18", "vulkan"]` |

Both tables also patch crates whose versions don't follow Bevy's: getrandom, inventory, lua-src, memmap2 and raw-window-handle, and for 0.19 also the second getrandom, polling, ring and rustix. [toolchain.md](toolchain.md#the-crate-patches) says what each patch changes. libc's patch is in `.cargo/config.toml` instead, because `-Z build-std` builds std as its own workspace, with its own lock file, and only a config file's `[patch]` reaches it (`.cargo/config.toml:5-11`).

cpal follows Bevy: Bevy 0.19's bevy_audio plays through rodio 0.22 and cpal 0.17.3, Bevy 0.18's through rodio 0.20.1 and cpal 0.15.3, so each set has its own cpal with the same Horizon host on libnx's audio renderer ([the audio host](input-and-audio.md#the-audio-host)). Without it, cpal falls back to a host with no devices, bevy_audio logs `No audio device found.` and plays nothing (`bevy_audio-0.18.1/src/audio_output.rs:33-40`). Both copies link libnx from their own build scripts, so a game's `build.rs` needs nothing for sound, and both close a stream still open at exit, since Bevy 0.18 never drops its own ([the audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit)). The SDL2 hosts they replaced linked SDL's EGL video code too: 6.1 MiB of Skate 3's NRO, against about 72 KiB now.

Build with `scripts/build_game.sh`, with the game checked out next to this repository:

```bash
BEVY=0.18 JOBS=3 CARGO_ARGS="--no-default-features" scripts/build_game.sh ../sk8 skate-game skate3rust "Skate 3"
```

The arguments are the game's workspace, the package with its binary, the binary, and the title HOME shows. `CARGO_ARGS` goes to `cargo build`; `JOBS` caps cargo's parallel jobs, since each rustc on a large Bevy crate can take 1-2 GB of RAM. The script (`scripts/build_game.sh`):

1. Moves each patched crate in the game's `Cargo.lock` to the patch's version, one `cargo update --precise` at a time, and stops if a crate in the patch's semver range is still locked to crates.io (`:49-99`). This changes the game's lock file in place. Skate 3's moved only libc, 0.2.189 to 0.2.190.
2. Moves std's own lock onto the libc patch, then runs `cargo build --release` for `aarch64-nintendo-horizon.json` with `-Z build-std` (`:101-119`).
3. Packs the ELF into an NRO with a NACP and the default icon, and checks that the NRO has the asset section sphaira forwarders need (`:121-151`). The NRO lands in the game's `target/aarch64-nintendo-horizon/release/`.

If the game's Bevy is neither 0.18 nor 0.19, make a new set first: re-make each Bevy and wgpu patch on that release, as `patches/bevy-0.18/` was made from `patches/`, and add a `.cargo/bevy-<version>.toml`.

## 5. Audit the dependency tree

**Why:** the hardest porting work is usually not your game loop. It is a crate that assumes Windows, Linux, Vulkan, a process model, dynamic libraries, or desktop file paths.

Use `cargo tree`, `cargo metadata`, and targeted source searches. For each dependency, ask these questions.

| Area | Search for | What can go wrong |
|---|---|---|
| OS cfgs | `target_os`, `cfg(windows)`, `cfg(unix)`, `cfg(target_family)` | Code chooses Linux, Windows, Android, or wasm paths and has no Horizon case. |
| Build scripts | `build.rs`, `cc`, `cmake`, `bindgen`, `pkg-config` | The script may call host tools, compile native code, or search a desktop SDK. |
| Native libraries | `*-sys`, `dlopen`, `libloading`, `steamworks`, `mlua`, `openssl` | devkitA64/newlib may not build them, or the platform service does not exist. |
| Rendering | `Backends::VULKAN`, custom WGSL, storage buffers, timestamp queries | The current plan is wgpu's GL backend on [switch-mesa](glossary.md#switch-mesa), not Vulkan. |
| Audio | `bevy_audio`, `cpal`, `rodio`, `kira` | Only cpal 0.17.3 and 0.15.3 have a Horizon host here ([§4](#4-match-the-patch-set-to-your-bevy-version-and-build)); a crate on another cpal version plays nothing. The 0.17.3 host plays on hardware. |
| Networking | `UdpSocket`, `TcpStream`, QUIC, Steam, TLS | Rust socket [ABI](glossary.md#abi) still has known mismatches. QUIC/TLS bring more dependencies. |
| Threads | `std::thread`, `rayon`, `tokio`, `detach`, Bevy `multi_threaded` | Threads now get TLS, but you still have only three usable application cores and must join or shut down workers. |
| Files | absolute paths, `current_exe`, app-data folders, case assumptions | On Switch, use `sdmc:/switch/<app>/...`; FAT-style case and separators can expose bugs. |
| Bevy plugins | [DefaultPlugins](glossary.md#defaultplugins), [Plugin](glossary.md#plugin) groups, `bevy_winit`, `bevy_gilrs` | winit has no Switch backend. Input comes from bevy_horizon's HID plugin, not gilrs. |

For Bevy itself, remember that an [ECS](glossary.md#ecs) game can run a lot of logic without rendering. Use that. Remove work until you get a clean frame loop, then add it back.

## 6. Switch facts that will bite any game

**Why:** these are platform rules, not game-specific bugs. Design with them from the start.

- **Launch mode changes memory.** The current measured HOME-forwarder path runs as an application, not an applet, with 3,189.0 MiB total and a 3,132.7 MiB malloc heap. Older 450 MB applet-mode guidance is not the current measured path. See [platform-notes.md](platform-notes.md) when it lands.
- **GPU memory is heap memory.** NVK allocates textures and buffers from the same malloc heap as everything else, and Bevy uploads every new asset in the next frame, where each image briefly exists three times (pixels, staging copy, texture). A game that loads hundreds of MiB at once should set bevy_render's `RenderAssetBytesPerFrame` (Skate 3: 64 MiB, `sk8/crates/skate-game/src/horizon.rs`) and log `bevy_horizon::memory_report()` at its load stages. Every buffer of 8 MiB or more is one contiguous heap block, so cap Bevy's mesh slabs with `MeshAllocatorSettings` (Skate 3: 64 MiB, down from 512 MiB), and give meshes and images the CPU never reads back `RenderAssetUsages::RENDER_WORLD`, so the main world doesn't keep a second copy ([platform-notes.md](platform-notes.md#gpu-memory-comes-from-the-same-heap)).
- **The loader reuses your heap.** Services such as sockets, [audren](glossary.md#audren), and [nv](glossary.md#nv) can leave transfer memory locked. bevy_horizon's exit hook releases sockets and checks heap pages (`crates/bevy_horizon/src/lifecycle.rs:77-132`). See [lifecycle-and-exit.md](lifecycle-and-exit.md).
- **Panic is abort.** The target uses `panic-strategy = "abort"` (`aarch64-nintendo-horizon.json:24`). The panic hook writes `panic.txt`, then the app exits like a normal exit: to HOME, with no crash report, whenever threads are still running, which is always true of a Bevy app ([lifecycle](lifecycle-and-exit.md#the-userappexit-hook)); see `crates/bevy_horizon/src/logging.rs:343-378`.
- **Quitting drops the App inside `App::run`, and with it the audio stream.** Closing the stream stops the cpal host's audio thread, which closes the audio renderer, waiting at most 1 s. Bevy 0.18 leaks the stream instead, and the host closes it at exit the same way. Save what matters before sending `AppExit` ([lifecycle](lifecycle-and-exit.md#quitting-the-app-drop)).
- **Without `bevy_render`, register `ImageLoader` yourself.** In Bevy 0.19, `bevy_image` only announces its image loader (`bevy_image-0.19.1/src/image.rs:245`); `bevy_render` registers it (`bevy_render-0.19.1/src/texture/mod.rs:59`). Without it, image loads stay `Loading` forever, even for a missing file. captest does this in `examples/switch_captest/src/bevy_phase.rs:73-76`.
- **Crash addresses need the ELF.** The NRO is relocated at runtime. `log.txt` prints the NRO base so you can subtract it from a PC and look up the address in the unstripped ELF (`crates/bevy_horizon/src/logging.rs:243-264`).
- **Input is a backend.** `HorizonInputPlugin` creates one Bevy gamepad, polls libnx pad state, maps Switch buttons to Bevy buttons, and writes raw gamepad events before `InputSystems` (`crates/bevy_horizon/src/input.rs:1-235`).
- **Display mode is only partially used.** `HorizonDisplayPlugin` can switch the Bevy window resource between 1280x720 and 1920x1080 (`crates/bevy_horizon/src/display.rs:1-46`), but docked mode is not hardware-proven yet.

## Worked examples

**Why:** a port plan is easier to judge when you compare real games instead of an empty template.

### IW4L

IW4L is a Rust rewrite of the MW2 engine on Bevy. The checkout used for this survey is `/home/johnuzoka/iw4l-switch`, HEAD `c294a9d`. Its workspace has 63 members. It pins Bevy through workspace dependencies as `bevy = { version = "0.19", ... }`, and its lock file currently resolves Bevy to 0.19.0 and wgpu to 29.0.4 (`/home/johnuzoka/iw4l-switch/Cargo.toml:83-104`, `Cargo.lock:690-693`, `Cargo.lock:7068-7071`).

It already has Switch-oriented patch entries for `quinn-proto`, `bevy_asset`, `cpal`, and `memmap2` (`/home/johnuzoka/iw4l-switch/Cargo.toml:1-8`). The build script uses the bevy_horizon image, `-Z build-std`, the bevy_horizon target spec, and packages `launcher` with `elf2nro` (`/home/johnuzoka/iw4l-switch/scripts/build_switch.sh:14-32`).

### Skate 3 Rust engine

The Skate 3 Rust engine is [SK8-ENGINE/skate-3-rust-engine](https://github.com/SK8-ENGINE/skate-3-rust-engine) (GPL-3.0), read at commit `4488651c35c44365faa1ba38eed5758b6ebde714`. Below, `sk8/` is the root of that repository. Its workspace pins `bevy = "=0.18.1"` and patches `bevy_pbr` and `bevy_core_pipeline` from `vendor/` (`sk8/Cargo.toml:11`, `sk8/Cargo.toml:27-29`). Its lock file has wgpu 27.0.1 (`sk8/Cargo.lock:5616-5619`).

It is a much harder first target because it forces Vulkan (`sk8/crates/skate-game/src/app.rs:77-83`), uses Bevy audio and WAV (`sk8/crates/skate-game/Cargo.toml:26`), enables development dynamic linking by feature (`sk8/crates/skate-game/Cargo.toml:23`), has raw XInput assumptions while disabling gilrs (`sk8/crates/skate-game/src/app.rs:87-91`), uses UDP and a separate Steam relay (`sk8/crates/skate-game/src/multiplayer/transport.rs:4-42`, `sk8/crates/skate-steam-relay/Cargo.toml:12`), embeds vendored Lua through `mlua` (`sk8/crates/skate-mods/Cargo.toml:9`), and has explicit map-loader worker threads (`sk8/crates/skate-game/src/map_transition.rs:107-124`).

## Requirements table

| Requirement | IW4L | Skate 3 Rust engine |
|---|---|---|
| Bevy | 0.19.0 in lock; workspace asks `0.19` with selected features including `bevy_winit`, `bevy_render`, `bevy_gilrs`, and `multi_threaded` (`Cargo.toml:83-104`). | Exact 0.18.1 (`sk8/Cargo.toml:11`). Needs a Bevy 0.18 patch set, not the current 0.19 one. |
| wgpu | 29.0.4 (`Cargo.lock:7068-7071`). | 27.0.1 (`sk8/Cargo.lock:5616-5619`). |
| Patches | `quinn-proto` git rev `f650e0f`, plus bevy_horizon `bevy_asset`, `cpal`, and `memmap2` (`Cargo.toml:1-8`). | Vendored `bevy_pbr` and `bevy_core_pipeline` (`sk8/Cargo.toml:27-29`, `sk8/vendor/README.md:1-41`). |
| Platform code | Has a Switch build script already (`scripts/build_switch.sh:14-32`). | Windows resource build branch uses `rc.exe` (`sk8/crates/skate-game/build.rs:39-52`). |
| Networking | Has a `net` crate and QUIC patch pressure through `quinn-proto` (`Cargo.toml:1-5`, `:53`, `:154`). | Direct UDP in game; Steam raw SDK in separate relay (`sk8/crates/skate-game/src/multiplayer/transport.rs:4-42`, `sk8/crates/skate-steam-relay/Cargo.toml:12`). |
| Input | Uses Bevy/gilrs features today; must route through bevy_horizon or game-specific input. | Comments say gameplay and menu use raw XInput, and gilrs is disabled (`sk8/crates/skate-game/src/app.rs:87-91`). Keyboard/mouse paths also exist. |
| Audio | CPAL patch is listed, but bevy_horizon audio is not hardware-proven. | Bevy audio and WAV are enabled (`sk8/crates/skate-game/Cargo.toml:26`). |
| Files/assets | Many asset crates and updater are workspace members (`Cargo.toml:13-75`). Needs SD asset layout. | `AssetPlugin.file_path` comes from `config.asset_root` (`sk8/crates/skate-game/src/app.rs:64-68`); startup loads many converted assets before app run (`sk8/crates/skate-game/src/main.rs:68-138`). |
| Threads | Bevy `multi_threaded` is enabled in workspace features (`Cargo.toml:83-104`). | At least two explicit map-loader threads (`sk8/crates/skate-game/src/map_transition.rs:107-124`). |
| Build system | Uses bevy_horizon container and target spec. | Has Windows-specific resource build and native dependency risks. |

## What blocks each today

**IW4L blockers:** rendering still needs wgpu GL on switch-mesa; `multi_threaded`, `bevy_winit`, and `bevy_gilrs` need a first-run decision (`Cargo.toml:83-104`); QUIC/networking should wait for socket ABI work; and `scripts/build_switch.sh:24-32` should copy the NACP/icon packaging and ASET verifier from `scripts/build_example.sh:60-95`.

**Skate 3 blockers:** it needs a Bevy 0.18 patch set, not the current Bevy 0.19 patches; it forces Vulkan (`sk8/crates/skate-game/src/app.rs:77-83`); raw XInput must be replaced (`sk8/crates/skate-game/src/app.rs:87-91`); Steamworks should be isolated (`sk8/crates/skate-steam-relay/Cargo.toml:12`); vendored Lua needs a devkitA64/newlib plan (`sk8/crates/skate-mods/Cargo.toml:9`); and its loader threads and Windows resource build need review (`sk8/crates/skate-game/src/map_transition.rs:107-124`, `sk8/crates/skate-game/build.rs:39-52`).

## What bevy_horizon covers today

**Why:** use the layer for the parts it owns, and do not assume it solved the rest of Bevy.

bevy_horizon currently provides:

- startup logging before `main`, panic files, abort files, crash files, and NRO base reporting;
- main-thread and spawned-thread TLS setup;
- a Bevy runner at 60 Hz when no other runner is installed (`crates/bevy_horizon/src/lib.rs:31`, `:52-54`);
- libnx HID to Bevy gamepad events (`crates/bevy_horizon/src/input.rs:1-235`);
- handheld/docked window resource updates, not yet docked-proven (`crates/bevy_horizon/src/display.rs:1-46`);
- RomFS mount attempt and socket initialize/release (`crates/bevy_horizon/src/lifecycle.rs:27-75`, `:77-132`);
- SD-card asset-root setup before `DefaultPlugins` (`crates/bevy_horizon/src/logging.rs:171-210`);
- crate patches for the current Bevy 0.19 path.

It does not yet provide:

- a working wgpu backend;
- proven audio playback;
- proven Rust networking;
- a general solution for Windows-only build scripts or native library ports;
- a guarantee that your custom WGSL will run on GLES/Mesa;
- emulator coverage or a capability-test matrix.

Port in that order. Make each missing surface small, logged, and reversible.
