# Getting started

This page takes you from an empty folder to bevy-horizon's `hello_switch` example running on a Switch as a [homebrew](glossary.md#homebrew) [NRO](glossary.md#nro). Read it first if you know Rust and [Bevy](glossary.md#bevy), have a Switch running [Atmosphère](glossary.md#atmosphère), and are new to [Horizon](glossary.md#horizon), the Switch's OS. Your own game builds with the same toolchain image and NVK, so you need this setup either way.

## In short

- **You need** an x86_64 Linux PC (or WSL2 on Windows) with Docker or Podman, about 20 GB of free disk, and a Switch running Atmosphère with a homebrew menu such as [sphaira](glossary.md#sphaira).
- **Build in this order:** the toolchain image, then NVK (the Vulkan driver), then the example. You build the image and NVK once.
- **The build leaves** `hello_switch.nro` in the repository's root. Copy it to `sdmc:/switch/hello_switch/hello_switch.nro`.
- **Start it with a game's memory:** hold R while you start any game, or use a sphaira [forwarder](glossary.md#forwarder) on the HOME menu. Don't start it from the Album.
- **Read `log.txt`** in the same folder. After you press (+) to exit, a good run ends with `[Exit] PASS`.
- **Test on the console, not in an [emulator](glossary.md#emulator).** Stock Ryujinx can't run the Vulkan builds.

The whole build, from an empty folder:

```sh
git clone https://github.com/JohnUzoka/bevy-horizon
git clone https://github.com/danfromtico/mesa-switch
git -C mesa-switch checkout 606441c
cd bevy-horizon
docker build -t bevy-horizon-toolchain:local -f Containerfile .
scripts/build_nvk.sh
scripts/localize_nvk.sh
scripts/build_example.sh hello_switch
```

## What you need

| Need | Details |
|---|---|
| **An x86_64 PC** | Linux, or WSL2 on Windows. devkitPro publishes its build image for x86_64 only. |
| **Docker or Podman** | Everything compiles in containers, so you don't install [devkitPro](glossary.md#devkitpro) or Rust on the host. The scripts use `CONTAINER_ENGINE` if it's set, otherwise `docker` if it's installed, otherwise `podman`. |
| **Git and a network connection** | The builds download the devkitPro image, a Rust nightly, crates, and Mesa's build tools. |
| **About 20 GB of disk** | The toolchain image is about 6.6 GB, and the NVK build image adds LLVM on top of it. The mesa-switch checkout with its build folders is about 2 GB, the crate cache about 1.5 GB, and `target/` about 5 GB once every example is built. |
| **Memory for `rustc`** | Each `rustc` on a big Bevy crate can take 1–2 GB. Cargo runs one job per CPU thread unless you set `JOBS`; `JOBS=4` needs up to about 8 GB. |
| **A Switch with Atmosphère** | With a homebrew menu: sphaira, which all testing used, or the stock hbmenu through [hbloader](glossary.md#hbloader). You also need a way to copy files to its SD card. |

If you have both Docker and Podman, build the image with the engine the scripts pick, or set `CONTAINER_ENGINE`. The containers run as root, so with rootful Docker the files they write, such as `target/` and the NROs, belong to root. Rootless Docker and Podman map them to your user.

All testing so far used one Switch Lite in [handheld mode](glossary.md#handheld-mode). [Docked mode](glossary.md#docked-mode) is untested: the window switches to 1920×1080 when docked, but nothing has run docked yet.

## 1. Get the code

Clone bevy-horizon and danfromtico's mesa-switch into the same folder, and put mesa-switch on the commit that bevy-horizon's NVK patches are made for:

```sh
git clone https://github.com/JohnUzoka/bevy-horizon
git clone https://github.com/danfromtico/mesa-switch
git -C mesa-switch checkout 606441c
cd bevy-horizon
```

- **mesa-switch** is a port of [Mesa](glossary.md#mesa)'s NVK Vulkan driver to Horizon. bevy-horizon's own patches to it are in `patches/mesa-switch/`.
- **Stay on `606441c`** (Mesa 26.2.4). It's the commit the patches are made for, and mesa-switch's main branch has moved on since.
- **`scripts/build_nvk.sh` looks for `../mesa-switch`.** For a checkout elsewhere, pass its path: `scripts/build_nvk.sh /path/to/mesa-switch`.

## 2. Build the toolchain image

From bevy-horizon's folder:

```sh
docker build -t bevy-horizon-toolchain:local -f Containerfile .
```

With Podman, run `podman build` with the same arguments. The image has [devkitA64](glossary.md#devkita64) (GCC and newlib for the Switch's CPU), [libnx](glossary.md#libnx), `elf2nro` and `nacptool`, and a pinned Rust nightly with `rust-src`. Every script uses it.

`scripts/build_example.sh` builds the image if it's missing, but it never rebuilds it, so run the command above again after the `Containerfile` changes. `scripts/build_nvk.sh` doesn't build the image at all, which is why this step comes before NVK.

The image used for testing had:

| Package | Version | Used for |
|---|---|---|
| Rust | `1.101.0-nightly (db8f076d2)` | Compiling the game and Rust's std |
| devkitA64 | `r30-1`, with GCC 16.1.0 | GCC, binutils and newlib for the Switch's CPU |
| libnx | `4.12.0-1` | Horizon's services: startup, HID, files, sockets, audio |
| switch-tools | `1.13.1-1` | `elf2nro` and `nacptool` |
| switch-mesa | `20.1.0-5` | OpenGL ES, used only by `switch_captest` |
| switch-sdl2 | `2.28.5-4` | Nothing; it's installed but unused ([why not SDL2](input-and-audio.md#why-not-sdl2)) |

- **The base image isn't pinned.** The `Containerfile` starts from `devkitpro/devkita64:latest`, so a fresh build can bring a newer devkitA64. The `build.rs` files expect GCC 16.1.0; see [Troubleshooting](#troubleshooting) if linking can't find `-lgcc`.
- **The nightly is pinned by `ARG RUST_NIGHTLY`** and installed under the plain name `nightly`. `-Z build-std` writes std's path into the debug info, so the plain name keeps rebuilds byte-identical. `--build-arg RUST_NIGHTLY=nightly` takes the latest nightly instead.

To see what an image has:

```sh
docker run --rm bevy-horizon-toolchain:local bash -c \
  'rustc -V; aarch64-none-elf-gcc --version | head -1; dkp-pacman -Q devkitA64 libnx switch-tools switch-mesa'
```

## 3. Build NVK

Bevy draws through [wgpu](glossary.md#wgpu)'s [Vulkan](glossary.md#vulkan) backend, and on the Switch the Vulkan driver is NVK, linked into the NRO. bevy_horizon's `vulkan` feature does the linking, and `hello_switch`, `breakout` and `render_check` turn it on. Build NVK before them:

```sh
scripts/build_nvk.sh
scripts/localize_nvk.sh
```

- **`[JOBS=n] scripts/build_nvk.sh [mesa-switch checkout]`** applies `patches/mesa-switch/` to the checkout's files without committing them, skips patches that are there already, and stops if one doesn't apply. It then builds a second image, `bevy-horizon-nvk:local`, from the toolchain image plus Mesa's build tools, and compiles NVK in it, 4 jobs at a time unless `JOBS` says otherwise. It writes `nvk/lib/libvulkan.a`, `nvk/include/`, and `nvk/BUILD_INFO`, which records the Mesa commit, the patches and the toolchain versions. The build folders stay in the mesa-switch checkout, so a second run rebuilds only what changed.
- **`scripts/localize_nvk.sh`** turns `libvulkan.a` into `nvk/lib/libvulkan_bh.a`, the file a game links. NVK's shader compiler (NAK) and image-layout library (NIL) are Rust static libraries, each with its own copy of Rust's std, which would clash with the game's. The script leaves only the `vk*` entry points and a few `nouveau_horizon_*` functions visible. `build_nvk.sh` doesn't run it, so run it after every NVK build. Without an NVK build it stops with `error: nvk/lib/libvulkan.a is missing; run scripts/build_nvk.sh first`.

`nvk/` isn't in git, so every checkout of bevy-horizon builds its own. Run both scripts again only when mesa-switch or its patches change, or after you rebuild the toolchain image. `build_nvk.sh` builds its own image only if it's missing, so in that last case remove the old one first: `docker image rm bevy-horizon-nvk:local`.

If the library is missing, the `vulkan` feature's build script stops the build with:

```text
bevy_horizon's `vulkan` feature links .../nvk/lib/libvulkan_bh.a, which is missing: run scripts/build_nvk.sh and scripts/localize_nvk.sh, or set BEVY_HORIZON_NVK_LIB to the directory that has it
```

`BEVY_HORIZON_NVK_LIB` is read at build time, but it only takes effect when you run Cargo yourself: `build_example.sh` and `build_game.sh` don't pass it into their containers, so with the scripts the library must be in `nvk/lib/`. [toolchain.md](toolchain.md#linking-nvk-the-vulkan-feature) has the details of the build and the link.

`switch_captest` doesn't use the `vulkan` feature, so it builds without NVK. A game that draws but leaves the feature out panics when Bevy creates its renderer, with `Unable to find a GPU! Make sure you have installed required drivers!`.

## 4. Build hello_switch

```sh
scripts/build_example.sh hello_switch
```

- **Usage:** `[JOBS=n] scripts/build_example.sh [example] [title]`. The example is a folder under `examples/`, `hello_switch` by default. The title is the name the HOME menu and sphaira show; by default it comes from the example's name ("Hello Switch").
- **The first build takes a long time,** because it compiles Rust's std and all of Bevy for the Switch. The three examples that draw use the same Bevy features, so once one is built, the others compile little more than their own code.
- **If the build is killed for lack of memory,** cap cargo's jobs: `JOBS=4 scripts/build_example.sh hello_switch`.

A good build prints:

```text
==> Ensuring toolchain image exists...
==> Compiling hello_switch for Nintendo Switch (Horizon OS)...
...
==> Checking every crate patch replaced its crates.io version...
==> Packaging into hello_switch.nro Homebrew binary with romfs from examples/hello_switch/romfs...
==> Verifying NRO asset section...
    ok: image 0x... + ASET section, romfs ... bytes, file 0x... bytes
==> Done! Output: .../bevy-horizon/hello_switch.nro
```

[How the build works](#how-the-build-works) explains each step and its errors.

### The other examples

Build them the same way, with the example's name as the argument. Each NRO lands in the repository's root as `<name>.nro`, and goes in a folder of its own on the SD card, `sdmc:/switch/<name>/`.

| Example | What it's for | On a Switch Lite |
|---|---|---|
| `breakout` | Bevy's Breakout example with gamepad input: the paddle follows the D-pad or the left stick. | Plays to the end at about 59 fps |
| `render_check` | Tests the 3D path one setting at a time: six cameras, each with one render setting changed, look at four cubes each. It checks every tile, reads a buffer back and saves a screenshot. It reads `options.txt` from `sdmc:/switch/render_check/` and saves its screenshots there, so keep it at `sdmc:/switch/render_check/render_check.nro`. | All 24 cubes at about 20 fps (it's a stress test) |
| `switch_captest` | The capability test: it checks what a Bevy game needs from the console and writes `captest_results.txt` beside the NRO. It doesn't need NVK. See [testing.md](testing.md). | 179 pass, 0 fail, 2 warn |

In `hello_switch`, `breakout` and `render_check`, holding (−) and pressing (+) shows or hides the FPS overlay, and (+) exits.

### What the build leaves

| File | What it is |
|---|---|
| `hello_switch.nro`, in the repository's root | The program. This is what you copy to the SD card. |
| `target/aarch64-nintendo-horizon/release/hello_switch` | The unstripped [ELF](glossary.md#elf). Keep it with every NRO you test: with the `nro base` from `log.txt`'s header, it turns a crash address into a function name ([logging-and-crash-reports.md](logging-and-crash-reports.md) shows how). |
| `target/aarch64-nintendo-horizon/release/hello_switch.nacp` | The [NACP](glossary.md#nacp): the title, author and version that the HOME menu and sphaira show. |

The NRO ends in an asset section that holds the icon, the NACP and the [RomFS](glossary.md#romfs). `elf2nro` writes that section only when it's given `--icon`, `--nacp` or a RomFS file. `--romfsdir` on its own isn't enough: without the section, the RomFS is left out too. `elf2nro` also silently ignores `--name`, `--author` and `--version`, which are `nacptool`'s. A sphaira forwarder refuses to start an NRO without the section. It aborts before any of your code runs, so no `log.txt` appears, and Atmosphère's [crash report](glossary.md#crash-report) lists only the loader. The scripts always pass `--nacp` and `--icon`, and check for the section after packaging.

## 5. Copy it to the SD card

Give each NRO a folder of its own on the SD card ([sdmc](glossary.md#sdmc)), named like the NRO:

```text
sdmc:/switch/hello_switch/
    hello_switch.nro
    log.txt       written at each launch
    assets/       Bevy's asset folder
```

- **The log goes to `sdmc:/switch/<NRO name>/`,** wherever the NRO is, and the folder is made if it's missing. If that fails, the log goes to `sdmc:/log.txt`.
- **Assets load from beside the NRO.** bevy_horizon sets `BEVY_ASSET_ROOT` to the NRO's folder before `main`, so Bevy loads from the `assets` folder there. With the folder named like the NRO, the log and the assets share it.
- **hello_switch needs nothing else.** Its asset check writes `assets/a2_check.txt` itself, and its RomFS is inside the NRO.

With the SD card in the PC:

```sh
SD=/path/to/sd-card
mkdir -p "$SD/switch/hello_switch"
cp hello_switch.nro "$SD/switch/hello_switch/"
sync
```

Unmount the card before you take it out.

## 6. Start it with a game's memory

Start the NRO as a game, in one of two ways:

- **Title override:** hold R while you start any game from the [HOME menu](glossary.md#home-menu). Atmosphère opens the homebrew menu in the game's place, with the game's memory. Choose hello_switch there.
- **A HOME-menu forwarder:** sphaira can install an icon on the HOME menu that starts the NRO as a game.

Don't start it from the Album. The homebrew menu there runs in [applet mode](glossary.md#applet-mode), which isn't enough for a Vulkan game:

- **GPU memory comes out of the malloc heap.** NVK takes the memory for textures, buffers and shaders from the same heap as the rest of the game ([GPU memory comes from the same heap](platform-notes.md#gpu-memory-comes-from-the-same-heap)). Started as a game, that heap is about 3.1 GiB; as an applet it is far smaller.
- **HOME needs an application.** Only an application can set the [focus-handling mode](glossary.md#focus-handling-mode), so in applet mode the game isn't told when the player goes to HOME ([HOME and focus](lifecycle-and-exit.md#home-and-focus)). The log then says `[Applet] focus handling left as it was (...): only an application can change it, so focus changes won't be reported`.

The first `[Memory]` line in `log.txt` shows which way it started:

```text
[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
[Memory] applet type 2 = LibraryApplet: applet mode, e.g. started from the Album, so it gets an applet's memory
```

On the screen, hello_switch shows a lit, spinning cube and a panel listing its controls. The left stick changes how fast the cube spins, (A) changes its colour, holding (−) and pressing (+) shows or hides the [FPS overlay](rendering.md#the-fps-overlay), and (+) exits.

The first launch is slower: NVK compiles its shaders and caches them under `sdmc:/.mesa/`, so later launches start faster. Deleting that folder is safe; the next launch compiles the shaders again.

## 7. Read log.txt

`sdmc:/switch/hello_switch/log.txt` is rewritten at each launch. bevy_horizon opens it before `main`, in libnx's [userAppInit](glossary.md#userappinit), and sends stdout, stderr and Bevy's log there. Other files appear beside it only when something goes wrong:

| File | Written when |
|---|---|
| `panic.txt` | A Rust panic |
| `crash.log` | A CPU exception, such as a bad pointer |
| `abort.log` | libnx aborts with a [result code](glossary.md#result-code) |

These three are appended to, and the same report goes into `log.txt`. Atmosphère writes its own reports to `sdmc:/atmosphere/crash_reports`. [logging-and-crash-reports.md](logging-and-crash-reports.md) explains how to read all of them.

A good hello_switch run looks like this. `...` marks lines left out, `<time>` is a timestamp, and the `[Check]` lines can come in a different order:

```text
==== bevy_horizon log: hello_switch ====
opened by:      userAppInit, before Rust's runtime setup and main
...
nro base:       0x... (crash address - base = address in the ELF)
========================================
[Boot] launched from sdmc:/switch/hello_switch/hello_switch.nro
[Boot] TPIDR_EL0 = 0x... (Rust's thread-local base, copied from libnx ThreadVars.tls_tp)
[Boot] TLS template at nro+0x..., align ...: Rust and libnx agree on thread-local addresses
[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
[Memory] process: ... MiB in total, ...
[Memory] malloc heap: ... MiB, set up by the homebrew loader; ...
[Assets] BEVY_ASSET_ROOT = sdmc:/switch/hello_switch (Bevy's default asset folder is sdmc:/switch/hello_switch/assets)
[Boot] next: C constructors, Rust's runtime setup (std::rt::lang_start), then main
[Main] init_switch_logging("hello_switch"): Rust's runtime is up and main is running
...
WARNING: NVK is not a conformant Vulkan implementation, testing use only.
<time>  INFO bevy_render::renderer: AdapterInfo { name: "NVIDIA Tegra X1 (GM20B) (NVK GM20B)", ... }
...
<time>  INFO bevy_horizon::lifecycle: [Applet] focus handling: NoSuspend, so HOME sends FocusStateChanged; the app is suspended after its last frame
...
[Init] Step 6: Calling app.run()...
[Check] fs: PASS, stat and fstat say a2_check.txt is a ...-byte file, ...
[Check] romfs: PASS, read romfs:/hello_switch.txt (... bytes) and it matches
...
<time>  INFO bevy_horizon::input: [Input] HID configured: 1 player, standard controllers; reading player 1 and handheld
...
[Check] asset: PASS, loaded by the Update of frame ..., ... ms after the request; the text matches
...
[Engine] 60 frames updated
...
[Check] thread: PASS in frame 60, ...
...
<time>  INFO bevy_horizon::lifecycle: [Heartbeat] frame ..., ... s after the first frame, ... fps on average
...
[Input] (+) pressed: exiting
[Exit] app.run() returned Success
[Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
...
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

- **`[Exit] PASS`** means the [heap check](glossary.md#heap-check) passed: the homebrew loader can load the next NRO.
- **The `[Memory]` lines** show the memory you got. Started as a game on the tested Switch Lite, the malloc heap is about 3.1 GiB (3,132.7 MiB), less the NRO's own size: about 3,114 MiB for an NRO that links NVK, as hello_switch does. [platform-notes.md](platform-notes.md) explains the budget.
- **A `[Heartbeat]` line every 10 s** gives the frame count and the average frame rate, which for hello_switch is about 60 fps ([heartbeat lines](logging-and-crash-reports.md#heartbeat-lines)).
- **Some lines look alarming but are normal:**
  - Mesa's `WARNING: NVK is not a conformant Vulkan implementation, testing use only.`;
  - at exit, `[Exit] heap check, before releasing services: ... would fail the loader's read:` and the blocks listed under it. Services such as the sockets still hold that memory, and bevy_horizon releases them next; the check after that is the one that ends in `[Exit] PASS`;
  - in NROs without RomFS, such as breakout, `romfsMountSelf: ... this NRO has no RomFS, which is normal unless elf2nro was given --romfsdir`.
- **These are bad:**
  - `applet type 2` ([step 6](#6-start-it-with-a-games-memory));
  - `MISALIGNED` in the `[Boot] TLS template` line: Rust and libnx disagree on where thread-locals are;
  - a `[Check] ...: FAIL` line;
  - `[Exit] WARN: heap pages are still locked, so the loader would crash reading the next NRO (2001-0106); exiting through AM to HOME instead` ([the userAppExit hook](lifecycle-and-exit.md#the-userappexit-hook)).
- **If the log just stops,** look for `panic.txt`, `crash.log` or `abort.log` beside it, then in `sdmc:/atmosphere/crash_reports`.

## Emulators

Test on the console. Don't expect `hello_switch.nro` to run in an emulator:

- **Stock [Ryujinx](glossary.md#ryujinx) (Ryubing 1.3.3) can't run the Vulkan builds,** which are `hello_switch`, `breakout` and `render_check`. NVK hits emulator bugs at startup and in the first frames; [rendering.md](rendering.md#what-the-emulator-gets-wrong) lists them.
- **The project tests in a Ryujinx with local fixes** that aren't published.
- **`switch_captest` runs in stock Ryujinx,** since it doesn't use Vulkan, with the differences that [testing.md](testing.md#what-the-emulator-gets-wrong) lists.
- **Other emulators, Sudachi among them, are untested.**
- **Emulator speed says nothing about the console.** The project's Ryujinx draws on the PC's CPU through [lavapipe](glossary.md#lavapipe).

## Troubleshooting

| Symptom | Fix |
|---|---|
| `libvulkan_bh.a, which is missing`, or `error: build bevy-horizon-toolchain:local first (scripts/build_example.sh does)` | A step was skipped. Build the image, then NVK, then the example ([steps 2 to 4](#2-build-the-toolchain-image)). |
| A crate fails with `(signal: 9, SIGKILL: kill)`, or the PC runs out of memory | `rustc` ran out of memory. Cap cargo's jobs: `JOBS=4 scripts/build_example.sh hello_switch`, or `JOBS=2`. |
| `rust-lld: error: unable to find library -lgcc` | The image has a newer GCC than the 16.1.0 that the `build.rs` files name. List the image's GCC with `docker run --rm bevy-horizon-toolchain:local ls /opt/devkitpro/devkitA64/lib/gcc/aarch64-none-elf`, then change `16.1.0` to match in each file that `grep -l 16.1.0 crates/*/build.rs examples/*/build.rs` lists. |
| `error: patches/... is ..., but Cargo.lock still has crates.io's ..., so the patch isn't used.` | Cargo locked a newer crates.io version than a crate patch has, for example after `cargo update`. Restore the repository's `Cargo.lock`, or re-make the patch from the version the message names ([how to check that a patch is really applied](toolchain.md#how-to-check-that-a-patch-is-really-applied)). |
| A sphaira forwarder won't start the NRO: no `log.txt` appears, and the crash report lists only the loader | The NRO has no asset section. Build it with the scripts, which add one ([What the build leaves](#what-the-build-leaves)). |
| `log.txt` shows `applet type 2`, or the game runs out of memory | It started in applet mode. Start it with a game's memory ([step 6](#6-start-it-with-a-games-memory)). |

Other problems, from the build to the exit, are in [troubleshooting.md](troubleshooting.md).

## How the build works

This section is for when a step fails, or for building without the scripts. `scripts/build_example.sh` runs each step in a fresh container:

1. **It makes sure the image exists,** and builds it only if it's missing.
2. **It compiles the example** with `cargo build --release -Z build-std=core,alloc,std,panic_abort -Z json-target-spec --target /workspace/aarch64-nintendo-horizon.json --package <example>`. [build-std](glossary.md#build-std) compiles Rust's std from source for bevy-horizon's [target specification](glossary.md#target-specification). std is its own workspace with its own lock file, so the script first moves that lock onto `patches/libc`, and stops with `error: std would build against crates.io libc, not patches/libc` if that didn't work. Downloaded crates stay in the `bh-cargo-registry` volume between builds.
3. **It checks the [crate patches](glossary.md#crate-patch).** A `[patch.crates-io]` entry replaces only the crates.io versions it's semver-compatible with, and Cargo says nothing when a patch that is also a workspace member goes unused. The script compares the patches with `Cargo.lock` and stops on any that isn't used ([the crate patches](toolchain.md#the-crate-patches)).
4. **It packages the NRO** with the title, the author `bevy-horizon`, the version `0.1.0`, libnx's default icon, and `examples/<example>/romfs` if that folder exists.
5. **It checks the asset section,** and stops with `error: <example>.nro has no asset section (...); sphaira forwarders will refuse to load it` if it's missing.

Step 4 for hello_switch, as the container runs it:

```sh
OUT=target/aarch64-nintendo-horizon/release
nacptool --create "Hello Switch" "bevy-horizon" "0.1.0" "$OUT/hello_switch.nacp"
elf2nro "$OUT/hello_switch" hello_switch.nro \
    --nacp="$OUT/hello_switch.nacp" \
    --icon=/opt/devkitpro/libnx/default_icon.jpg \
    --romfsdir=examples/hello_switch/romfs
```

`scripts/build_game.sh` does the same for your own game, and takes `AUTHOR`, `VERSION`, `ICON` and `ROMFS` for the packaging step.

## Next steps

- **[Porting a Bevy game](porting-a-bevy-game.md):** add bevy_horizon to your game and build it with `scripts/build_game.sh`, for example `scripts/build_game.sh ../my-game my-game my-game "My Game"`. The NRO lands in the game's `target/aarch64-nintendo-horizon/release/`, and the script moves the game's `Cargo.lock` onto the patched crates' versions. Its [step 4](porting-a-bevy-game.md#4-match-the-patch-set-to-your-bevy-version-and-build) matches the patch set to your Bevy version.
- **[Logging and crash reports](logging-and-crash-reports.md):** everything in `log.txt`, the crash files, and turning a crash address into a line of code.
- **[Testing](testing.md):** captest, render_check, and testing in an emulator.
