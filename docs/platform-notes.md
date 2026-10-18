# Horizon: platform notes

This page collects the facts about the Nintendo Switch, [Horizon](glossary.md#horizon), [libnx](glossary.md#libnx), homebrew loaders, and the SD card that matter to a Rust or [Bevy](glossary.md#bevy) program. It is for you if you want to use `bevy_horizon` as the base for another Switch [homebrew](glossary.md#homebrew) game, or if a console result does not match what a desktop build suggests.

The short rule is: read the code and the console log before you trust a guess. The current evidence is from `hello_switch`, measured on a Switch in handheld mode, under [Atmosphère](glossary.md#atmosphère), launched from sphaira 1.0.0. Build A2 passed the hardware checks. Build A3 contains an exit fix, but has not yet been run on a console.

## In short

- **The tested screen is 1280x720 handheld.** [Handheld mode](glossary.md#handheld-mode) is proven. [Docked mode](glossary.md#docked-mode) is not. `display.rs` follows `appletGetOperationMode()` and changes the Bevy window to 1920x1080 when Horizon reports docked (`crates/bevy_horizon/src/display.rs:27-52`).
- **The measured process total is 3,189.0 MiB, not about 3,285 MiB.** `log2.txt` says 3,189.0 MiB total, 3,185.0 MiB used, and a 3,132.7 MiB malloc [heap](glossary.md#heap) (`iw4l-switch-findings/LOGS/log2.txt:12-14`). The missing 96 MiB is held back for [gameplay recording](glossary.md#gameplay-recording), because the sphaira [forwarder](glossary.md#forwarder) [NACP](glossary.md#nacp) asks for automatic capture.
- **Rust `std` needs `TPIDR_EL0` before `main`.** The first hardware crash in our code was before `main`, because Rust [thread-local storage](glossary.md#thread-local-storage) reads `TPIDR_EL0` directly and libnx did not set it. `bevy_horizon` fixes the main thread in [`userAppInit`](glossary.md#userappinit) and every new thread with `--wrap=threadCreate` (`compat.rs:28-64`, `compat.rs:108-156`, `aarch64-nintendo-horizon.json:28`).
- **Threads must be joined.** `pthread_detach` returns `ENOSYS` on Switch, and release Rust ignores that detach failure. A dropped `JoinHandle` can leave a locked 2 MiB stack in the heap. Bevy task pools built with `multi_threaded` are not joined by this layer yet.
- **C layouts are part of the platform.** The Rust `libc` crate had 3DS layouts for Horizon. The ABI probe compares 167 file and pthread items; after the fixes it compares 348 items, networking included, with 0 mismatches (`scripts/check_libc_abi.sh:2-8`, findings `12-RUST-AND-C-DISAGREED-ON-STRUCT-LAYOUTS.md:1-18`, `:519-559`).
- **A socket transfer memory can crash the loader after your program exits.** libnx's default sockets lend bsd 0x234000 bytes, 2.20 MiB, from your heap. If it is still locked, sphaira's loader reads the next [NRO](glossary.md#nro) into its whole heap and the kernel rejects the buffer with result 2001-0106.
- **Rendering works on the console, with one open failure.** Bevy draws through wgpu's Vulkan backend and NVK, which bevy_horizon's `vulkan` feature links. In hardware run 5, nvk_probe's triangle and hello_switch's lit cube held 60 fps, and render_check lost the GPU device about 4 seconds in; in the Ryujinx harness 2D and 3D draw and `examples/breakout` plays to the end ([rendering.md](rendering.md)). This replaced the plan to use wgpu's GL backend over [switch-mesa](glossary.md#switch-mesa).
- **Constructors run before `main`.** libnx calls newlib's `__libc_init_array`, so `.init_array` entries run as on Linux. Crates that register things from constructors still need Horizon in their own target lists ([Constructors before `main`](#constructors-before-main)).
- **Audio worked on the console through SDL2, and the host that replaced it works there with Bevy 0.18.** In Skate 3 the 0.18 host played clean sound through play, never ran dry, came back after HOME and after sleep, and closed at exit in 19.0 ms (perf35, findings 54); the 0.19 host hasn't run there yet. The `cpal` patches now feed the audio renderer ([audren](glossary.md#audren)) themselves: 48 kHz, stereo, the game's f32 converted to 16-bit, in three buffers of 256 to 8192 frames, the size the stream asks for unless `BH_AUDIO_FRAMES` sets it (`patches/cpal/src/host/horizon/mod.rs:49-58`, `:195-290`, `:552-559`, `:669-674`). Through the [SDL2](glossary.md#sdl2) host before it, captest on hardware kept up with real time, opened a second stream after the first closed, played bevy_audio's chime, and got audio back after HOME (Bevy 0.19). SDL's Switch driver can stop the sound for good ([why not SDL2](input-and-audio.md#why-not-sdl2)); in Ryujinx, Skate 3's sound stopped after 6 s through SDL (run au4) and kept up with real time through the new host (run au5). Bevy 0.18's host, `patches/bevy-0.18/cpal`, is the same code on cpal 0.15.3.

## Modes and screen

Screen mode matters because it changes how much you draw, and it changes what a user expects from a port. A game that is easy at 720p can be twice as many pixels at 1080p.

| Mode | Resolution | Status | Where it is selected |
|---|---:|---|---|
| Handheld | 1280x720 | Tested on hardware | `hello_switch` starts its Bevy window at 1280x720 (`examples/hello_switch/src/main.rs:33-40`), and A2 logged handheld controller style `0x2` (`LOGS/log2.txt:35-36`). |
| Docked | 1920x1080 | Expected, not tested | `monitor_dock_status` maps `appletGetOperationMode() == 1` to docked and writes `WindowResolution::new(1920, 1080)` (`display.rs:36-52`). |

`bevy_horizon` stores the current mode as a Bevy resource. A [plugin](glossary.md#plugin), `HorizonDisplayPlugin`, inserts `HorizonDisplayMode::Handheld` first and adds a PreUpdate system only on Horizon (`display.rs:13-18`). That system polls the [applet](glossary.md#applet) operation mode, then resizes the primary Bevy window when the mode changes (`display.rs:31-52`).

What can go wrong:

- **Assuming docked is proven.** It is not. The code path exists, but current hardware tests are handheld only.
- **Assuming a mode change redraws something today.** There is no renderer yet. The Bevy window resource changes, but no [wgpu](glossary.md#wgpu), [Mesa](glossary.md#mesa), [EGL](glossary.md#egl), or [OpenGL ES](glossary.md#opengl-es) presentation path is active.
- **Testing a renderer at 1080p first.** Handheld is the baseline. Where settings must differ, prefer the 1280x720 path until docked has its own run.

## Memory budget and loader heap

Memory matters because the loader, libnx, Rust, Bevy, services, thread stacks, assets, and later the GPU all share one process. On this launch path, libnx does not choose a small heap. The homebrew loader gives the process a heap override, and libnx takes it as-is.

Measured A2 boot lines:

```text
[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
[Memory] process: 3189.0 MiB in total, 3185.0 MiB used (svcGetInfo; "used" already counts the whole malloc heap below)
[Memory] malloc heap: 3132.7 MiB, set up by the homebrew loader; malloc has 0.0 MiB in use, 0.0 MiB taken from the 3132.7 MiB heap
```

These are from `LOGS/log2.txt:12-14`. `memory.rs` prints the same fields by reading the applet type, `svcGetInfo(InfoType_TotalMemorySize)`, `svcGetInfo(InfoType_UsedMemorySize)`, libnx's `envHasHeapOverride()`, `fake_heap_start`, `fake_heap_end`, and newlib `mallinfo()` (`memory.rs:35-97`, `memory.rs:103-111`).

The budget breakdown:

| Figure | Measured | Meaning |
|---|---:|---|
| Process total | 3,189.0 MiB | The memory limit after the 96 MiB gameplay-recording boost was removed. |
| Used at boot | 3,185.0 MiB | Misleading as “used”: it already counts the whole loader-provided heap. |
| Malloc heap | 3,132.7 MiB | The heap available to Rust, C, Bevy, thread stacks, transfer memories, assets, and later GPU/audio code. |
| Gap between process total and heap | about 56.3 MiB | Mostly the NRO image at the start of the loader heap, plus loader/system overhead. |
| Heap in use at frame 600 and later | 9.6 MiB in use, 10.0 MiB taken | Flat through frame 4200 in A2 (`LOGS/log2.txt:48-66`). |

The 96 MiB difference is important. The loader first sized its heap from the full application memory, then the system lowered the process limit because the sphaira forwarder's NACP has `video_capture = 2`, automatic capture. sphaira and stock hbloader both have rules around this hold-back; the findings cite sphaira 1.0.0 `hbl/source/main.c:106-121, 123-133, 409-411`, sphaira `owo.cpp:485`, nx-hbloader v2.4.5 `source/main.c:126-127, 233`, Atmosphère `pm_spec.cpp:238-278`, and switchbrew's NACP and process-manager pages.

For a game budget, use the malloc heap number first: **3,132.7 MiB**. Do not repeat the old “about 450 MB in applet mode” claim as measured for this launch path. Launching from the HOME-menu forwarder ran as an Application and got the full application memory, minus the gameplay-recording hold-back. Applet mode remains unmeasured here.

The NRO sits at the start of the loader's heap. sphaira unmaps the NRO before reading the next one, and A3's heap check scans from `fake_heap_start` to `fake_heap_end` (`memory.rs:143-162`, `memory.rs:189-232`). That means an NRO that grows by tens of MiB directly reduces the remaining malloc budget.

### GPU memory comes from the same heap

With bevy_horizon's `vulkan` feature, NVK takes the memory behind every Vulkan memory allocation from this malloc heap and maps it for the GPU through nvmap. mesa-switch `memalign`s it (`src/nouveau/horizon/nouveau_horizon_memory.c:591` at `d4a00ea`); bevy-horizon's NVK build takes it from the top of the heap down instead ([below](#nvk-takes-gpu-memory-from-the-top-of-the-heap)). Either way, textures, vertex and index buffers, and wgpu's staging buffers all count against the same heap as the game's own data: 3,113.6-3,132.7 MiB in our hardware runs. There is no separate video memory.

Six things follow:

- **Allocator blocks.** wgpu's default `MemoryHints::Performance` sets its allocator to take memory in 128-256 MiB device blocks (wgpu 27's gpu-alloc: free-list chunks of 128-512 MiB). The wgpu-hal patches give Horizon `MemoryUsage`'s 8-64 MiB sizes instead ([toolchain.md](toolchain.md#the-crate-patches)).
- **Uploads.** Bevy uploads each new image and mesh in the next frame, all of them at once by default. While an image uploads it is in the heap up to three times: its pixels, wgpu's staging copy, and the GPU texture. wgpu frees the staging copy once the GPU has finished the submission that used it and wgpu has polled for it, which `Queue::submit` does without waiting (wgpu-core 27.0.3 `src/device/life.rs:298-326`, `src/device/queue.rs:1418-1423`). bevy_render's `RenderAssetBytesPerFrame` spreads images and other render assets over frames. The limit never splits an asset: one larger than the limit goes up alone in its frame. It doesn't hold back meshes. Bevy's mesh allocator copies every new mesh to the GPU in `allocate_and_free_meshes`, which runs before `prepare_assets` counts them against the limit (bevy_render 0.18.1 `src/mesh/allocator.rs:326-333`, `:371-389`). bevy_horizon's `render` feature paces both; see [Pacing uploads](#pacing-uploads-the-render-feature).
- **One block per large buffer.** With those sizes, gpu-alloc gives any buffer or texture of 8 MiB or more its own Vulkan memory object (gpu-alloc 0.6.2 `src/allocator.rs:240-247`), and NVK backs each memory object with one contiguous run of the heap. Bevy keeps the meshes that share a vertex layout in one buffer, a slab, which grows by half again each time it fills, up to 512 MiB, and it creates the new buffer while the old one still holds the data to copy (bevy_render 0.18.1 `src/mesh/allocator.rs`, `reallocate_slab`). A game with hundreds of MiB of mesh data in one layout can therefore ask for a block of hundreds of MiB beside the one being replaced, after loading has left the heap in pieces. Insert a smaller `MeshAllocatorSettings` into the `RenderApp` sub-app after `DefaultPlugins` (Skate 3: 64 MiB slabs, `sk8/crates/skate-game/src/horizon.rs`).
- **Main-world copies.** A mesh or image with the default `RenderAssetUsages` keeps its data in the main world after upload, and Bevy clones it for the render world, so it is in the heap three times while it uploads and twice afterwards. `RenderAssetUsages::RENDER_WORLD` moves the data to the render world instead, which frees it once it is on the GPU (bevy_render 0.18.1 `src/render_asset.rs`, `extract_render_asset`). Use it for anything the CPU never reads back. Frustum culling still works for such a mesh: Bevy 0.18 records its bounds as it moves the data (bevy_mesh 0.18.1 `Mesh::take_gpu_data`, `final_aabb`).
- **Morph targets.** Bevy's glTF loader gives every primitive with morph targets its own R32Float 3D texture, with one layer per target and 9 floats per vertex in each layer. Each layer is the rectangle of at most 2048 per side with the fewest spare cells, so a few texels wide and over a thousand rows tall (bevy_gltf 0.18.1 `src/loader/mod.rs:741-768`; bevy_mesh 0.18.1 `src/morph.rs:51-98`, `lowest_2d` at 248-257). NVK tiles a 3D image with more than 10 layers in tiles of 64 bytes by 8 rows by 16 layers (mesa-switch `src/nouveau/nil/tiling.rs:379-445`), and gpu-alloc rounds the block up to a power of two (gpu-alloc 0.6.2 `src/buddy.rs:306-318`). A 498-vertex mesh with 22 targets, a `3x1494x22` texture, has 385.2 KiB of data and takes a 4 MiB block. Skate 3's customiser loaded 480 such parts at startup, which would need 1858.1 MiB of blocks, and on Horizon now loads each part when an outfit needs it (`sk8/crates/skate-game/src/customiser_parts.rs`). Load morph-target meshes when they're needed, and share a mesh between entities rather than loading copies.
- **Measuring.** `bevy_horizon::memory_report()` returns the heartbeat's `[Memory]` text plus how much of the in-use memory nvmap objects hold, for a game's own logging at load stages. It walks the heap with `svcQueryMemory`, so don't call it every frame. The `[Memory]` text also splits the free memory in two: what is free at the top of the heap (the part below NVK's region that `sbrk` hasn't handed out, plus malloc's top chunk) and what is free in holes below it. While NVK's region holds memory, the text adds its size, free runs inside it included, then how much of it blocks hold and the most they have held: `the top-down region above the top: 1992.8 MiB, 994.1 MiB of it in blocks (16.8 MiB GPU, 977.3 MiB Rust), 1971.6 MiB at the most` (Skate 3 in Ryujinx, run `sk8h`). Without the [`top_down_alloc`](#large-rust-blocks-in-the-region-the-top_down_alloc-feature) feature, every block is the GPU's and the middle reads `… MiB of it in GPU blocks`. newlib's malloc is Doug Lea's malloc 2.6.5, whose `mallinfo` counts the top chunk in `fordblks` and alone in `keepcost` (newlib `libc/stdlib/_mallocr.c`, `malloc_update_mallinfo`). A Rust panic's report in `log.txt` and `panic.txt` includes the same figures (its `memory:` line). On Bevy 0.18, wgpu-hal logs the size and memory type of any Vulkan allocation that fails (`patches/bevy-0.18/wgpu-hal/src/vulkan/device.rs`), and keeps a ledger of the memory wgpu's buffers and textures hold, in four kinds: textures, buffers only the GPU uses, staging buffers (wgpu's `MemoryFlags::TRANSIENT`) and buffers the CPU maps. It logs `[GPU]` lines under the target `bevy_horizon::gpu`: one for each resource of 4 MiB or more, with its label, size, format and usage, and one each time the total passes the last high it logged by 64 MiB. The failure line ends with the same tally. The ledger counts the block each resource holds from wgpu's allocator (gpu-alloc), as wgpu's own `buffer_memory` and `texture_memory` counters do. NVK's GPU figure in `[Memory]` also takes in the chunks gpu-alloc hasn't handed out, NVK's own memory (command buffers, descriptors, shaders, the swapchain), and up to 128 MiB of freed blocks NVK keeps for reuse (`NOUVEAU_HORIZON_BO_CACHE_DEFAULT_MB`), so the gap between the two is what wgpu's resources don't account for.

### Pacing uploads: the `render` feature

bevy_horizon's `render` feature (`render_0_18` for a Bevy 0.18 game) adds `HorizonUploadPlugin` (`crates/bevy_horizon/src/uploads.rs`). It does three things, each set by a field of `HorizonUploadSettings`:

| Field | Default | What it does |
|---|---:|---|
| `mesh_bytes_per_frame` | 32 MiB | The most new mesh data (vertices plus indices) handed to Bevy's mesh allocator in a frame. The rest waits in the render world, oldest first. At least one mesh goes each frame however large, the rule `RenderAssetBytesPerFrame` uses. A mesh already on the GPU that changes is never held. |
| `reclaim_after_bytes` | 8 MiB | After a frame that uploaded at least this much, wait up to 10 s for the GPU to finish it, so wgpu frees that frame's staging buffers before the next frame makes more. |
| `default_asset_bytes_per_frame` | 64 MiB | Bevy's `RenderAssetBytesPerFrame`, if the game hasn't set it when the app starts. |

`None` turns each one off. To change them, insert `HorizonUploadSettings` into the main app before `App::run`; the plugin copies it into the render world when the app starts.

Holding a mesh back takes one more step. When Bevy extracts an entity with a `Mesh3d`, its GPU mesh building records where the mesh is in the allocator. A mesh that isn't there yet is recorded as zero vertices, and Bevy extracts the entity again later only if its material wasn't ready (bevy_pbr 0.18.1 `src/render/mesh.rs:831-836`, `:1143-1176`; 0.19.1 `:1087-1092`, `:1446-1462`). An entity whose mesh was held would draw nothing until something else changed it. So when a mesh that was held goes to the allocator, the plugin marks the `Mesh3d` and `Mesh2d` components that use it as changed, which is how Bevy has an entity extracted again when its mesh is modified (bevy_mesh 0.18.1 `src/components.rs:118-147`, `mark_3d_meshes_as_changed_if_their_assets_changed`). Until then the entity draws nothing, like one whose mesh hasn't loaded.

The log shows what it did:

```text
[Upload] GPU uploads a frame: new meshes at most 32.0 MiB, other render assets at most 64.0 MiB (RenderAssetBytesPerFrame); waits for the GPU after frames that upload 8.0 MiB or more
[Upload] holding 580 new meshes (165.3 MiB) for later frames: Bevy's mesh allocator gets at most 32.0 MiB of new meshes a frame (HorizonUploadSettings)
[Upload] the last held mesh went to the GPU: <n> meshes (<size> MiB) waited, over <n> frames (<time> s)
[Upload] waited for the GPU after <n> frames that uploaded <size> MiB between them, <time> ms in all, so wgpu could free their staging buffers
```

The first two lines are from Skate 3 in Ryujinx (`sk8e`). The other two are the formats; the plugin prints them when the held meshes have all gone and after 60 frames without a wait.

The cost is time, not memory: a level's meshes reach the GPU over more frames, and a frame that uploads waits for the GPU instead of overlapping with the next one. The pacing doesn't make the heap less fragmented; it only stops one frame's uploads, and their staging copies, from needing the memory all at once. Keeping GPU blocks out of the CPU heap's holes is the next section's job.

### NVK takes GPU memory from the top of the heap

mesa-switch's NVK `memalign`ed each GPU block, so every block needed a malloc hole it fit in. A game that builds its scene and then frees its scratch data leaves the heap in holes, and the next large GPU block can fail with plenty of memory free. Skate 3 in Ryujinx (run `sk8f`) failed to allocate a 16 MiB block with 14.0 MiB free at the top of the heap and 508.3 MiB free in holes below it; at the heartbeat before, 832.9 MiB of the heap was GPU memory, in 133 blocks.

bevy-horizon's NVK build takes GPU blocks from the top of the heap downward instead. The change is `patches/mesa-switch/0001-top-down-gpu-backing.patch`, which `scripts/build_nvk.sh` applies to the mesa-switch checkout:

- newlib's malloc grows the heap upward through `sbrk`, and devkitA64's `_sbrk_r` refuses to move the break past `fake_heap_end`, which it reads again on every call (devkitPro/newlib at `57bd8e1`, `libgloss/libsysbase/sbrk.c`; no other library in the toolchain defines `_sbrk_r`). Lowering `fake_heap_end` hands the memory above it to the GPU with no fixed split: CPU memory grows up from the bottom of the heap, GPU memory grows down from the top, and neither leaves holes in the other.
- NVK changes `fake_heap_end` only while it holds newlib's malloc lock, which malloc holds around every `sbrk` call; the lock is recursive (`__malloc_lock`, newlib `newlib/libc/stdlib/mlock.c`).
- The region stops 16 MiB above the CPU heap's break, so the CPU heap always has room to grow. A block that doesn't fit, in a free run inside the region or by extending it, comes from `memalign` as before, so the holes below are still a fallback. NVK logs a warning the first time that happens: `GPU backing: the top-down region (<n> MiB) can't grow: …`.
- A freed block's granules go back to the region. Once the bottom of the region is free, NVK raises `fake_heap_end` again so malloc can use that memory. Blocks that NVK's BO cache keeps for reuse stay in the region: up to 32 MiB each and 128 MiB in all by default (mesa-switch `src/nouveau/horizon/nouveau_horizon_private.h:45-46`). When an allocation fails, NVK frees the whole cache and tries once more (`nouveau_horizon_memory.c`, `nouveau_horizon_device_bo_cache_trim`).
- The region is a static bitmap of 64 KiB granules, so its bookkeeping never allocates.
- It is on by default. `NOUVEAU_HORIZON_TOP_DOWN=false` in the environment turns it off; NVK reads it with Mesa's `debug_get_bool_option` at its first allocation.

The first allocation logs `GPU backing: top-down region on, heap end 0x…` at INFO, which mesa-switch prints only with NVK's timing diagnostics on (`nouveau_horizon_runtime.c`, `nouveau_horizon_log`).

Two figures change meaning. libnx's `fake_heap_end` no longer marks the end of the heap, so bevy_horizon finds the end from the loader's heap override (`envGetHeapOverrideAddr` plus `envGetHeapOverrideSize`) or, without one, by walking the heap's blocks with `svcQueryMemory` (`crates/bevy_horizon/src/memory.rs`, `heap_end`). And Mesa's `os_get_available_system_memory` counts the heap up to `fake_heap_end` (mesa-switch `src/util/os_misc.c:481-498`), so it leaves out free granules inside the region. Only Vulkan memory budgets use that figure, and wgpu doesn't ask for them by default.

In Ryujinx, render_check ran with the region on and passed its GPU readback test; its screenshots match the run before the change (run `rc11`).

### Large Rust blocks in the region: the `top_down_alloc` feature

Moving GPU blocks out of malloc's way wasn't enough for Skate 3. In run `sk8g`, with the region on, a 64 MiB GPU block failed. The region couldn't grow, because only 21 MiB lay between it and malloc's break and 16 MiB of that stays for malloc, and malloc had 503.2 MiB free in holes, none of them big enough. The holes were the game's own: it loads the map's data in large blocks among its small ones, then frees most of it once the scene is built.

On a desktop, malloc puts large blocks in their own `mmap` mappings, away from its small ones, so freeing one returns its memory whole. newlib's malloc is Doug Lea's malloc 2.6.5, which would map blocks of 128 KiB and up (`DEFAULT_MMAP_THRESHOLD`), but newlib's `libc/stdlib/_mallocr.c` defines `HAVE_MMAP` as 0. So every block shares one heap.

bevy_horizon's `top_down_alloc` feature, which turns on `vulkan`, installs a global allocator that uses NVK's region the way malloc would use `mmap` (`crates/bevy_horizon/src/allocator.rs`):

- A Rust block of 256 KiB or more goes in the region, in whole 64 KiB granules, through two functions bevy-horizon's NVK patch exports, `nouveau_horizon_region_alloc` and `nouveau_horizon_region_free`. When the block is freed, its granules can hold GPU blocks and other large blocks, and once they are at the region's bottom they go back to malloc.
- Smaller blocks go to newlib's malloc, as with Rust's `System` allocator.
- If the region can't place a large block, malloc takes it. If malloc can't place a small block, a free run in the region takes it.
- `realloc` keeps a region block where it is when its granule count stays the same, and when it shrinks but stays 256 KiB or more, frees the granules past its new end.
- The allocator tells the region's blocks from malloc's by address. malloc's lie below `fake_heap_end`, the region's at or above it, and NVK moves `fake_heap_end` only across free memory, so the check needs no lock.

With the feature, the `[Memory]` text splits the region's blocks between the GPU and Rust, and sizes up Rust's malloc blocks:

```text
[Memory] scene published, map data freed: malloc has 291.4 MiB in use, 364.0 MiB taken from the 3192.0 MiB heap; 835.2 MiB free at the top, 72.6 MiB free in holes below it; the top-down region above the top: 1992.8 MiB, 994.1 MiB of it in blocks (16.8 MiB GPU, 977.3 MiB Rust), 1971.6 MiB at the most; Rust's malloc blocks: 0.0 MiB of 256 KiB and up, 140.6 MiB of 64-256 KiB; 16.6 MiB of the heap is mapped for the GPU (nvmap), in 8 blocks
```

That line is from Skate 3 in Ryujinx with the feature on (run `sk8h`). The text ends with a count when a block went the other way: `N large blocks the region couldn't place went to malloc`, `N small blocks malloc couldn't place went to the region`, and, from NVK, `N GPU blocks the region couldn't place came from malloc`.

In sk8h, malloc's holes never passed 171 MiB, and from the first frame on they stayed under 26 MiB, against sk8g's 503.2 MiB. The run still ran out of memory, but this time no free space was left anywhere. At the panic, the region's blocks held 2,630.8 MiB (1,296.6 MiB GPU, 1,334.2 MiB Rust), malloc had 434.2 MiB in use, and 8.1 MiB was free at the top with 21.9 MiB in holes. The GPU's share had grown to about three times the size of the world's textures and meshes (about 410 MiB); finding out why is next.

What it costs:

- **One global allocator per program.** A game that sets its own `#[global_allocator]` can't use the feature; Rust rejects a second one at build time.
- **Granule rounding.** A large block takes whole 64 KiB granules, so it can waste up to 64 KiB less a byte. That is up to a quarter of a block just over 256 KiB, and less in proportion for bigger ones.
- **Locks.** Each large allocation and free takes the region's lock, and newlib's malloc lock as well when `fake_heap_end` moves.
- **Small blocks and the region.** malloc's small blocks reach the region's free runs only when malloc itself is full, at a granule each.

### Small blocks: a cache in front of malloc

newlib's malloc keeps the whole heap behind one lock (`__malloc_lock`), and its `free` sorts each block into its size bin there and then: Doug Lea's malloc 2.6.5 has no fast bins (newlib `libc/stdlib/_mallocr.c`, lines 300-315 and `fREe`). Bevy allocates and frees many small blocks a frame on every thread, so the threads queue for that lock. In Skate 3's perf4 CPU profile (30 s, a sample every 2 ms), a fifth of the game's running samples were in the allocator, most of them in `_malloc_r` and `_free_r`, and the main and render threads waited about 9 and 6 ms a frame for it.

So `top_down_alloc` also keeps a cache of Rust's small blocks (`crates/bevy_horizon/src/allocator/small_blocks.rs`). A freed block of up to 8 KiB, aligned to at most 16 bytes, stays in the cache, and the next allocation of its size class takes it back without malloc's lock:

- **Classes.** There are 32: 16 to 128 bytes in steps of 16, then four to each doubling, up to 8 KiB. A block is allocated at its class's size, so any block of a class can serve any request in it.
- **Magazines.** The design is Bonwick and Adams's (*Magazines and Vmem*, USENIX 2001). Each of 8 shards keeps two magazines (lists of free blocks) per class. Behind them, a depot keeps up to 4 full magazines per class, which carries blocks one thread frees to another that allocates them; when the depot is full, a full magazine goes back to malloc. A thread's shard comes from the address of its thread-local region. Horizon places those 0x200 bytes apart, eight to a page (Atmosphère `libraries/libvapours/include/vapours/svc/svc_select_thread_local_region.hpp:44`), so threads made one after another use different shards.
- **Locks that are only tried.** Horizon runs each core's highest-priority ready thread (Atmosphère `libraries/libmesosphere/source/kern_k_scheduler.cpp:120-145`). A thread spinning on a lock that a lower-priority thread on its own core holds would never let that thread finish. So the cache's locks are flags that are tried and never waited for: a thread that finds its shard held tries one other, then goes to malloc.
- **Bounded.** A cached block is a malloc block, so malloc counts it as in use. The cache holds at most about 3.4 MiB in the shards and 0.85 MiB in the depot. When malloc runs out, the allocator gives it the cache's blocks and asks again, so the cache can't turn an allocation that would fit into an out-of-memory panic.
- **`realloc`.** A block that stays in its class stays put. One that moves to another class, or into or out of the cache's sizes, is copied to a new block, so that every block the cache keeps is its class's size. Larger blocks use malloc's `realloc`, as before.
- **Blocks that aren't the cache's.** A freed block smaller than its class's size, such as one allocated while the cache was off, goes to `free` instead.

`BH_ALLOC_CACHE=0` in the game's environment turns the cache off when `HorizonPlugin` is added, and gives malloc back what it holds. The `[Memory]` text then ends with `the small-block cache is off (BH_ALLOC_CACHE=0)`. With the cache on, it says what the cache holds and has done:

```text
the small-block cache holds 1.2 MiB; of the allocations it takes, it served 4996744 and malloc 4485230 (0 with the cache busy), and of the frees, 0 went to malloc with the cache busy and 0 for blocks under their class's size; its depot took 11749 magazines and gave out 11700, and 147174 went back to malloc
```

That is Skate 3 in Ryujinx (run `sk8zm`), five minutes after its first frame; the counts run from launch. At the first frame they stood at 1,574,935 served and 4,356,767 by malloc, with 145,455 magazines gone back: loading allocates far more than it frees, and when it frees, it frees more blocks of a size at once than the cache keeps. In the five minutes of play after it, the cache served 96% of the allocations it took (3,421,809 against 128,463 by malloc), and 1,719 magazines went back. `0 with the cache busy` means no thread ever found both shards it tried held. If malloc ran out, the text adds `malloc ran out N times, and took back the cache's blocks`.

How it was tested: on the host, with the allocator's code and the cache's copied into a test program and glibc's malloc in place of newlib's (the findings' `tools/alloctest`). 16 threads made 3.2 million random allocations, reallocations and frees, of random sizes and alignments, some freed on other threads, while another thread turned the cache off and on, and the blocks' contents were checked before each free. The cache served 98% of the allocations it took. With glibc's own caches off, malloc ended within 5 KB of where it started. The same test is clean under AddressSanitizer with leak checks, and under ThreadSanitizer with 8 and 16 threads.

What it costs:

- **Memory.** Up to about 4.3 MiB of the heap in cached blocks.
- **A miss.** An allocation the cache can't serve tries a lock or two before going to malloc.
- **Unmeasured on hardware.** Whether it raises the frame rate is the perf5 runs' question. Skate 3 is GPU-bound, so less waiting on the CPU may not show as more frames.

## Thread-locals and `TPIDR_EL0`

Thread-local storage matters because Rust `std` uses it before your `main` runs. A normal Rust program on this target crashed before the first log line until this was fixed.

The crash mechanism was:

```text
crt0 -> Rust std::rt::lang_start_internal
     -> Rust reads current thread id from thread-local storage
     -> LLVM emits mrs x8, tpidr_el0
     -> TPIDR_EL0 is 0
     -> first access faults near 0x700
```

The fix in this repo has three parts:

1. **Set the main thread pointer in `userAppInit`.** libnx calls this weak hook after services and SD are up, before C constructors, Rust runtime setup, and `main`. `compat.rs` calls `set_thread_pointer()`, then starts the boot log (`compat.rs:28-64`).
2. **Wrap every new libnx thread.** The [target specification](glossary.md#target-specification) passes `--wrap=threadCreate` (`aarch64-nintendo-horizon.json:28`). The wrapper records the real entry point, calls libnx's real `threadCreate`, and starts the thread in `thread_start`, which sets `TPIDR_EL0` before calling Rust or C code (`compat.rs:91-156`).
3. **Log the pointer values.** A2 proved this on a second thread: `TPIDR_EL0 = libnx tls_tp`, followed by a `thread_local!` check passing (`LOGS/log2.txt:42-44`).

The reason this is needed is Switch-specific. libnx and devkitA64's C runtime keep the thread pointer in libnx `ThreadVars.tls_tp`, reached through `TPIDRRO_EL0`. Rust/LLVM expects `TPIDR_EL0`. Copying libnx's value into Rust's register makes both worlds agree.

## Threads and stacks

Threads matter at exit because a Switch thread stack is also heap memory. libnx maps stack pages with `svcMapMemory`, which leaves the original heap pages locked until the thread is closed.

The rule is simple: **join every thread you create.** A2's check does exactly that: it spawns `a2-thread-check`, reads a thread-local twice, joins it, and logs PASS (`examples/hello_switch/src/checks.rs:224-259`, `LOGS/log2.txt:42-44`).

The trap is detach:

| Action | What happens on this target | Exit risk |
|---|---|---|
| `JoinHandle::join()` | Calls through pthread join and libnx closes the thread object. | Good. The stack can unlock. |
| Dropping `JoinHandle` | Rust tries to detach. devkitPro newlib returns `ENOSYS`; release Rust ignores the error. | Bad. The stack can stay locked in the heap. |
| Bevy `multi_threaded` task pools | Pool threads live in globals, so nothing joins them. The `bevy_tasks` patch adds `shut_down_global_task_pools`, which bevy_horizon calls at exit (`lifecycle.rs:345`). | Good with bevy_horizon. |
| A crate's own detached pool, such as `blocking`'s | `blocking` (behind `async-fs`) spawns threads and drops their handles (blocking 1.7.0 `src/lib.rs:347-365`). A thread that idles 500 ms exits, but its stack stays locked. | Bad. Keep such pools out of the game; bevy_asset's patch keeps Bevy's file reads off `blocking` on Horizon. |

The findings cite devkitPro newlib `libgloss/libsysbase/pthread.c:477-482`, libnx `newlib.c:217-237`, libnx `thread.c:103, 132, 234-248`, and Rust `library/std/src/sys/thread/unix.rs:149-154`. The practical symptom is the same as a leaked transfer memory: A3's heap check would list locked, no-access heap blocks of about 2 MiB each.

## C struct layouts and the libc patch

The C ABI matters because Rust `std` asks newlib about files and then reads the fields through Rust's copy of C structs. If the layouts differ, you get wrong answers, not a clean error.

The bug was concrete:

| Struct or value | Wrong Rust value did this | How you would notice |
|---|---|---|
| `struct stat.st_mode` | Read `st_nlink` and `st_uid` as mode. | `is_file()` and `is_dir()` false for files and folders. |
| `struct stat.st_size` | Read a timestamp as the file length. | A 45-byte file can look about 1.79 billion bytes long. |
| `struct dirent.d_name` | Started two bytes too late. | `a2_check.txt` would be listed as `_check.txt`. |
| `pthread_condattr_t` | Rust reserved 4 bytes, C wrote 8. | A future lock slow path could overwrite nearby stack data. |

`patches/libc` fixes the Switch-specific newlib types. The script `scripts/check_libc_abi.sh` asks devkitA64's GCC and Rust for the same list of sizes, alignments, offsets, and constants (`scripts/check_libc_abi.sh:2-8`, `:57-64`). It compares 167 items and now reports 0 mismatches for the file and pthread set. The `.cargo/config.toml` patch is also required because [`-Z build-std`](glossary.md#build-std) builds Rust `std` as its own workspace; otherwise `std` would still use crates.io `libc` (`.cargo/config.toml:5-11`, `scripts/build_example.sh:14-29`).

Networking was fixed later: the first socket probe found 41 mismatches among 178 items, and the probe now covers them, with all 348 items matching. The console test for sockets is captest's `net-*` checks; in Ryujinx several fail for emulator reasons (findings `15-CAPTEST-IN-THE-EMULATOR-AND-YOUR-STEPS.md` §7.3).

## Constructors before `main`

Some crates register things before `main` from constructors: functions the linker lists in the `.init_array` section. Bevy's `reflect_auto_register` feature works this way, through the `inventory` crate, and so does the `ctor` crate.

libnx runs them. Its `__libnx_init` ends by calling newlib's `__libc_init_array`, which runs every `.init_array` entry, after the services are up and before `main` (libnx `nx/source/runtime/init.c`, `__libnx_init`, the "Call constructors" step).

A crate still has to put its constructor in `.init_array` on Horizon, and it decides that from its own list of targets. `inventory` 0.3.24 lists Linux, Android, the BSDs and others, but not Horizon. On an unlisted target its constructor is a plain static with no link section, so it never runs and every `submit!` is silently lost (`inventory-0.3.24/src/lib.rs`, the `__do_submit` macro). The inventory patch adds Horizon to the list (`patches/inventory/src/lib.rs:543-547`). Check any other crate that registers at startup the same way: search it for `link_section = ".init_array"` and the `target_os` list around it.

## IPC buffers and transfer memory

[IPC](glossary.md#ipc) buffers matter because the kernel validates the whole buffer before a [service](glossary.md#service) sees the request. A buffer page must be readable and writable by the caller, and it must not have disallowed memory attributes. If a page is locked, the request can fail with `InvalidCurrentMemory`.

A [transfer memory](glossary.md#transfer-memory) is memory your process lends to another process. libnx sockets use one for the [bsd](glossary.md#bsd) service. With default settings, the size is:

```text
4 * round_up(0x40000 + 0x40000 + 0x2400 + 0xA500, 0x1000)
= 4 * 0x8D000
= 0x234000 bytes
= 2.20 MiB
```

Those 2.20 MiB come from malloc heap. While open, the pages have permission `0x0` for this process and attribute `0x1` locked. A2 did not call `socketExit`, so the pages were still locked after `app.run()` returned. sphaira's loader then issued one `fsFileRead` into a 3,184 MiB buffer, the whole heap. One bad page made the kernel return 2001-0106.

A3's heap check uses the same rule in code:

- read each heap block with `svcQueryMemory` (`memory.rs:143-162`);
- require read/write permission `0x3` (`memory.rs:114`, `memory.rs:162`);
- ignore only attributes `0x2`, `0x4`, and `0x10`; any other attribute, especially `0x1` locked, is a fail (`memory.rs:116-121`, `memory.rs:162`);
- list up to eight failing blocks with memory type, permission, and attributes (`memory.rs:189-232`).

libnx's normal file layer has a fallback for ordinary file I/O: if a large read or write fails with `0xD401`, it retries through a small stack buffer. sphaira's loader calls `fsFileRead` directly, so it does not get that fallback.

## Loaders: sphaira and stock hbloader

The [loader](glossary.md#loader) matters because your NRO runs inside its process and gives control back to it when the NRO exits.

| Loader behavior | sphaira 1.0.0 loader | stock [hbloader](glossary.md#hbloader) v2.4.5 | Effect here |
|---|---|---|---|
| Asset section requirement | Requires enough bytes for an asset section. | Reads the image size it needs. | Early NROs without icon+NACP crashed in sphaira before our code ran. `scripts/build_example.sh` now uses `nacptool` and `elf2nro --nacp --icon` (`scripts/build_example.sh:60-74`). |
| Next NRO read | Reads into its whole heap in one `fsFileRead`. | Reads only the image path described by source. | sphaira sees a locked page anywhere in the heap and aborts with 2001-0106. |
| 96 MiB rule | Holds back 96 MiB in the loader heap arithmetic. | Holds back 96 MiB only when NACP asks for automatic recording. | With the sphaira forwarder, malloc still measured 3,132.7 MiB after the gameplay-recording boost. |
| Menu sockets | sphaira menu releases sockets in its own `userAppExit`. | Not the tested path. | Shows the loader expects NROs to release what they lend to services. |

Crashes 1-3 were the asset-section case: only `hbl` appeared in the crash report, so no `bevy_horizon` code had run. A2's exit crash was different: `log2.txt` ended with `[Exit] app.run() returned Success` (`LOGS/log2.txt:70`), then the new crash report listed only `hbl` with result 2001-0106 (`LOGS/crash_reports/01791176509_05446530aca7e000.log:2`, `:52-54`, `:121-125`).

## Files and the SD card

The [sdmc](glossary.md#sdmc) filesystem matters because early diagnostics, Bevy assets, and crash evidence all go there. Most Switch SD cards are [FAT32](glossary.md#fat32). The practical limits are:

- **4 GiB minus 1 byte per file.** Do not plan a single asset archive above that.
- **Loose files cost directory work.** Avoid tens of thousands of files in one folder.
- **Case is not a Linux test.** FAT32 is case-insensitive. A casing bug may disappear on the Switch and still break on a case-sensitive desktop path.
- **`sdmc:/` paths aren't absolute to Rust.** Rust's `Path` counts only a leading `/` as absolute, so `Path::new("sdmc:/switch/game").is_absolute()` is false, and `base.join("sdmc:/...")` appends to `base` instead of replacing it. The bevy_asset patches keep such asset roots as they are (`under_base_path`, `patches/bevy_asset/src/io/file/mod.rs:31-45`). Relative paths are safer: at startup libnx changes the working directory to the folder of `argv[0]`, the NRO's own path (`__libnx_init_cwd` in libnx's `nx/source/runtime/devices/fs_dev.c`), so `assets/x.png` means the folder beside the NRO.
- **Writes are expensive enough to show in frames.** A2's heartbeat frame writes three log lines and also hits the once-per-second `fsync`. The gaps between heartbeat log lines were about 2.7-2.9 ms, then about 6.0-7.0 ms, and the worst frame in each 10 s window was about 19.7-20.8 ms after startup.

The code is direct: every 10 seconds, `flush_logs_system` logs the heartbeat and a window of work times, and once a minute malloc figures (`lifecycle.rs:737-770`; every heartbeat until `mallinfo` was found to take about 41 ms in Skate 3). Once a second, it has `log.txt` synced. A2's build counted frames instead, 600 and 60, which at its 60 fps is the same. In Skate 3 these writes and syncs held heartbeat frames for about 20 ms, and once 1.2 s, so a log writer thread now does them, and the frame only queues its lines (`lifecycle.rs:722-799`; [logging-and-crash-reports.md](logging-and-crash-reports.md#flushing-and-the-cost-of-flushing)).

[RomFS](glossary.md#romfs) is optional. `HorizonLifecyclePlugin` tries `romfsMountSelf` and treats 2345-0010 as normal when the NRO has no RomFS (`lifecycle.rs:23-31`, `lifecycle.rs:51-58`). A2 logged:

```text
romfsMountSelf: 0x1559 = 2345-0010 (libnx IoError): this NRO has no RomFS, which is normal unless elf2nro was given --romfsdir
```

That is from `LOGS/log2.txt:24`. Assets today come from the SD card through `BEVY_ASSET_ROOT`, set before `main` (`logging.rs:120-155`).

## Panics, exits, and crash reports

Where you land is evidence. A clean exit and a Rust panic both go back to the loader. A CPU exception and a libnx fatal error end the process and return to the [HOME menu](glossary.md#home-menu) with an Atmosphère [crash report](glossary.md#crash-report).

| End | Where you land | Files you should read | Why |
|---|---|---|---|
| `main` returns after (+) | sphaira menu, if the loader continues | `log.txt` | `main -> exit -> _exit -> __libnx_exit -> __nx_exit` returns to the loader. |
| `std::process::exit` | sphaira menu, if the loader continues | `log.txt` | It also reaches newlib `exit`. |
| Rust [panic](glossary.md#panic) | sphaira menu | `log.txt`, `panic.txt` | The target has `panic-strategy = abort`; newlib `abort()` calls `_exit(1)` here. |
| CPU exception | HOME menu with new report | `crash.log`, Atmosphère report | libnx enters the exception handler, then calls `svcBreak`; creport terminates the process. |
| libnx fatal result | HOME menu with new report | `abort.log`, Atmosphère report | `diagAbortWithResult` records the result and calls `svcBreak`. |
| Closing from HOME without an exit lock | HOME menu, usually no report | Last synced `log.txt` | The system ends the whole loader process. `userAppExit` does not run; the kernel frees the process. |
| Loader crash after our exit | HOME menu with new report | End of `log.txt`, Atmosphère report | Our NRO ended, then hbl failed loading the next NRO. This is what A2 did. |

If you see sphaira's menu, read the end of `log.txt` before assuming success. If you see HOME with a new report, check whether the module list names your NRO or only `hbl`.

## Result codes

A Horizon [result code](glossary.md#result-code) is a packed integer. libnx prints and `bevy_horizon` formats it this way (`logging.rs:351-358`):

```text
module      = code & 0x1ff
description = (code >> 9) & 0x1fff
shown       = (2000 + module)-description
```

Two real examples:

| Raw | Decoded | Meaning here |
|---:|---|---|
| `0xD401` | 2001-0106 | Kernel module 1, description 106: `InvalidCurrentMemory`. sphaira's loader tried to use the whole heap as an IPC write buffer while 2.20 MiB were locked (`crash_reports/01791176509_05446530aca7e000.log:2`). |
| `0x1559` | 2345-0010 | libnx module 345, description 10: `IoError`. `romfsMountSelf` found no RomFS, which is harmless for this NRO (`LOGS/log2.txt:24`). |

## What is proven, expected, and untested

| Area | Status |
|---|---|
| Launch from sphaira forwarder | Proven after the asset-section fix. |
| Rust main thread TLS | Proven. |
| Rust new thread TLS | Proven by A2's joined thread check. |
| 60 Hz Bevy frame loop | Proven: frame 600 through 4200 stayed about 59.9-60.0 fps (`LOGS/log2.txt:46-66`). |
| SD-card file and Bevy AssetServer load | Proven by A2 (`LOGS/log2.txt:31-38`). |
| Memory total and heap | Proven for the forwarder launch path: 3,189.0 MiB total, 3,132.7 MiB malloc heap. |
| A2 exit | Proven not clean overall: our code returned Success, then sphaira's loader crashed with 2001-0106. |
| A3 exit fix | Proven: hello_switch's exit in hardware run 5 logged PASS and returned to sphaira with no crash report (`LOGS/hello_switchlog.txt:84-95`). |
| Docked mode | Code exists; not tested. |
| Rendering | Works through wgpu's Vulkan backend and NVK: in hardware run 5, nvk_probe and hello_switch drew at 60 fps and exited cleanly. render_check lost the GPU device about 4 seconds in, still open ([rendering.md](rendering.md)). |
| Audio | Proven: through SDL2, captest's bevy_audio checks passed on hardware, and audio came back after HOME (Bevy 0.19). Through the audren host, Skate 3's sound played clean, came back after HOME and after sleep, and closed at exit (Bevy 0.18; perf35, findings 54). |
| Small-block allocator cache | Expected to work: the host stress test is clean, also under AddressSanitizer and ThreadSanitizer, and in Ryujinx Skate 3 ran five minutes with it (`sk8zm`), drew as before, and the cache served 96% of the small allocations made during play. Not yet run on hardware. |

## Sources used for these notes

The repo files are the primary source for code locations. Hardware facts come from the current logs and reports in `iw4l-switch-findings/LOGS/`. Upstream citations, copied from the findings, include libnx v4.12.0 (`init.c`, `newlib.c`, `bsd.c`, `tmem.c`, `fs_dev.c`, `nv.c`, `audren.c`, `hid.c`, `svc.h`), Atmosphère commit `c8b7316` (`KPageTableBase`, `KServerSession`, `pm_spec.cpp`), sphaira 1.0.0 (`hbl/source/main.c`, `sphaira/source/main.cpp`, `owo.cpp`), nx-hbloader v2.4.5 (`source/main.c`), devkitPro newlib `57bd8e1`, and Rust `db8f076d2`. The section on NVK's top-down region also rests on devkitPro newlib `57bd8e1` (`libgloss/libsysbase/sbrk.c`, `newlib/libc/stdlib/mlock.c`) and mesa-switch `d4a00ea` (`src/nouveau/horizon/nouveau_horizon_memory.c`, `nouveau_horizon_private.h`, `nouveau_horizon_runtime.c`, `src/util/os_misc.c`). The section on the small-block cache rests on devkitPro newlib `57bd8e1` (`newlib/libc/stdlib/_mallocr.c`), Atmosphère commit `36cc9a9f` (`svc_select_thread_local_region.hpp`, `svc/arch/arm64/svc_thread_local_region.hpp`, `kern_k_thread_local_page.hpp`, `kern_k_scheduler.cpp`, `kern_svc_thread.cpp`), libnx `feebd026` (`newlib.c`, `__syscall_nanosleep`), and Jeff Bonwick and Jonathan Adams's [*Magazines and Vmem*](https://www.usenix.org/legacy/event/usenix01/bonwick.html) (USENIX 2001).
