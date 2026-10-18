# Rendering

This page is for anyone whose Bevy game draws on the Switch: the Bevy features and render settings a game needs, what to expect on the console, how to diagnose a lost GPU device, the known issues, and what the [emulator](glossary.md#emulator) gets wrong. From [How a frame reaches the screen](#how-a-frame-reaches-the-screen) on, it describes the internals, for contributors.

## In short

- **Bevy draws through [wgpu](glossary.md#wgpu)'s [Vulkan](glossary.md#vulkan) backend and [NVK](glossary.md#nvk), which is linked into the [NRO](glossary.md#nro).** Turn on bevy_horizon's `vulkan` feature. NVK is [Mesa](glossary.md#mesa)'s Vulkan driver for NVIDIA GPUs, built for the Switch by [mesa-switch](glossary.md#mesa-switch); Horizon has no driver loader, so the driver is part of the executable ([Linking NVK](toolchain.md#linking-nvk-the-vulkan-feature)).
- **It works on the console.** On a Switch Lite, [hello_switch](glossary.md#hello_switch)'s lit cube holds 60 fps, Bevy's [Breakout](glossary.md#breakout) plays to the end at about 59 fps, [render_check](#render_check) draws all 24 of its cubes, and a full 3D Bevy 0.18 game reaches about 30 fps in its heaviest view ([On the Switch](#on-the-switch)).
- **A 3D game needs Bevy's `tonemapping_luts`, `ktx2` and `zstd_rust` features,** which Bevy's defaults include. Without them, whatever a camera with Bevy's default tonemapping draws comes out magenta ([Bevy features a game needs](#bevy-features-a-game-needs)).
- **The default settings are safe; a 3D game should also set `BH_RT_COMPRESSION=1`,** which saves GPU time on the console. Set variables with `std::env::set_var` at the start of `main` ([Render settings](#render-settings)).
- **Large 3D games are GPU-bound at [handheld](glossary.md#handheld-mode) clocks.** The first launch also compiles every shader; NVK caches them on the [SD card](glossary.md#sdmc) under `sdmc:/.mesa/`, so later launches start faster ([Performance](#performance)).
- **When the GPU stops, the log says why.** Every Horizon build logs NVK's own messages, the Vulkan error and the wgpu call that lost the device. A [panic](glossary.md#panic) after a [lost device](glossary.md#device-lost) is a consequence, not a second bug ([Diagnosing a lost device or a GPU hang](#diagnosing-a-lost-device-or-a-gpu-hang)).
- **One Bevy 0.18 scene hangs the GPU,** for a reason not yet known ([Known issues](#known-issues)).
- **Stock [Ryujinx](glossary.md#ryujinx) can't run the Vulkan builds.** NVK hits emulator bugs at startup and on the first frames, and the project's fixes are local and unpublished. The emulator's speed says nothing about the console's ([What the emulator gets wrong](#what-the-emulator-gets-wrong)).

## Bevy features a game needs

A game turns these on in its own `bevy` dependency; `examples/hello_switch/Cargo.toml` has a working list.

| Feature | Why |
|---|---|
| `tonemapping_luts`, `ktx2`, `zstd_rust` | Bevy's default tonemapper, TonyMcMapface, reads a lookup table that ships only with `tonemapping_luts`. The table is a KTX2 file compressed with Zstandard: `tonemapping_luts` turns on `ktx2` itself, and it also needs a Zstandard decoder. `zstd_rust` is the pure-Rust one, which builds for Horizon without a C toolchain. Bevy's `3d` feature set, part of its defaults in 0.18 and 0.19, includes all three |
| `bevy_ui_render`, `bevy_text`, `default_font` | UI and text, [the FPS overlay](#the-fps-overlay) included |

**Without the table,** Bevy logs an error that begins ``TonyMcMapFace tonemapping requires the `tonemapping_luts` feature`` and samples a one-texel magenta placeholder instead (bevy_core_pipeline 0.19.1, `tonemapping/mod.rs`, `lut_placeholder`). Whatever the camera tonemaps then comes out magenta: its meshes, or with HDR its whole view. Bevy's own note on the feature says the same: "If everything is pink, you need to enable this feature" (its `Cargo.toml`, in 0.18 and 0.19). To ship without the table, give every camera a `Tonemapping` that doesn't use one: `None`, `Reinhard`, `ReinhardLuminance`, `AcesFitted`, `SomewhatBoringDisplayTransform` or, in Bevy 0.19, `KhronosPbrNeutral`. `AgX`, `BlenderFilmic` and `TonyMcMapface` need it.

## Render settings

These settings change how wgpu and NVK draw on Horizon. The defaults are safe; change one for speed or to track down a bug.

| Setting | Default | Change it to | Details |
|---|---|---|---|
| `BH_ROBUST_BUFFER_ACCESS` | `clamp`: naga keeps every shader's buffer accesses in bounds | `driver` to find an index bug; `off` only to measure what clamp costs | [Buffer robustness](#buffer-robustness) |
| `BH_RT_COMPRESSION` | Off | `1` for a 3D game on the console | [Render-target compression](#render-target-compression) |
| wgpu's indirect-call validation | Off in release builds, on in debug builds | On, through `WgpuSettings`, to check indirect draws your game writes | [Indirect-call validation](#indirect-call-validation) |
| `BEVY_HORIZON_ZCULL_DIR` | `greater`: [ZCULL](glossary.md#zcull) set up for Bevy's reversed depth | `less`, NVK's own direction, only to compare | [Performance](#performance) |
| `MESA_SHADER_CACHE_DISABLE` | Unset: NVK caches compiled shaders on the SD card | `true` to compile every shader at every launch | [NVK's shader cache](#nvks-shader-cache-on-the-sd-card) |
| `NAK_DEBUG` | Switch builds turn on the [NAK](glossary.md#nak) optimisations that measured faster | `stats` to log what each shader costs | [logging-and-crash-reports.md](logging-and-crash-reports.md); the other flags, and `NAK_GPR_LIMIT`, in [toolchain.md](toolchain.md#linking-nvk-the-vulkan-feature) |

**Setting a variable.** Horizon has no shell and no launch arguments, so a game sets these itself, with `std::env::set_var` at the start of `main`, before it builds the App. bevy_horizon reads `BEVY_HORIZON_ZCULL_DIR` when `HorizonPlugin` is added. Bevy makes the GPU instance and device as its plugins are added: wgpu-hal reads `BH_ROBUST_BUFFER_ACCESS` when Bevy looks for a GPU and `BH_RT_COMPRESSION` when wgpu makes its first texture, and Mesa reads its own once Bevy starts using the driver.

```rust
fn main() {
    // SAFETY: nothing else has started yet, so no other thread reads the environment.
    unsafe {
        std::env::set_var("BH_RT_COMPRESSION", "1");
    }
    App::new()
        // ... your plugins and systems
        .run();
}
```

`set_var` is `unsafe` from Rust's 2024 edition on. To change a setting without rebuilding, read it from a file on the SD card: [Skate 3 Rust Engine](https://github.com/JohnUzoka/skate-3-rust-engine), a full 3D Bevy 0.18 game ported with bevy-horizon, reads `env NAME=VALUE` lines, such as `env BH_RT_COMPRESSION=1`, from an `options.txt` beside its NRO (branch `horizon-port`, `crates/skate-game/src/horizon.rs`).

### Indirect-call validation

With `InstanceFlags::VALIDATION_INDIRECT_CALL`, wgpu checks every [indirect draw](glossary.md#indirect-draw) before the GPU runs it: a [compute](glossary.md#compute-shader) pass zeroes the arguments of any draw that breaks wgpu's rules, such as one whose indices or instances run past the end of their buffers. Without the check, such a draw is undefined behaviour (wgpu-types 29.0.4, `InstanceFlags::VALIDATION_INDIRECT_CALL`). Bevy draws each batch with an indirect call by default, so the pass runs every frame.

- **On Horizon it's off in release builds and on in debug builds.** bevy_horizon's wgpu-core patch drops the flag unless `InstanceFlags::VALIDATION` is set too, as debug builds do (`Instance::new` in `patches/wgpu-core/src/instance.rs`, and in `patches/bevy-0.18/wgpu-core/src/instance.rs` for Bevy 0.18).
- **That's Bevy's own rule, applied to the only backend the Switch has.** In release builds bevy_render removes the flag unless its backends include DX12, because "Removing this flag improves performance" (`bevy_render-0.19.1/src/settings.rs:130-139`). Bevy's default backends name DX12 on every platform, so upstream the flag stays on; wgpu on Horizon has only Vulkan.
- **Bevy writes its indirect arguments itself, on the GPU,** so they should always be valid. If your game writes its own indirect draws, test them in a debug build or with the validation on.

To turn it on in a release build, insert both flags, as render_check's `indirect_validation` option does. On its own, `VALIDATION` asks for the Khronos validation layer, which the Switch doesn't have, and wgpu carries on without it.

```rust
use bevy::render::RenderPlugin;
use bevy::render::settings::{InstanceFlags, WgpuSettings};

let mut wgpu_settings = WgpuSettings::default();
wgpu_settings
    .instance_flags
    .insert(InstanceFlags::VALIDATION | InstanceFlags::VALIDATION_INDIRECT_CALL);
app.add_plugins(DefaultPlugins.set(RenderPlugin {
    render_creation: wgpu_settings.into(),
    ..default()
}));
```

### Buffer robustness

[Buffer robustness](glossary.md#buffer-robustness) keeps a shader's access past the end of a buffer from reaching other memory. Upstream, wgpu turns on the driver's robustness whenever the device has it, and NVK has it. Without any, an access past the end is undefined behaviour: it can read other memory or lose the device.

- **On the Switch's GPU the driver's robustness is expensive.** NVK checks each storage-buffer access against the buffer's size, and NAK, NVK's shader compiler, can fold that check into the load only on SM 7.3 and later. The Switch's GM20B is SM 5.3, so every check is a branch around its access (mesa-switch `src/nouveau/vulkan/nvk_shader.c`, and `has_load_global_bounded` in `src/nouveau/compiler/nak/api.rs`).
- **Bevy's shaders have no bounds checks of their own:** Bevy creates shader modules with `ShaderRuntimeChecks::unchecked()`, so [naga](glossary.md#naga) adds none.
- **So on Horizon, `BH_ROBUST_BUFFER_ACCESS` chooses what keeps buffer accesses in bounds.** It's read when Bevy looks for a GPU. Other platforms keep the driver's robustness, as upstream (`horizon_buffer_checks` in `patches/wgpu-hal/src/vulkan/adapter.rs`, and in `patches/bevy-0.18/wgpu-hal/src/vulkan/adapter.rs` for Bevy 0.18).

| `BH_ROBUST_BUFFER_ACCESS` | Driver robustness | naga's bounds checks | Logs (target `wgpu_hal::vulkan::adapter`) |
|---|---|---|---|
| Unset, empty or `clamp` (the default) | Off | `Restrict` in every shader, Bevy's unchecked ones included | Info: `BH_ROBUST_BUFFER_ACCESS: naga's clamping bounds checks in every shader, no driver buffer robustness` |
| `1` or `driver` | On | Upstream's, so none in Bevy's shaders | Info: `BH_ROBUST_BUFFER_ACCESS: the driver's buffer robustness` |
| `0` or `off` | Off | None in Bevy's shaders | Warning: `BH_ROBUST_BUFFER_ACCESS=0: buffer robustness is off` |

Any other value logs a warning, `BH_ROBUST_BUFFER_ACCESS="…" isn't 1, clamp or 0; using clamp`. A log filter such as `wgpu=warn` hides the info lines. Changing the mode changes the compiled shaders, so the next launch compiles them again.

**How clamp works.** naga's `Restrict` policy turns an index into a buffer's array into `min(index, length - 1)`, with no branch (naga 29.0.4, `write_restricted_index` in `src/back/spv/index.rs`), so an access past the end reaches the array's last element, which wgpu-core's binding-size checks keep inside the binding. Image loads keep the driver's image robustness, and binding arrays are unchecked in every mode, as upstream.

**What each costs.** By NAK's statistics, clamp adds 1 to 5% more instructions than `off`, and no loads or branches. The driver's checks add loads and branches: about 15% more instructions in a large fragment shader, and 3 to 5 times as many in Bevy's mesh vertex shaders and its mesh preprocessing compute shader, which read Bevy's per-mesh data from storage buffers. On the console, in a full 3D game at 1280×720, the driver's checks made each world fragment take about 30% longer and cut the frame rate by about a fifth. Clamp can cost registers: two variants of Bevy's mesh vertex shader took 8 more with it, so fewer of their warps fit on the GPU at once.

**Which to use.** Keep `clamp`. Use `driver` to find out whether a rendering bug is an index past the end of a buffer, and `off` only to measure what clamp costs: with `off`, such an index reads whatever the address holds, another buffer (wrong pixels or stretched triangles) or nothing mapped (a GPU fault), and a compute shader's write past the end changes another buffer. The indices most at risk come from data: instance and mesh indices, material slots, clustered lights and skin joints.

### Render-target compression

The Switch's Maxwell GPU can keep a render target compressed, which cuts the memory traffic of clears, depth tests and blending ([render target compression](glossary.md#render-target-compression)). NVK compresses an image only when its memory was allocated for it alone (`VkMemoryDedicatedAllocateInfo`), but wgpu places every texture inside a larger allocation, so by default nothing Bevy draws to is compressed.

- **`BH_RT_COMPRESSION=1` gives render targets memory of their own on Horizon,** for NVK to compress. Only the value `1` turns it on, and it's read when wgpu makes its first texture. It applies to single-sampled colour attachments of 4, 8 or 16 bytes a texel and `Depth32Float` depth attachments, each over 1 MiB, as NVK prefers; storage textures and the screen's own images are left out (`horizon_rt_compression` in `patches/wgpu-hal/src/vulkan/device.rs`; for Bevy 0.18, `Device::horizon_dedicated_texture` in `patches/bevy-0.18/wgpu-hal/src/vulkan/device.rs`).
- **What it logs.** A warning when it's on, `BH_RT_COMPRESSION=1: render targets get memory of their own, for NVK to compress`, then one for each of the first 32 textures it applies to, such as `BH_RT_COMPRESSION: "view_depth_texture" (1280x720x1 Depth32Float, 3904 KiB) gets memory of its own`. NVK then reports each of the first 32 such images: `[NVK] compression: a 1280x720x1 VK_FORMAT_D32_SFLOAT image with memory of its own is compressed, PTE kind 0x86`. `not compressed`, with the plain kind `0xfe`, means NVK didn't compress it.
- **On the console it works.** In a full 3D game at 1280×720, 14 render targets were compressed: `Depth32Float` depth, shadow maps included (kind `0x86`), `Rgba16Float` colour (`0xe6`) and `Rgba8UnormSrgb` colour (`0xd8`). The picture was right, loading was no slower, and the GPU's busy time fell by about 1.5 ms a frame (37.4 to 35.9 ms), worth about 1 fps, mostly from cheaper clears and shadow-map depth. Skate 3 Rust Engine's release turns it on.
- **It's off by default.** Leave it off in Ryujinx: the emulator draws the right picture with it, but its GPU side falls minutes behind.

## On the Switch

Tested on a Switch Lite under [Atmosphère](glossary.md#atmosphère), so in handheld mode only, each app started from [sphaira](glossary.md#sphaira) with a game's memory (about 3.1 GiB of [heap](glossary.md#heap)):

| App | Result |
|---|---|
| A bare NVK triangle test, without wgpu or Bevy | 60 fps |
| hello_switch's lit cube | 60 fps |
| Bevy's Breakout (`examples/breakout`) | Plays to the end (`[Game] score 56, 0 bricks left`) at about 59 fps |
| render_check | All 24 cubes in all six cameras, at about 20 fps; it's a stress test. Its readback test passes and its screenshot saves |
| Skate 3 Rust Engine, a full 3D Bevy 0.18 game | About 30 fps in its heaviest view, GPU-bound. Its large world takes 40 to 70 s to load |

The first four exit with `[Exit] PASS`. [Docked mode](glossary.md#docked-mode) is [not yet tested](#not-yet-tested).

### Performance

- **Small scenes hold 60 fps; a large 3D game is GPU-bound at handheld clocks.** Skate 3 Rust Engine reaches about 30 fps in its heaviest view, after optimisation in the game itself. render_check's 20 fps comes from drawing its scene through six cameras.
- **What helps on the GPU's side:** keep [buffer robustness](#buffer-robustness) at `clamp` (the driver's checks cost a full 3D game about a fifth of its frame rate), set [`BH_RT_COMPRESSION=1`](#render-target-compression) (about 1.5 ms a frame back in the same game), and keep [indirect-call validation](#indirect-call-validation) off in release builds.
- **ZCULL is set up for Bevy's reversed depth.** NVK sets up ZCULL, the GPU's coarse depth test, for `LESS`, which rejects nothing in Bevy's passes, so `HorizonPlugin` sets `GREATER` and logs `[NVK] ZCULL direction: greater (default; BEVY_HORIZON_ZCULL_DIR=less for NVK's own)`. In a full 3D game it made frames about 0.15 ms faster, with the same picture (`configure_from_env` in `crates/bevy_horizon/src/nvk_tune.rs`).
- **To see where the time goes,** time passes on the GPU with timestamp queries, as Bevy's `RenderDiagnosticsPlugin` does ([GPU timings](#gpu-timings) explains the fix they need), `NAK_DEBUG=stats` for what each shader costs, and bevy_horizon's CPU profiler ([logging-and-crash-reports.md](logging-and-crash-reports.md)). `bevy_horizon::nvk_tune` has switches for how NVK drives the GPU, which a game can flip between frames to time a frame with and without each; they're for measuring, and `EarlyZ` draws a wrong picture ([toolchain.md](toolchain.md#linking-nvk-the-vulkan-feature)).
- **GPU memory comes out of the game's heap,** so render targets, textures and meshes compete with everything else for it ([GPU memory comes from the same heap](platform-notes.md#gpu-memory-comes-from-the-same-heap)).

### NVK's shader cache on the SD card

`scripts/build_nvk.sh` builds NVK with Mesa's [shader cache](glossary.md#shader-cache) on disk. bevy_horizon leaves `HOME` unset, so the cache goes to `sdmc:/.mesa/mesa_shader_cache_sf/<driver build ID>/nvk_012b/`, in Mesa's single-file format: `foz_cache.foz` and an index.

- **An app's first launch compiles its shaders, and later launches reuse them,** so the first launch is slower. Every app shares the cache.
- **Changing `BH_ROBUST_BUFFER_ACCESS`, `NAK_DEBUG` or `NAK_GPR_LIMIT` changes the compiled shaders,** so the next launch compiles them again.
- **Deleting the folder is safe;** Mesa makes it again. `MESA_SHADER_CACHE_DISABLE=true` turns the cache off.

### What a healthy log shows

A Vulkan build's `log.txt` on the console has these lines near the start (render_check's, without timestamps):

```text
WARNING: NVK is not a conformant Vulkan implementation, testing use only.
INFO bevy_render::renderer: AdapterInfo { name: "NVIDIA Tegra X1 (GM20B) (NVK GM20B)", vendor: 4318, device: 4064, device_type: IntegratedGpu, … driver: "NVK", driver_info: "Mesa 26.2.4 (git-606441c90e)", backend: Vulkan, … }
INFO bevy_pbr::cluster: GPU clustering is supported on this device.
INFO bevy_render::batching::gpu_preprocessing: GPU preprocessing is fully supported on this device.
INFO bevy_horizon::window: primary window attached to libnx's default NWindow
```

- **The NVK warning is expected** on the Switch's GPU; it isn't an error.
- **`GPU preprocessing is fully supported`** means Bevy builds mesh data and culls on the GPU, its default.
- **[Heartbeat lines](logging-and-crash-reports.md#heartbeat-lines) follow,** each with the average frame rate so far, such as `[Heartbeat] frame 600, 31.6 s after the first frame, 19.0 fps on average`.
- **A clean exit ends with:**

```text
[Exit] nvdrv: already closed (the GPU driver released it), so there is nothing for nvExit to do
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

## Diagnosing a lost device or a GPU hang

When the GPU stops, or a Vulkan call fails in a way wgpu can't recover from, wgpu marks the device lost, and Bevy then quits or panics. Every Horizon build logs enough to tell what happened; you don't need a debug build. For other rendering symptoms, see [troubleshooting.md](troubleshooting.md).

### What every Horizon build logs

| What | Log line | Source |
|---|---|---|
| NVK's own message, which usually says why | `WARN wgpu_hal::vulkan::instance: GENERAL [<file>:<line> (0x0)]`, or `ERROR`, with the message on the next line, indented | A `VK_EXT_debug_utils` messenger that wgpu-hal registers on Horizon, since a release build of Mesa sends its messages nowhere else (`desired_extensions` in `patches/wgpu-hal/src/vulkan/instance.rs`) |
| The Vulkan error behind one wgpu calls unexpected | `Vulkan returned ERROR_…, which wgpu reports as an unexpected driver error` | `get_unexpected_err` in `patches/wgpu-hal/src/vulkan/mod.rs` |
| The call that lost the device | `wgpu-core: the device is lost to <error>, returned to the call at <file>:<line>:<column>` | `handle_hal_error` in `patches/wgpu-core/src/device/resource.rs` |
| A long wait for the window's next image | `wgpu-hal: waited 8.5 s for the window's next image, longer than the 1.0 s wgpu asks for; on Horizon it waits for as long as it takes` | `acquire` in `patches/wgpu-hal/src/vulkan/swapchain/native.rs` |

`<error>` is `OutOfMemory (Out of memory)`, `Lost (Device is lost)` or `Unexpected (Unexpected error variant (driver implementation is at fault))`. Running out of memory while making a buffer, texture, sampler or query set is reported to the caller instead, and doesn't lose the device ([GPU memory comes from the same heap](platform-notes.md#gpu-memory-comes-from-the-same-heap)).

### Reading a loss

Read the first lines of the loss, not the last. A GPU hang logs, in order, a long wait for the next image, NVK's message saying why, and the call that lost the device. This is the [known hang](#known-issues) in a Bevy 0.18 game, whose log format leaves out each line's target:

```text
WARN wgpu-hal: waited 8.5 s for the window's next image, longer than the 1.0 s wgpu asks for; on Horizon it waits for as long as it takes
ERROR GENERAL [../src/nouveau/vulkan/nvkmd/switch/nvkmd_switch_dev.c:207 (0x0)]
	nvkmd-switch: channel 2 lost: notification_rc=0x0 notification={type=8 (fifo idle timeout) info=0 status=65535} error_rc=0x0 error={type=4 info=[0x0,0x0,0x0,0x0]}
WARN GENERAL [../src/nouveau/vulkan/nvkmd/switch/nvkmd_switch_dev.c:163 (0x0)]
	nvkmd-switch: channel submit failed: device lost (native=0x0, notification={type=8, info=0, status=65535}, channel={type=4, info=[0,0,0,0]}) (VK_ERROR_DEVICE_LOST)
ERROR wgpu-core: the device is lost to Lost (Device is lost), returned to the call at <path>/wgpu-core/src/device/queue.rs:<line>:<column>
```

NVK's `channel N lost` message gives the reason Horizon's GPU driver reported, in `notification={type=…}`. The names come from mesa-switch's `src/nouveau/horizon/nouveau_horizon_channel.c`: 8 `fifo idle timeout` (the channel's work stopped making progress: a GPU hang), 13 `graphics engine error (sw notify)`, 24 `graphics semaphore timeout`, 25 `graphics illegal notify`, 31 `mmu fault` (a GPU page fault), 32 `pbdma error`, 43 `channel reset verification error` and 80 `pushbuffer crc mismatch`. Lines after the first loss, such as `nvkmd-switch: native fence wait failed: device lost (VK_ERROR_DEVICE_LOST)`, are consequences.

### The panic is a consequence, not a second bug

Once the device is lost, wgpu-core refuses to make anything on it (`check_is_valid`, at the start of `Device::create_buffer` and the like, in `patches/wgpu-core/src/device/resource.rs`), and each refusal is a validation error caused by `Parent device is lost`. A few calls panic on it, such as `Buffer::get_mapped_range` (Bevy making a buffer with data in it), and with Bevy 0.18 `Surface::present`: `Error in Surface::present: Validation Error` (`handle_error_fatal` in `patches/wgpu/src/backend/wgpu_core.rs`). The rest go to wgpu's error handler. Bevy 0.18 keeps wgpu's default, which logs `Handling wgpu errors as fatal by default` and panics with `wgpu error: Validation Error`, naming the call, such as `Texture::create_view`. Bevy 0.19 logs the loss, such as `Caught DeviceLost error: Unknown Device is lost`, and each refusal as `Caught rendering error: …`, and its default `RenderErrorHandler` then quits the app with `Quitting the application due to DeviceLost RenderError`. Either way, the cause is in the lines before the loss.

### The exit after a loss

After a loss, Mesa can still hold GPU memory, so Horizon's GPU driver ([nv](glossary.md#nv)) never shuts down, and `nvExit` could then wait forever. bevy_horizon's exit checks for that, skips `nvExit`, and leaves through [AM](glossary.md#am) to HOME, because the loader would crash reading the next NRO into heap pages that are still locked (`release_nvdrv` in `crates/bevy_horizon/src/lifecycle.rs`; [The `userAppExit` hook](lifecycle-and-exit.md#the-userappexit-hook)). The log then ends with:

```text
[Exit] nvdrv: still open, and N heap blocks (X MiB) are still held for the GPU, so the GPU driver never shut down; skipping nvExit, which can wait forever in this state, and exiting through AM to HOME
[Exit] WARN: heap pages are still locked, so the loader would crash reading the next NRO (2001-0106); exiting through AM to HOME instead
```

### Narrowing it down

- **Make progress lines reach the SD card.** A log cut short shows how far the app got only if each line was flushed. render_check logs chosen frames, `[Engine] frame N updated, T ms after frame 1`, and flushes each one.
- **Take reading back out.** A screenshot, or any copy from the GPU to the CPU, is extra work that can fail on its own. render_check's `no_screenshot` option skips its readback test and its screenshot; if its tiles keep drawing until you press (A) and go black after it, reading back is what loses the device.
- **Turn on the driver's buffer robustness** (`BH_ROBUST_BUFFER_ACCESS=driver`) to see whether an index past the end of a buffer is the cause.
- **Run render_check with the same settings** to tell a problem in your game from one in the drawing path.

## Known issues

- **A GPU hang in one Bevy 0.18 scene.** A simple test scene, with lit `StandardMaterial` meshes, a non-HDR camera that tonemaps in the shader, a sun and a ground plane, stops the GPU within its first few frames. About 10 to 13 s after the first frame, the log shows `channel N lost: … notification={type=8 (fifo idle timeout) info=0 status=65535}`, then wgpu reports the device lost, and the final panic is in `Surface::present` or `Texture::create_view` ([Reading a loss](#reading-a-loss) has the log). It isn't memory: more than 2 GiB was free. None of these helped: the driver's buffer robustness, no shadows, no depth prepass, an unlit ground, or `WGPU_SETTINGS_PRIO=compatibility`, which also turns off bindless materials, indirect draws and GPU culling. This repo's Bevy 0.19 examples draw the same stock path on the same driver build, and the same game's main world plays. The cause is unknown.
- **Large 3D games are GPU-bound** at handheld clocks ([Performance](#performance)).
- **The first launch compiles every shader,** so it starts slower than later ones ([NVK's shader cache on the SD card](#nvks-shader-cache-on-the-sd-card)).

## Not yet tested

- **Docked mode.** The window becomes 1920×1080, and the GPU runs at docked clocks; a Switch Lite can't dock.
- **wgpu's indirect-call validation on the console.** In the emulator, with the project's fixes, render_check draws all 24 cubes with it on.
- **A full 3D game on Bevy 0.19.** On the console, Bevy 0.19 has run this repo's examples; the full 3D game is on Bevy 0.18.
- **Emulators other than Ryujinx.**

## The FPS overlay

bevy_horizon's `fps_overlay` feature (Bevy 0.19 only) adds `HorizonFpsOverlayPlugin`, which wraps Bevy's own `FpsOverlayPlugin` from bevy_dev_tools: the FPS as text, with a frame-time graph under it (`crates/bevy_horizon/src/fps_overlay.rs`).

- **It's hidden at launch.** Hold (−) and press (+) to show or hide it; pressing both together works too. `HorizonFpsOverlayPlugin { visible_at_launch: true }` shows it from the first frame.
- **A game should ignore (+) while (−) is held,** because many games use (+) to pause or quit. `fps_chord_held` tells it when; hello_switch and render_check both use it.
- **The graph's colour is frame time.** Each bar mixes green to red by its frame time over 1/30 s, Bevy's default `min_fps` of 30. Below 30 fps every bar is fully red; that's correct, not a rendering fault.
- **It needs the UI and text features** ([Bevy features a game needs](#bevy-features-a-game-needs)), or a font of the game's own in `FpsOverlayConfig::text_config`.
- **It covers the top-left corner,** over whatever the game draws there, such as Breakout's score. Bevy 0.19.1 places it there, above everything else, and `FpsOverlayConfig` can't move it.
- **Status Monitor Overlay can't show a Bevy game's FPS,** going by its code. Its FPS line comes from [SaltyNX](https://github.com/masagrator/SaltyNX), which hooks the present calls a game imports from the modules loaded at launch. A Bevy NRO is loaded later, by the homebrew loader, and calls NVK directly, and SaltyNX skips homebrew anyway. Status Monitor's CPU, GPU load, RAM and temperature lines still work.

## GPU timings

NVK reports `timestampPeriod = 1.0`, right for desktop GPUs, whose timer counts nanoseconds. The [Tegra X1](glossary.md#tegra-x1)'s GPU timer is built for a 31.25 MHz reference, adding 32 a tick, but runs from 19.2 MHz, so each unit is 1.6276 ns. Without a fix, every GPU time read from timestamp queries, wgpu's and those of Bevy's `RenderDiagnosticsPlugin`, comes out 1.63 times too short. `patches/mesa-switch/0002-nvk-switch-timestamp-period.patch` sets the right period on the Switch; its header gives the sources, and UnleashedRecomp-NX measured the same tick against wall-clock time on the console. `scripts/build_nvk.sh` applies it, and `scripts/localize_nvk.sh` then remakes `libvulkan_bh.a`, the library games link.

## render_check

`examples/render_check` draws the same four cubes through six cameras, each changing one thing about Bevy's 3D path, and checks every cube on a screenshot. Use it to check the 3D path on a console or in an emulator, or as a model for your own checks: it also tests the debug messenger and a copy from the GPU to the CPU, and logs its progress so that a log cut short still shows how far it got.

| Tile | Camera |
|---|---|
| 1 | Bevy's defaults: GPU culling and indirect draws, [MSAA](glossary.md#msaa) ×4 |
| 2 | `NoIndirectDrawing`: direct draws, no GPU culling, MSAA ×4 |
| 3 | Indirect draws, MSAA off |
| 4 | Direct draws, MSAA off |
| 5 | HDR, which adds a tonemapping pass |
| 6 | Orthographic projection |

The cubes, left to right: lit cyan (`StandardMaterial`); unlit magenta; unlit yellow with alpha blending; unlit green with depth testing off. Build it like any example, copy `render_check.nro` to `sdmc:/switch/render_check/`, and start it:

```text
JOBS=4 scripts/build_example.sh render_check "Render check"
```

Each tile shows its name, a legend, and `checks at frame 240` until the check runs, then its result, such as `lit ok, unlit ok, blend ok, no-depth ok`; a cube it can't find shows `MISSING`. (A) checks again, and (+) quits unless (−) is held.

**Options.** An optional `options.txt` next to the NRO holds any of these words, separated by spaces or new lines. The `[Init]` lines say which are in effect.

| Word | Effect |
|---|---|
| `cpu_preprocessing` | Bevy builds mesh data on the CPU instead of the GPU (`PbrPlugin { use_gpu_instance_buffer_builder: false }`) |
| `indirect_validation` | Turns on wgpu's indirect-call validation, which is off by default in release builds |
| `no_indirect_validation` | Turns it off in a debug build too |
| `no_screenshot` | Skips the readback test at frame 200 and the screenshot at frame 240, so the tiles keep drawing without them; (A) still takes one |
| `panic` | Panics at frame 120, to test the exit with the GPU driver still running |

**What it writes,** next to the NRO: `log.txt`, and `render_check_1.png`, the screenshot it judged; (A) adds `_2`, `_3` and so on. Its log filter is `wgpu=warn,naga=warn`. After the `[Init]` lines and the `[Render]` lines ([What wgpu sees](#what-wgpu-sees)), `log.txt` has, in order:

| When | Log lines |
|---|---|
| The messenger self-test | `[Check] debug messenger self-test: …`, NVK's expected warning naming `nvk_image.c`, then `[Check] debug messenger self-test: NVK returned ERROR_FORMAT_NOT_SUPPORTED`. Without the warning, NVK's messages aren't reaching the log |
| Chosen frames from 1 to 600 | `[Engine] frame N updated, T ms after frame 1`, each flushed to the SD card |
| Frame 200 | `[Check] readback test: all 16384 bytes came back as written`, or what went wrong. This tests a copy to the CPU on its own, before the screenshot needs one |
| Frame 240 | `[Check] screenshot 1: 1280x720, Rgba8UnormSrgb`, a line for each tile, and a summary such as `[Check] screenshot 1: 24/24 cubes drawn, mesh preprocessing on the GPU`. With `no_screenshot`, `[Check] frame 240: no screenshot, …` instead |

A tile's line looks like `[Check] T2 NoIndirectDrawing, MSAA x4: lit DRAWN (625/625 px at 498,180), … | background #0D290D`. A cube counts as DRAWN when at least half of the 25×25 pixels around its projected centre differ from the tile's background by more than 0.08 in some sRGB channel; one that projects outside its view is `OFF-SCREEN`.

**How to read a result:**

| Pattern | Points to |
|---|---|
| Direct-draw tiles (2, 4) draw and indirect ones (1, 3, 5, 6) don't | Indirect draws or GPU culling |
| Only the green, depth-test-off cube draws | Depth: the clear, the compare or the viewport's depth range |
| MSAA-off tiles (3, 4) draw and the others don't | Multisampling or its resolve |
| Unlit cubes draw and the lit one doesn't | Lighting data (lights, clusters, shadows) |
| Nothing draws, with or without `cpu_preprocessing` | Vertex input, the view uniform or dynamic offsets |

In the emulator, the results for the bottom row (tiles 4 to 6) are wrong because of a readback bug ([What the emulator gets wrong](#what-the-emulator-gets-wrong)); judge those from the screen.

## What the emulator gets wrong

Stock Ryujinx ([Ryubing](glossary.md#ryubing) 1.3.3) can't run bevy_horizon's Vulkan builds: NVK hits the bugs below at startup and on the first frames. The project tests in a Ryujinx with local fixes for them, which aren't published. The bugs are Ryujinx's, not NVK's, wgpu's or bevy_horizon's. The non-Vulkan [captest](glossary.md#captest) runs in stock Ryujinx, and [testing.md](testing.md#what-the-emulator-gets-wrong) lists the differences that don't involve rendering. Other emulators are untested.

**The emulator's speed says nothing about the console's.** In the project's tests Ryujinx draws on the host CPU through [lavapipe](glossary.md#lavapipe): making the Vulkan device takes about 46 s, and a frame 50 to 200 ms.

These Ryujinx bugs stop NVK or Bevy; the project's local fixes cover all of them:

| Ryujinx bug | What it does to NVK or Bevy |
|---|---|
| `AllocSpace` with a fixed offset records the reservation at the start of the free block, not at the address asked for | NVK's next fixed `MapBufferEx` fails, so there's no device |
| For an NRO without a RomFS, `OpenDataStorageByCurrentProcess` throws `KeyNotFoundException`. Ryujinx starts NROs without a homebrew loader, so libnx's `romfsMountSelf` asks it | Ryujinx aborts at startup; an NRO with a RomFS avoids it. The fix returns 2002-1002 (`TargetNotFound`), which bevy_horizon logs and carries on from |
| The JIT has no vector `FCVTPU` or `FCVTMU` | A crash in Bevy's light clustering on the first frame |
| The shader translator reads storage-buffer sizes from constant buffer 0, where [NVN](glossary.md#nvn), Nintendo's API, puts them | NVK keeps push constants there, so Ryujinx makes enormous buffers and fails with `OUT_OF_DEVICE_MEMORY` |
| The shader translator lacks `PRMT` with an immediate | Shaders that permute bytes, text's among them, draw nothing |
| The GPU address-space allocator doesn't merge a freed range with the free range to its left | A false `Out of Memory` later, in apps that free GPU memory as they go, such as a game that loads levels |
| The shader translator expects a storage buffer's address and size where NVN puts them | NVK keeps those of buffers bound with a dynamic offset in its root table, so wgpu's indirect-call validation reads zeros and every indirect draw is empty: only `NoIndirectDrawing` cameras draw |
| The GPU buffer cache's `MultiRegionHandle` assumes the tracking handles it takes over from older buffers are in order and inside the new one | `IndexOutOfRangeException`, and Ryujinx aborts; a full 3D game hit it |
| The Vulkan backend binds a sparse buffer's memory without the queue lock or a wait, and the buffer can be destroyed right after | lavapipe runs the bind after the buffer is gone: a segmentation fault |
| A fence the guest keeps waiting on is reported as passed after 1 s, whether the GPU reached it or not | NVK and wgpu can reuse memory, commands included, that the emulated GPU hasn't read yet, and Ryujinx aborts |
| When the shader optimiser removes an unused instruction, it also removes the instructions that fed it, without asking whether they have side effects | NAK gives a shared-memory atomic whose result is unused a destination register anyway. Ryujinx removes the copy out of that register, then the instructions that work out the atomic's address, so in a full 3D game a GPU triangle cull kept almost no triangles |

These are known and not fixed:

| Problem | What to do |
|---|---|
| Ryujinx rarely dies with a segmentation fault, with nothing in its log | Run it again |
| Reading back a texture larger than about 1 MiB returns wrong data past the first MiB, so render_check's results for its bottom row are wrong | Judge from the screen, not from render_check's count |
| With `BH_RT_COMPRESSION=1`, the picture is right but the emulator's GPU side falls minutes behind | Leave compression off in the emulator |

If you patch Ryujinx yourself, pass `-p:Version=1.3.3` to `dotnet build`, or Ryubing can't load the rebuilt DLL and aborts with a `FileNotFoundException`. `Ryujinx.ShaderTools` translates a shader binary to GLSL without running the emulator: with `NAK_DEBUG=print`, NAK prints each shader's code after "Encoded shader:"; write those words out little-endian and run `dotnet Ryujinx.ShaderTools.dll --compute --target-api Vulkan in.bin out.glsl` for a compute shader.

## How a frame reaches the screen

This section is for contributors: what bevy_horizon and its crate patches change so that Bevy's renderer runs on Horizon.

| Step | What happens | Source |
|---|---|---|
| The window | `HorizonWindowPlugin` takes [libnx](glossary.md#libnx)'s default window (`nwindowGetDefault`, an [NWindow](glossary.md#nwindow)) and, in `First`, gives it to Bevy's primary window as a raw-window-handle `Horizon` handle | `attach_default_nwindow` in `crates/bevy_horizon/src/window.rs` |
| The handle type | The raw-window-handle patch adds `HorizonWindowHandle`, which holds the `NWindow` pointer, and an empty `HorizonDisplayHandle` | `patches/raw-window-handle/src/horizon.rs` |
| The driver's entry point | Horizon has no dynamic loader, so wgpu-hal calls `vkGetInstanceProcAddr` as a linked symbol, through `ash::Entry::from_static_fn`. Without the `vulkan` feature, a stand-in returns null for every name: wgpu reports `no Vulkan driver is linked into this executable`, and Bevy panics with `Unable to find a GPU!` | `Instance::init_with_callback` in `patches/wgpu-hal/src/vulkan/instance.rs`; `vkGetInstanceProcAddr` in `crates/bevy_horizon/src/compat.rs` |
| The adapter | NVK reports Vulkan conformance version 0 on every Tegra GPU, and wgpu-hal hides adapters like that. The patch keeps it on Horizon, where it's the only one | `expose_adapter` in `patches/wgpu-hal/src/vulkan/adapter.rs` |
| The surface | The Horizon handle becomes `vkCreateViSurfaceNN` (`VK_NN_vi_surface`) on the `NWindow` | `create_surface_from_vi` in `patches/wgpu-hal/src/vulkan/instance.rs` |
| The next image | wgpu-core waits at most 1 s for the window's next image, and Bevy panics when that runs out (`Couldn't get swap chain texture, operation unrecoverable`). On Horizon the patch waits as long as it takes, as wgpu-hal already does on Android before 11, and logs a warning when the wait is longer than 1 s | `acquire` in `patches/wgpu-hal/src/vulkan/swapchain/native.rs`; for Bevy 0.18, `acquire_texture` in `patches/bevy-0.18/wgpu-hal/src/vulkan/instance.rs` |
| The rest of wgpu | wgpu and wgpu-core accept the Horizon handle and leave out the X11, Wayland, DRM and RenderDoc code, which can't build for Horizon | `patches/wgpu/src/backend/wgpu_core.rs`, `patches/wgpu-core/src/instance.rs`, `patches/wgpu-hal/src/auxil/renderdoc.rs` |
| NVK | The `vulkan` feature links `nvk/lib/libvulkan_bh.a`, made by `scripts/build_nvk.sh` and `scripts/localize_nvk.sh`. NVK compiles shaders with NAK, submits work through nvdrv GPFIFO channels, and presents into the `NWindow`'s buffer queue | `link_nvk` in `crates/bevy_horizon/build.rs`; mesa-switch `src/nouveau/horizon/` |

### What wgpu sees

render_check's `[Render]` lines and Bevy's `AdapterInfo` line show what the driver reports:

| Item | Value |
|---|---|
| Adapter | NVIDIA Tegra X1 (GM20B) (NVK GM20B), Vulkan, an integrated GPU |
| Driver | NVK, Mesa 26.2.4, followed by `git-` and the first 10 characters of the mesa-switch commit NVK was built from |
| `INDIRECT_FIRST_INSTANCE`, `IMMEDIATES` | Yes, so Bevy builds mesh data and culls on the GPU (its default) |
| `MULTI_DRAW_INDIRECT_COUNT` | No, so Bevy issues one indirect draw per batch |
| Storage textures and buffers per stage | 524288 each |
| Compute workgroup storage | 49152 bytes |

The adapter's name and IDs aren't read from the GPU: mesa-switch fills them in from a fixed description of the GM20B (device ID 0x0fe0, chipset 0x12b; `src/nouveau/horizon/nouveau_horizon_device.c`), so every console reports the same.

## The OpenGL ES path (captest)

captest still checks the [OpenGL ES](glossary.md#opengl-es) driver directly, without wgpu: devkitPro's [switch-mesa](glossary.md#switch-mesa) (Mesa 20.1, nouveau), through [EGL](glossary.md#egl), drawing a spinning triangle (`examples/switch_captest/src/gpu.rs`). On the console the triangle draws, and you confirm it with the `gpu-seen` check. Bevy doesn't use this path. wgpu's GL backend was the plan before NVK; the Vulkan path replaced it, and NVK is a far newer Mesa (26.2).
