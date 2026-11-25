# deko3d

This page is for anyone who wants a Bevy game to draw through [deko3d](https://github.com/devkitPro/deko3d), the Switch's own GPU API, instead of Vulkan and [NVK](glossary.md#nvk), and for contributors to the backend. It is an alternative to the default renderer on the `deko3d` branch; [Rendering](rendering.md) describes the default.

## In short

- **Turn on bevy_horizon's `deko3d` feature instead of `vulkan`, and give Bevy's `RenderPlugin` `bevy_horizon::deko3d::render_creation()`.** [hello_switch](glossary.md#hello_switch) does this with its own `deko3d` feature: `FEATURES=deko3d scripts/build_example.sh`.
- **Build UAM and deko3d first:** `scripts/build_uam.sh` leaves `deko3d/lib/libuam.a`, deko3d's shader compiler as a library, and `deko3d/lib/libdeko3d_bh.a`, deko3d itself with a blend fix (below). Shaders are compiled on the console, from the GLSL naga writes for each pipeline, the first time a game uses them; the [shader cache](#shader-cache) keeps them on the SD card for later launches.
- **The game's code doesn't change.** `crates/wgpu_deko3d` is a [wgpu](glossary.md#wgpu)-hal backend, so Bevy and the game use wgpu as usual.
- **It saves most where the driver costs CPU time.** In Horizon's GPU-bound benchmark deko3d runs at 25.7 fps against NVK's 24.4, with better lows; CPU-bound ports have run about twice as fast ([Performance in a game](#performance-in-a-game)). A wgpu 30 game vendors the backend ([Using it from a wgpu 30 game](#using-it-from-a-wgpu-30-game)).
- **What doesn't work yet** is listed under [Limits](#limits): above all, a depth texture that a shader both compares against and reads directly.

## How it fits together

| Piece | What it does |
|---|---|
| `crates/wgpu_deko3d/native/bridge.c` | Thin C over deko3d with plain structs: devices, the queue and fences, memory, images, samplers, pipelines, command recording, swapchains. deko3d's state structs are bitfields, which Rust can't build directly |
| `crates/wgpu_deko3d/src` | The `wgpu_hal::Api`: bind group slots, texture/sampler pairing, immediates, queries, surface |
| `crates/wgpu_deko3d/src/shader_cache.rs` | Keeps UAM's output on the SD card, so launches after the first skip compiling ([Shader cache](#shader-cache)) |
| `patches/uam/console-library.patch` | UAM v1.1.0 as a static library with a C API, and three scheduling fixes (below) |
| `patches/deko3d` | deko3d v0.5.0, the source of devkitPro's package, with its alpha blend factor fixed, its release library's errors reported, and a multi-draw indirect macro added (below) |
| `crates/bevy_horizon/src/deko3d.rs` | Creates the device and queue Bevy's automatic setup would, on deko3d's one adapter |
| `patches/wgpu-hal` | Two accessors an out-of-tree backend needs: `BufferBinding::buffer` and `InstanceError::new` |
| `patches/naga` | naga 29.0.4's GLSL writer, changed for the depth reads below |

**Shaders.** wgpu hands the backend naga IR it has validated. The backend writes GLSL 4.50 with naga, numbering each resource class per stage across bind groups, so a bind group's slots depend only on the groups before it, and compiles it with UAM. GLSL combines each texture with its sampler; the backend binds a texture handle made from both at draw time, from the pairs naga reports. Immediates (push constants) become a uniform buffer that the encoder fills before each draw or dispatch that follows a change.

**Depth reads.** Upstream naga declares every depth texture `sampler2DShadow`, which only comparisons can use, so `textureLoad`, gathers and plain samples of depth failed (SSAO's depth prepass). The patched writer declares a depth texture no shader in the module compares against as a plain sampler and takes the depth from `.x`. Shadow lookups at level 0 on cube arrays (point lights) used `textureLod`, which needs `GL_EXT_texture_shadow_lod` and UAM lacks it; they use core `texture` instead, which reads level 0 outside fragment shaders and, inside them, as long as the shadow map has one mip level.

**Memory.** Buffers up to 1 MiB are slots in shared 64 KiB+ blocks: wgpu-core makes a staging buffer for each `queue.write_buffer`, and mapping a block per buffer cost about 0.3 ms. Command memory is pooled the same way. Every block is CPU mapped (CPU uncached). Mappable buffers that only copies touch (staging, readback) are GPU uncached too, so neither side needs cache maintenance. All others are GPU cached, mappable ones the GPU draws or computes with included (`MAPPABLE_PRIMARY_BUFFERS`): the CPU writes those only between submissions, deko3d starts each submission by invalidating the GPU's L2, and the fence of every submission flushes it before wgpu maps a buffer for reading.

**Features beyond WebGPU's.** `SHADER_EARLY_DEPTH_TEST`: naga writes `layout(early_fragment_tests) in;`, UAM records it in the program's header and deko3d forces the hardware's early depth test for the program (Horizon's occlusion culling needs this: its box shader writes storage, which the depth test must stop). `VERTEX_WRITABLE_STORAGE`: storage buffers and images bind per stage, the vertex stage too. `MAPPABLE_PRIMARY_BUFFERS`, as above. hello_switch's `[Check] early depth`, `vertex storage` and `mappable primary` prove each on the console: fragments that count themselves in storage behind an occluder ran 0 times with the forced early test and 4096 of 4096 without it; 256 of 256 vertex shader writes landed; three rounds of values written through a VERTEX | MAP_WRITE mapping came back right through a STORAGE | MAP_READ one.

**Submission.** The GPU reads command memory through a ring of 0x800 gpfifo entries, and deko3d ends an entry at every full barrier (two), every chunk of command memory, and every indirect draw or dispatch (two each). deko3d appends a list's entries to libnx's channel and kicks them off when it is nearly full or the queue is flushed, and never waits for the ring to have room. A single command list with thousands of entries, such as a texture array uploaded one `write_texture` per layer, failed in `Queue::appendGpfifoEntries`; and when the GPU fell behind, as Horizon's first frames at render distance 12 make it, a kickoff of 62 entries failed with libnx's Timeout (`0xd5c`) because the ring was still full of earlier submissions (both LIBNX ABORT 2359-0001). The bridge cuts each recording into deko3d command lists of about 128 entries and counts each list as the larger of the recording's generous estimate and the entries its control commands make (deko3d 0.5.0's `CtrlCmdHeader` stream: each `GpfifoList`'s entries plus one for the queue's own commands). `dkp_submit` never waits for the GPU: the submission joins a backlog, and lists go to the queue in batches of at most 384, each kicked off with a fence, while the kickoffs the GPU hasn't finished, presents included, stay within 1792 entries (256 are left for deko3d's own kickoffs when its queue's command memory runs low). A fence poll sends what then fits; a present, a fence wait, `dkp_wait_idle` or a new swapchain waits for the GPU until the backlog is on the queue, and a submission's fence is signaled only after its last list. Before, `dkp_submit` itself waited for room within 1024 entries, a wait for the GPU that Horizon counts as CPU time: in the forest benchmark (before the multi-draw patch) 26–60 ms of each frame, and the frame took 49.5 ms at the median; with the backlog, submit takes 0.2 ms, the ring stays about 1680 entries deep, and the frame takes 33.9 ms (26.0 fps over 2 runs, from 21.2; NVK 24.4). Back-to-back barriers, such as wgpu-core's buffer then texture transitions before a copy, are merged into one. hello_switch's `[Check] upload` sends 6000 `write_texture` calls in one submission and reads the texture back. Every 10 s of presented frames the backend logs (`deko3d::timing`) the time per frame the bridge spent in each queue operation (ring waits, kickoffs, acquire, present, fence polls and waits, the queue lock) and the gpfifo entries per submission.

**Queue memory.** deko3d's queue keeps command memory of its own for fences, compute jobs and presents: 64 KiB in 16 fenced slices by default, kicked off whenever 8 KiB are in flight. Once the backlog let Horizon's frames run ahead, and occlusion culling added compute dispatches, `dkQueueSubmitCommands` waited there for the GPU: 26–31 ms of each frame, plus 9–16 ms of kickoffs, inside `dkp_submit`. The bridge creates the queue with 1 MiB of command memory and a 256 KiB flush threshold; both fell to about 0.1 ms, the wait for the GPU moved to present, and the forest benchmark's 1% low rose from 6.2 to 16.2 fps (p99 frame 116.5 → 59.7 ms; NVK 14.3 fps and 69.5 ms).

**Multi-draws.** deko3d draws an indirect draw with one macro call whose parameters the GPU's command fetch reads from the buffer as a gpfifo entry of its own, not prefetched. Horizon's world, one `multi_draw_indexed_indirect` of about 500 sections a frame, was therefore about 1000 entries and 500 fetch stalls a frame, and its render thread waited inside `dkp_submit` for ring room. `patches/deko3d/multi-draw-indirect.patch` adds `dkCmdBufMultiDraw(Indexed)Indirect`: one call of a looping macro (`DrawMulti`, `DrawIndexedMulti`) reads up to 1024 packed draws from one entry, as NVK's multi-draw does. In the forest benchmark, gpfifo entries fell from about 580 to 26 per presented submission and the render thread's CPU time per frame from 26.6 to 6.4 ms.

**Barriers.** Each wgpu-hal transition gets the weakest deko3d barrier its usages need (`buffer_barrier`, `texture_barrier` in `src/commands.rs`): none for reads after reads, copies after copies (the copy engine runs them in order) and render targets after render targets; `Fragments` (plus the texture cache) for sampling what was rendered, or rendering over what was sampled; `Primitives` around shader stores, which also waits for compute jobs (the bridge upgrades a `Fragments` to it before a dispatch); `Full` only where the copy engine, the 2D engine or the command fetch (indirect arguments) is on one side. A recording ends with a `Full` barrier only if it gave the copy, 2D or compute engine work, since the queue's fence is signalled from the 3D engine. Query resets zero the reports from the 3D engine's command stream, so they need no barrier against the reports. Horizon's frames went from about 43 Full barriers to about 9. A depth image copied into, or new where an old one was, gets its ZCULL data invalidated. The log line `per second over …` (`wgpu_deko3d::commands`) counts barriers by kind, draws and gpfifo entries every 5 s.

**Indirect-call validation.** `wgpu_deko3d::instance()` wraps the backend with `wgpu::Instance::from_hal`, and wgpu-core 29 gave such an instance `InstanceFlags::default()`, which keeps `VALIDATION_INDIRECT_CALL` in release builds. The adapter reports `INDIRECT_EXECUTION` and `COMPUTE_SHADERS`, so wgpu-core checked every indirect draw with a compute pass of its own, which bevy-horizon's patch already dropped for Vulkan in `Instance::new`. The patch now applies the same rule to an instance made from a hal instance: the flag stays only alongside `VALIDATION`, as in debug builds ([Indirect-call validation](rendering.md#indirect-call-validation)). Horizon's wgpu-core 30 patch does the same.

**UAM fixes.** Upstream UAM read an instruction's scheduling data before setting it, so stall counts depended on heap contents and the console's output differed from the host tool's; ended a block with a stall of 1 after an instruction that sets a barrier, where Mesa uses 2; and dual-issued instructions, which Mesa never does on GM107 and which has been reported to corrupt lighting. With the patch, the console and a host build of the same source produce identical bytes.

**deko3d fixes.** devkitPro's deko3d 0.5.0 writes a blend state's alpha destination factor from its color one ([devkitPro/deko3d#27](https://github.com/devkitPro/deko3d/issues/27), open and unfixed on master), so a blend whose two differ, such as color blended over while alpha keeps the destination's, came out wrong. Its release library also aborts on an error with only the `DkResult` (LIBNX ABORT 2359-0001) and drops the GPU's fault details. `scripts/build_uam.sh` builds v0.5.0 with the one-line blend fix and `patches/deko3d/release-reports.patch`, which hands the release library's errors and warnings, and a failed kickoff's libnx result, to the bridge's `dkBhReport`; it builds the debug library too, under `_bh` names so the link can't pick devkitPro's copies in `libnx/lib`.

## wgpu versions

One backend source serves two wgpu releases. `crates/wgpu_deko3d/Cargo.toml`, which bevy-horizon builds, is on wgpu 29 (Bevy 0.19). `Cargo.wgpu-30.toml` is the same package on wgpu, wgpu-hal, naga and wgpu-types 30.0.1 with the `wgpu-30` feature on; that feature selects the few places where wgpu-hal 30 differs (renamed dispatches, ray-tracing pipelines this backend reports unsupported, a shared submit fence, `Queue::wait_for_idle`, per-format surface color spaces, gaps in a pipeline's vertex buffers). Cargo never reads that manifest here; enabling `wgpu-30` with the wgpu 29 manifest doesn't build.

A Bevy 0.19 game built with `scripts/build_game.sh` gets the backend from this repository and the `patches/wgpu-hal` and `patches/naga` it needs from `.cargo/bevy-0.19.toml`, like the rest of the [Bevy 0.19 set](toolchain.md#the-bevy-019-set). A game on wgpu 30 vendors the crate: `scripts/vendor_deko3d.sh <game checkout>` replaces `<game>/third_party/wgpu_deko3d` with `src/`, `native/`, `build.rs` and the wgpu 30 manifest as its `Cargo.toml`, plus a README naming the commit it came from. Change the backend here and run the script again rather than editing the copy. The game patches crates.io's wgpu-hal 30 with the same two accessors as `patches/wgpu-hal` and naga 30 with the same [depth reads](#how-it-fits-together), and builds with `BEVY_HORIZON_UAM_LIB` pointing at this repository's `deko3d/lib`.

## Using it from a wgpu 30 game

Horizon, a Rust voxel game on wgpu 30 without Bevy (`JohnUzoka/horizon`, branch `deko3d`), is the worked example:

| Part | In Horizon |
|---|---|
| The backend | `third_party/wgpu_deko3d`, written by `scripts/vendor_deko3d.sh` |
| The patches | `third_party/wgpu-hal` (the two accessors) and `third_party/naga` (the depth reads, described in its `THIRD_PARTY_NOTICES.md`), in the workspace's `[patch.crates-io]` |
| The feature | `deko3d` on the Switch build, passed down from `horizon-game` through `horizon-client` to `horizon-client-render` and `horizon-nx`; with `vulkan` too, a `compile_error!` stops the build |
| The device | `crates/horizon-client-render/src/gfx/device.rs` builds no wgpu backend of its own (`Backends::NOOP`) and takes its instance from `wgpu_deko3d::instance()`; the rest of the renderer is unchanged |
| The build | `packaging/switch/build.sh deko3d [out.nro]` (the default is `vulkan`). It links no NVK, and finds `libuam.a` and deko3d in `HX_DEKO3D_LIB`, by default `~/bevy-horizon/deko3d/lib`, the output of this repository's `scripts/build_uam.sh` |

Its occlusion culling, which needs `SHADER_EARLY_DEPTH_TEST` ([Features beyond WebGPU's](#how-it-fits-together)), runs on deko3d as on NVK (`gfx/cull.rs`).

## Shader cache

UAM compiles a shader on the console in 5 ms to 3 s; Bevy's PBR fragment shader, 96 KB of GLSL, takes 2.9 s. Bevy without `multi_threaded` creates pipelines on the main thread as it first needs them, so each compile holds up a frame. hello_switch compiles 33 programs (21 of them compute) in its first three frames, 6.1 s of UAM, and without a cache the screen keeps its first frame for 6.4 s. That frame is black: Bevy presents it before its blit pipeline exists, so nothing draws to it, and the backend clears the swapchain's images when the surface is configured. Before the clear, it showed whatever the memory last held, such as a scrambled frame of the previous app.

The backend therefore keeps every program it compiles in `sdmc:/.deko3d/shader_cache/uam-<UAM>-v<format>/`, one file per program, as NVK keeps its own in `sdmc:/.mesa`. Games share it: a program is found by a hash of everything that decides UAM's output, which is the GLSL, the stage, the bytes of the linked `libuam.a` (`<UAM>` in the directory name) and the cache's format. `scripts/build_uam.sh` writes `libuam.a` without timestamps, so rebuilding the same UAM keeps the same directory. A new UAM build or format starts a new directory and leaves the old one.

- **Writes are atomic:** a file is written under a temporary name and renamed into place.
- **Reads are checked:** a file is used only if its header, key, length and checksum all match. Otherwise, and on any SD card error, the shader is compiled as if there were no cache, a damaged file is replaced, and `log.txt` says why.
- **To clear it,** delete `sdmc:/.deko3d/shader_cache` (or one `uam-…` directory in it). Nothing else refers to it.
- **`log.txt` shows each program** under `deko3d::shader_cache`: compiled (and how long it took) or read (and how much time that saved), with running totals. Programs created while `DefaultPlugins` are being added, before Bevy's log is set up, don't appear (two of hello_switch's), but the totals count them.
- **Writing costs about 28 ms a program** on the SD card, on the thread that compiled it: 0.9 s of hello_switch's first launch. A writer thread would hide it, but one still running when the game exits keeps its stack locked, and the homebrew loader can't load the next program into a locked heap.

hello_switch on the console, in seconds from the device opening at the start of `main` (loading the NRO comes before that):

| Launch | First frame | First 60 frames | Stall after the first frame | Shaders |
|---|---|---|---|---|
| Before the cache | 2.06 | 9.56 | 6.45 | 33 compiled, 6.13 s |
| First, empty cache | 2.27 | 10.42 | 7.07 | 33 compiled (6.12 s) and stored (0.91 s) |
| Later, warm cache | 1.30–1.32 | 3.56–3.58 | 1.27 | 33 read in 0.19 s; 5.9 s saved |

Most of what remains of the stall, about 1.1 s, comes before the backend sees a shader: Bevy composing its shaders and wgpu parsing and validating the WGSL (0.4 s for the PBR shader alone). NVK does the same work. The backend's part, writing GLSL and reading programs, is 0.18 s.

## Performance in a game

Horizon's forest benchmark (`--benchmark forest`: seed 1234, render distance 12, 1280×720, a fixed 12 s flight, a fixed player name so the same skins load) on a Switch Lite at stock handheld clocks, measured with `hx-bench.py` (outside this repo). "Before" is the backend before the changes listed under the table.

| | NVK (3 runs) | deko3d before (2 runs) | deko3d now (2 runs) |
|---|---|---|---|
| Average fps | 24.4 | 21.2 | 25.7 |
| 1% low fps | 14.3 | 11.6 | 16.1 |
| Frame time, p50 / p99 (ms) | 35.8 / 69.5 | 49.6 / 83.2 | 34.7 / 60.0 |
| Render thread CPU, p50 (ms) | 7.1 | 31.1 | 6.6 |
| GPU, p50 (ms) | 34.2 | 32.4 | 33.8 |

What moved the numbers, each described under [How it fits together](#how-it-fits-together):

- **Multi-draws:** gpfifo entries per presented submission fell from about 580 to 26, and with them most of the render thread's CPU time.
- **Submission:** `dkp_submit` no longer waits for the GPU; submissions join a backlog, and the median frame fell from about 49.5 to 33.9 ms.
- **Barriers:** the weakest barrier each transition needs, about 9 `Full` barriers a frame instead of about 43.
- **Occlusion culling,** turned on for deko3d in Horizon: about 2.4 ms less GPU time where terrain is hidden, about 1 ms more in this flight, which sees most of what it draws.
- **Queue memory:** 1 MiB instead of 64 KiB, so `dkQueueSubmitCommands` stopped waiting for the GPU (16–31 ms a frame before). This is most of the change in the 1% low and p99.

Tried without a gain:

- **`DkImageFlags_HwCompression` on color targets:** GPU 36.3 ms a frame with it against 35.9 ms without, so no faster; not merged.
- **Swap interval 0 (Mailbox's behaviour) instead of 1:** 25.7 fps, the same, with one hitch. The surface offers only Fifo, which sets interval 1 (`src/surface.rs`).

**Frames come in whole refreshes.** Both backends present with Fifo at 60 Hz, so a frame takes a whole number of 16.7 ms refreshes: here 33.4 or 50 ms. deko3d's median GPU time, 33.8 ms, sits on the two-refresh line: frames whose GPU work ends just after it wait for the next refresh. A GPU saving shows in the frame rate only as far as it moves frames under 33.4 ms.

**What the GPU time is made of.** Lowering Horizon's resolution scale (one run each at 75% and 50%):

| Resolution scale | Pixels | GPU, p50 (ms) | Average fps |
|---|---|---|---|
| 100% | 1 | 33.8 | 25.7 |
| 75% | 0.56 | 27.7 | 31.7 |
| 50% | 0.25 | 20.7 | 39.0 |

A straight line through these puts about 17 ms of the 33.8 on the pixels and about 16 ms on work that doesn't scale with them, such as geometry and culling. That split is an inference from the fit, not a measurement of each pass.

**Why deko3d doubled other ports and not Horizon.** sa2-nx and nfsmw-nx, two other Switch ports, ran about twice as fast on deko3d as on NVK because they were CPU-bound: PM4 command emulation plus NVK's cost per draw, while SA2's GPU needed only 5–6 ms of its 16.7 ms frame. Horizon draws its world with one multi-draw a frame, so NVK's per-draw cost was small to begin with (7.1 ms of CPU), and its frame is GPU-bound at about 34 ms on both backends. deko3d's gain there is in the lows and in the CPU time left for the game; the frame rate follows the GPU.

## Limits

- **A depth texture one shader both compares against and reads directly fails pipeline creation,** as does reading depth in a module that passes a depth texture to a function: GLSL would need two sampler types for one binding. None of hello_switch's pipelines do either.
- **UAM emulates non-constant integer division in floating point** and warns about it in `log.txt` (`program uses non-constant integer division/modulo`): one of hello_switch's shaders divides integers this way, and results lose precision past 2^24.
- **One texture with two samplers in one shader fails pipeline creation:** GLSL combines them at the texture's binding.
- **Not supported:** atomic storage textures, binding arrays, pipeline-statistics queries, multiview, conservative rasterization, rendering into a 3D texture's slice, gaps in a pass's color attachments, dual-source blending, `multi_draw_indirect_count`.
- **Bevy 0.19 only:** the `deko3d` feature turns on `render`, which is for Bevy 0.19. A wgpu 30 game vendors the backend ([Using it from a wgpu 30 game](#using-it-from-a-wgpu-30-game)).
- **The swapchain presents with Fifo only.** The surface reports no other present mode.

Open questions, not measured yet:

- **Whether ZCULL engages for a game's depth.** The backend invalidates a depth image's ZCULL data when it is copied into or replaced, but no measurement shows how much the coarse depth test rejects in Horizon's passes.
- **The compute-job ring.** deko3d's queue has 128 compute-job slots (`maxConcurrentComputeJobs`, which the bridge leaves at deko3d's default); when they run out, it waits for the compute engine to go idle before reusing them (`source/maxwell/gpu_compute.cpp`). That wait is the next suspect for GPU stalls in games that dispatch often, such as Horizon's culling.
- **Title-screen GPU time** in Horizon grew from 6–8 ms to 19–23 ms after minutes in its menus; not investigated.

## Testing

`~/wgpu-deko-proof` (outside this repo) runs the same wgpu program on deko3d and NVK and compares their output: a lit, textured scene, and twelve feature tests (compute, storage textures, MSAA, immediates, queries, cube/array/3D textures, depth comparison, stencil, copies, indirect draws). Every one matched NVK byte for byte on the console.

**The blend fix** was checked with a small deko3d program (outside this repo) that blends with differing color and alpha destination factors over a known clear and reads the target back: devkitPro's library gave alpha 0 where the destination's 0.5 should survive and 0.75 where 0.25 was expected; the patched one gives 127 and 64 of 255, as expected, with the same color results.

**When the GPU faults** or deko3d gives up, the bridge appends deko3d's report to `sdmc:/switch/deko3d.log`, from the release library too: the engine class and the method a fault rejected, or the libnx result of a failed kickoff. It also notes there a submission that waited half a second or more for the GPU to make room on the gpfifo ring. `BEVY_HORIZON_DEKO3D_DEBUG=1` at build time links deko3d's debug library, which also reports API misuse there; it takes effect only when you run Cargo yourself, since `build_example.sh` and `build_game.sh` don't pass it into their containers.
