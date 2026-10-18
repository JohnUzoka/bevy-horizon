# The toolchain: how Rust code becomes a Switch program

This page is the reference for everything between `cargo build` and an [NRO](glossary.md#nro): the target specification, nightly Rust, the C libraries, the NVK build and the crate patches. Read it when a dependency won't build for [Horizon](glossary.md#horizon), when a link or patch error isn't in [troubleshooting.md](troubleshooting.md), or when you work on bevy-horizon itself. The build commands are in [building.md](building.md), and the steps for porting a game in [porting-a-bevy-game.md](porting-a-bevy-game.md).

## In short

- **Target:** `aarch64-nintendo-horizon.json` is a custom [target specification](glossary.md#target-specification) for Switch [homebrew](glossary.md#homebrew). It links with `rust-lld` and aborts on a [panic](glossary.md#panic).
- **Nightly only.** The toolchain image pins `nightly-2026-10-04`, and every build compiles std from source with [build-std](glossary.md#build-std).
- **C libraries:** `bevy_horizon`'s build script links [libnx](glossary.md#libnx), [newlib](glossary.md#newlib) and libgcc from [devkitA64](glossary.md#devkita64), so your game needs no build script for them.
- **GPU driver:** the `vulkan` feature links [NVK](glossary.md#nvk) into the NRO. Build it once, in this order: the toolchain image, `scripts/build_nvk.sh`, `scripts/localize_nvk.sh`.
- **[Crate patches](glossary.md#crate-patch):** 17 for Bevy 0.19 and 13 for Bevy 0.18, libc included. `scripts/build_game.sh` passes them to Cargo, so your game's `Cargo.toml` needs no `[patch]`.
- **A patch is used only if `Cargo.lock` locks its exact version.** Otherwise Cargo only warns, so both build scripts check and stop.
- **Build in release mode,** as both build scripts do.
- **After any change to `patches/libc`,** run `scripts/check_libc_abi.sh`.

## The target specification

Rust has no built-in Switch target, so `aarch64-nintendo-horizon.json`, at the root of bevy-horizon, describes one. Cargo reads it through `--target` with `-Z json-target-spec`.

| Field | Value | Why |
|---|---|---|
| `arch`, `llvm-target` | `aarch64`, `aarch64-unknown-none` | The Switch's Cortex-A57. LLVM emits freestanding code; libnx supplies the OS layer. |
| `os`, `env`, `vendor`, `target-family` | `horizon`, `newlib`, `nintendo`, `unix` | Crates take their `cfg(target_os = "horizon")` and Unix paths. |
| `features` | `+v8a,+neon,+crypto,+crc` | The CPU features C code also gets, from `.cargo/config.toml`. |
| `panic-strategy` | `abort` | No unwinding: a panic writes `panic.txt` and ends the app. |
| `linker`, `linker-flavor` | `rust-lld`, `gnu-lld` | LLD does the final link, not GCC. |
| `pre-link-args` | `--wrap=threadCreate`, `--wrap=fchmod`, `--wrap=rename`, `--wrap=rmdir` | Send these calls through `bevy_horizon` ([New threads](#new-threads), [File calls](#file-calls)). |
| `has-thread-local`, `tls-model` | `true`, `local-exec` | [Thread-locals](glossary.md#thread-local-storage) read [TPIDR_EL0](glossary.md#tpidr_el0) directly. |
| `position-independent-executables`, `relocation-model`, `dynamic-linking` | `true`, `pic`, `true` | The [loader](glossary.md#loader) puts the NRO at any address, and libnx's startup applies the relocations. |
| `frame-pointer` | `always` | The CPU profiler and crash backtraces walk the frame records. |

The [ELF](glossary.md#elf) linker script is in the JSON, as `link-script`. It puts libnx's [crt0](glossary.md#crt0) first in `.text`, gives libnx's startup the symbols it reads (`__tdata_lma`, `__tls_align`, `__argdata__` and the main thread's `.main.tls`), and aligns the thread-local segment so that rust-lld and libnx agree on where each thread-local is. If they wouldn't, the link stops with `TLS segment is not aligned: thread-locals would be read at the wrong offset`, and `log.txt` checks again at every launch ([below](#thread-locals-on-the-main-thread-before-main)).

## Nightly Rust and build-std

Rust ships no std for this target, so every build compiles `core`, `alloc` and `std` from source with `-Z build-std`, which needs nightly Rust. The `Containerfile` installs `nightly-2026-10-04` (rustc `1.101.0-nightly`, `db8f076d2`) with `rust-src` and `llvm-tools-preview`, and renames it to plain `nightly`: build-std writes the sysroot's path into debug info, and a fixed name keeps rebuilds identical. `--build-arg RUST_NIGHTLY=nightly` builds the image with the latest nightly instead. The build scripts run Cargo like this, here for [hello_switch](glossary.md#hello_switch):

```sh
cargo build --release -Z build-std=core,alloc,std,panic_abort -Z json-target-spec \
    --target /workspace/aarch64-nintendo-horizon.json \
    --package hello_switch
```

**std has its own lock file.** build-std builds std as a separate workspace with its own `Cargo.lock`. A `[patch]` in a `Cargo.toml` never reaches it, but one in a Cargo config file does, so the libc patch is in `.cargo/config.toml`, and both build scripts run `cargo update -p libc` in std's workspace before building. If std would still use crates.io's libc, they stop with `error: std would build against crates.io libc, not patches/libc`. Without the patch, std gets crates.io libc's Horizon layouts, which are the 3DS's: the build succeeds, and file metadata comes back wrong on the console.

## Linking to devkitA64, libnx, and newlib

Startup, files, sockets, controllers, `malloc` and the system services come from libnx and newlib. `crates/bevy_horizon/build.rs` (`main`) links them whenever the target OS is Horizon: it adds these folders from the toolchain image and links `nx`, `sysbase`, `c`, `m` and `gcc`. The `pic` folders hold the position-independent newlib and libgcc.

```text
/opt/devkitpro/libnx/lib
/opt/devkitpro/portlibs/switch/lib
/opt/devkitpro/devkitA64/aarch64-none-elf/lib/pic
/opt/devkitpro/devkitA64/lib/gcc/aarch64-none-elf/16.1.0/pic
```

The image starts from [devkitPro](glossary.md#devkitpro)'s `devkitpro/devkita64` and installs the current packages with `dkp-pacman`, unpinned. Today that gives devkitA64 r30-1 (GCC 16.1.0), libnx 4.12.0, switch-sdl2 2.28.5, switch-tools 1.13.1 and [switch-mesa](glossary.md#switch-mesa) 20.1.0, the [OpenGL ES](glossary.md#opengl-es) driver that [captest](glossary.md#captest)'s GLES probe uses.

**`-z notext`.** The examples' build scripts (`examples/*/build.rs`) repeat these folders and libraries and add `-z notext`, which lets the link keep relocations in read-only code. `bevy_horizon` doesn't, and [Skate 3 Rust Engine](https://github.com/JohnUzoka/skate-3-rust-engine), a full 3D game, links without it. Add it to your game's `build.rs`, for Horizon only, as `examples/hello_switch/build.rs` does, if rust-lld stops on a dynamic relocation in a read-only segment.

**C code in dependencies.** Crates that build C through the `cc` crate get devkitA64's GCC and these flags from `.cargo/config.toml`:

```toml
[env]
CC_aarch64_nintendo_horizon = "aarch64-none-elf-gcc"
CFLAGS_aarch64_nintendo_horizon = "-march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE"
```

With `-mtp=soft`, C code reads the thread pointer through libnx, while Rust's thread-locals read `TPIDR_EL0`. `bevy_horizon` keeps the two equal on every thread ([Internals](#internals-startup-exit-and-thread-creation)).

### Linking NVK (the `vulkan` feature)

Horizon has no dynamic loader, so the [Vulkan](glossary.md#vulkan) driver is linked into the NRO. With `bevy_horizon`'s `vulkan` feature, `crates/bevy_horizon/build.rs` (`link_nvk`) links `libvulkan_bh.a`, plus devkitPro's `expat`, [`zstd`](glossary.md#zstd), `z` and `stdc++`. It takes the library from the bevy-horizon checkout's `nvk/lib/`, or from the directory `BEVY_HORIZON_NVK_LIB` names. That variable is read at build time, and only when you run Cargo yourself: `build_example.sh` and `build_game.sh` don't pass it into their containers. If the library is missing, the build stops with:

```text
bevy_horizon's `vulkan` feature links <dir>/libvulkan_bh.a, which is missing: run scripts/build_nvk.sh and scripts/localize_nvk.sh, or set BEVY_HORIZON_NVK_LIB to the directory that has it
```

Without the feature, `crates/bevy_horizon/src/compat.rs` defines a `vkGetInstanceProcAddr` that returns null, so an app with bevy_render still links. [wgpu](glossary.md#wgpu) then reports `no Vulkan driver is linked into this executable`, and Bevy's renderer panics with `Unable to find a GPU! Make sure you have installed required drivers!`.

#### Building NVK

NVK, [Mesa](glossary.md#mesa)'s Vulkan driver, comes from [mesa-switch](glossary.md#mesa-switch) ([danfromtico/mesa-switch](https://github.com/danfromtico/mesa-switch)), danfromtico's and NaGaa95's port of Mesa to Horizon. bevy-horizon builds it from mesa-switch commit `606441c` (Mesa 26.2.4) plus the patches in `patches/mesa-switch/`. Build the toolchain image first, with `docker build -t bevy-horizon-toolchain:local -f Containerfile .` in bevy-horizon or by running `scripts/build_example.sh`. Then check out mesa-switch next to bevy-horizon and run the two scripts:

```sh
git clone https://github.com/danfromtico/mesa-switch
git -C mesa-switch checkout 606441c
cd bevy-horizon
scripts/build_nvk.sh
scripts/localize_nvk.sh
```

**`[JOBS=n] scripts/build_nvk.sh [mesa-switch checkout]`** uses `../mesa-switch` and 4 jobs by default. It builds `bevy-horizon-nvk:local`, the toolchain image plus Mesa's build tools, so Mesa is compiled by the same devkitA64 and libnx as the NRO. It applies `patches/mesa-switch/*.patch` to the checkout in order, without committing them, skipping any already applied and stopping at one that doesn't apply. Then it builds only the Vulkan driver, for the Switch's GM20B GPU, with the [shader cache](glossary.md#shader-cache) on; the image's nightly builds [NAK](glossary.md#nak), NVK's shader compiler, which is Rust. A rerun is incremental. The script leaves `nvk/lib/libvulkan.a`, the Vulkan headers in `nvk/include/vulkan`, and `nvk/BUILD_INFO`, which records the mesa-switch commit, the patches applied, the checkout's uncommitted changes and the toolchain versions.

**`scripts/localize_nvk.sh`** turns that into `nvk/lib/libvulkan_bh.a`, one relocatable object whose only global symbols are the `vk*` entry points and the ones `bevy_horizon` calls: `nouveau_horizon_region_alloc`, `nouveau_horizon_region_free` and `nouveau_horizon_region_stats` (patch 0001), for the memory report and the `top_down_alloc` allocator, and `nouveau_horizon_tune` (patch 0008), for `bevy_horizon::nvk_tune`. Everything else becomes local: NAK and NIL are Rust static libraries, each with its own copy of a Linux std, and Mesa defines common C names, so left global they would clash with the game's std and with devkitPro's Mesa. The script lists the symbols the object still needs in `nvk/lib/undefined_bh.txt`. Without `libvulkan.a` it stops with `error: nvk/lib/libvulkan.a is missing; run scripts/build_nvk.sh first`.

**No COMDAT groups.** A linker keeps only the first copy of each COMDAT group. NAK's unwind tables use one, `DW.ref.rust_eh_personality`, and so does any game crate with `extern "C-unwind"` calls, even under `panic = abort` (mlua is one). If the game's copy came first, NVK's would be dropped and the link would fail with:

```text
rust-lld: error: relocation refers to a symbol in a discarded section: DW.ref.rust_eh_personality
```

`localize_nvk.sh` links with `ld -r --force-group-allocation`, which turns each group into a plain section private to NVK, and fails if a group survives. If you see this error, your `libvulkan_bh.a` was made without that step: run `scripts/localize_nvk.sh` again.

`nvk/` isn't in git, so every checkout builds its own. Run both scripts again when mesa-switch or `patches/mesa-switch/` changes.

#### The mesa-switch patches

The 24 patches are in `patches/mesa-switch/`, numbered in the order they apply; each one's header says what it changes and why. Lowercase names are `NAK_DEBUG` flags. NVK's reports go to stderr, which is `log.txt`.

- **GPU memory (0001):** NVK takes GPU memory from the top of the heap down ([platform-notes.md](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap)).
- **On by default in Switch builds** (0019 and 0021), each measured faster on the console:
  - `xbar` (0004): loads stay in flight across forward branches;
  - `xbrk` (0007): with `xbar`, also past the `BRK` that ends a texture lookup whose descriptor may differ across the warp;
  - `sink` and `remat` (0005): fewer live registers in fragment shaders;
  - `lsink` (0011): a fragment input used only inside a large loop is read in the loop;
  - `plat` (0015): the scheduler before register allocation waits out latencies, instead of putting each texture read just above its use;
  - `ofold` (0021): a storage access's constant offset is folded into the access.
- **Off by default:** `ubra` (0006), plain branches for an `if` that is uniform across the warp; `share` (0023), a load shares a scoreboard when all six are taken; `jthr` (0024), jump threading; and `wgbar` (0022), only for [Ryujinx](glossary.md#ryujinx) on [lavapipe](glossary.md#lavapipe).
- **Reports:** `stats` (0003), one line per compiled shader ([logging-and-crash-reports.md](logging-and-crash-reports.md)); `final` (0004), each shader's final code; whether the depth passes use [ZCULL](glossary.md#zcull) (0010); and which images NVK compresses (0013, [rendering.md](rendering.md#render-target-compression)).
- **Register cap (0014):** `NAK_GPR_LIMIT=n`, a multiple of 8 from 24 to 248, caps every shader; NAK ignores any other value and says so. An entry point whose name ends in `_gpr` and a number, such as `fragment_gpr48`, caps one shader.
- **Switches for `nvk_tune`:** early Z, its hysteresis, ZCULL and ZCULL's serialisation (0008); ZCULL's direction (0009); and skipping [indirect draws](glossary.md#indirect-draw) with no instances, which Bevy's GPU culling issues for every culled batch (0012).
- **Fixes:** GPU timestamps count the [Tegra X1](glossary.md#tegra-x1)'s 1.6276 ns tick (0002); NAK's unit tests build on a PC (0016); and pipeline-statistics queries, such as Bevy's per-pass statistics, count compute invocations correctly without stalling the GPU between compute passes (0017, 0018 and 0020).

**Setting NAK's options.** `NAK_DEBUG` takes a comma-separated list: `name` turns a flag on and `-name` turns it off, so `NAK_DEBUG=stats,-plat` logs shader statistics and turns `plat` off. NAK prints the flags in effect. It reads `NAK_DEBUG` and `NAK_GPR_LIMIT` once, and both are part of the shader cache's key, so changing them compiles the shaders again. Set them with `std::env::set_var` at the start of `main`, before you build the App.

**`bevy_horizon::nvk_tune`** (Horizon with `vulkan` only) has `set(Knob, i32) -> bool` for the switches: `EarlyZHysteresis`, `Zcull`, `ZcullSerialization`, `EarlyZ`, `ZcullDir` and `SkipEmptyDraws`. -1 is NVK's own behaviour, and `set` returns `false` and changes nothing for a value a switch can't take. They are for measuring, and `EarlyZ` draws a wrong picture. One is set by default: `HorizonPlugin` calls `nvk_tune::configure_from_env`, which sets ZCULL's direction to `GREATER` to suit Bevy's reversed depth (with NVK's `LESS`, ZCULL rejects nothing), unless `BEVY_HORIZON_ZCULL_DIR=less`, and logs `[NVK] ZCULL direction: greater (…)`.

## The crate patches

Stock crates assume a desktop, mobile or web OS. Horizon is close enough to Unix that most of Bevy's dependency tree compiles, but some crates need a crate patch. Each patch in `patches/` is the crates.io source of one release plus the changes for Horizon; `diff -r` against that release shows them. A patch replaces only the crates.io versions in its semver family, and only when `Cargo.lock` locks its exact version ([How to check that a patch is really applied](#how-to-check-that-a-patch-is-really-applied)).

| File | Used by | What it patches |
|---|---|---|
| `.cargo/config.toml` | Every build | `libc`. A Cargo config file also reaches std's own workspace ([Nightly Rust and build-std](#nightly-rust-and-build-std)). |
| `.cargo/bevy-0.19.toml` | A game on Bevy 0.19.1 | [The Bevy 0.19 set](#the-bevy-019-set). |
| `.cargo/bevy-0.18.toml` | A game on Bevy 0.18.1 | [The Bevy 0.18 set](#the-bevy-018-set). |
| `Cargo.toml` (`[patch.crates-io]`) | This workspace: `bevy_horizon` and the examples | The 0.19 set without `inventory` (the examples leave `reflect_auto_register` off), plus `bevy_tasks_018` for building `bevy_horizon` with `bevy_0_18`. |

`scripts/build_game.sh` passes `--config .cargo/config.toml --config .cargo/bevy-$BEVY.toml` to Cargo, so your game's `Cargo.toml` needs no `[patch]` and its desktop builds don't change. `BEVY` defaults to the version of `bevy_app` in the game's `Cargo.lock`; [porting-a-bevy-game.md](porting-a-bevy-game.md#4-match-the-patch-set-to-your-bevy-version-and-build) has the steps. A patch for a crate your game doesn't build only makes Cargo warn that it `was not used in the crate graph`.

### The Bevy 0.19 set

`.cargo/bevy-0.19.toml`, plus `libc` from `.cargo/config.toml`. Each patch is in `patches/<crate>`, except getrandom 0.2's, in `patches/getrandom-0.2`.

| Crate | Version | What it changes |
|---|---|---|
| `bevy_asset` | 0.19.1 | Reads and writes files synchronously, even with `multi_threaded`, whose async reads take an 8 MiB pipe per file and up to 500 threads. Uses device paths ([`sdmc:/`](glossary.md#sdmc), `romfs:/`) as they are, and doesn't build `async-io`. |
| `bevy_tasks` | 0.19.1 | Sizes the [task pools](glossary.md#task-pool) from the process's core mask, and lets `bevy_horizon` shut them down at exit ([below](#what-the-larger-patches-change)). |
| [`cpal`](glossary.md#cpal) | 0.17.3 | Adds a Horizon audio host on libnx's [audio renderer](glossary.md#audren) ([input-and-audio.md](input-and-audio.md#the-audio-host)). |
| [`getrandom`](glossary.md#getrandom) | 0.4.3 | Gets random bytes from newlib's `arc4random_buf`, which `bevy_horizon` backs with libnx's `randomGet`. |
| `getrandom` (key `getrandom02`) | 0.2.17 | Uses newlib's `getrandom()` on any Horizon target, not only the 3DS. [ring](glossary.md#ring) 0.17 uses getrandom 0.2. |
| `inventory` | 0.3.24 | Puts constructors in `.init_array`, which libnx's startup runs, so Bevy's `reflect_auto_register`, a default feature, works. |
| `libc` | 0.2.190 | devkitA64 newlib's types, struct layouts and constants for AArch64 Horizon, sockets included. std uses it too. |
| `lua-src` | 551.0.2 | Builds [Lua](glossary.md#lua) as plain ISO C, for games that embed it through `mlua`: no `dlopen`, and `io.popen` reports "not supported". |
| `memmap2` | 0.9.11 | Holds maps in the heap, since Horizon has no `mmap` ([below](#what-the-larger-patches-change)). |
| `polling` | 3.10.0 | Uses its plain-pipe fallback, since rustix has no `pipe_with` on Horizon. |
| `raw-window-handle` | 0.6.2 | Adds `HorizonWindowHandle`, which holds the [NWindow](glossary.md#nwindow) pointer that `bevy_horizon` fills in, and an empty `HorizonDisplayHandle`. |
| `ring` | 0.17.14 | Builds ring's AArch64 assembly for Horizon with the `linux64` [perlasm](glossary.md#perlasm) flavour. |
| `rustix` | 1.1.5 | Uses devkitA64's types where upstream has the 3DS's: `ClockId` is `u64`, poll flags `c_short`, `sun_family` `u8`, ioctl opcodes `c_int`. |
| `wgpu` | 29.0.4 | Builds the Vulkan backend for Horizon; a DRM surface target returns an error. |
| `wgpu-core` | 29.0.4 | Adds Horizon to the Vulkan platforms, keeps [indirect-call validation](rendering.md#indirect-call-validation) only with `VALIDATION`, and logs which call [lost the device](glossary.md#device-lost). |
| `wgpu-core-deps-windows-linux-android` | 29.0.4 | Adds Horizon to the targets that pull in wgpu-hal's Vulkan backend. |
| `wgpu-hal` | 29.0.4 | Calls the linked driver directly, makes the Switch's surface, and sizes memory blocks and buffer checks for the Switch ([below](#what-the-larger-patches-change)). |

### The Bevy 0.18 set

`.cargo/bevy-0.18.toml`, plus `libc` from `.cargo/config.toml`. Seven patches are made from Bevy 0.18.1's versions and live in `patches/bevy-0.18/<crate>`. Each makes the same changes as its 0.19 counterpart, plus:

| Crate | Version | Plus |
|---|---|---|
| `bevy_asset`, `bevy_tasks` | 0.18.1 | Nothing. |
| `cpal` | 0.15.3 | Nothing: this is the cpal that Bevy 0.18 reaches through [rodio](glossary.md#rodio) 0.20.1. |
| `wgpu` | 27.0.1 | An arm that refuses the `Drm` surface target, which wgpu 27 has on every Unix. |
| `wgpu-core` | 27.0.3 | Nothing. |
| `wgpu-core-deps-windows-linux-android` | 27.0.0 | Nothing. |
| `wgpu-hal` | 27.0.4 | gpu-alloc's chunk sizes, and a log of large GPU allocations ([below](#what-the-larger-patches-change)). |

The other six are the 0.19 set's: `getrandom` 0.4.3, `inventory` 0.3.24, `libc` 0.2.190, `lua-src` 551.0.2, `memmap2` 0.9.11 and `raw-window-handle` 0.6.2. glTF scenes need `inventory`, because Bevy 0.18 spawns them by reflection.

The 0.18 file leaves out `getrandom` 0.2, `polling`, `ring` and `rustix`, which Skate 3 Rust Engine doesn't build for the Switch. If your game builds one (the dependency audit in [porting-a-bevy-game.md](porting-a-bevy-game.md) shows how to find out), copy the file to `.cargo/bevy-<name>.toml` in bevy-horizon, add the missing entries from `.cargo/bevy-0.19.toml`, and build with `BEVY=<name>`: `build_game.sh` uses `BEVY` only to pick the file.

### What the larger patches change

**`bevy_tasks`.** Bevy's global task pools are never dropped ([bevyengine/bevy#9477](https://github.com/bevyengine/bevy/issues/9477)), so their threads' stacks would keep heap pages locked at exit. The patch adds `TaskPool::shut_down` and `shut_down_global_task_pools`, which `bevy_horizon` calls at exit. std's `available_parallelism` is `Unsupported` on Horizon, so the patch counts the cores in the process's [core mask](glossary.md#core-mask): homebrew on [Atmosphère](glossary.md#atmosphère) gets cores 0–3, which gives 2 compute threads, 1 IO and 1 async compute.

**`memmap2`** (`patches/memmap2/src/horizon.rs`). `map` and `map_copy` read the file's range into the heap when the map is made, so later changes to the file don't show, and bytes past its end read as zero. `map_anon` is zeroed heap memory. `map_mut`, `map_exec` and `make_mut` on a file return `Unsupported`, and a map too big for the heap returns `OutOfMemory`. There is no `pread`, so don't use the same `File` from another thread while a map is being made.

**`wgpu-hal`:**

- **Driver and surface:** calls the linked driver's `vkGetInstanceProcAddr` directly, through ash without its `loaded` feature. Makes surfaces with `VK_NN_vi_surface`, and keeps adapters that report conformance version 0, as NVK does on Tegra.
- **Messages:** registers a debug messenger even in release builds, so NVK's messages reach `log.txt`, and names the result behind an "unexpected" error: `Vulkan returned …, which wgpu reports as an unexpected driver error`.
- **Memory:** `MemoryHints::Performance` gets blocks of 8–64 MiB for device memory and 4–32 MiB for host-visible memory, instead of 128–256 MiB and 64–128 MiB, because GPU memory comes from the game's heap ([platform-notes.md](platform-notes.md#gpu-memory-comes-from-the-same-heap)).
- **Buffer checks:** by default, asks for neither `robustBufferAccess` nor `robustBufferAccess2`, and has [naga](glossary.md#naga) clamp buffer indices in every shader instead. `BH_ROBUST_BUFFER_ACCESS=driver` (or `1`) gives the driver's checks, and `off` (or `0`) neither ([rendering.md](rendering.md#buffer-robustness)).
- **Compression:** `BH_RT_COMPRESSION=1` gives large render targets memory of their own, so NVK can compress them ([rendering.md](rendering.md#render-target-compression)).
- **Swapchain:** waits as long as it takes for the window's next image, and logs a long wait: `wgpu-hal: waited … s for the window's next image, …`.
- **wgpu 27 only:** gpu-alloc's chunks are 8–64 MiB instead of 128–512 MiB. A tally under the log target `bevy_horizon::gpu` writes `[GPU] … MiB for …` for each resource of 4 MiB or more, `[GPU] a new high for wgpu's buffers and textures: …` each time the total passes the last high by 64 MiB, and `Vulkan could not allocate … MiB of memory type …` when an allocation fails.

## How to check that a patch is really applied

Cargo uses a `[patch]` only if the patch's version is the one `Cargo.lock` locks. A `bevy_asset` 0.19.0 patch does nothing when the lock has 0.19.1: Cargo builds the crates.io crate and only warns that the patch `was not used in the crate graph`. The build succeeds, and the failure shows up on the console.

- **`scripts/build_example.sh`** checks this workspace after every build. For each entry in `Cargo.toml`'s `[patch.crates-io]`, it looks in `Cargo.lock` for a crates.io package of the same name in the patch's semver family.
- **`scripts/build_game.sh`** first moves the game's lock onto the patches: it runs `cargo update -p <name>@<locked> --precise <patch version>` for each locked crates.io version in a patch's semver family, which changes the game's `Cargo.lock` in place. Then it checks the same way.

Either script stops with an error like this:

```text
error: patches/bevy_asset is bevy_asset 0.19.0, but Cargo.lock still has crates.io's bevy_asset 0.19.1, so the patch isn't used. Re-make it from bevy_asset 0.19.1.
```

**By hand:** find the crate in `Cargo.lock`. A patched crate's `[[package]]` entry has no `source` line; an entry in the same semver family with `source = "registry+https://github.com/rust-lang/crates.io-index"` comes from crates.io. std's libc is checked separately ([Nightly Rust and build-std](#nightly-rust-and-build-std)).

**Re-making a patch.** When a dependency needs a newer release than a patch has, take that release's crates.io source and make the same changes to it. `diff -r` between the patch and the release it was made from shows them.

## The libc ABI probe

Rust and C can compile and link while disagreeing about a struct's layout; the failure shows up later, as wrong file sizes or types, broken timeouts or bad socket addresses. `scripts/check_libc_abi.sh` checks the [ABI](glossary.md#abi) that `patches/libc` gives Rust against devkitA64's headers. Run it after every change to `patches/libc`:

```sh
./scripts/check_libc_abi.sh
```

In the toolchain image, a C probe built by devkitA64's GCC and a `no_std` Rust probe built against `patches/libc` emit the same table: type sizes and alignments, struct sizes and field offsets, and constants (errno, open, file, clock, pthread, sysconf, socket, IP, TCP, `MSG_*`, `POLL*`, `EAI_*`, `AI_*`, `NI_*`, `FIONBIO` and `FIONREAD`). The script compares them, and exits with status 1 on any mismatch. The current result:

```text
348 items compared: 0 mismatches, 3 documented missing-in-C constants, 9 opaque pthread sizes or alignments bigger in Rust (safe).
```

The three constants libnx doesn't define are `MSG_MORE`, `IPV6_ADD_MEMBERSHIP` and `IPV6_DROP_MEMBERSHIP`; it has `IPV6_JOIN_GROUP` and `IPV6_LEAVE_GROUP` instead.

## Build profiles

Build for the Switch in release mode; both build scripts pass `--release`. A debug Bevy build is too slow and too large to tell you anything. bevy-horizon sets no `[profile.release]`, so builds use Cargo's release defaults and each crate's own settings. LTO, `codegen-units` and the like are your game's choice: set them in its own `Cargo.toml`, and measure on the console. `build_game.sh` adds anything in `CARGO_ARGS` to the `cargo build` line.

If your game turns Bevy's default features off, keep `tonemapping_luts`, `ktx2` and `zstd_rust`, which `Camera3d`'s default tonemapping needs. Without `tonemapping_luts`, everything the camera draws is magenta ([troubleshooting.md](troubleshooting.md#everything-a-3d-camera-draws-is-magenta)); without `zstd_rust` (or `zstd_c`), the build stops with `Choosing a zstd backend is required for zstd support`. hello_switch's `Cargo.toml` is a minimal feature list for a 3D game, and [porting-a-bevy-game.md](porting-a-bevy-game.md) lists `bevy_horizon`'s own features.

## What the toolchain cannot do yet

- **Nightly only.** A nightly newer than the pinned one may break the build.
- **wgpu prints two future-incompatibility warnings,** `overflow evaluating the requirement`, when `patches/wgpu-core` and `patches/wgpu` build ([rust-lang/rust#159228](https://github.com/rust-lang/rust/issues/159228)). They show only because the crates are patched, and are harmless. If a later nightly makes them errors, add `#![recursion_limit = "256"]` to each crate's `lib.rs`. A game crate that derives Bevy's `AsBindGroup` can hit the same limit; [render_check](glossary.md#render_check)'s `main.rs` raises it this way.
- **GCC's library folder is hard-coded.** devkitPro's packages aren't pinned, but `crates/bevy_horizon/build.rs` and the examples' build scripts name GCC 16.1.0's folder. If an image rebuild brings a new GCC, change that path.
- **NVK is built locally.** There is no prebuilt `libvulkan_bh.a`.
- **Each patch set is tied to exact releases:** Bevy 0.19.1 with wgpu 29.0.4 and cpal 0.17.3, and Bevy 0.18.1 with wgpu 27 and cpal 0.15.3. A dependency that needs a newer release of a patched crate means re-making that patch.
- **No unwinding.** `catch_unwind` can't recover from a panic, and a panic on any thread ends the app.
- **The Bevy 0.19 audio host has passed only in the [emulator](glossary.md#emulator).** The 0.18 host, the same code on cpal 0.15.3, works on the console.
- **No game has used networking on the console yet.** The socket layouts match, and captest's socket checks run on the console; [platform-notes.md](platform-notes.md) has the socket limits.
- **`fs::rename` onto an existing file isn't atomic** ([File calls](#file-calls)), and **memmap2's maps are copies in the heap,** so a map costs its full size in memory.
- **Stock Ryujinx can't run the Vulkan builds:** NVK hits emulator bugs at startup. [testing.md](testing.md) says what runs where.

## Internals: startup, exit, and thread creation

`bevy_horizon` hooks code that runs before `main`, after it, and on threads `main` never sees. The hooks are in `crates/bevy_horizon/src/compat.rs` (module `horizon_stubs`), and you don't call any of them. [platform-notes.md](platform-notes.md) explains the Horizon behaviour behind each, and [logging-and-crash-reports.md](logging-and-crash-reports.md) the `[Boot]`, `[Thread]` and `[Exit]` lines they write.

### Thread-locals on the main thread, before `main`

libnx keeps each thread's thread pointer in its `ThreadVars`, as `tls_tp`, which is where C code built with `-mtp=soft` reads it; Rust's thread-locals read `TPIDR_EL0`. libnx calls the weak [userAppInit](glossary.md#userappinit) hook once its services and the SD card are up, before C constructors and Rust's runtime setup. `horizon_stubs::userAppInit` calls `set_thread_pointer`, which copies `tls_tp` (at offset 0x1f8 of the TLS area that `TPIDRRO_EL0` points to) into `TPIDR_EL0`. Then `start_boot_log` (`crates/bevy_horizon/src/logging.rs`) opens `log.txt`, so a crash before `main` still leaves a log, and checks the layout. Its `[Boot] TLS template …` line should end `Rust and libnx agree on thread-local addresses`; `MISALIGNED: Rust and libnx put thread-locals at different addresses` means thread-locals will be read at the wrong offset ([The target specification](#the-target-specification)).

### New threads

Every thread goes through libnx's `threadCreate`: std's `thread::spawn` and Bevy's task pools through newlib's `pthread_create`, and C libraries' threads directly. `--wrap=threadCreate` sends each call to `horizon_stubs::__wrap_threadCreate`. It closes detached threads that have exited (`close_detached_threads`), picks the thread's cores by the policy set with `bevy_horizon::set_thread_core_policy`, and starts the thread in `thread_start`, which calls `set_thread_pointer` before anything can touch a `thread_local!`, then the real entry point. A thread made with `svcCreateThread` directly skips all this, and its first `thread_local!` access faults.

Each thread logs two `[Thread] #N:` lines. The second should end `TPIDR_EL0 = libnx tls_tp = 0x…, so thread_local! works`; `FAILED with …` in the first or `MISMATCH, thread_local! will fault` in the second is a failure. `bevy_horizon::set_thread_logging(false)` turns the lines off, but failures are logged anyway. hello_switch's `thread_check` (`examples/hello_switch/src/checks.rs`) tests the wrapper.

### File calls

libnx's SD-card driver (fsdev, reached through newlib's libsysbase) differs from Unix in three ways that std relies on. A `--wrap` in the target specification fixes each, so only calls that reach newlib change. captest's file checks (`examples/switch_captest/src/files.rs`) cover all three.

| Wrapper | Without it | With it |
|---|---|---|
| `__wrap_fchmod` | fsdev fails `fchmod` with `ENOSYS`, so std's `fs::copy` leaves an empty file and fails with "Function not implemented". | Returns success: Horizon's file systems have no permission bits. |
| `__wrap_rename` | Horizon won't rename onto an existing file (`EEXIST`), so saving through a temporary file and a rename fails after the first save. | When both paths are files, moves the old file aside to `<new>.bh-replaced`, renames, then deletes it, or moves it back if the rename failed. Not atomic: if the app dies in between, the old contents are in `<new>.bh-replaced`. |
| `__wrap_rmdir` | fsdev reports most errors as `EIO`, so removing a folder that isn't empty fails with "I/O error". | Returns `ENOTEMPTY` when `fsdevGetLastResult` reports the [result code](glossary.md#result-code) `DirectoryNotEmpty` (module 2, description 8), so std reports `ErrorKind::DirectoryNotEmpty`. |

### Detached threads and other C symbols

`horizon_stubs` also defines C symbols that newlib or std expect and libnx leaves out:

- **`__syscall_thread_detach`:** libnx has no back end for `pthread_detach`, so a finished detached thread would keep its kernel thread and its stack (2 MiB for a std thread), with the heap pages under it locked until the process ends. The stub records the thread, up to 256 at a time, and `close_detached_threads` closes it once it has exited: whenever a thread is created or detached, once a second, and at exit.
- **`sysconf`:** `_SC_PAGESIZE` is 0x1000, and `_SC_NPROCESSORS_CONF` and `_SC_NPROCESSORS_ONLN` are 3, the cores an application runs on.
- **`getrandom` and `_getentropy_r`,** on libnx's `randomGet`, for std and the getrandom patches; **`_init` and `_fini`,** empty.
- **`__nx_exception_stack`** (64 KiB), **`__libnx_exception_handler`**, which writes `crash.log`, and **`diagAbortWithResult`**, which writes `abort.log`.

### Exit

libnx calls [userAppExit](glossary.md#userappexit) whenever the NRO ends itself: `main` returning, `std::process::exit`, or an abort, but not when the app is closed from HOME. `horizon_stubs::userAppExit` calls `release_horizon_services` (`crates/bevy_horizon/src/lifecycle.rs`). It stops Bevy's task pools, runs the hooks registered with `bevy_horizon::register_release_hook`, and closes detached threads, the socket service, [RomFS](glossary.md#romfs) and nvdrv, so that nothing locks the [heap](glossary.md#heap) when the loader reads the next NRO into it. Then it logs the [heap check](glossary.md#heap-check): `[Exit] PASS: nothing in the heap is locked, …` or `[Exit] WARN: heap pages are still locked, …`. [lifecycle-and-exit.md](lifecycle-and-exit.md#the-userappexit-hook) explains each step, and what happens after a `WARN`.
