# The toolchain: how Rust code becomes a Switch program

This page explains what the build script does after you run it: how Rust code is compiled for [Horizon](glossary.md#horizon), how it links to [libnx](glossary.md#libnx) and [newlib](glossary.md#newlib), why crate patches are needed, and what this toolchain cannot do yet. For commands, see [building.md](building.md).

## In short

- **Target:** `aarch64-nintendo-horizon.json` describes AArch64 Nintendo Switch [homebrew](glossary.md#homebrew): `os = "horizon"`, `env = "newlib"`, `panic-strategy = "abort"`, `linker = "rust-lld"`, [thread-local storage](glossary.md#thread-local-storage) enabled, and `tls-model = "local-exec"`; see `aarch64-nintendo-horizon.json:2-39`.
- **Rust:** the image uses nightly Rust `1.101.0-nightly (db8f076d2 2026-10-03)`. Builds use `-Z build-std=core,alloc,std,panic_abort` because this is not a built-in Rust target; see `scripts/build_example.sh:29-31`.
- **C side:** the current image has devkitA64 `r30-1`, GCC `16.1.0`, libnx `4.12.0-1`, `switch-mesa` `20.1.0-5`, `switch-sdl2` `2.28.5-4`, and `switch-tools` `1.13.1-1`.
- **Linking:** `crates/bevy_horizon/build.rs` adds libnx, portlibs, newlib `pic`, GCC `pic`, and links `nx`, `sysbase`, `c`, `m`, and `gcc`; see `crates/bevy_horizon/build.rs:3-18`. With the `vulkan` feature it also links NVK; see [Linking NVK](#linking-nvk-the-vulkan-feature). The example adds `-z notext`; see `examples/hello_switch/build.rs:13-14`.
- **Thread and file fixes:** the target spec links with four `--wrap` flags (`aarch64-nintendo-horizon.json:26-33`). `--wrap=threadCreate` sets [TPIDR_EL0](glossary.md#tpidr_el0) on every new thread before Rust touches a `thread_local!` (`crates/bevy_horizon/src/compat.rs:286-405`). `--wrap=fchmod`, `--wrap=rename` and `--wrap=rmdir` make std's `fs::copy`, `fs::rename` and `fs::remove_dir` behave as they do on Unix (`compat.rs:156-284`, [File calls](#file-calls)).
- **Patch rule:** every [crate patch](glossary.md#crate-patch) must match the version in `Cargo.lock`. Cargo can otherwise keep the registry crate and say nothing useful; `scripts/build_example.sh:33-58` now stops that build.
- **ABI check:** `scripts/check_libc_abi.sh` compares 348 C/Rust layout items, networking included. Current result: 0 mismatches, 3 constants libnx doesn't have (documented as harmless), and 9 opaque pthread sizes or alignments bigger in Rust (safe).
- **Rendering:** wgpu, wgpu-core, wgpu-hal and raw-window-handle are patched so Bevy draws through wgpu's Vulkan backend, with NVK (mesa-switch's Vulkan driver) linked into the NRO. That path runs in the Ryujinx harness; it has not run on hardware yet. See [rendering.md](rendering.md).

## The target specification

**Why this matters:** Rust has to know the object format, linker, panic style, CPU features, TLS model, and startup layout before `std` or your game can be compiled.

The file is `aarch64-nintendo-horizon.json`.

| Field | Value | Why it matters |
|---|---|---|
| `arch` | `aarch64` (`:2`) | The Switch application CPU is ARMv8-A 64-bit. |
| `llvm-target` | `aarch64-unknown-none` (`:11`) | LLVM emits freestanding AArch64 code; the OS behavior is supplied by the rest of the spec and libnx. |
| `os`, `env`, `target-family` | `horizon`, `newlib`, `unix` (`:19-23`) | Crates that use `cfg(target_os = "horizon")` and Unix APIs choose the Switch path. |
| `features` | `+v8a,+neon,+crypto,+crc` (`:6`) | Matches the CPU options also passed to C code in `.cargo/config.toml:3`. |
| `panic-strategy` | `abort` (`:24`) | There is no unwinding runtime. Panics go through the panic hook, write `panic.txt`, and exit. |
| `linker` and `linker-flavor` | `rust-lld`, `gnu-lld` (`:9-10`) | Rust drives the final link with LLD, not GCC. |
| `pre-link-args` | `--wrap=threadCreate`, `--wrap=fchmod`, `--wrap=rename`, `--wrap=rmdir` (`:26-33`) | Sends libnx thread creation and three newlib file calls through bevy_horizon's wrappers ([New threads](#new-threads), [File calls](#file-calls)). |
| `has-thread-local`, `tls-model` | `true`, `local-exec` (`:38-39`) | Lets Rust use fast local-exec TLS once `TPIDR_EL0` is set correctly. |
| `position-independent-executables`, `relocation-model` | `true`, `pic` (`:25`, `:32`) | The loader places the NRO at a runtime address. |

The linker script is embedded in the JSON at `aarch64-nintendo-horizon.json:8`. The parts to care about are:

- **`KEEP (*(.crt0))` first in `.text`.** libnx startup must be at the beginning of the NRO image.
- **TLS sections and symbols.** `PT_TLS`, `.tdata`, `.tbss`, `.main.tls`, `__tdata_lma`, and `__tls_align` let libnx and Rust agree on thread-local addresses (`crates/bevy_horizon/src/logging.rs:150-169`).
- **`__argdata__`.** The script reserves the argument buffer that libnx expects.

## Nightly Rust and build-std

**Why this matters:** Rust's distributed `std` does not include this target. Cargo must build `std` from source each time the target is used in a fresh toolchain cache.

The build command is:

```sh
cargo build --release -Z build-std=core,alloc,std,panic_abort -Z json-target-spec \
    --target /workspace/aarch64-nintendo-horizon.json \
    --package hello_switch
```

That is in `scripts/build_example.sh:29-31`.

There is one trap: `-Z build-std` builds `std` as its own Cargo workspace under Rust's `library/` source tree. The workspace has its own `Cargo.lock`. A `[patch]` in this repository's `Cargo.toml` is not enough for `std`. That is why `.cargo/config.toml` also patches `libc` (`.cargo/config.toml:5-11`), and why the script updates Rust's `library/Cargo.lock` before building (`scripts/build_example.sh:22-28`).

If the lock would still use crates.io `libc`, the script prints:

```text
error: std would build against crates.io libc, not patches/libc
```

Do not remove this guard. The failure it prevents compiled and linked, but file metadata was wrong at runtime because Rust read C structs with the wrong layout.

## Linking to devkitA64, libnx, and newlib

**Why this matters:** Rust code calls C functions for startup, files, sockets, HID, malloc, panic aborts, and system services. The final ELF must find those libraries in the devkitPro tree.

`crates/bevy_horizon/build.rs` adds these search paths when `target_os == "horizon"`:

```text
/opt/devkitpro/libnx/lib
/opt/devkitpro/portlibs/switch/lib
/opt/devkitpro/devkitA64/aarch64-none-elf/lib/pic
/opt/devkitpro/devkitA64/lib/gcc/aarch64-none-elf/16.1.0/pic
```

Those are `crates/bevy_horizon/build.rs:6-9`. It links `nx`, `sysbase`, `c`, `m`, and `gcc` (`crates/bevy_horizon/build.rs:10-14`). The example repeats the same paths and libraries and adds `-z notext` (`examples/hello_switch/build.rs:4-14`).

The `.cargo/config.toml` file also sets the C compiler and C flags for crates that compile C code:

```toml
CC_aarch64_nintendo_horizon = "aarch64-none-elf-gcc"
CFLAGS_aarch64_nintendo_horizon = "-march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE"
```

Those lines are `.cargo/config.toml:1-3`. `-mtp=soft` is important context: devkitA64's C code reads libnx's software thread pointer, while Rust local-exec TLS reads `TPIDR_EL0`. bevy_horizon bridges that difference.

### Linking NVK (the `vulkan` feature)

**Why this matters:** Horizon has no dynamic loader, so a Vulkan driver can't be found at run time. It has to be linked into the NRO.

bevy_horizon's `vulkan` feature (`crates/bevy_horizon/Cargo.toml:23-26`) makes `build.rs` link `libvulkan_bh.a` plus `expat`, `zstd`, `z` and `stdc++` (`crates/bevy_horizon/build.rs:15-17,25-44`). The library comes from two scripts:

1. `scripts/build_nvk.sh` builds NVK, Mesa's Vulkan driver, from [danfromtico/mesa-switch](https://github.com/danfromtico/mesa-switch) with the same devkitA64 and libnx as the NRO. First it applies bevy-horizon's patches in `patches/mesa-switch/` to the checkout, in order, skipping any that are already there. There are twenty-four:
   - `0001` makes NVK take GPU memory from the top of the heap down ([platform notes](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap)).
   - `0002` converts GPU timestamps to nanoseconds with the Tegra X1's tick (1.6276 ns) instead of 1 ns.
   - `0003` adds `NAK_DEBUG=stats`, one line of statistics per compiled shader ([logging](logging-and-crash-reports.md#part-4-what-a-shader-costs-nak_debugstats)).
   - `0004` adds `NAK_DEBUG=xbar`, which lets loads stay in flight across forward branches on Maxwell and Pascal, and `NAK_DEBUG=final`, which prints each shader's final code.
   - `0005` adds `NAK_DEBUG=sink` and `NAK_DEBUG=remat`, which cut the registers a fragment shader keeps live.
   - `0006` adds `NAK_DEBUG=ubra`, which gives Maxwell and Pascal plain branches, without `SSY` and `SYNC`, for an `if` whose condition is the same across the warp. No run on a Switch has turned it on yet.
   - `0007` adds `NAK_DEBUG=xbrk`, which, with `xbar`, lets loads stay in flight past a loop's `BRK` too. Before Turing, NVK makes each texture lookup whose descriptor may differ across the warp a loop that ends in a `BRK`, so without it each such lookup waits for its texture before the next starts.
   - `0008` adds `nouveau_horizon_tune`, switches that change how NVK drives the GPU between frames, so a game can time a frame with and without each in one launch (`crates/bevy_horizon/src/nvk_tune.rs`): early Z's hysteresis, ZCULL off, ZCULL's serialisation, and early Z for every fragment shader, which draws a wrong picture and is only for measuring.
   - `0009` adds a switch for ZCULL's direction. Bevy draws with reversed depth, and with NVK's `LESS` ZCULL rejects nothing in its passes. bevy_horizon sets `GREATER` at launch unless `BEVY_HORIZON_ZCULL_DIR=less`.
   - `0010` says on stderr whether the depth passes use ZCULL: nvhost's ZCULL description, and each depth image's first passes with ZCULL.
   - `0011` adds `NAK_DEBUG=lsink`, which moves a fragment input read only inside a large loop into the loop, so it isn't live across the whole loop.
   - `0012` skips the setup of indirect draws with no instances before Turing. Bevy's GPU culling issues one for every culled batch. A switch turns the skipping off.
   - `0013` says on stderr which images NVK compresses, for the first 32 bound to memory of their own, which bevy-horizon's wgpu-hal gives a texture only with `BH_RT_COMPRESSION=1`.
   - `0014` lets NAK cap a shader's registers: every shader with `NAK_GPR_LIMIT=n`, or one whose entry point's name ends in `_gpr` and the number.
   - `0015` adds `NAK_DEBUG=plat`, with which NAK's scheduler before register allocation waits out latencies. Without it, that scheduler ignores them and puts each texture read just above its use.
   - `0016` lets NAK's unit tests build again on a PC.
   - `0017` makes NVK count compute invocations in a pipeline-statistics query that counts graphics work too, as Bevy's per-pass statistics do, and keep counting them after `vkCmdExecuteCommands`.
   - `0018` clears the result of the MME's multiply before Turing, which made an indirect dispatch's count of compute invocations wrong.
   - `0019` turns on `xbar`, `sink`, `remat`, `xbrk`, `lsink` and `plat` by default in Switch builds. Each made Skate 3's frames faster on a Switch Lite. `NAK_DEBUG=-name` turns one off; NAK prints the flags in effect on stderr.
   - `0020` makes a pipeline-statistics query add up its direct dispatches' compute invocations and write them once, when it ends. Before, each dispatch added its own with a 3D-class method, which stalled the GPU between compute passes.
   - `0021` adds `NAK_DEBUG=ofold`, which folds a storage access's constant offset into the access, saving one or two adds per load. Switch builds start with it on too.
   - `0022` adds `NAK_DEBUG=wgbar`, which keeps the barriers of a workgroup that is one warp. It is only for running in Ryujinx on lavapipe, which runs a warp as four groups of 8.
   - `0023` adds `NAK_DEBUG=share`. When all six scoreboards are taken, a texture fetch or memory load shares one with others of its kind instead of waiting for the oldest, so more loads are in flight at once. It is off by default, since only a run on a Switch can show it is right.
   - `0024` adds `NAK_DEBUG=jthr`, which on Maxwell and Pascal sends a branch straight past blocks that only branch on, and drops branches to the next block, as NAK already does from Volta on. `SYNC`, `BRK` and `CONT` keep their targets. With `ubra`, it takes Skate 3's world shader from 119 control ops to 70. It is off by default until a run on a Switch shows it is faster.

   The other `NAK_DEBUG` flags and `NAK_GPR_LIMIT` are opt-in, set per launch with an `env` line in the game's `options.txt`. Each patch's header explains what it changes and why.

   The script leaves `nvk/lib/libvulkan.a`, which exports `vkGetInstanceProcAddr` and every `vk*` function itself. `nvk/BUILD_INFO` records the mesa-switch commit, the patches applied, and the checkout's uncommitted changes, the patched files among them.
2. `scripts/localize_nvk.sh` turns that into `nvk/lib/libvulkan_bh.a`: one relocatable object whose only global definitions are the `vk*` entry points and the top-down region's `nouveau_horizon_region_alloc`, `_free` and `_stats` (`patches/mesa-switch/`), which `memory.rs` and the `top_down_alloc` allocator call ([platform notes](platform-notes.md)). Mesa's shader compiler (NAK) is Rust, and it brings its own copy of Rust's `std` for Linux. Left global, those symbols clash with the game's `std`.

The object must also carry no COMDAT groups, and the script fails if one survives. A linker keeps only the first copy of each group it sees. NAK's code has one, `DW.ref.rust_eh_personality`, the pointer its unwind tables use to find the personality routine. Any game crate with `extern "C-unwind"` calls has the same group, even though the target is `panic-strategy: abort`, because rustc gives those calls abort-on-unwind landing pads. Skate 3's mlua is one. Its copy came first in the link, so NVK's was discarded, and NVK's references to it, local after step 2, pointed into nothing:

```text
rust-lld: error: relocation refers to a symbol in a discarded section: DW.ref.rust_eh_personality
          >>> defined in .../nvk/lib/libvulkan_bh.a(vulkan_bh.o)
          >>> referenced by vtn_structured_cfg.c
          >>>               vulkan_bh.o:(.eh_frame+0x9664b) in archive .../nvk/lib/libvulkan_bh.a
```

The examples never hit this: switch_captest links mlua but not NVK (no `vulkan` feature), and the three that link NVK have no mlua. The script now runs `ld -r --force-group-allocation`. GNU ld's manual says the option makes the linker "place section group members like normal input sections, and to delete the section groups", which is what a final link does anyway. The pointer becomes a plain section, `.data.DW.ref.rust_eh_personality`, private to NVK, and it still points at NAK's own `rust_eh_personality` (`scripts/localize_nvk.sh`).

The build looks in `nvk/lib` unless `BEVY_HORIZON_NVK_LIB` names another directory, and it stops with a message naming both scripts if the file is missing. The patched wgpu-hal calls the linked driver's `vkGetInstanceProcAddr` directly (see the `wgpu-hal` row in [The crate patches](#the-crate-patches)).

## Startup, exit, and thread creation

**Why this matters:** code can fail before `main`, after `main`, or on a background thread. The toolchain has hooks for all three.

### Main thread TLS before `main`

libnx calls the weak [userAppInit](glossary.md#userappinit) hook after services and SD are ready, but before C constructors and Rust runtime setup. bevy_horizon overrides it in `compat.rs`. The first thing it does is copy libnx's thread pointer into `TPIDR_EL0`, then it opens the boot log (`crates/bevy_horizon/src/compat.rs:136-143`).

The setter is small assembly:

```text
mrs tpidrro_el0
ldr [tpidrro_el0 + 0x1f8]
msr tpidr_el0
```

See `crates/bevy_horizon/src/compat.rs:93-118`. The boot log records the value and the TLS template alignment (`crates/bevy_horizon/src/logging.rs:129-159`).

### New threads

Rust threads go through libnx's `threadCreate`: `std::thread::spawn` -> `pthread_create` -> libnx -> `threadCreate`. The target spec wraps that symbol. bevy_horizon's `__wrap_threadCreate` allocates a tiny start record, changes the entry point to `thread_start`, calls the real function, and logs the stack size, priority, and core (`crates/bevy_horizon/src/compat.rs:286-388`). The first code on the new thread calls `set_thread_pointer`, logs the pointer, and then calls the real entry point (`crates/bevy_horizon/src/compat.rs:391-405`).

A2 proved this on hardware with the example's check thread. The check is in `examples/hello_switch/src/checks.rs:232-267`: it spawns `a2-thread-check`, reads a `thread_local!`, changes it, joins, and confirms the main thread's copy stayed separate.

### File calls

libnx's SD card driver (`fsdev`, reached through newlib's libsysbase) differs from Unix in three places that std relies on. Each is fixed with a `--wrap` in the target spec, so only calls that reach newlib change. Defining the symbols outright doesn't work: a game's build script can put `-lc` ahead of every rlib, so newlib's copy wins and the link fails with a duplicate symbol.

| Wrapper | Stock behavior | What the wrapper does |
|---|---|---|
| `__wrap_fchmod` (`compat.rs:156-177`) | fsdev has no `fchmod` and fails it with `ENOSYS`; Horizon's file systems have no permission bits. std's `fs::copy` calls it on the new file, so every copy left an empty file and failed with "Function not implemented". | Turns `ENOSYS` into success, since there is nothing to set. |
| `__wrap_rename` (`compat.rs:179-252`) | Horizon won't rename onto an existing file (`EEXIST`), where POSIX, and std's `fs::rename` on Unix, replace it. Saving through a temporary file and a rename failed for every save after the first. | When both paths are files, moves the destination aside to `<new>.bh-replaced`, renames, then deletes it, or moves it back if the rename failed. Unlike POSIX it isn't atomic. |
| `__wrap_rmdir` (`compat.rs:254-284`) | fsdev maps four FS results to errno values and the rest to `EIO`, so removing a folder that isn't empty failed with "I/O error". | When the FS result was `DirectoryNotEmpty` (module 2, description 8, from Atmosphère's `fs_results.hpp`), returns `ENOTEMPTY`, which std reports as `ErrorKind::DirectoryNotEmpty`. |

captest's file checks exercise all three (`examples/switch_captest/src/files.rs`).

### Exit

libnx calls [userAppExit](glossary.md#userappexit) when the NRO ends itself. bevy_horizon releases services there (`crates/bevy_horizon/src/compat.rs:145-154`). `release_horizon_services` writes an exit line, puts focus handling back, calls `socketExit`, unmounts RomFS, releases nvdrv, waits up to one second for the heap to unlock, and logs PASS, or a WARN and an exit through AM to HOME if pages stay locked (`crates/bevy_horizon/src/lifecycle.rs:219-280`; the steps are explained in [lifecycle-and-exit.md](lifecycle-and-exit.md#the-userappexit-hook)).

This exists because sphaira's loader reads the next NRO into the old heap. If a [transfer memory](glossary.md#transfer-memory) region from sockets, audio, or GPU work is still locked, the loader's read can fail with a [result code](glossary.md#result-code) like `2001-0106`.

## The crate patches

**Why this matters:** stock crates assume a desktop, mobile, or web OS. Horizon is close enough to Unix to compile many APIs, but not close enough for Bevy's full dependency tree.

| Patch | Files to read | What stock did | What the patch changes |
|---|---|---|---|
| `bevy_asset` | `patches/bevy_asset/Cargo.toml:195-196`, `patches/bevy_asset/src/io/file/mod.rs:4-13,36-50,66,99` | Bevy 0.19.1 pulled `async-io` on non-Windows, non-wasm targets. That pulled `polling` and `rustix` even though this Switch path did not need their open-file-limit helper. With `multi_threaded`, files were read through `async-fs`, whose `blocking::Unblock` allocates an 8 MiB pipe for each file being read (blocking 1.7.0 `src/lib.rs`) and runs the reads on blocking's own thread pool, which adds threads (2 MiB stacks from the heap) while reads queue up, to 500 by default. Skate 3's first run, on Ryujinx, started 43 of them while loading and then failed an 8 MiB allocation. `FileAssetReader` and `FileAssetWriter` put their folder under the executable's (`get_base_path().join(path)`), and Rust's `Path` counts only a leading `/` as absolute, so a device path such as `sdmc:/switch/game/assets` was appended to the base. | Excludes `async-io` on `target_os = "horizon"`, so `async-io`, `polling`, and `rustix` are not compiled just for that helper. On Horizon, uses Bevy's synchronous `FileAssetReader` and `FileAssetWriter` (`sync_file_asset.rs`, which `multi_threaded` otherwise replaces), so reading a file takes no pipe and no extra thread. On Horizon a path that names a device (`sdmc:/`, `romfs:/`) is used as it is (`under_base_path`); other paths are joined as before. |
| `bevy_tasks` | `patches/bevy_tasks/src/task_pool.rs:142-145,224-275,662-663`, `patches/bevy_tasks/src/usages.rs:78-135`, `patches/bevy_tasks/src/lib.rs:82-85,128-137,145-173` | The global task pools are never dropped, so their threads were never joined ([bevyengine/bevy#9477](https://github.com/bevyengine/bevy/issues/9477)), and their stacks kept the heap locked for the loader. Pool sizes came from std's `available_parallelism`, which is `Unsupported` on Horizon. | Adds `TaskPool::shut_down` and `shut_down_global_task_pools`, which bevy_horizon calls at exit. Sizes the pools from the process's core mask (`svcGetInfo`). |
| `cpal` | `patches/cpal/src/host/mod.rs:41-53`, `patches/cpal/src/platform/mod.rs:846-856`, `patches/cpal/src/host/horizon/mod.rs:1-824`, `patches/cpal/build.rs:10-20` | No CPAL host existed for Horizon, so Bevy audio had no output backend. | Adds a Horizon host on libnx's audio renderer ([audren](glossary.md#audren)), fed from a thread of its own, and links libnx. It exposes one output device, f32 samples converted to 16-bit, 48 kHz, 256-8192 frames, and no input device, and closes at exit a stream the game never dropped. The SDL2 host before it was heard on hardware in captest; this one has played in Ryujinx (run au5), and its Bevy 0.18 copy has played Skate 3 on hardware (perf35, findings 54); this 0.19 one hasn't run there yet ([input and audio](input-and-audio.md#the-audio-host)). |
| `getrandom` | `patches/getrandom/src/backends.rs:122-124`, `patches/getrandom/src/backends/horizon.rs:1-17` | AArch64 Horizon would fall to unsupported or the 3DS path. | Uses devkitPro newlib's `arc4random_buf`. That calls `getentropy`, which calls `_getentropy_r`, which nothing in devkitA64 defined; bevy_horizon defines it, and exports a `getrandom` symbol for std, both on libnx's `randomGet` (`crates/bevy_horizon/src/compat.rs:752-781`). |
| `getrandom` 0.2 (`patches/getrandom-0.2`) | `patches/getrandom-0.2/src/lib.rs:254-257`, `patches/getrandom-0.2/src/util_libc.rs:24` | `ring` 0.17 still uses getrandom 0.2, which takes newlib's `getrandom()` only on the 3DS (`target_arch = "arm"`), so the Switch had no source of randomness. | Takes that path on any Horizon target, where bevy_horizon's `getrandom` symbol answers it. `Cargo.toml` patches it under a renamed key, `getrandom02`, next to the 0.4 patch. |
| `inventory` | `patches/inventory/src/lib.rs:543-547` | Each `inventory::submit!` runs from a constructor that inventory places in the platform's constructor section, and it named none for Horizon, so nothing was submitted. Bevy's `reflect_auto_register` collects reflected types this way, and Bevy registers many of its own (`Transform`, for one) only through it. | Puts Horizon's constructors in `.init_array`, as on Linux. libnx's startup ends by calling newlib's `__libc_init_array`, which runs every `.init_array` entry. Only in the game tables (`.cargo/bevy-*.toml`): the examples don't use automatic registration. Not yet run on hardware. |
| `libc` | `patches/libc/src/unix/newlib/horizon/mod.rs:1-78` plus `.cargo/config.toml:10-11` | Rust's Horizon newlib layout came from 3DS-era assumptions. Some types and structs did not match devkitA64. | Defines the layouts used by the ABI probe and ensures Rust `std` uses this patch too. Networking socket structs still need more work. |
| `lua-src` | `patches/lua-src/src/lib.rs:192-194` | Its build script stops with "don't know how to build Lua for" any target it doesn't list, the Switch included. | Builds Lua for Horizon as plain ISO C with no platform define: no `dlopen`, and `io.popen` reports "not supported". For games that embed Lua through `mlua`, as Skate 3's mod support does. |
| `memmap2` | `patches/memmap2/src/lib.rs:59-96`, `patches/memmap2/src/horizon.rs:1-198` | Horizon has no `mmap`, and memmap2's fallback (`stub.rs`) failed every map with `Unsupported`. | Holds maps on the heap. Read-only and private (copy-on-write) maps read the range in when they're made, anonymous maps are zeroed memory, and writable shared or executable maps return `Unsupported`. Later changes to the file don't show up in a map. captest's map checks pass in the emulator; they haven't run on hardware yet. |
| `polling` | `patches/polling/src/os.rs:1-22` | The crate selects OS poll backends such as kqueue or IOCP. Horizon has neither. | Leaves no Horizon OS backend selected. This patch stays for games whose dependencies still pull `polling`; the current `bevy_asset` patch avoids compiling it for the example. |
| `raw-window-handle` | `patches/raw-window-handle/src/horizon.rs:1-77`, `patches/raw-window-handle/src/lib.rs:40-41,53,213-217,352-356,429,451` | No handle type existed for Horizon, so nothing could describe the Switch's window to wgpu. | Adds `HorizonWindowHandle` (libnx's `NWindow` pointer) and `HorizonDisplayHandle` (empty: libnx opens the display before `main`), shaped like the Haiku ones. bevy_horizon's `window.rs` fills them in. |
| `ring` | `patches/ring/build.rs:198-205` | ring's build script knew 32-bit ARM Horizon (the 3DS) but not AArch64, so it built none of its assembly for the Switch, and the link failed on the AArch64 routines ring calls. | Builds the `linux64` assembly for AArch64 Horizon: Switch binaries are ELF, and devkitA64's GCC defines `__ELF__`, which is all those files check for. |
| `rustix` | `patches/rustix/src/clockid.rs:15-33`, `patches/rustix/src/bitcast.rs:1-29` | `ClockId` used `repr(i32)`, but devkitA64's `clockid_t` is an 8-byte unsigned long on the Switch. | Uses `repr(u64)` for AArch64 Horizon and keeps the bitcast macro able to cast constants safely. |
| `wgpu` | `patches/wgpu/build.rs:20-21`, `patches/wgpu/src/backend/wgpu_core.rs:799-800,826-827` | wgpu 29.0.4 compiled no backend for Horizon. | Enables the Vulkan backend on Horizon when the `vulkan` feature is on, and keeps the DRM surface path off. |
| `wgpu-core` | `patches/wgpu-core/build.rs:4-5,23`, `patches/wgpu-core/Cargo.toml:211-212`, `patches/wgpu-core/src/instance.rs:106-117,320-321,1014-1015`, `patches/wgpu-core/src/device/resource.rs:702-710` | Same as `wgpu`; the Vulkan backend also needs the `wgpu-core-deps-windows-linux-android` dependency, which was not pulled on Horizon. Bevy keeps wgpu's indirect-call validation in release builds whenever its backends include DX12, which its default, `Backends::all()`, does on every platform. A lost device didn't say which call lost it. | Adds Horizon to the Vulkan platform lists and the dependency's target list; DRM surfaces stay off. On Horizon, keeps indirect-call validation only alongside `VALIDATION`, Bevy's rule for a platform without DX12 ([rendering.md](rendering.md#indirect-call-validation)), and logs the wgpu-core call behind a device loss. |
| `wgpu-core-deps-windows-linux-android` | `patches/wgpu-core-deps-windows-linux-android/Cargo.toml:41-42` | Its `wgpu-hal` dependency was not pulled on Horizon, so `vulkan` reached nothing. | Adds Horizon to that target list. |
| `wgpu-hal` | `patches/wgpu-hal/Cargo.toml:315,319-326,343`, `patches/wgpu-hal/src/vulkan/instance.rs:246-247,269-273,278,288-296,558-580,609,617-640,960-962`, `patches/wgpu-hal/src/vulkan/adapter.rs:15-71,2255-2259,2318-2330,2365`, `patches/wgpu-hal/src/vulkan/device.rs:18-71,718-729,1125-1130,1144,1820-1830`, `patches/wgpu-hal/src/vulkan/mod.rs:355-358,1471-1474`, `patches/wgpu-hal/src/vulkan/drm.rs:1-7`, `patches/wgpu-hal/src/auxil/renderdoc.rs:3-4,15-16,39,43-52`, `patches/wgpu-hal/src/lib.rs:439-448` | Loaded the Vulkan driver with libloading, which has a `compile_error!` for Horizon. Had no Switch surface. Hid adapters reporting conformance version 0, which NVK does for every Tegra GPU. Built DRM and RenderDoc code that needs a dynamic loader or `makedev`. Sent NVK's error messages nowhere in release builds, and dropped the VkResult behind an "unexpected" error. Bevy's default `MemoryHints::Performance` has the allocator take device memory in 128-256 MiB blocks and host-visible memory in 64-128 MiB blocks (wgpu 27's gpu-alloc: free-list chunks of 128-512 MiB). NVK takes all of it from the malloc heap (mesa-switch `nouveau_horizon_memory.c`), the same ~3 GiB the game's own allocations use. wgpu asks for buffer robustness whenever the device has it, and NVK then checks every storage-buffer access against the buffer's size in the shader, a component at a time; before SM 7.3, the Switch's Maxwell included, each check is a branch (mesa-switch `src/nouveau/vulkan/nvk_shader.c`, `nvk_ssbo_addr_format`). Bevy's shaders have no checks of their own: Bevy turns naga's off. NVK compresses an image only when its memory was allocated for it alone, which wgpu never does for a texture. | Uses ash without its `loaded` feature and calls the statically linked driver's `vkGetInstanceProcAddr` (a clear error if no driver is linked). Makes surfaces with `VK_NN_vi_surface`. Keeps NVK's adapter visible. Leaves out DRM, and makes RenderDoc report "not available". On Horizon, always registers a debug messenger, so NVK's messages reach `log.txt`, and logs the VkResult ([rendering.md](rendering.md#on-the-switch)). On Horizon, `Performance` gets `MemoryUsage`'s sizes: 8-64 MiB device blocks and 4-32 MiB host blocks (wgpu 27: 8-64 MiB chunks, at `patches/bevy-0.18/wgpu-hal/src/vulkan/adapter.rs:2351-2359`). On Horizon, asks for neither `robustBufferAccess` nor `robustBufferAccess2`, and has naga clamp buffer indices in every shader instead, Bevy's included: a few percent more instructions than no checks, where the driver's cost 15% and more, and an index past the end reads the last element rather than zero. `BH_ROBUST_BUFFER_ACCESS=1` gives the driver's checks, as upstream, and `0` neither; each logs what it chose ([rendering.md](rendering.md#buffer-robustness); wgpu 27: `patches/bevy-0.18/wgpu-hal/src/vulkan/adapter.rs:13-69,1822-1825,1859,1871`, `device.rs:1139-1150,2248-2258`, `mod.rs:616-619`). On Horizon, `BH_RT_COMPRESSION=1` gives large render targets memory of their own, for NVK to compress; off by default ([rendering.md](rendering.md#render-target-compression); wgpu 27, which allocates through gpu-alloc: `patches/bevy-0.18/wgpu-hal/src/vulkan/device.rs:18-67,842-843,1018-1083,1548-1552,1615-1621`). |

### Patches for a game, and Bevy 0.18

The table above is this workspace's (`Cargo.toml`'s `[patch.crates-io]`, for Bevy 0.19.1 and wgpu 29). A game in its own repository gets the patches from config files instead, so its `Cargo.toml` needs none and its desktop builds don't change. `scripts/build_game.sh` passes `--config .cargo/config.toml` (libc, which reaches std too) and `--config .cargo/bevy-$BEVY.toml`:

- `.cargo/bevy-0.19.toml`: the table above plus `inventory`.
- `.cargo/bevy-0.18.toml`: `patches/bevy-0.18/`, which makes the same changes to Bevy 0.18.1's `bevy_asset` and `bevy_tasks`, to wgpu 27 (`wgpu` 27.0.1, `wgpu-core` 27.0.3, `wgpu-hal` 27.0.4, `wgpu-core-deps-windows-linux-android` 27.0.0) and to cpal 0.15.3, the one Bevy 0.18 reaches through rodio 0.20.1 (the same audio host, [input and audio](input-and-audio.md#the-audio-host)), plus the version-independent patches Skate 3 builds: `getrandom`, `inventory`, `lua-src`, `memmap2` and `raw-window-handle`. In wgpu 27 the `Drm` surface variant exists on every Unix, Horizon included, so that patch also adds an arm that refuses it. bevy_horizon's `bevy_0_18` feature builds bevy_horizon itself against Bevy 0.18.

[Porting a Bevy game](porting-a-bevy-game.md) has the steps.

## How to check that a patch is really applied

**Why this matters:** Cargo can skip a patch if the patch's version does not satisfy the locked dependency. That happened with `bevy_asset`: the patch was 0.19.0 while the lock wanted 0.19.1.

The example build script checks this after every build (`scripts/build_example.sh:53-80`). It reads the workspace `[patch.crates-io]` table (`Cargo.toml:25-59`), reads each patch crate's version, then searches `Cargo.lock` for a registry package with the same name and semver-compatible family.

A game's lock usually has other versions than the patches, so `scripts/build_game.sh` moves them first (`scripts/build_game.sh:49-99`): for each table the build reads, it finds locked crates.io versions in a patch's semver family and runs `cargo update -p <name>@<locked> --precise <patch version>`, one at a time, then fails the same way if any are left. For Skate 3 only libc moved (0.2.189 to 0.2.190); its other patched crates were already at the patches' versions.

The failure looks like this:

```text
error: patches/bevy_asset is bevy_asset 0.19.0, but Cargo.lock still has crates.io's bevy_asset 0.19.1, so the patch isn't used. Re-make it from bevy_asset 0.19.1.
```

You can also inspect the lock file yourself. A patched crate should not have a `source = "registry+..."` entry for the same semver family. For git patches, such as IW4L's `quinn-proto`, the lock entry should show the git URL and revision.

## The libc ABI probe

**Why this matters:** Rust and C can compile while disagreeing about struct field offsets. The failure only appears later, as wrong file sizes, wrong file types, broken timeouts, or bad socket addresses.

Run:

```sh
./scripts/check_libc_abi.sh
```

The script writes a C probe and a Rust probe inside the container. The C side includes devkitA64 headers, compiles with GCC, and emits an array of constants. The Rust side builds a `no_std` crate against `patches/libc`, emits the same array, reads both object files back, and compares them (`scripts/check_libc_abi.sh:66-122`).

It compares C type sizes and alignments, struct sizes and field offsets, and errno/open/file/clock/pthread/sysconf constants (`scripts/check_libc_abi.sh:25-64`). The current non-networking probe reports 167 compared items and 0 mismatches. A separate socket probe still has 41 mismatches out of 178 socket items, so do not treat Rust networking as proven until that is fixed and tested.

## Build profiles

**Why this matters:** Switch builds are release builds. Debug Bevy is too slow and too large to tell you much.

The workspace root uses resolver 3 and lists both the layer, the example, and the patch crates as members (`Cargo.toml:1-14`). The example depends on Bevy 0.19 with default features off and enables only the pieces it uses: window, render crates, PBR, assets, logging, text, UI, default font, PNG, and `tonemapping_luts` with the `ktx2` and `zstd_rust` loaders it needs (`examples/hello_switch/Cargo.toml:9-26`). `Camera3d` tonemaps with TonyMcMapface by default, and that needs `tonemapping_luts`. Without it Bevy logs `TonyMcMapFace tonemapping requires the tonemapping_luts feature` and samples a 1-texel placeholder, which flattens the picture. Bevy's own `3d` feature set includes all three, so only games that switch default features off can miss them. bevy_horizon's features are `vulkan` (see [Linking NVK](#linking-nvk-the-vulkan-feature)), `fps_overlay` (see [rendering.md](rendering.md#the-fps-overlay)), `render`, or `render_0_18` for a Bevy 0.18 game, which paces GPU uploads (see [platform-notes.md](platform-notes.md#pacing-uploads-the-render-feature)), and `top_down_alloc`, which puts Rust's large blocks in NVK's top-down region (see [platform-notes.md](platform-notes.md#large-rust-blocks-in-the-region-the-top_down_alloc-feature)).

There is no special `[profile.release]` in this repository today. The build relies on Cargo's release defaults plus each dependency's own settings. If a game needs LTO, size tuning, or split debug symbols, add that in the game repository and remeasure.

## What the toolchain cannot do yet

**Why this matters:** a successful NRO is not a rendered game.

- **Rendering works on the Switch, with one open failure.** In hardware run 5, nvk_probe's triangle and hello_switch's lit cube held 60 fps through NVK and exited cleanly, and Breakout played. render_check lost the GPU device about 4 seconds in; run 6's builds log which call lost it and NVK's reason ([rendering.md](rendering.md#on-the-switch)). The emulator gets some of NVK's shaders wrong, so the harness can't stand in for hardware here.
- **Two future-incompatibility warnings come from wgpu.** Building `patches/wgpu-core` and `patches/wgpu` with this nightly prints `overflow evaluating the requirement` for wgpu's own auto-trait checks ([rust-lang/rust#159228](https://github.com/rust-lang/rust/issues/159228)). Registry crates' lints are capped, so these only show because the crates are patched. They are harmless today. If a later nightly makes them errors, the fix the compiler suggests is `#![recursion_limit = "256"]` in each crate's `lib.rs`.
- **The Bevy 0.19 audio host hasn't run on hardware yet; the 0.18 one has.** Through SDL2, captest's sounds and bevy_audio's chime were heard on the Switch with Bevy 0.19 ([platform notes](platform-notes.md)). SDL's Switch driver can stop the sound for good, so both cpal hosts now feed audren themselves. On the Switch, the 0.18 host played Skate 3's sound clean through play, never ran dry, came back after HOME and after sleep, and closed at exit in 19.0 ms (perf35, findings 54). The 0.19 host, the same code on cpal 0.17.3, waits for captest's next hardware run ([input and audio](input-and-audio.md#the-audio-host)).
- **Networking is not proven from Rust.** The socket ABI still has known mismatches. libnx sockets can be initialized and released; game-level Rust networking still needs its own ABI fix and test.
- **Docked mode is not proven.** The display plugin knows handheld and docked sizes (`crates/bevy_horizon/src/display.rs:1-46`), but the verified runs were handheld.
- **Emulator and capability-test notes** are in [testing.md](testing.md): captest, the Ryujinx harness, and what the emulator gets wrong.
