# Building an NRO with bevy_horizon

This page shows how to build the `hello_switch` example into a Nintendo Switch [homebrew](glossary.md#homebrew) [NRO](glossary.md#nro). It is for you if you are checking this layer on a real console, or if you want to copy the same build path into your own [Bevy](glossary.md#bevy) game.

## In short

- **Host system:** Linux with Docker or Podman. The scripts use `CONTAINER_ENGINE` if it's set, otherwise Docker if it's installed, otherwise Podman, and run the build inside `bevy-horizon-toolchain:local`; see `scripts/build_example.sh:4-13`.
- **Current image:** Rust `1.101.0-nightly (db8f076d2 2026-10-03)` (pinned as `nightly-2026-10-04`), devkitA64 `r30-1`, GCC `16.1.0`, [libnx](glossary.md#libnx) `4.12.0-1`, `switch-mesa` `20.1.0-5`, `switch-sdl2` `2.28.5-4`, and `switch-tools` `1.13.1-1`.
- **Build command:** `./scripts/build_example.sh`. It builds `std` with `-Z build-std`, checks that crate patches were used, creates a [NACP](glossary.md#nacp), and runs `elf2nro` with `--nacp` and `--icon`; see `scripts/build_example.sh:13-97`.
- **Current outputs:** `target/aarch64-nintendo-horizon/release/hello_switch` is 109,147,384 bytes, SHA-256 `12fa8c0e967c20cec285f51720d3eac9770a6c4a034f8519d88a60c07778f472`. `hello_switch.nro` is 53,745,861 bytes, SHA-256 `36941504c59d3456c6b4b5d3665d70d3130ab36ab4c161fe4aed296821458ec6`.
- **Console status:** the last hardware-proven build A2 ran in [handheld mode](glossary.md#handheld-mode), held a 60 fps frame loop, read controller input, loaded an asset from the SD card, used `std::thread` with [thread-local storage](glossary.md#thread-local-storage), and then returned from the app. The loader crash after return is fixed in A3 code but A3 has not been run yet.
- **Pictures through Vulkan:** with bevy_horizon's `vulkan` feature, Bevy draws through [wgpu](glossary.md#wgpu)'s Vulkan backend and NVK. It works in the Ryujinx harness and hasn't run on a console yet; see [rendering.md](rendering.md).

## What you need

**Why this matters:** most build failures are not Rust errors. They are missing SDK pieces, an old image, or a patch that Cargo silently skipped.

| Need | What to check | Why |
|---|---|---|
| Linux | `docker --version` or `podman --version` | The build scripts use Docker or Podman containers, not a host devkitPro install. |
| Disk space | Several GB free for the image and `target/` | `-Z build-std` builds Rust `std` for this target, and Bevy brings many crates. |
| RAM | Enough for a release Bevy build | The current example is much smaller than a full game, but first builds still compile Rust `std`, Bevy, and patches. |
| Console | Atmosphère, sphaira or another [hbloader](glossary.md#hbloader), and an SD card mounted as [sdmc](glossary.md#sdmc) | Logs and assets are written beside the NRO on the SD card. |
| Launch mode | sphaira or a HOME-menu [forwarder](glossary.md#forwarder), handheld | The verified runs were handheld only. [Docked mode](glossary.md#docked-mode) is not proven yet. |

A first build is much longer than an incremental build because it compiles Rust `std` for the custom target. The owner measured later incremental builds at about 30 seconds. Treat that as a rough local number, not a promise for a clean checkout.

## 1. Build the toolchain image

**Why:** the Switch build needs [devkitPro](glossary.md#devkitpro), [devkitA64](glossary.md#devkita64), libnx, portlibs, `elf2nro`, and nightly Rust in one known environment.

From the repository root:

```sh
docker build -t bevy-horizon-toolchain:local -f Containerfile .
```

`podman build` takes the same arguments.

The `Containerfile` starts from `docker.io/devkitpro/devkita64:latest` (`Containerfile:1`). It installs:

| Package | Containerfile lines | Used for |
|---|---:|---|
| `switch-mesa` | `Containerfile:5` | Planned [Mesa](glossary.md#mesa) / [switch-mesa](glossary.md#switch-mesa) rendering path. Not used by the current headless example. |
| `switch-sdl2` | `Containerfile:6` | Unused. The patched `cpal` hosts played through [SDL2](glossary.md#sdl2) until they moved to libnx's audio renderer ([why not SDL2](input-and-audio.md#why-not-sdl2)); nothing links it now, and it can go at the next image rebuild. |
| `switch-tools` | `Containerfile:7` | `nacptool` and `elf2nro`. |
| `libnx` | `Containerfile:8` | Startup, services, HID, sockets, RomFS, and NRO packaging support. |
| `devkitA64` | `Containerfile:9` | GCC, newlib, and static libraries for AArch64 Switch programs. |

The image adds Rust nightly and the `rust-src` and `llvm-tools-preview` components (`Containerfile:19-32`). The nightly is pinned to `nightly-2026-10-04` and installed under the name `nightly`, because `-Z build-std` puts std's sysroot path in every binary's debug info and the plain name keeps rebuilds byte-identical to earlier ones. `--build-arg RUST_NIGHTLY=nightly` takes the latest nightly instead. The image also sets `PATH`, `DEVKITPRO`, and `DEVKITARM` (`Containerfile:13-17`). In the current image, GCC is under `/opt/devkitpro/devkitA64/bin`; if a shell cannot find `aarch64-none-elf-gcc`, check `PATH` first.

To inspect the image without building anything:

```sh
timeout 120 docker run --rm bevy-horizon-toolchain:local bash -lc \
  'rustc -Vv; cargo -V; /opt/devkitpro/devkitA64/bin/aarch64-none-elf-gcc --version | head -1; dkp-pacman -Q libnx devkitA64 switch-mesa switch-sdl2 switch-tools'
```

## 2. Build the example

**Why:** `hello_switch` is the smallest current proof that Bevy schedules, input, file I/O, assets, threads, and exit cleanup work on the console.

Run:

```sh
./scripts/build_example.sh
```

The script does five separate jobs.

1. **Ensure the image exists.** If `bevy-horizon-toolchain:local` is missing, the script builds it from the repository `Containerfile`; see `scripts/build_example.sh:7-11`.
2. **Move Rust `std` onto the patched libc.** Rust `std` is built from source with [build-std](glossary.md#build-std), and `std` has its own lock file. The script runs `cargo update -p libc` in Rust's `library/` workspace, then stops if that lock file would still use crates.io `libc`; see `scripts/build_example.sh:14-28` and `.cargo/config.toml:5-11`.
3. **Compile the example.** Cargo runs with `--release -Z build-std=core,alloc,std,panic_abort -Z json-target-spec --target /workspace/aarch64-nintendo-horizon.json --package hello_switch`; see `scripts/build_example.sh:29-31`.
4. **Check crate patches.** The script reads `[patch.crates-io]` and `Cargo.lock`. If a registry package with a semver-compatible version is still present, it prints an error and exits; see `scripts/build_example.sh:33-58`.
5. **Package and verify the NRO.** `nacptool` writes `hello_switch.nacp`, `elf2nro` adds the NACP and libnx default icon, then a Python check requires an `ASET` asset section; see `scripts/build_example.sh:60-95`.

The important output lines look like this:

```text
==> Compiling hello_switch for Nintendo Switch (Horizon OS)...
==> Checking every crate patch replaced its crates.io version...
==> Packaging into hello_switch.nro Homebrew binary...
==> Verifying NRO asset section...
    ok: image 0x... + ASET section, file 0x... bytes
==> Done! Output: /home/johnuzoka/bevy-horizon/hello_switch.nro
```

## 3. Know what the files are

**Why:** you need the unstripped [ELF](glossary.md#elf) for crash addresses, but the console launches the NRO.

| File | Meaning | Current size |
|---|---|---:|
| `target/aarch64-nintendo-horizon/release/hello_switch` | The linked ELF. Keep it with any test build so `addr2line` can translate crash PCs. | 109,147,384 bytes |
| `target/aarch64-nintendo-horizon/release/hello_switch.nacp` | Title metadata made by `nacptool`. | generated during packaging |
| `hello_switch.nro` | The homebrew file to copy to the SD card. | 53,745,861 bytes |

Check a build before copying it:

```sh
stat -c '%n %s bytes' target/aarch64-nintendo-horizon/release/hello_switch hello_switch.nro
sha256sum target/aarch64-nintendo-horizon/release/hello_switch hello_switch.nro
```

The current checksums are:

```text
12fa8c0e967c20cec285f51720d3eac9770a6c4a034f8519d88a60c07778f472  target/aarch64-nintendo-horizon/release/hello_switch
36941504c59d3456c6b4b5d3665d70d3130ab36ab4c161fe4aed296821458ec6  hello_switch.nro
```

## 4. Why `--nacp` and `--icon` are required

**Why:** this was a real loader failure. A previous script passed `--name`, `--author`, and `--version` to `elf2nro`. `elf2nro` ignored them, the NRO had no asset section, and sphaira's loader rejected it before any Rust code ran.

The fixed script does this instead:

```sh
nacptool --create "Hello Switch" "bevy-horizon" "0.1.0" "$OUT/hello_switch.nacp"
elf2nro "$OUT/hello_switch" hello_switch.nro \
    --nacp="$OUT/hello_switch.nacp" \
    --icon=/opt/devkitpro/libnx/default_icon.jpg
```

Those exact commands are in `scripts/build_example.sh:69-74`. The verification step reads the NRO header, seeks to the image size, and requires `ASET`; see `scripts/build_example.sh:82-94`.

If you want packed read-only files inside the NRO later, add [RomFS](glossary.md#romfs) with `elf2nro --romfsdir=<directory>`. The current example does not use RomFS. At runtime `romfsMountSelf` returns libnx `IoError`, and this layer logs that as normal unless `--romfsdir` was used; see `crates/bevy_horizon/src/lifecycle.rs:27-66`.

## 5. Copy to the SD card and launch

**Why:** paths matter. The logging and asset code derive the app folder from the NRO path.

Use this layout:

```text
sdmc:/switch/hello_switch/
    hello_switch.nro
    assets/
```

Copy the file:

```sh
mkdir -p /media/$USER/<sd-card>/switch/hello_switch
cp hello_switch.nro /media/$USER/<sd-card>/switch/hello_switch/hello_switch.nro
sync
```

Then launch it from sphaira, or from a HOME-menu forwarder that starts sphaira's loader. The latest verified hardware run used handheld mode. Starting from the HOME menu as an application gave the process 3,189.0 MiB total memory and a 3,132.7 MiB malloc heap; that replaced an older unverified applet-mode estimate.

Do not expect a picture yet. `hello_switch` builds scene data, UI, meshes, and materials, but on Horizon it disables wgpu backend creation (`examples/hello_switch/src/main.rs:40-52`). Use `log.txt` to decide whether it worked.

## 6. Read the logs

**Why:** most current proof comes from text logs, not the screen.

This layer opens `log.txt` before `main` through libnx's [userAppInit](glossary.md#userappinit) hook, redirects stdout and stderr, and sets the asset root before Bevy's `DefaultPlugins` run; see `crates/bevy_horizon/src/logging.rs:1-22` and `crates/bevy_horizon/src/logging.rs:132-179`.

Files land beside the NRO:

| File | When it appears | Source |
|---|---|---|
| `sdmc:/switch/hello_switch/log.txt` | Every run. Holds `println!`, Bevy/tracing lines, memory, frame heartbeat, input, and exit checks. | `crates/bevy_horizon/src/logging.rs:1-18` |
| `sdmc:/switch/hello_switch/panic.txt` | Appended by the Rust panic hook. | `crates/bevy_horizon/src/logging.rs:343-378` |
| `sdmc:/switch/hello_switch/crash.log` | Appended by the libnx exception override for CPU faults. | `crates/bevy_horizon/src/compat.rs:274-357` |
| `sdmc:/switch/hello_switch/abort.log` | Appended when libnx aborts with a `Result`. | `crates/bevy_horizon/src/compat.rs:258-272` |
| `sdmc:/atmosphere/crash_reports/*.log` | Written by [Atmosphère](glossary.md#atmosphère) / [creport](glossary.md#creport) after a fatal break or exception. | Console output, not repository code. |

Good lines include:

```text
[Boot] TPIDR_EL0 = ... Rust's thread-local base ...
[Memory] applet type 0 = Application: running as a game ...
[Input] HID configured: 1 player, standard controllers; reading player 1 and handheld
[Engine] 60 frames updated
[Check] fs: PASS, ...
[Check] asset: PASS, ...
[Check] thread: PASS in frame 60, ... thread_local ...
[Exit] app.run() returned Success
```

The A3 build also logs a heap check at exit. It should end with:

```text
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

A3 has been built but not yet run on hardware, so treat that last line as the expected result, not a proven console result.

## Troubleshooting

**Why:** these are not theoretical. Each row is a failure that already happened while bringing up this layer.

| Symptom | Likely cause | How you notice it | Fix |
|---|---|---|---|
| sphaira returns a crash report before any `log.txt` | NRO has no `ASET` asset section | Atmosphère report names only the loader, not the app | Use `nacptool` plus `elf2nro --nacp --icon`; keep the verifier in `scripts/build_example.sh:76-95`. |
| Build says the `bevy_asset` patch is unused | Patch version does not match the locked Bevy crate family | `error: patches/bevy_asset is bevy_asset ..., but Cargo.lock still has crates.io's bevy_asset ...` | Re-make the patch from the exact locked crate version; see [toolchain.md](toolchain.md#how-to-check-that-a-patch-is-really-applied). |
| `std` file checks fail on console | Rust `std` used crates.io `libc` instead of `patches/libc` | `[Check] fs: FAIL`, wrong file/folder metadata, or asset read sizing is wrong | Keep `.cargo/config.toml:10-11` and the `cargo update -p libc` guard in `scripts/build_example.sh:22-28`. |
| Panic while Bevy creates the renderer | No Vulkan driver is linked: without bevy_horizon's `vulkan` feature the app gets a stub, and wgpu finds no GPU | `Unable to find a GPU! Make sure you have installed required drivers!` (`bevy_render-0.19.1/src/renderer/mod.rs:286`) | Build with the `vulkan` feature, after building NVK ([rendering.md](rendering.md)). A game that draws nothing can set `RenderPlugin`'s `backends` to `None` instead. |
| App runs once and returns immediately | No runner calls `App::update` every frame | You see setup logs but no heartbeats | `HorizonPlugin` adds `ScheduleRunnerPlugin::run_loop` at 60 Hz when needed (`crates/bevy_horizon/src/lib.rs:31`, `:52-54`). |
| Crash after `app.run()` returns to sphaira | A service still locks heap pages used by the loader | Crash report result `2001-0106` in loader after a clean app exit | A3 calls `socketExit`, unmounts RomFS, waits for heap unlock, and logs a [heap-check](glossary.md#heap-check); see `crates/bevy_horizon/src/lifecycle.rs:77-132`. |
| No audio | bevy_audio's cpal isn't the patch for the game's Bevy version: `patches/cpal` (cpal 0.17.3) for 0.19, `patches/bevy-0.18/cpal` (0.15.3) for 0.18 | `No audio device found.` in the log (`bevy_audio-0.19.1/src/audio_output.rs:22-25`; 0.18.1 `:33-40`) | Build with `scripts/build_game.sh`, which adds the right one through `.cargo/bevy-$BEVY.toml` ([porting](porting-a-bevy-game.md#4-match-the-patch-set-to-your-bevy-version-and-build)). In Ryujinx under `run.sh`, the sound plays into `/dev/null` ([testing](testing.md#what-the-emulator-gets-wrong)). |
| Sound crackles or drops out | The audio thread refilled a buffer too late, so the renderer ran out of sound | With `BH_AUDIO_STATS=1`, `[Audio]` lines every 10 s whose `ran dry` count isn't 0 outside loading | Longer buffers: `BH_AUDIO_FRAMES=2048`, or 4096, in the game's environment (Skate 3: an `env` line in `options.txt`), at the cost of latency ([the audio host](input-and-audio.md#the-audio-host)). |

## Building your own game

**Why:** the example is a check program, not a template for every dependency tree.

Start with the same script shape, but change `--package hello_switch` to your game package and package your binary name instead of `hello_switch`; see `scripts/build_example.sh:29-31` and `scripts/build_switch.sh` in the IW4L checkout for a game-specific example. Then read [porting-a-bevy-game.md](porting-a-bevy-game.md). Your first porting job is not rendering. It is matching the Bevy version, proving the same plugin set on PC, and making sure every crate patch is actually selected.
