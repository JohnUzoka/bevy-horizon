# Horizon: platform notes

This page collects the facts about the Switch and its OS, [Horizon](glossary.md#horizon), that a Rust or [Bevy](glossary.md#bevy) game runs into: memory, GPU memory, threads, C struct layouts, constructors, the [loaders](glossary.md#loader) that start [homebrew](glossary.md#homebrew), the SD card, how a program ends, and [result codes](glossary.md#result-code). Read it when a rule in the other guides needs its reason, or when the console does something a desktop build doesn't. Each section starts with what to do; internals for contributors come after.

## In short

- **Start your game with a game's memory.** Started through [Atmosphère](glossary.md#atmosphère)'s [title override](glossary.md#title-override) (hold R while starting a game) or a [forwarder](glossary.md#forwarder) on the [HOME menu](glossary.md#home-menu), an [NRO](glossary.md#nro) gets a malloc [heap](glossary.md#heap) of about 3.1 GiB. Started from the Album, it runs in [applet mode](glossary.md#applet-mode), with far less. The first `[Memory]` line in `log.txt` says which you got.
- **The GPU shares that heap.** With the `vulkan` feature, every texture, buffer and upload comes out of it. Turn on the `render` feature, cap Bevy's mesh slabs, and use `RenderAssetUsages::RENDER_WORLD` for data the CPU never reads back ([GPU memory comes from the same heap](#gpu-memory-comes-from-the-same-heap)).
- **If your game loads large assets, turn on `top_down_alloc`,** so freed level data doesn't leave holes too small for the GPU's next block.
- **Thread-locals just work.** bevy_horizon sets [`TPIDR_EL0`](glossary.md#tpidr_el0), the register Rust's [thread-locals](glossary.md#thread-local-storage) use, before `main` and on every new thread.
- **Join your threads, and close the [services](glossary.md#service) you open.** Thread stacks and memory lent to services lock heap pages, and a locked page at exit crashes the loader. bevy_horizon cleans up its own and checks the heap at exit.
- **Never spin-wait.** A thread of lower [priority](glossary.md#thread-priority) gets no turn while one of higher priority is ready on its [core](glossary.md#cpu-core).
- **Keep the `libc` patch.** Without it, Rust reads file sizes and types wrong, with no error.
- **Paths:** relative paths start at the NRO's folder, and `sdmc:/…` isn't absolute to Rust's `Path`. The SD card is usually [FAT32](glossary.md#fat32): files under 4 GiB, case-insensitive names, and syncs slow enough to cost frames.
- **Tested on a Switch Lite**, in [handheld mode](glossary.md#handheld-mode), under Atmosphère. [Docked mode](glossary.md#docked-mode) is untested ([status](#what-is-proven-expected-and-untested)).

## Modes and screen

| Mode | Resolution | Status |
|---|---:|---|
| Handheld | 1280×720 | Tested on a Switch Lite. |
| Docked | 1920×1080 | The code exists; untested, because a Switch Lite can't dock. |

Docked mode draws 2.25 times the pixels. `HorizonPlugin`'s display system (`crates/bevy_horizon/src/display.rs`, `monitor_dock_status`) polls [libnx](glossary.md#libnx)'s `appletGetOperationMode()` every frame. When the mode changes, it updates the `HorizonDisplayMode` resource, resizes the primary window and logs `Switch display mode changed to Docked: 1920x1080`.

- **Start your window at 1280×720**, as `examples/hello_switch` does. The resolution changes only when the mode does.
- **Read `HorizonDisplayMode`** if settings such as resolution scale or shadow quality should differ between the modes.
- **Tune for handheld first.** Large 3D games are already GPU-bound there ([rendering.md](rendering.md)).

## Memory budget and loader heap

One heap holds everything: libnx, Rust, Bevy, thread stacks, memory lent to services, your assets and, with the `vulkan` feature, every GPU resource. The homebrew loader sets it up before your NRO starts and passes it on through libnx's heap override. There is no separate video memory and no swap.

### Launch mode decides the budget

| Started as | How | Malloc heap |
|---|---|---|
| An application, with a game's memory | Hold R while starting any game (Atmosphère's title override) and pick the NRO in the homebrew menu, or start it from a HOME-menu forwarder made with [sphaira](glossary.md#sphaira). | About 3.1 GiB. |
| A library [applet](glossary.md#applet) (applet mode) | Start the homebrew menu from the Album. | Far less; not measured. |

Plan on the application launch, and tell your players to start the game that way. bevy_horizon logs which you got before `main` (`crates/bevy_horizon/src/memory.rs`, `boot_report`). On the tested Switch Lite:

```text
[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
[Memory] process: 3189.0 MiB in total, 3185.0 MiB used (svcGetInfo; "used" already counts the whole malloc heap below)
[Memory] malloc heap: 3132.7 MiB, set up by the homebrew loader; malloc has … MiB in use, … MiB taken from the 3132.7 MiB heap; …
```

In applet mode, the first line reads `applet type 2 = LibraryApplet: applet mode, e.g. started from the Album, so it gets an applet's memory`.

- **The NRO's image comes out of the heap.** The loader reads the NRO's code and data into the start of its heap, so an NRO that links [NVK](glossary.md#nvk) gets about 3,114 MiB instead of 3,132.7. [RomFS](glossary.md#romfs) files stay on the SD card.
- **Gameplay recording holds back 96 MiB.** When the [NACP](glossary.md#nacp) of the title the NRO runs as (the forwarder's, or that of the game you held R on) asks for automatic [gameplay recording](glossary.md#gameplay-recording) (`video_capture = 2`), the system keeps 96 MiB for it, which is why the total is 3,189.0 MiB rather than about 3,285. sphaira's loader leaves those 96 MiB out of its heap either way. Stock [hbloader](glossary.md#hbloader) leaves them out only when the NACP asks.

### Reading the memory lines

- **Once a minute**, the [heartbeat](logging-and-crash-reports.md#heartbeat-lines) logs a `[Memory]` line with the figures below. `BEVY_HORIZON_MEMORY_EVERY` sets the interval in seconds, and `0` turns the line off. Set it with `std::env::set_var` at the start of `main`; it is read when `HorizonPlugin` is added. Each line costs a hitch, because [newlib](glossary.md#newlib)'s `mallinfo` walks every free block under malloc's lock: about 41 ms in a full 3D game after loading. Skate 3 Rust Engine turns the line off in its release builds.
- **From your code**, `bevy_horizon::memory_report()` (Horizon only) returns the same text, plus how much of the heap is mapped for the GPU. It walks the heap, so call it at load stages, not every frame.
- **After a [panic](glossary.md#panic)**, `log.txt` and `panic.txt` carry a `memory:` line with the same figures.

| Text | What it counts |
|---|---|
| `malloc has A MiB in use` | What Rust and C code have allocated and not freed. |
| `B MiB taken from the H MiB heap` | How far malloc has grown, freed blocks included. `H` is the whole heap. |
| `T MiB free at the top` | Room malloc hasn't taken yet, below the heap's end or NVK's region. |
| `O MiB free in holes below it` | Freed blocks inside what malloc has taken. An allocation needs one hole of its size, so a large figure here doesn't mean a large block fits. |
| `the top-down region above the top: R MiB, U MiB of it in GPU blocks, P MiB at the most` | NVK's region: its size, what its blocks hold now, and their peak. With `top_down_alloc`, the middle reads `U MiB of it in blocks (G MiB GPU, R MiB Rust)`. |
| `N GPU blocks the region couldn't place came from malloc` | GPU blocks that fell back to malloc's free space. |

**On Bevy 0.18, a GPU ledger.** bevy-horizon's Bevy 0.18 wgpu-hal patch logs `[GPU]` lines under the target `bevy_horizon::gpu`: each resource of 4 MiB or more, each new high of [wgpu](glossary.md#wgpu)'s total in steps of 64 MiB, and the size and tally of any Vulkan allocation that fails.

```text
[GPU] X MiB for <label, size, format and usage>
[GPU] a new high for wgpu's buffers and textures: X MiB in all; textures: X MiB in N; …
```

The GPU figures in `[Memory]` are larger: they also count allocator chunks not yet handed out, NVK's own memory, and the freed blocks NVK keeps for reuse.

### GPU memory comes from the same heap

With the `vulkan` feature, NVK, [Mesa](glossary.md#mesa)'s [Vulkan](glossary.md#vulkan) driver, takes every Vulkan allocation from the malloc heap. Textures, buffers and wgpu's staging copies count against the same 3.1 GiB as your game's data. What follows for a game:

- **Pace uploads with the `render` feature.** Bevy uploads every new image and mesh in the next frame, and an image is in the heap up to three times while it uploads: its pixels, wgpu's staging copy and the GPU texture. Bevy's `RenderAssetBytesPerFrame` never splits an asset and doesn't hold back meshes (bevy_render 0.18.1, `allocate_and_free_meshes`). The `render` feature paces both ([below](#pacing-uploads-the-render-feature)).
- **Cap Bevy's mesh slabs.** Meshes that share a vertex layout share one buffer, a slab, that grows by half again each time it fills, up to 512 MiB. The new buffer is created while the old one still holds the data (bevy_render 0.18.1, `reallocate_slab`), so after loading has split the heap, a big slab can ask for hundreds of MiB in one piece.
- **Free the main-world copies.** With the default `RenderAssetUsages`, a mesh or image keeps its data in the main world after upload. `RenderAssetUsages::RENDER_WORLD` frees it once it is on the GPU; use it for everything the CPU never reads back. Frustum culling still works.
- **Load morph-target meshes when you need them, and share meshes.** Bevy's glTF loader gives each primitive with morph targets its own 3D texture, often a few texels wide and over a thousand rows tall. NVK pads it to whole tiles 16 layers deep (mesa-switch `src/nouveau/nil/tiling.rs`), and the allocator rounds the block up to a power of two: a 498-vertex mesh with 22 targets, 385 KiB of data, takes a 4 MiB block.
- **Expect each resource of 8 MiB or more to need one free run of the heap.** Bevy 0.18's allocator, gpu-alloc 0.6.2, gives it its own Vulkan memory object, which NVK backs with one contiguous block. Smaller resources share blocks of 8–64 MiB, or 4–32 MiB for memory the CPU maps; bevy-horizon's wgpu-hal patches pick these sizes instead of wgpu's default 128–256 MiB ([the crate patches](toolchain.md#the-crate-patches)).

To cap the slabs, insert `MeshAllocatorSettings` into the render sub-app after `DefaultPlugins`. [Skate 3 Rust Engine](https://github.com/JohnUzoka/skate-3-rust-engine), a full 3D Bevy 0.18 game on bevy-horizon (branch `horizon-port`, Switch setup in `crates/skate-game/src/horizon.rs`), uses 64 MiB slabs and gives each mesh of 32 MiB or more its own buffer:

```rust
// Bevy 0.18
if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
    render_app.insert_resource(bevy::render::mesh::allocator::MeshAllocatorSettings {
        max_slab_size: 64 << 20,
        large_threshold: 32 << 20,
        ..Default::default()
    });
}
```

In Bevy 0.19 the fields moved into `slab_allocator_settings`. This form hasn't run on the console yet:

```rust
// Bevy 0.19, in the same `if let`
render_app.insert_resource(bevy::render::mesh::allocator::MeshAllocatorSettings {
    slab_allocator_settings: bevy::render::slab_allocator::SlabAllocatorSettings {
        max_slab_size: 64 << 20,
        large_threshold: 32 << 20,
        ..Default::default()
    },
    ..Default::default()
});
```

### Pacing uploads: the `render` feature

bevy_horizon's `render` feature (`render_0_18` for Bevy 0.18) adds `HorizonUploadPlugin` (`crates/bevy_horizon/src/uploads.rs`), configured by `HorizonUploadSettings`:

| Field | Default | What it does |
|---|---:|---|
| `mesh_bytes_per_frame` | 32 MiB | The most new mesh data handed to Bevy's mesh allocator in a frame. The rest waits, oldest first; at least one mesh goes each frame. A changed mesh already on the GPU is never held. |
| `reclaim_after_bytes` | 8 MiB | After a frame that uploaded this much, wait up to 10 s for the GPU, so wgpu frees that frame's staging buffers before the next frame makes more. |
| `default_asset_bytes_per_frame` | 64 MiB | Bevy's `RenderAssetBytesPerFrame`, if the game hasn't set one. For no limit, set `usize::MAX`. |

`None` turns a field off. Insert your own settings before `App::run`:

```rust
app.insert_resource(bevy_horizon::HorizonUploadSettings {
    mesh_bytes_per_frame: Some(16 << 20),
    ..Default::default()
});
```

The entities that use a held mesh draw nothing until it reaches the GPU, as if it were still loading. Then the plugin marks their `Mesh3d` and `Mesh2d` components as changed, so Bevy extracts them again. The log:

```text
[Upload] GPU uploads a frame: new meshes at most 32.0 MiB, other render assets at most 64.0 MiB (RenderAssetBytesPerFrame); waits for the GPU after frames that upload 8.0 MiB or more
[Upload] holding <n> new meshes (<size> MiB) for later frames: Bevy's mesh allocator gets at most 32.0 MiB of new meshes a frame (HorizonUploadSettings)
[Upload] <n> held meshes reached the GPU; marked the <m> entities that use them as changed
[Upload] the last held mesh went to the GPU: <n> meshes (<size> MiB) waited, over <n> frames (<time> s)
[Upload] waited for the GPU after <n> frames that uploaded <size> MiB between them, <time> ms in all, so wgpu could free their staging buffers
```

The first comes once at startup, the next three follow a round of held meshes, and the last sums up the GPU waits after 60 frames without one. A failed wait logs `[Upload] waiting for the GPU to free upload buffers failed: …`. Pacing costs time, not memory: a level's meshes arrive over more frames. It doesn't stop the heap from fragmenting; the next two sections do that.

### NVK takes GPU memory from the top of the heap

Upstream mesa-switch's NVK `memalign`s each GPU block. After a game frees its loading data, the next large block can fail with plenty of memory free: in a full 3D game in the [emulator](glossary.md#emulator), a 16 MiB block failed with about 500 MiB free in holes. bevy-horizon's NVK build takes GPU blocks from the top of the heap down instead, while malloc grows up from the bottom. It is on by default (`patches/mesa-switch/0001-top-down-gpu-backing.patch`, applied by `scripts/build_nvk.sh`; see [linking NVK](toolchain.md#linking-nvk-the-vulkan-feature)).

| Variable | Default | What it does |
|---|---|---|
| `NOUVEAU_HORIZON_TOP_DOWN` | on | `false` turns the region off, so every GPU block comes from `memalign`. Read at NVK's first allocation. |
| `NOUVEAU_HORIZON_BO_CACHE_MB` | 128 | How much freed GPU memory NVK keeps for reuse, in blocks of up to 32 MiB; `0` turns the cache off. Read when the device is created. |

Set them with `std::env::set_var` at the start of `main`. The line to look for is this warning, logged the first time a GPU block doesn't fit:

```text
GPU backing: the top-down region (N MiB) can't grow: N MiB lie between it and the CPU heap, and N MiB stay for the CPU. This N KiB block, and later ones that don't fit, come from malloc's free space instead
```

It means malloc's part of the heap is in holes, and [`top_down_alloc`](#large-rust-blocks-in-the-region-the-top_down_alloc-feature) is the fix.

**Internals.** [devkitA64](glossary.md#devkita64)'s `_sbrk_r` won't move malloc's break past `fake_heap_end`, and rereads it on every call (devkitPro newlib `57bd8e1`, `libgloss/libsysbase/sbrk.c`). NVK lowers `fake_heap_end` under newlib's malloc lock to take memory, stops 16 MiB above malloc's break, tracks 64 KiB granules in a static bitmap, and raises `fake_heap_end` again once the region's bottom is free. A block that doesn't fit comes from `memalign`; a failed allocation empties NVK's reuse cache and retries. bevy_horizon finds the heap's end from the loader's heap override or with `svcQueryMemory` instead (`crates/bevy_horizon/src/memory.rs`, `heap_end`). Mesa's `os_get_available_system_memory` (mesa-switch `src/util/os_misc.c`) counts only up to `fake_heap_end`, which matters only for Vulkan memory budgets, and wgpu doesn't ask for those by default.

### Large Rust blocks in the region: the `top_down_alloc` feature

A game that loads its level in large blocks among small ones, then frees most of them, leaves malloc's part of the heap in holes the region can't grow past. In a full 3D game in the emulator, with the region on, a 64 MiB GPU block failed: 21 MiB lay between the region and malloc's break, and about 500 MiB was free in holes. A desktop malloc would `mmap` large blocks so each frees whole, but newlib's malloc has no `mmap` (`HAVE_MMAP` is 0).

The `top_down_alloc` feature, which turns on `vulkan`, installs a global allocator that uses NVK's region instead (`crates/bevy_horizon/src/allocator.rs`, `HorizonAllocator`). Turn it on for any game that loads large assets:

```toml
# Bevy 0.19
bevy_horizon = { path = "…/bevy-horizon/crates/bevy_horizon", features = ["vulkan", "render", "top_down_alloc"] }
# Bevy 0.18, as Skate 3 Rust Engine sets it
bevy_horizon = { path = "…/bevy-horizon/crates/bevy_horizon", default-features = false, features = ["bevy_0_18", "vulkan", "render_0_18", "top_down_alloc"] }
```

- **Blocks of 256 KiB or more go in the region**, in whole 64 KiB granules, through `nouveau_horizon_region_alloc` and `nouveau_horizon_region_free`, which bevy-horizon's NVK patch exports. Smaller ones go to newlib's malloc. Freed granules can hold GPU blocks, and at the region's bottom they go back to malloc.
- **Each side backs up the other.** A large block the region can't place goes to malloc, and a small block malloc can't place takes a granule of the region.
- **No lock is needed to tell them apart.** malloc's blocks lie below `fake_heap_end`, and the region's at or above it.

With it, the same game's malloc holes stayed under about 26 MiB from the first frame on, against about 500 MiB without. The `[Memory]` text adds:

```text
Rust's malloc blocks: X MiB of 256 KiB and up, Y MiB of 64-256 KiB
N large blocks the region couldn't place went to malloc
N small blocks malloc couldn't place went to the region
```

**Costs.** It is your program's `#[global_allocator]`, so a game that sets its own can't use it. A large block can waste up to 64 KiB to rounding, and each large allocation and free takes the region's lock.

### Small blocks: a cache in front of malloc

newlib's malloc has one lock for the whole heap and no fast bins, so Bevy's threads queue on it. In a full 3D game's CPU profile on the console, a fifth of the samples were in the allocator, and the main and render threads waited about 9 and 6 ms a frame. So `top_down_alloc` also caches freed blocks of up to 8 KiB, aligned to at most 16 bytes, and hands them back without malloc's lock (`crates/bevy_horizon/src/allocator/small_blocks.rs`, `Cache`). In a full 3D game in the emulator, it served about 96% of those allocations during play, and far fewer while loading.

The cache is on whenever `top_down_alloc` is. `BH_ALLOC_CACHE=0`, read when `HorizonPlugin` is added, turns it off, returns its blocks to malloc and logs `BH_ALLOC_CACHE=0: the small-block cache is off`. The `[Memory]` text then ends with `the small-block cache is off (BH_ALLOC_CACHE=0)`; otherwise it ends with the cache's figures:

```text
the small-block cache holds X MiB; of the allocations it takes, it served N and malloc N (N with the cache busy), and of the frees, N went to malloc with the cache busy and N for blocks under their class's size; its depot took N magazines and gave out N, and N went back to malloc
```

If malloc ever ran out, it adds `malloc ran out N times, and took back the cache's blocks`.

**Internals.** Blocks fall into 32 size classes (16–128 bytes in steps of 16, then four to each doubling, up to 8 KiB) and are allocated at their class's size; a freed block smaller than that, such as one allocated while the cache was off, goes to `free`. The design is Bonwick and Adams's magazines: 8 shards keep two magazines of free blocks per class, picked by the address of the thread's thread-local region (Horizon places those 0x200 bytes apart; Atmosphère `svc_select_thread_local_region.hpp`), and a depot of up to 4 full magazines per class moves blocks between threads. Allocations and frees only try its locks, never wait for them, because spinning could starve a lower-priority holder ([cores and priorities](#cores-and-priorities)); turning the cache off sleeps between tries instead, for up to about 10 ms. It holds at most about 4.3 MiB and gives its blocks back when malloc runs out, so it can't cause an out-of-memory panic.

A host stress test (16 threads, millions of random operations, the cache turned off and on meanwhile) is clean under AddressSanitizer and ThreadSanitizer. Skate 3 Rust Engine's release build runs with the cache on the console; its effect on frame rate hasn't been measured.

## Thread-locals and `TPIDR_EL0`

There's nothing to do here: bevy_horizon handles it. Rust's thread-local storage reads its base from `TPIDR_EL0`. libnx keeps its thread pointer in `ThreadVars.tls_tp` instead and leaves `TPIDR_EL0` at 0, so a plain Rust program faults near address 0x700 before its first log line.

bevy_horizon copies libnx's value into the register (`crates/bevy_horizon/src/compat.rs`, `set_thread_pointer`). On the main thread it does this in [`userAppInit`](glossary.md#userappinit), which libnx calls before C constructors, Rust's runtime and `main`. On every new thread it does it through `--wrap=threadCreate` in the [target specification](glossary.md#target-specification): each thread starts in `thread_start`, which sets the register first.

```text
[Boot] TPIDR_EL0 = 0x… (Rust's thread-local base, copied from libnx ThreadVars.tls_tp)
[Boot] TLS template at nro+0x…, align …: Rust and libnx agree on thread-local addresses
[Thread] #N: running on core …, ideal core …, may run on cores 0b…; TPIDR_EL0 = libnx tls_tp = 0x…, so thread_local! works
```

A `[Thread]` line that ends `MISMATCH, thread_local! will fault` means a thread was started some other way. `MISALIGNED` in the TLS template line means a changed linker script moved Rust's thread-locals. `crash.log`'s register dump compares the two values too.

## Threads and stacks

A thread's stack is heap memory that libnx maps with `svcMapMemory`, which locks the original pages until `threadClose`. A thread still open at exit fails the exit check ([IPC buffers and transfer memory](#ipc-buffers-and-transfer-memory)). std's threads get 2 MiB stacks.

| What you do | What happens on Horizon | At exit |
|---|---|---|
| `JoinHandle::join()` | The thread is closed and its stack freed. | Nothing left. Prefer this. |
| Drop the `JoinHandle` (detach) | newlib's `pthread_detach` needs `__syscall_thread_detach`, which libnx lacks. bevy_horizon provides it, and closes each detached thread once it has exited. | bevy_horizon closes the rest, waiting up to 1 s. A thread still running keeps its stack locked. |
| Bevy's [task pools](glossary.md#task-pool) | Nothing joins them; bevy-horizon's `bevy_tasks` patch adds `shut_down_global_task_pools`. | Stopped and joined, waiting up to 500 ms. A task that never yields keeps its thread. |
| `blocking`'s pool (behind `async-fs`) | Up to 500 threads with 2 MiB stacks, and an 8 MiB pipe per `Unblock`. Idle threads exit after 500 ms. | Closed once idle. bevy-horizon's bevy_asset patches keep Bevy's own file reads off it (`sync_file_asset`). |

```text
[Threads] N created since launch; detached: N closed after exiting, N still running
[Exit] Bevy's task pools: stopped and joined their N thread(s) in … ms
[Exit] detached threads: closed N now and N while the app ran (N threads were created in all); N still running after … ms
```

The first comes with the heartbeat. If a pool task doesn't yield, the second reads `N of M thread(s) still running after … ms, in a task that doesn't yield (or exit was called on a pool thread)`. Each new thread logs two `[Thread]` lines, each a separate SD-card write; `bevy_horizon::set_thread_logging(false)` turns them off.

### Cores and priorities

The CPU has four cores. Homebrew may use all four ([core mask](glossary.md#core-mask) 0–3), and core 3 also runs system work.

- **Priorities.** A lower number runs first. The main thread runs at 0x2C (44) and std's threads at 0x3B (59). The audio host's thread runs at 0x2B (43), set by `BH_AUDIO_PRIORITY` ([the audio host](input-and-audio.md#the-audio-host)).
- **No time-slicing down.** Horizon runs each core's highest-priority ready thread (Atmosphère `kern_k_scheduler.cpp`), so a main thread that spins waiting for a worker on its core starves that worker for good. Wait with a channel, `Mutex`, `Condvar`, `thread::park` or `thread::sleep`, never with a loop on an atomic.
- **Where threads run.** By default, std's threads may run on cores 0–3, with no preferred core.

`bevy_horizon::set_thread_core_policy` (`crates/bevy_horizon/src/compat.rs`, `ThreadCorePolicy`) changes that for every thread created afterwards with libnx's default core, including Bevy's task pools and C libraries' threads. Call it at the start of `main`, before `App::new()`:

```rust
#[cfg(target_os = "horizon")]
bevy_horizon::set_thread_core_policy(bevy_horizon::ThreadCorePolicy::AnyCore);
```

| `ThreadCorePolicy` | New threads |
|---|---|
| `Default` | libnx's behaviour, above. |
| `AnyCore` | May run on cores 0–2, with no preferred core. |
| `Spread` | Start on the next of cores 1, 2, 0 in turn; the kernel may move them. |
| `Pinned` | Fixed to the next of cores 1, 2, 0 in turn. |

Each `[Thread]` line shows where its thread runs, and [captest](glossary.md#captest)'s `cores-*` checks time each policy on the console.

## C struct layouts and the libc patch

Rust's `std` reads newlib's C structs through the `libc` crate. crates.io's `libc` gives Horizon the 3DS's layouts, so the answers come out wrong, with no error. bevy-horizon's `patches/libc` fixes them ([the crate patches](toolchain.md#the-crate-patches)). Without it:

| Struct or value | What Rust read | What you'd see |
|---|---|---|
| `struct stat.st_mode` | `st_nlink` and `st_uid` | `is_file()` and `is_dir()` are false, so `create_dir_all` fails on a folder that exists. |
| `struct stat.st_size` | A timestamp | A 45-byte file looks about 1.79 billion bytes long. |
| `struct dirent.d_name` | Two bytes too late | `read_dir` lists `save.txt` as `ve.txt`. |
| `pthread_condattr_t` | 4 bytes, where C writes 8 | A lock's slow path can overwrite the stack next to it. |

`scripts/check_libc_abi.sh`, run in the toolchain image, compares 348 sizes, alignments, field offsets and constants between devkitA64's GCC and Rust, and reports 0 mismatches; run it after any change to `patches/libc`. [`-Z build-std`](glossary.md#build-std) builds `std` as a workspace of its own, so `.cargo/config.toml` patches `libc` under `[patch.crates-io]`. The build scripts stop if `std` would still use crates.io's `libc` ([how to check that a patch is really applied](toolchain.md#how-to-check-that-a-patch-is-really-applied)).

## Constructors before `main`

Some crates register things before `main` from constructors, functions the linker lists in the [ELF](glossary.md#elf) section `.init_array`. Bevy's `reflect_auto_register` works this way, through the `inventory` crate. libnx runs them: `__libnx_init` ends with newlib's `__libc_init_array`, after the services are up and before `main` (libnx `nx/source/runtime/init.c`).

But each crate decides from its own `target_os` list whether to put its constructor there. `inventory` 0.3.24 doesn't list Horizon, so every `submit!` would be silently lost; bevy-horizon's patch adds it (`patches/inventory/src/lib.rs`). For any other crate that registers at startup, such as `ctor`, search it for `link_section = ".init_array"` and the `target_os` list around it.

## IPC buffers and transfer memory

When you pass a buffer to a service ([IPC](glossary.md#ipc)), the kernel checks every page first. Each must be readable and writable by your process and free of disallowed attributes, or the request fails with `InvalidCurrentMemory` (2001-0106). A [transfer memory](glossary.md#transfer-memory), heap memory lent to another process, fails that check while it is lent: its pages have permission `0x0` and the locked attribute `0x1`. libnx's sockets lend 2.2 MiB to the [bsd](glossary.md#bsd) service.

This matters at exit. sphaira's loader reads the next NRO into its whole heap with one `fsFileRead`, so one locked page anywhere fails the read with 2001-0106, and the loader crashes. So bevy_horizon closes what it opens (sockets, RomFS, the GPU driver) in [`userAppExit`](glossary.md#userappexit), then runs a [heap check](glossary.md#heap-check) by the kernel's rule (`crates/bevy_horizon/src/memory.rs`, `check_heap_for_exit`): every heap block needs permission `0x3`, and no attributes other than `0x2`, `0x4` and `0x10`. It logs `[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it`, or lists up to eight locked blocks and leaves through HOME instead:

```text
[Exit]   0x…-0x…, X MiB: memory type 0x…, permission 0x…, attributes 0x…: locked with no access, as an open transfer memory or a thread stack leaves it
[Exit] WARN: heap pages are still locked, so the loader would crash reading the next NRO (2001-0106); exiting through AM to HOME instead
```

If you open a service that lends memory yourself, such as [audren](glossary.md#audren), a libnx console or anything built on `tmemCreate`, close it in a function you pass to `bevy_horizon::register_release_hook`. It runs in `userAppExit`, before the check ([rules for services you add later](lifecycle-and-exit.md#rules-for-services-you-add-later)).

## Loaders: sphaira and stock hbloader

Your NRO runs inside the loader's process. When it exits, the loader reads the next NRO, usually its menu, into the same heap. bevy-horizon is tested under sphaira 1.0.0's loader.

| | sphaira 1.0.0 | Stock hbloader v2.4.5 | For you |
|---|---|---|---|
| Asset section | Required: an NRO without an icon and NACP crashes it before your code runs. | Not read. | `scripts/build_example.sh` and `scripts/build_game.sh` always add an icon and NACP, and check the section. |
| Reading the next NRO | Into its whole heap. | Only as much as the image needs. | Under sphaira, one locked page anywhere fails the read, hence the exit check. |
| The 96 MiB for gameplay recording | Always left out of its heap. | Left out only when the NACP asks. | See [Launch mode decides the budget](#launch-mode-decides-the-budget). |

A [crash report](glossary.md#crash-report) naming only `hbl`, not your NRO, means the loader crashed: before your code ran (no asset section), or after it ended (a locked heap page).

## Files and the SD card

The SD card ([sdmc](glossary.md#sdmc)) holds your NRO, assets, saves, `log.txt` and crash files. It is usually FAT32.

- **Files under 4 GiB.** No single file can reach 4 GiB.
- **Case-insensitive names.** `Foo.png` and `foo.png` are the same file, so a casing bug that works on the console can break on a desktop.
- **ASCII names.** A file created with a non-ASCII name didn't show up in `read_dir` on the console (captest's `fs-unicode-name` check warns).
- **No huge folders.** Tens of thousands of files in one folder make lookups slow.
- **Relative paths start at the NRO's folder.** libnx sets the working directory there at startup.
- **`sdmc:/…` isn't absolute to Rust.** `Path` counts only a leading `/`, so `base.join("sdmc:/…")` appends instead of replacing. bevy-horizon's bevy_asset patch takes a device path (`sdmc:/…` or `romfs:/…`) in `AssetPlugin::file_path` as it is (`patches/bevy_asset/src/io/file/mod.rs`, `under_base_path`).
- **Bevy's asset folder is beside the NRO.** bevy_horizon sets `BEVY_ASSET_ROOT` to the NRO's folder before `main`, and logs `[Assets] BEVY_ASSET_ROOT = … (Bevy's default asset folder is …/assets)`.

Where std's file functions differ on Horizon (bevy_horizon wraps the first three, in `crates/bevy_horizon/src/compat.rs`):

- **`fs::copy` works**, through `__wrap_fchmod`. libnx fails `fchmod` with `ENOSYS`, which would leave every copy empty.
- **`fs::rename` replaces an existing file, but not atomically.** `__wrap_rename` moves the old file to `<new>.bh-replaced`, renames, then deletes it. If the app dies in between, the old contents are in `<new>.bh-replaced`, so a save loader can look there.
- **`fs::remove_dir` on a folder that isn't empty** fails with `ErrorKind::DirectoryNotEmpty`, through `__wrap_rmdir`.
- **`fs::read` on a folder** fails with `NotFound`, not `IsADirectory`.

Writes cost frames. In a full 3D game, writing and syncing `log.txt` on the frame held frames for about 20 ms, and once for 1.2 s. So bevy_horizon's log writer thread does its writes, and syncs once a second ([flushing and the cost of flushing](logging-and-crash-reports.md#flushing-and-the-cost-of-flushing)). Write your saves from a thread too, or at a pause. NVK compiles shaders on the first launch and caches them under `sdmc:/.mesa/`, so later launches start faster.

RomFS is optional: a read-only file system packed into the NRO, read through `romfs:/` paths. `scripts/build_game.sh` packs the folder named by `ROMFS`, and `scripts/build_example.sh` packs `examples/<example>/romfs` if it exists. Without one, the startup warning `romfsMountSelf: 0x1559 = 2345-0010 (libnx IoError): this NRO has no RomFS, …` is normal.

## Panics, exits, and crash reports

Where the console lands tells you how the program ended. bevy_horizon's exit work runs in `userAppExit` whenever the NRO ends itself, including a panic, which aborts ([the userAppExit hook](lifecycle-and-exit.md#the-userappexit-hook)).

| How it ended | Where you land | What to read |
|---|---|---|
| `main` returned, or `std::process::exit` | The loader's menu | `log.txt`, ending with `[Exit] PASS`. |
| A heap page still locked at exit | The HOME menu, no crash report | `log.txt`: the locked blocks and `[Exit] WARN: …`. bevy_horizon left through [AM](glossary.md#am) so the loader wouldn't crash. |
| A panic before the GPU started | The loader's menu | `log.txt`, `panic.txt`. |
| A panic with the GPU in use | The HOME menu, no crash report | `log.txt`, `panic.txt` and `[Exit] nvdrv: still open, …`: bevy_horizon skipped `nvExit`, which could hang, and left through AM. |
| A CPU exception | The HOME menu, with a crash report | `crash.log` (bevy_horizon's exception handler) and Atmosphère's report, from [creport](glossary.md#creport). |
| A libnx fatal error | The HOME menu, with a crash report | `abort.log` and Atmosphère's report. |
| Closed from HOME | The HOME menu | The last synced lines of `log.txt`. bevy_horizon takes no [exit lock](glossary.md#exit-lock), so `userAppExit` doesn't run. |
| The loader crashed | The HOME menu, with a report naming only `hbl` | The end of `log.txt` ([loaders](#loaders-sphaira-and-stock-hbloader)). |

These files are written beside the NRO. [logging-and-crash-reports.md](logging-and-crash-reports.md) explains how to read them and where Atmosphère keeps its reports, and [troubleshooting.md](troubleshooting.md) goes from symptom to fix.

## Result codes

A Horizon result code packs a module and a description. libnx and bevy_horizon (`crates/bevy_horizon/src/logging.rs`, `ResultCode`) show it as `(2000 + module)-(description)`, where module is `code & 0x1ff` and description is `(code >> 9) & 0x1fff`.

| Raw | Shown | Meaning |
|---:|---|---|
| `0xD401` | 2001-0106 | Kernel `InvalidCurrentMemory`: a buffer page was locked or lacked permission, as when the loader reads into a heap with a lent page. |
| `0x1559` | 2345-0010 | libnx `IoError`. From `romfsMountSelf`, it means the NRO has no RomFS. |

## What is proven, expected, and untested

| Area | Status |
|---|---|
| Console | A Switch Lite, in handheld mode, under Atmosphère, launched from sphaira's menu or a HOME-menu forwarder with a game's memory. |
| The basics | Startup, thread-locals on the main and spawned threads, SD-card files, Bevy's AssetServer, controller input, HOME (suspend and resume), sleep, and exit to the loader with `[Exit] PASS` all work on the console. |
| Memory | With a game's memory: 3,189.0 MiB in total and a 3,132.7 MiB malloc heap (about 3,114 MiB with NVK linked). Applet mode: not measured. |
| Rendering | Works through wgpu's Vulkan backend and NVK. hello_switch's lit cube holds 60 fps. Breakout (`examples/breakout`) plays to the end at about 59 fps. render_check draws all 24 cubes in its six cameras at about 20 fps (it is a stress test), reads a buffer back intact and saves a screenshot. A bare NVK triangle test, without wgpu or Bevy, holds 60 fps. All exit with `[Exit] PASS` ([rendering.md](rendering.md)). |
| A full 3D game | Skate 3 Rust Engine (Bevy 0.18) plays, with releases 0.1.0 and 0.1.1. It reaches about 30 fps in its heaviest view, is GPU-bound, and takes about 40–70 s to load its world. |
| NVK's top-down region, `top_down_alloc` and the small-block cache | On in Skate 3 Rust Engine's release build. The cache's effect on frame rate hasn't been measured. |
| Audio | The Bevy 0.18 host (`patches/bevy-0.18/cpal`) is proven on the console in Skate 3 Rust Engine: no crackle, back after HOME and sleep, a 19 ms close at exit. The Bevy 0.19 host (`patches/cpal`) has passed only in the emulator. captest's bevy_audio checks passed on the console earlier, through the [SDL2](glossary.md#sdl2)-based host these replaced ([the audio host](input-and-audio.md#the-audio-host)). |
| Networking | captest's socket checks run on the console. libnx's default socket buffers allow about 42 UDP sockets before `ENOBUFS`. No game has used networking on the console yet. |
| captest | 179 pass, 0 fail, 2 warn (`fs-unicode-name`, and `stick-center`: the sticks rest at 0.081 against Bevy's 0.05 dead zone), at handheld clocks with Bevy 0.19, before the current audio host ([testing.md](testing.md)). |
| C layouts | All 348 items match devkitA64's headers. |
| Docked mode | Untested; the window becomes 1920×1080. |
| Known issue | One simple Bevy 0.18 scene stops the GPU within its first frames. About 10–13 s after the first frame, the log shows `channel N lost: notification={type=8 (fifo idle timeout) info=0 status=65535}`, then the device is lost. It isn't memory, and the cause is unknown ([rendering.md](rendering.md)). |

## Sources used for these notes

This repository's code is the main source; each section names the file and item it describes. Figures come from bevy-horizon's examples and Skate 3 Rust Engine on a Switch Lite, except those marked as from the emulator. Upstream sources:

- libnx v4.12.0 (`init.c`, `newlib.c`, `bsd.c`, `tmem.c`, `fs_dev.c`, `nv.c`, `audren.c`) and libnx `feebd026` (`newlib.c`).
- Atmosphère `c8b7316` (`KPageTableBase`, `KServerSession`, `pm_spec.cpp`) and `36cc9a9f` (thread-local regions, `kern_k_scheduler.cpp`).
- sphaira 1.0.0 (`hbl/source/main.c`) and nx-hbloader v2.4.5 (`source/main.c`).
- devkitPro newlib `57bd8e1` (`sbrk.c`, `pthread.c`, `mlock.c`, `_mallocr.c`).
- mesa-switch `606441c` (`src/nouveau/horizon/`, `src/util/os_misc.c`, `src/nouveau/nil/tiling.rs`).
- Rust `db8f076d2` (`library/std/src/sys/thread/unix.rs`).
- Crates: wgpu-core 27.0.3, gpu-alloc 0.6.2, bevy_render, bevy_pbr, bevy_mesh and bevy_gltf 0.18.1, bevy_pbr and bevy_render 0.19.1, blocking 1.7.0, inventory 0.3.24.
- switchbrew's NACP and process-manager pages.
- Jeff Bonwick and Jonathan Adams, [*Magazines and Vmem*](https://www.usenix.org/legacy/event/usenix01/bonwick.html) (USENIX 2001).
