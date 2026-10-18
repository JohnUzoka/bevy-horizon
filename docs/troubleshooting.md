# Troubleshooting

This page is for you when a [Bevy](glossary.md#bevy) game you're porting goes wrong on the Switch: it won't build, won't start, quits, draws wrong, stays silent, or exits badly. Each entry starts from what you see, then says why it happens and what to do, and links to the page with the details. `bevy_horizon` now prevents several of these failures itself; for those, the entry says how you can still run into them.

## In short

- **Where you land is the first clue.** The [homebrew](glossary.md#homebrew) menu means the game exited, or [panicked](glossary.md#panic), and gave the [heap](glossary.md#heap) back. [HOME](glossary.md#home-menu) with a new [crash report](glossary.md#crash-report) means a crash. HOME without one means heap pages were still locked at exit, which can follow a panic too, or the game was closed from HOME.
- **Read `sdmc:/switch/<app>/log.txt` first,** on the [SD card](glossary.md#sdmc), then `panic.txt`, `crash.log` or `abort.log` beside it, then the newest [Atmosphère](glossary.md#atmosphère) report. The next launch rewrites `log.txt`, so copy it before you try again.
- **In a crash report, read the module list first.** If it lists only the [loader](glossary.md#loader), `hbl`, your code wasn't running: decode the [Result](glossary.md#result-code) instead of looking up addresses.
- **Most build surprises are a patch Cargo didn't use.** Cargo only warns; `scripts/build_game.sh` stops instead.
- **Most exit surprises are heap pages still held.** Close every [service](glossary.md#service) you open, with `register_release_hook`, and make your threads end.
- **GPU memory is heap memory.** "Out of memory" usually means upload spikes, large buffers or a heap full of holes, not a full console.
- **The emulator isn't the console.** Stock [Ryujinx](glossary.md#ryujinx) can't run [Vulkan](glossary.md#vulkan) builds, and [emulator](glossary.md#emulator) speed says nothing about the Switch.

## First, collect

Copy these before you launch again:

| What | Where | What it tells you |
|---|---|---|
| `log.txt` | `sdmc:/switch/<app>/`, where `<app>` is the [NRO](glossary.md#nro)'s file name without `.nro` | Everything the game logged, with `bevy_horizon`'s `[Boot]`, `[Heartbeat]` and `[Exit]` lines. Rewritten at every launch |
| `panic.txt` | The same folder | Rust panic reports, appended; the last one is the newest |
| `crash.log`, `abort.log` | The same folder | `bevy_horizon`'s own report of a CPU exception or a [libnx](glossary.md#libnx) fatal error, appended |
| The crash report | `sdmc:/atmosphere/crash_reports/`, named `<unix seconds>_<program id>.log` | Atmosphère's record of a crash: modules, registers and stacks |
| The unstripped [ELF](glossary.md#elf) | `target/aarch64-nintendo-horizon/release/<binary>` on your PC | The symbols for crash addresses. Keep it with every NRO you test |

If `bevy_horizon` can't use that folder, the files go to the root of the SD card instead: `sdmc:/log.txt`, `sdmc:/panic.txt`, and so on. Note how the game ended, too: your game's own quit, closing it from HOME, or a crash.

Then read them in this order:

1. **Where you landed.** See the table below.
2. **The crash report's module list.** If it lists your NRO, the fault is in your code. `crash.log`'s `pc:` line already gives the ELF address, as `nro+0x…`. For an address from the report, subtract the `nro base` value at the top of `log.txt`. Look the result up with devkitA64's `aarch64-none-elf-addr2line -e <ELF> 0x…`. If the list names only `hbl`, the loader crashed, and your ELF's symbols don't apply.
3. **The Result.** A [Horizon](glossary.md#horizon) result code packs a module and a description: module = code & 0x1ff, description = (code >> 9) & 0x1fff, shown as (2000 + module)-description.

| Where you land | What it means | Read |
|---|---|---|
| The homebrew menu | The game exited, or panicked, and nothing held the heap | The end of `log.txt`, which should say `[Exit] PASS`; `panic.txt` if you didn't quit |
| HOME, no new crash report, and `log.txt`'s `[Exit]` lines include a `WARN` or an nvdrv line | Heap pages were still locked, so `bevy_horizon` exited through AM to HOME rather than let the loader crash. A panic can end here too | [Exit problems](#exit-problems); `panic.txt` |
| HOME, no new crash report, and no `[Exit]` lines | The game was closed from HOME, which ends the process at once | The last lines of `log.txt` |
| HOME with a new crash report that lists your NRO | A CPU exception or a libnx fatal error in your code | `crash.log` or `abort.log`, then the report |
| HOME with a new crash report that lists only `hbl` | The loader crashed, before your NRO started or after it ended | The Result, below |

| Result | Shown as | Where you see it |
|---|---|---|
| `0x0` | 2000-0000 | From `hbl`, before your NRO starts: [the NRO has no asset section](#home-at-once-with-a-crash-report-that-lists-only-hbl) |
| `0xD401` | 2001-0106 | The kernel's `InvalidCurrentMemory`. From `hbl`, after your NRO exits: [a heap page was still locked](#home-after-exit-with-a-crash-report-that-lists-only-hbl) |
| `0x1559` | 2345-0010 | libnx's `IoError`. In the `romfsMountSelf` line it's normal |

[logging-and-crash-reports.md](logging-and-crash-reports.md) explains every file and how to read a crash report.

## Lines that look wrong but aren't

- `fsdevMountSdmc: 0x559 (0x559 = already mounted by libnx, which is normal)`.
- `romfsMountSelf: 0x1559 = 2345-0010 (libnx IoError): this NRO has no RomFS, which is normal unless elf2nro was given --romfsdir`. [RomFS](glossary.md#romfs) is optional.
- **The first [heap check](glossary.md#heap-check) at exit lists locked blocks,** for example `[Exit] heap check, before releasing services: 1 block, 2.20 MiB, would fail the loader's read:`. That is what is still open before `bevy_horizon` closes its services, such as the sockets' 2.20 MiB. The check `after releasing services`, and the `[Exit] PASS` or `[Exit] WARN` after it, is the one that counts.
- **`wgpu-hal: waited X s for the window's next image, longer than the 1.0 s wgpu asks for; on Horizon it waits for as long as it takes`** is a slow frame, not an error. It matters only if device-lost lines follow ([below](#the-device-is-lost-or-the-gpu-stops)).
- **No `[Boot]` lines in Ryujinx.** It starts the NRO without a loader.

## The build fails, or a patch isn't applied

### Cargo says a patch "was not used in the crate graph"

- **What you see:** a Cargo warning that a patch "was not used in the crate graph", and the build carries on. `scripts/build_game.sh` and `scripts/build_example.sh` stop instead, for example with `error: patches/bevy_asset is bevy_asset 0.19.0, but Cargo.lock still has crates.io's bevy_asset 0.19.1, so the patch isn't used. Re-make it from bevy_asset 0.19.1.`
- **Why:** Cargo uses a [crate patch](glossary.md#crate-patch) only if it can resolve the dependency to the patch's exact version. If `Cargo.lock` already holds another version, such as a newer release from crates.io, Cargo keeps it, builds the crates.io crate, and only warns. A skipped [wgpu](glossary.md#wgpu) patch leaves wgpu without its Switch changes, a skipped [cpal](glossary.md#cpal) patch plays nothing, and a skipped libc patch reads files wrong.
- **Fix:** build with `scripts/build_game.sh`. It moves each patched crate in your lock file to the patch's version, one `cargo update -p <name>@<locked> --precise <version>` at a time, and stops if any is still locked to crates.io. If it stops, re-make the patch from the version the message names. To check by hand, find the crate in `Cargo.lock`: the patched entry has no `source = "registry+…"` line. See [porting, step 4](porting-a-bevy-game.md#4-match-the-patch-set-to-your-bevy-version-and-build) and [toolchain.md](toolchain.md#how-to-check-that-a-patch-is-really-applied).

### `build_game.sh` can't tell which Bevy the game uses

- **What you see:** `error: can't tell <game>'s Bevy version from bevy_app in its Cargo.lock (found: …); set BEVY=0.18 or BEVY=0.19`, or `error: no .cargo/bevy-<version>.toml for Bevy <version>`.
- **Why:** bevy-horizon has one patch set per Bevy version, 0.18 and 0.19, and the script picks it from `bevy_app` in your lock file.
- **Fix:** set `BEVY=0.18` or `BEVY=0.19`. For another Bevy release, make a patch set first: re-make each Bevy and wgpu patch on that release, and add a `.cargo/bevy-<version>.toml`. Don't build with the other version's table: Cargo only warns, and the game builds against crates.io's wgpu, which lacks the Switch changes.

### `bevy_horizon` stops the build with a feature error

- **What you see:** one of `bevy_horizon`'s compile errors:
  - `` bevy_horizon: enable one of `bevy_0_19` and `bevy_0_18`; for Bevy 0.18 set `default-features = false` ``
  - ``bevy_horizon: a Bevy 0.18 game turns on `render_0_18`; `render` is for Bevy 0.19``
  - ``bevy_horizon: `fps_overlay` needs Bevy 0.19's bevy_dev_tools``
- **Why:** `bevy_0_19` is a default feature, so a Bevy 0.18 game has to turn it off. `render` and `fps_overlay` are for Bevy 0.19.
- **Fix:** for Bevy 0.18, depend on `bevy_horizon` with `default-features = false, features = ["bevy_0_18", "vulkan"]`, and use `render_0_18` in place of `render`. The FPS overlay needs Bevy 0.19. See [porting-a-bevy-game.md](porting-a-bevy-game.md).

### The link fails on `libvulkan_bh.a`

- **What you see:** the build script stops with ``bevy_horizon's `vulkan` feature links <dir>/libvulkan_bh.a, which is missing: run scripts/build_nvk.sh and scripts/localize_nvk.sh, or set BEVY_HORIZON_NVK_LIB to the directory that has it``. Or the link fails with `rust-lld: error: relocation refers to a symbol in a discarded section: DW.ref.rust_eh_personality`.
- **Why:** the `vulkan` feature links NVK, [Mesa](glossary.md#mesa)'s Vulkan driver, which you build once. The second error comes from a `libvulkan_bh.a` made by an older `localize_nvk.sh`, in a game with a crate that makes `extern "C-unwind"` calls, such as mlua.
- **Fix:** run `scripts/build_nvk.sh`, then `scripts/localize_nvk.sh`, which now makes the library without the COMDAT groups that clash, and fails if one survives. `BEVY_HORIZON_NVK_LIB` is read at build time. See [toolchain.md](toolchain.md#linking-nvk-the-vulkan-feature).

### The build says std would use crates.io's libc

- **What you see:** `error: std would build against crates.io libc, not patches/libc`.
- **Why:** [`-Z build-std`](glossary.md#build-std) builds std as its own workspace, with its own lock file, and only a `[patch]` in `.cargo/config.toml` reaches it. Without the patch, std reads [newlib](glossary.md#newlib)'s structs with the wrong layout ([below](#file-types-and-sizes-are-wrong)).
- **Fix:** keep the libc patch in `.cargo/config.toml`, and build through the scripts, which move std's lock onto it. See [toolchain.md](toolchain.md#the-crate-patches).

## It doesn't start

### HOME at once, with a crash report that lists only `hbl`

- **What you see:** [sphaira](glossary.md#sphaira) goes back to HOME with a new crash report whose module list has only `hbl`, with Result `0x0` (2000-0000). `log.txt` is missing, or still the previous launch's. The build scripts reject such an NRO with `error: <nro> has no asset section (image 0x…, file 0x…); sphaira forwarders will refuse to load it`.
- **Why:** sphaira's loader expects an asset section after the NRO's image, with the icon and the [NACP](glossary.md#nacp). `elf2nro` writes one only when it's given them; title options such as `--name` belong to `nacptool`, and `elf2nro` ignores them. Without the section the loader's size check fails, and it aborts before any of your code runs.
- **Fix:** pack the NRO with `nacptool` and `elf2nro --nacp=… --icon=…`, as `scripts/build_game.sh` and `scripts/build_example.sh` do. Their check prints `ok: image 0x… + ASET section, file 0x… bytes`. See [building.md](building.md).

### A crash before `main`

- **What you see:** HOME with a new crash report that lists your NRO. With `bevy_horizon` linked, `log.txt` ends at `[Boot] next: C constructors, Rust's runtime setup (std::rt::lang_start), then main`, and `crash.log` holds the fault. Without it there's no `log.txt` at all. A fault near address `0x700`, with [`TPIDR_EL0`](glossary.md#tpidr_el0) at 0 in the report's thread dump, is the [thread-local](glossary.md#thread-local-storage) fault.
- **Why:** Rust reads its thread-locals at `TPIDR_EL0` plus an offset, and its runtime reads one before `main`. libnx keeps its own thread pointer elsewhere and leaves `TPIDR_EL0` at 0. `bevy_horizon`'s [`userAppInit`](glossary.md#userappinit) copies libnx's value into the register before Rust starts. Other code that runs before `main`, such as C constructors, can crash too; `crash.log`'s `pc:` line says where.
- **You can still hit it if** `bevy_horizon` isn't linked. A dependency your code never uses isn't linked at all, and then neither are its `userAppInit` and [`userAppExit`](glossary.md#userappexit). Or if a changed linker script makes Rust and libnx place thread-locals differently: the `[Boot] TLS template` line then says `MISALIGNED: Rust and libnx put thread-locals at different addresses`.
- **Fix:** use `bevy_horizon` in the Horizon build, by adding `HorizonPlugin`, and keep bevy-horizon's `aarch64-nintendo-horizon.json`. See [platform-notes.md](platform-notes.md).

### A new thread crashes as it starts

- **What you see:** a crash report that lists your NRO, and `crash.log` says `tpidr_el0: … (Rust thread-local base; libnx tls_tp …, MISMATCH: thread_local! access faults)`.
- **Why:** each new thread needs `TPIDR_EL0` set too. `bevy_horizon` wraps libnx's `threadCreate`, and the wrapper sets it before the thread runs any Rust or C code. Every thread made through the wrapper logs `[Thread] #N:` lines in `log.txt`, ending `TPIDR_EL0 = libnx tls_tp = 0x…, so thread_local! works`, unless the game has turned those lines off.
- **You can still hit it if** your own [target specification](glossary.md#target-specification) lacks `--wrap=threadCreate`, so the main thread starts and the first new thread faults. Or if code makes a thread without `threadCreate`, with `svcCreateThread` directly.
- **Fix:** keep `--wrap=threadCreate` in the target specification, and create threads through std or `threadCreate`. See [toolchain.md](toolchain.md).

## It quits without a crash report

### The game vanishes, and `panic.txt` has a new report

- **What you see:** the game is gone, to the homebrew menu or to HOME, with no crash report. `panic.txt` ends with a new block:

  ```text
  ==================== RUST PANIC ====================
  app:      …
  thread:   …
  location: …
  message:  …
  memory:   …
  ```

- **Why:** the target uses `panic-strategy = "abort"`. A panic writes its report, then the game leaves through the normal exit path: to the homebrew menu, or to HOME if other threads are still running. `log.txt`'s `[Exit]` lines say which. It isn't a CPU exception, so Atmosphère writes no report.
- **Fix:** read `location` and `message`; `memory` shows how full the heap was. `panic.txt` keeps the panics of every launch, so read the last block. Without `bevy_horizon::init_switch_logging` at the start of `main` there's no `panic.txt`, but Rust's own panic message still reaches `log.txt`. One that's easy to hit while porting is `Cannot disable a plugin that does not exist.`, from `.disable::<…>()` on a plugin your Bevy features leave out of `DefaultPlugins`, such as `PipelinedRenderingPlugin`. See [logging-and-crash-reports.md](logging-and-crash-reports.md).

### The game returns to the menu right after its setup

- **What you see:** `log.txt` has your setup lines, and you're back at the homebrew menu without a single frame drawn.
- **Why:** nothing calls `App::update` every frame. winit has no Switch backend, and `DefaultPlugins` adds a [ScheduleRunnerPlugin](glossary.md#schedulerunnerplugin) only when `bevy_window` is off. With no runner, `App::run` runs one update and returns, and `main` ends.
- **Fix:** add `HorizonPlugin`. It adds `ScheduleRunnerPlugin::run_loop` at 60 Hz when no runner is present.

## Nothing draws, or drawing is wrong

### Bevy panics "Unable to find a GPU!"

- **What you see:** `Unable to find a GPU! Make sure you have installed required drivers!` as Bevy builds its renderer.
- **Why:** no Vulkan driver is linked. Without `bevy_horizon`'s `vulkan` feature, the NRO gets a stub `vkGetInstanceProcAddr` that returns nothing, so wgpu finds no adapter. If the feature is on, check that the wgpu patches were applied ([how to check](toolchain.md#how-to-check-that-a-patch-is-really-applied)).
- **Fix:** build NVK, then turn on the `vulkan` feature. A game that draws nothing can set `backends: None` in `RenderPlugin`'s `WgpuSettings` instead. See [toolchain.md](toolchain.md#linking-nvk-the-vulkan-feature).

### Everything a 3D camera draws is magenta

- **What you see:** meshes come out magenta, and the log says ``TonyMcMapFace tonemapping requires the `tonemapping_luts` feature.``
- **Why:** Bevy's default tonemapping for `Camera3d` reads a lookup table that ships only with the `tonemapping_luts` feature. Without the table, Bevy samples a one-texel magenta placeholder instead. The table is a KTX2 file compressed with Zstandard.
- **Fix:** turn on Bevy's `tonemapping_luts`, `ktx2` and `zstd_rust` features. `zstd_rust` is the pure-Rust decoder, which builds for Horizon without a C toolchain. Or give every camera another `Tonemapping`. See [rendering.md](rendering.md).

### The device is lost, or the GPU stops

- **What you see:** the picture freezes or goes black. `log.txt` may first show `wgpu-hal: waited X s for the window's next image, …`, then lines such as these:
  - `Vulkan returned ERROR_…, which wgpu reports as an unexpected driver error`
  - `wgpu-core: the device is lost to … (…), returned to the call at <file>:<line>`
  - NVK's own messages, as `WARN wgpu_hal::vulkan::instance: GENERAL [<file>:<line> (0x0)]`, with the message on the next line
  - with Bevy 0.19, `Caught DeviceLost error: …` and `Quitting the application due to DeviceLost RenderError`; with Bevy 0.18, the panic `Couldn't get swap chain texture, operation unrecoverable: …`

  A panic in `Buffer::get_mapped_range` after these is a consequence of the loss, not a second bug. At exit, the nvdrv line [below](#the-exit-skips-nvexit-and-goes-to-home) follows.
- **Why:** the GPU faulted or stopped answering. Two causes in a game's own code are worth ruling out first: an index past the end of a buffer, which can fault only with the bounds checks off, and a bad indirect draw.
- **Fix:**
  - Read the first of these lines: it names the Vulkan error and the wgpu call.
  - Check `BH_ROBUST_BUFFER_ACCESS`. Unset, naga clamps every index, which is the default. `driver` uses the driver's buffer robustness instead, which tells you whether a bug is an index past a buffer. `off` removes the checks, so an index out of bounds can fault the GPU: use it only to measure. See [buffer robustness](rendering.md#buffer-robustness).
  - If you write your own indirect draws, test them in a debug build, or insert `VALIDATION` and `VALIDATION_INDIRECT_CALL` into `WgpuSettings::instance_flags`. Release builds on Horizon skip wgpu's indirect-call validation ([indirect-call validation](rendering.md#indirect-call-validation)).
  - Rule out memory with `bevy_horizon::memory_report()` ([out of memory](#wgpu-reports-out-of-memory)).

### A Bevy 0.18 scene stops the GPU in its first frames (known issue)

- **What you see:** the GPU stops within the first few frames. About 10–13 s after the first frame, the log shows a `channel N lost: …` line with `notification={type=8 (fifo idle timeout) info=0 status=65535}`. Then wgpu reports the device lost, and the final panic is in `Surface::present` or `Texture::create_view`.
- **Why:** unknown. It has been seen in one simple Bevy 0.18 test scene: a lit `StandardMaterial`, a non-HDR camera that tonemaps in the shader, a sun and a ground plane. It isn't memory: more than 2 GiB was free. bevy-horizon's Bevy 0.19 examples draw the same stock path without trouble.
- **Fix:** none is known yet. See [rendering.md](rendering.md).

### The FPS overlay doesn't show, or (−)+(+) quits the game

- **What you see:** no frame counter; or the buttons that show it also pause or quit your game.
- **Why:** the overlay is hidden at launch, and (+) is many games' pause or quit button.
- **Fix:** hold (−) and press (+), or set `HorizonFpsOverlayPlugin { visible_at_launch: true }`. In your game, ignore (+) while `bevy_horizon::fps_chord_held` returns true. The overlay needs Bevy 0.19, and Bevy's `bevy_ui_render`, `bevy_text` and `default_font` features or a font of your own. It covers the top-left corner. Status Monitor Overlay's FPS line isn't expected to count a Bevy game's frames. See [the FPS overlay](rendering.md#the-fps-overlay).

### GPU timings look too short

- **What you see:** GPU times from timestamp queries, wgpu's or Bevy's `RenderDiagnosticsPlugin`'s, about 1.63 times shorter than they should be.
- **Why:** stock NVK reports the timestamp period of a desktop GPU, whose timer counts nanoseconds. Each unit of the Switch's GPU timer is about 1.63 ns.
- **Fix:** rebuild NVK with `scripts/build_nvk.sh`, which applies `patches/mesa-switch/0002-nvk-switch-timestamp-period.patch`, then run `scripts/localize_nvk.sh`. See [rendering.md](rendering.md).

## No sound, or bad sound

### "No audio device found."

- **What you see:** `No audio device found.` in `log.txt`, and silence.
- **Why:** only bevy-horizon's cpal patches have a Horizon host, which plays through the audio renderer, [audren](glossary.md#audren): cpal 0.17.3 for Bevy 0.19, and cpal 0.15.3 for Bevy 0.18. Any other cpal, or a skipped patch, falls back to a host with no devices.
- **Fix:** build with `scripts/build_game.sh` and the right `BEVY`. `Cargo.lock` should have one cpal, the patch, with no `source = "registry+…"` line. An audio crate on another cpal version plays nothing. See [the audio host](input-and-audio.md#the-audio-host).

### The sound crackles or drops out

- **What you see:** gaps or crackle. With `BH_AUDIO_STATS=1`, an `[Audio]` line every 10 s whose `ran dry` count isn't 0 outside loading.
- **Why:** the audio thread refilled a buffer too late, so the renderer ran out of sound.
- **Fix:** use longer buffers with `BH_AUDIO_FRAMES`, 256 to 8192 frames, at the cost of latency. Unset, a Bevy 0.19 game gets 2048 and a Bevy 0.18 game 1024; try 2048 or 4096. `BH_AUDIO_PRIORITY` sets the audio thread's priority; the default, `0x2B`, runs it ahead of the game's threads at `0x3B`. On the console, [Skate 3 Rust Engine](https://github.com/JohnUzoka/skate-3-rust-engine), a full 3D Bevy 0.18 game, plays clean with 1024- and 2048-frame buffers. See [the audio host](input-and-audio.md#the-audio-host).

### The sound stops for good, and the game won't close

- **What you see:** the sound stops after a while and never comes back, and closing the audio device, or the game, waits forever.
- **Why:** that is devkitPro's SDL2 audio backend for the Switch. Its thread can miss the moment a buffer starts playing and then wait forever. bevy-horizon's cpal hosts don't use SDL2.
- **Fix:** play sound through bevy_audio, or cpal with the patches, not through SDL2. See [why not SDL2](input-and-audio.md#why-not-sdl2).

## Out of memory, or slow loading

### wgpu reports "Out of Memory"

- **What you see:** wgpu's out-of-memory error, which is NVK's `VK_ERROR_OUT_OF_HOST_MEMORY`, usually while a level loads. NVK may warn first: `GPU backing: the top-down region (<n> MiB) can't grow: …`. The `[Memory]` lines, or the panic report's `memory:` line, can still show plenty of memory free.
- **Why:** the Switch has no video memory. NVK takes every GPU block from the same heap as malloc, from the top down. Bevy uploads every new asset in the next frame, and an image is in the heap up to three times while it uploads. Every buffer of 8 MiB or more needs one unbroken run of the heap, and Bevy's mesh slabs grow up to 512 MiB. Free memory split into holes can't hold such a block.
- **Fix:**
  - Log `bevy_horizon::memory_report()` at your load stages. It walks the heap, so don't call it every frame.
  - Turn on `bevy_horizon`'s `render` feature (`render_0_18` for Bevy 0.18). It paces mesh uploads and sets Bevy's `RenderAssetBytesPerFrame`, and `HorizonUploadSettings` tunes both ([pacing uploads](platform-notes.md#pacing-uploads-the-render-feature)).
  - Cap Bevy's mesh slabs with `MeshAllocatorSettings` in the render app. Skate 3 Rust Engine uses 64 MiB, and a 64 MiB `RenderAssetBytesPerFrame` (`crates/skate-game/src/horizon.rs` on its `horizon-port` branch).
  - Give meshes and images the CPU never reads back `RenderAssetUsages::RENDER_WORLD`, so the main world keeps no copy.
  - If your own large allocations leave the heap in holes, try the `top_down_alloc` feature. It puts Rust blocks of 256 KiB and more in NVK's region at the top of the heap. It turns on `vulkan`, and it can't be combined with another `#[global_allocator]` ([NVK's region](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap)).
  - Launch with a game's memory, from sphaira's homebrew menu or a HOME [forwarder](glossary.md#forwarder), as the tested setup does: about 3.1 GiB of heap. An NRO started in [applet mode](glossary.md#applet-mode), from the album for example, gets less.
  - See [GPU memory comes from the same heap](platform-notes.md#gpu-memory-comes-from-the-same-heap).

### The first launch takes much longer to start

- **What you see:** the first launch takes far longer to reach its first frames than later ones.
- **Why:** NVK compiles each shader the first time it's used and caches the result on the SD card, under `sdmc:/.mesa/`.
- **Fix:** nothing to fix. Deleting `sdmc:/.mesa/` is safe; the next launch compiles again. Apart from that, a large 3D world can take a while: Skate 3 Rust Engine's takes about 40–70 s to load.

## It's slow, or it stutters

### The frame rate is low

- **What you see:** `[Heartbeat] frame N, X s after the first frame, Y fps on average`, with Y well under 60.
- **Why:** a large 3D game can be limited by the GPU: Skate 3 Rust Engine runs at about 30 fps, GPU-bound. The heartbeat's `work per frame` line times the main schedule only, without the render step or the runner's sleep, so low fps with little work there points at the GPU.
- **Fix:** [rendering.md](rendering.md) has the settings that help and how to time the GPU, and [logging-and-crash-reports.md](logging-and-crash-reports.md) has the CPU profiler (`BEVY_HORIZON_CPU_PROFILE`). Don't judge speed in an emulator.

### A hitch at a steady interval

- **What you see:** a stutter about once a minute, or every few seconds. The heartbeat's work line names the worst frame, `which spent … ms of it in flush_logs_system (…)`.
- **Why:** two things `flush_logs_system` does can take a frame's time.
  - The `[Memory]` line, once a minute, comes from newlib's `mallinfo`, which walks every free block with malloc's lock held. In a large game that takes about 41–50 ms.
  - Without the log writer thread, each line goes to the SD card on the thread that logs it, and the frame syncs `log.txt` once a second. The work line then ends `(heartbeat lines, [Memory], and writing and syncing log.txt)`.
- **Fix:**
  - `BEVY_HORIZON_MEMORY_EVERY=0` drops the `[Memory]` line, and the log says `[Heartbeat] no [Memory] line (BEVY_HORIZON_MEMORY_EVERY=0)`; any other whole number is the interval in seconds.
  - Call `bevy_horizon::init_switch_logging` at the start of `main`. It starts the writer, and the log says `[Log] log writer started: …`.
  - See [heartbeat lines](logging-and-crash-reports.md#heartbeat-lines) and [flushing and the cost of flushing](logging-and-crash-reports.md#flushing-and-the-cost-of-flushing).

### A game without a window keeps a core busy

- **What you see:** with Bevy's `bevy_window` feature off, the game's loop runs as fast as it can and keeps a CPU core busy.
- **Why:** without `bevy_window`, `DefaultPlugins` adds a `ScheduleRunnerPlugin` that loops without waiting, so `HorizonPlugin` doesn't add its 60 Hz one.
- **Fix:** replace it, as [captest](glossary.md#captest)'s `build_app` does (`examples/switch_captest/src/bevy_phase.rs`): `DefaultPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 60.0)))`. Without `bevy_window` there are no `AppLifecycle` or `WindowFocused` messages either; `build_app` adds them with `add_message`.

## HOME and controls

### HOME doesn't pause the game, or focus changes aren't reported

- **What you see:** `[Applet] focus handling left as it was (…): only an application can change it, so focus changes won't be reported`. Or the game gets no `AppLifecycle` messages when HOME is pressed.
- **Why:** in the first case the NRO runs as a library applet, started from the album for example, and only an application can change its [focus-handling mode](glossary.md#focus-handling-mode). In the second, the game has no `WindowPlugin`, and the messages exist only with it. Either way, the system still suspends the game while HOME is up.
- **Fix:** launch from sphaira with a game's memory, or from a HOME forwarder. In the game, pause gameplay and music on `AppLifecycle::WillSuspend` or `Suspended`, and resume on `WillResume` or `Running`. See [HOME and focus](lifecycle-and-exit.md#home-and-focus).

### The wrong button does the action

- **What you see:** a game that jumps on `GamepadButton::South` jumps on B, not A.
- **Why:** buttons map by position, as Bevy names them: Nintendo A is East, B is South, X is North and Y is West.
- **Fix:** map actions by position, or let players remap them. See [input-and-audio.md](input-and-audio.md).

### Released sticks still move things

- **What you see:** drift with the sticks at rest. captest's `stick-center` check warns.
- **Why:** the console's sticks rest at about 0.081. `Gamepad::left_stick()`, `right_stick()` and `get()` return the raw value: Bevy's `GamepadSettings` dead zone (±0.05 by default) applies only to `GamepadAxisChangedEvent`, and 0.081 is outside it anyway.
- **Fix:** apply your own dead zone to what you read from `Gamepad`: about 0.1, or a little more than `stick-center` reads on your console. If you read `GamepadAxisChangedEvent` instead, widen the dead zone in `GamepadSettings`. See [input-and-audio.md](input-and-audio.md).

### A second controller does nothing

- **What you see:** input from a second controller is ignored.
- **Why:** `HorizonInputPlugin` reads one player: player 1 and the [handheld](glossary.md#handheld-mode) controls, merged into one gamepad.
- **Fix:** none yet. See [input-and-audio.md](input-and-audio.md).

## Exit problems

### HOME after exit, with a crash report that lists only `hbl`

- **What you see:** `log.txt` ends as if the game exited normally, then HOME with a new crash report: only `hbl` in the module list, with Result `0xD401` (2001-0106).
- **Why:** after your NRO ends, sphaira's loader reads the next NRO into the heap your game used. If any page there is still locked, as an open [transfer memory](glossary.md#transfer-memory) or a thread stack leaves it, the kernel refuses the read and the loader crashes. `bevy_horizon`'s `userAppExit` checks the heap first, and if anything is still locked it exits through AM to HOME instead ([next entry](#exit-warn-home-instead-of-the-homebrew-menu)).
- **You can still hit it if** `bevy_horizon` isn't linked, so its `userAppExit` never runs. Or if the check itself failed: the log then ends `[Exit] heap check failed: svcQueryMemory returned an error; returning to the loader`.
- **Fix:** link `bevy_horizon`, then fix what the heap check lists, as below.

### `[Exit] WARN`: HOME instead of the homebrew menu

- **What you see:** the game goes to HOME, with no crash report, and `log.txt` ends like this, with one line for each locked block:

  ```text
  [Exit] heap check, after releasing services: 1 block, 2.20 MiB, would fail the loader's read:
  [Exit]   0x…-0x…, 2.20 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, as an open transfer memory or a thread stack leaves it
  [Exit] WARN: heap pages are still locked, so the loader would crash reading the next NRO (2001-0106); exiting through AM to HOME instead
  ```

- **Why:** something still holds heap pages: a service's transfer memory, or a thread's stack. At exit `bevy_horizon` stops Bevy's [task pools](glossary.md#task-pool), runs your release hooks, closes the threads that have ended, closes the sockets, RomFS and the GPU driver it opened, and waits up to a second for the pages to unlock.
- **Fix:** match the blocks to the lines before them.
  - **2.20 MiB:** the [bsd](glossary.md#bsd) sockets' transfer memory. `HorizonPlugin` closes the sockets it opened. If your game calls `socketInitialize` itself, without `HorizonPlugin`, it must call `socketExit` too.
  - **About 2 MiB per thread:** thread stacks, which stay locked until the thread ends and is closed. `[Exit] Bevy's task pools: N of M thread(s) still running after X ms, in a task that doesn't yield (or exit was called on a pool thread)` names a task that never gives its thread back. If the `[Exit] detached threads: …` line counts any `still running`, detached threads didn't end within a second. More than 256 detached threads at once leave stacks locked too, with an `[Exit] detached threads: WARN:` line. Join the threads you start, and have long-running ones stop when the app exits.
  - **The audio renderer:** `[Audio] exit: the audio thread didn't stop within 1000 ms; audren keeps its memory`, or, when the App was dropped, `[Audio] the audio thread didn't stop within 1000 ms; audren stays open`. The audio thread was stuck, for example in a callback in your code ([the audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit)).
  - **GPU memory:** an nvdrv line, [below](#the-exit-skips-nvexit-and-goes-to-home).
  - **A service you opened yourself,** such as the libnx console, `audren` directly, or anything built on `tmemCreate`: close it in a function you pass to `bevy_horizon::register_release_hook`. Hooks run at exit, newest first, before `bevy_horizon` closes its own services. Keep them quick, and don't panic in them. There are 8 slots, and the call returns `false` when they're full.
  - See [the userAppExit hook](lifecycle-and-exit.md#the-userappexit-hook) and [rules for services you add later](lifecycle-and-exit.md#rules-for-services-you-add-later).

### The exit skips `nvExit` and goes to HOME

- **What you see:** `[Exit] nvdrv: still open, and N heap blocks (X MiB) are still held for the GPU, so the GPU driver never shut down; skipping nvExit, which can wait forever in this state, and exiting through AM to HOME`.
- **Why:** the GPU driver didn't shut down, usually because the device was lost earlier. `nvExit`, which closes the [nv](glossary.md#nv) service, can wait forever in that state, so `bevy_horizon` skips it rather than hang.
- **Fix:** nothing at exit. Fix the device loss ([above](#the-device-is-lost-or-the-gpu-stops)).

### The log stops with no `[Exit]` lines

- **What you see:** HOME, no crash report, and `log.txt` simply stops.
- **Why:** closing the game from HOME ends the whole process. Without an [exit lock](glossary.md#exit-lock), which `bevy_horizon` doesn't take, `userAppExit` never runs, and lines still queued for the SD card are lost.
- **Fix:** nothing to fix, but save when the game goes to the background (`AppLifecycle::WillSuspend`), since a player can close it from HOME at any time. Save before you send `AppExit`, too: the runner drops the App, with all its resources, inside `App::run` ([quitting](lifecycle-and-exit.md#quitting-the-app-drop)).

## File and asset problems

### File types and sizes are wrong

- **What you see:** `is_file()` is false for a file, `len()` returns a number the size of a timestamp, `create_dir_all` fails on a folder that exists, or `read_dir` names lose their first two letters.
- **Why:** std reads newlib's `struct stat` and `struct dirent` through crates.io libc's layouts, which don't match devkitA64's newlib. The C [ABI](glossary.md#abi) has to match exactly.
- **Fix:** keep the libc patch in `.cargo/config.toml`; the build scripts check that std uses it. After any change to `patches/libc`, run `scripts/check_libc_abi.sh`. It compares 348 type sizes, struct field offsets and constants between devkitA64's headers and Rust's libc, and should report 0 mismatches. See [platform-notes.md](platform-notes.md).

### `fs::copy`, `fs::rename` or `fs::remove_dir` fails

- **What you see:** `fs::copy` fails with "Function not implemented" and leaves an empty file. `fs::rename` onto an existing file fails with `EEXIST`. `fs::remove_dir` on a folder that isn't empty fails with "I/O error", not `DirectoryNotEmpty`.
- **Why:** Horizon's file systems have no permission bits and won't rename onto an existing file, and libnx reports most file-system errors as `EIO`. `bevy_horizon` wraps `fchmod`, `rename` and `rmdir` so std gets what it expects, through `--wrap` flags in the target specification.
- **You can still hit it if** your own target specification lacks `--wrap=fchmod`, `--wrap=rename` and `--wrap=rmdir`.
- **Note:** a rename onto an existing file isn't atomic here. If the game dies halfway, the old file may be left as `<name>.bh-replaced`. See [toolchain.md](toolchain.md).

### An asset or file isn't found

- **What you see:** a load fails as not found, or a path comes out as `<root>/sdmc:/…`.
- **Why:** Rust counts only a leading `/` as absolute, so `Path::new("sdmc:/…").is_absolute()` is false and `base.join("sdmc:/…")` appends. bevy_asset's patch keeps such asset roots as they are, but your own path code doesn't.
- **Fix:** use relative paths. Bevy loads assets from the `assets` folder beside the NRO, and the working directory is the NRO's folder. The `[Assets] BEVY_ASSET_ROOT = …` line in `log.txt` says where Bevy looks. Also:
  - The SD card is usually [FAT32](glossary.md#fat32), which ignores case. A name with the wrong case loads on the console and then fails on Linux.
  - Keep each file under 4 GiB, and don't put tens of thousands of files in one folder.
  - Keep file names ASCII: captest's `fs-unicode-name` check warns on the console.
  - See [platform-notes.md](platform-notes.md).

### Image loads stay `Loading` forever

- **What you see:** in a Bevy 0.19 game without `bevy_render`, every image load stays `Loading`, with no error.
- **Why:** `bevy_image` only pre-registers its image loader, and `bevy_render` registers the real one. Without a renderer, every load waits for a loader that never comes.
- **Fix:** register one yourself, as captest's `build_app` does: `app.register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()));`.

### A crate's startup registrations are missing

- **What you see:** things a crate registers before `main` never arrive, for example types from Bevy's `reflect_auto_register`, and nothing reports an error.
- **Why:** crates such as `inventory` put their constructors in `.init_array` only on the targets they list, and libnx runs only what's there. inventory 0.3.24 doesn't list Horizon; bevy-horizon's patch adds it.
- **Fix:** for any other crate that registers at startup, search it for `link_section = ".init_array"` and the `target_os` list around it, and add Horizon. See [platform-notes.md](platform-notes.md).

## Networking

### Opening sockets fails with `ENOBUFS`

- **What you see:** sockets open until one fails with `ENOBUFS`, after about 42 UDP sockets.
- **Why:** libnx's default socket buffers, which `HorizonPlugin` uses, run out.
- **Fix:** use fewer sockets. No game has used networking on the console yet; captest's `net-*` checks are the reference. See [testing.md](testing.md).

## A setting has no effect

### An environment variable does nothing

- **What you see:** you set `BH_AUDIO_FRAMES`, `BH_ROBUST_BUFFER_ACCESS` or another variable, and the log shows no change.
- **Why:** Horizon has no shell and no launch arguments, and each variable is read once: `bevy_horizon`'s own when `HorizonPlugin` is added, wgpu-hal's when Bevy creates the GPU instance, device and first textures, and the cpal host's when a stream opens. `BEVY_HORIZON_NVK_LIB` is read at build time.
- **Fix:** set them with `std::env::set_var` at the start of `main`, before you build the App. Skate 3 Rust Engine reads `env NAME=VALUE` lines from an `options.txt` beside its NRO, so a setting can change without a rebuild. [porting-a-bevy-game.md](porting-a-bevy-game.md) lists the variables.

## Emulator-only problems

### Stock Ryujinx can't run a Vulkan build

- **What you see:** a build with the `vulkan` feature gets no GPU device in stock Ryujinx (Ryubing 1.3.3), so it can't draw.
- **Why:** NVK runs into emulator bugs. bevy-horizon is tested in a Ryujinx with local fixes that aren't published; [rendering.md](rendering.md#what-the-emulator-gets-wrong) lists the bugs.
- **Fix:** test Vulkan builds on the console. The non-Vulkan captest runs in stock Ryujinx. Other emulators are untested.

### The emulator and the console disagree

- **Speed:** Ryujinx draws through [lavapipe](glossary.md#lavapipe), on the PC's CPU, so its frame rate says nothing about the console.
- **Behaviour:** there are no `[Boot]` lines, since it starts the NRO without a loader. Focus is always `InFocus`, and there's no HOME button. Several `net-*` checks fail for emulator reasons. Deleting a folder that isn't empty crashes Ryujinx. Texture readback is wrong past the first MiB.
- See [testing.md](testing.md#what-the-emulator-gets-wrong).

## General rules

- **Read the module list first.** If your NRO isn't listed, your ELF can't explain the addresses.
- **Log from before `main`.** C constructors and Rust's runtime setup run first, and they're part of the port; the `[Boot]` lines cover them.
- **A panic isn't a crash report.** It goes to `panic.txt`, and the game exits as it would normally.
- **A steady 60 fps proves little.** The runner sleeps away the spare time; read the heartbeat's work per frame.
- **Check C layouts with code, not by eye.** Rust's libc must match devkitA64's newlib exactly.
- **Check that every patch is used.** Cargo only warns.
- **Give back every locked heap page before the loader runs.** Sockets, audio, the GPU and thread stacks all count.
- **Trust the console over the emulator.**
- **Know what hasn't been tested:** [docked mode](glossary.md#docked-mode), the Bevy 0.19 audio host on the console, and networking in a game. A problem there may be a new one.
