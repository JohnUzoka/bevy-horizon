# Rendering

How Bevy draws on the Switch: the Vulkan path through wgpu and NVK, what each emulator run has shown, what the emulator gets wrong, the Bevy features a game needs, the FPS overlay, and `render_check`, the example that tests the 3D path one setting at a time.

## In short

- **Bevy draws through wgpu's Vulkan backend, with NVK linked into the NRO.** Turn on bevy_horizon's `vulkan` feature. NVK is Mesa's Vulkan driver for NVIDIA GPUs, built for the Switch by mesa-switch. There's no loader on the Switch, so the driver is part of the executable ([How a frame reaches the screen](#how-a-frame-reaches-the-screen)).
- **In the Ryujinx harness, 2D works.** The clear colour, bevy_ui panels, text and the FPS overlay draw, and + exits cleanly (run hsvk7). Bevy's Breakout plays: the paddle follows the D-pad, the ball clears all 56 bricks, and (+) exits cleanly (bo1). Frames are slow there, 5 to 19 per second. The emulator draws on the PC's CPU (lavapipe), and the logs don't time Bevy's render step ([What has been seen](#what-has-been-seen), bo1), so how fast Bevy draws on the Switch is for the hardware runs to show.
- **3D works in the harness too.** render_check's 24 cubes draw in all six of its cameras, its screenshot works, and + exits cleanly (run rc5). hello_switch's lit cube draws (hsvk8). wgpu's indirect-call validation is off by default on Horizon in release builds, as Bevy intends for Vulkan. With it on, the emulator drew only the cameras that use direct draws, because Ryujinx misread where NVK keeps one of the validation pass's buffers. With that patched, all 24 cubes draw with it on too (rc8b, [Indirect-call validation](#indirect-call-validation)).
- **On the Switch (run 5), NVK and hello_switch draw at 60 fps, but render_check loses the GPU device after about 4 seconds.** nvk_probe's triangle and hello_switch's lit cube held 60 fps and exited cleanly. render_check showed two squares, then went black: wgpu reported the device lost, and the app panicked. The run-5 log can't say which call failed; run 6 carries diagnostics that can ([On the Switch](#on-the-switch)).
- **The harness's Ryujinx carries seven local fixes for NVK,** in six patch files. Five stop it failing at startup or on the first frames. The sixth fixes a bug in its GPU address-space allocator that can report false out-of-memory errors; it's shown outside the emulator, but no run has been seen to hit it. The seventh lets indirect draws work with wgpu's validation on. All seven fix emulator bugs, not bevy_horizon ones ([What the emulator gets wrong](#what-the-emulator-gets-wrong)).
- **A 3D game needs Bevy's `tonemapping_luts`, `ktx2` and `zstd_rust` features.** Without them, a camera with Bevy's default tonemapping draws nothing but its clear colour ([Bevy features a game needs](#bevy-features-a-game-needs)).

## How a frame reaches the screen

| Step | What happens | Source |
|---|---|---|
| The window | `HorizonWindowPlugin` takes libnx's default window (`nwindowGetDefault`) and, in `First`, gives it to Bevy's primary window as a raw-window-handle `Horizon` handle | `crates/bevy_horizon/src/window.rs:49-82` |
| The handle type | raw-window-handle has no Horizon variant upstream. The patch adds `HorizonWindowHandle`, which holds the `NWindow` pointer, and an empty `HorizonDisplayHandle` | `patches/raw-window-handle/src/horizon.rs`; `lib.rs:213`, `:352` |
| The driver's entry point | wgpu-hal normally loads `libvulkan` at run time. Horizon has no dynamic loader, so the patch calls `vkGetInstanceProcAddr` as an ordinary linked symbol, through `ash::Entry::from_static_fn`. Without the `vulkan` feature, bevy_horizon links a stand-in that returns null for every name, and wgpu reports that no Vulkan driver is linked | `patches/wgpu-hal/src/vulkan/instance.rs:612-634`; `crates/bevy_horizon/src/compat.rs:959-972` |
| The adapter | NVK reports Vulkan conformance version 0 on every Tegra GPU, and wgpu-hal hides adapters like that. The patch keeps it on Horizon, where it's the only one | `patches/wgpu-hal/src/vulkan/adapter.rs:2220-2223` |
| The surface | The Horizon handle becomes `vkCreateViSurfaceNN` (`VK_NN_vi_surface`) on the `NWindow` | `patches/wgpu-hal/src/vulkan/instance.rs:553-575`, `:955-957` |
| The next image | Each frame, wgpu-core waits at most 1 s for the window's next image (`FRAME_TIMEOUT_MS`), and Bevy panics when that runs out (`Couldn't get swap chain texture, operation unrecoverable`). On Horizon the patch waits for as long as it takes, which NVK's swapchain supports, as wgpu-hal already does for Android before 11. A longer wait than wgpu asked for logs `wgpu-hal: waited … s for the window's next image`. Skate 3 hit the limit in the emulator, where the GPU fell more than a second behind. Run 6's NROs were built before this change | `patches/wgpu-hal/src/vulkan/swapchain/native.rs:404-415`, `:495-505`; `patches/bevy-0.18/wgpu-hal/src/vulkan/instance.rs:1153-1164`, `:1245-1255`; mesa-switch `src/vulkan/wsi/wsi_common_switch.c:734-819` |
| The rest of wgpu | wgpu and wgpu-core accept the Horizon handle and leave out the X11, Wayland, DRM and RenderDoc code, which can't build for Horizon | `patches/wgpu/src/backend/wgpu_core.rs:799`, `:826`; `patches/wgpu-core/src/instance.rs:320`, `:1014`; `patches/wgpu-hal/src/vulkan/instance.rs:246`; `patches/wgpu-hal/src/auxil/renderdoc.rs` |
| NVK | The `vulkan` feature links `nvk/lib/libvulkan_bh.a` from `scripts/build_nvk.sh` and `scripts/localize_nvk.sh` ([Linking NVK](toolchain.md#linking-nvk-the-vulkan-feature)). NVK compiles shaders with NAK, submits work through nvdrv GPFIFO channels, and presents into the `NWindow`'s buffer queue | `crates/bevy_horizon/build.rs:25-45`; mesa-switch `src/nouveau/horizon/` |

What wgpu sees, from render_check's `[Render]` lines (run rc2). The adapter's name and IDs aren't read from the GPU: mesa-switch fills them in from a fixed description of the GM20B (device ID 0x0fe0, chipset 0x12b; `src/nouveau/horizon/nouveau_horizon_device.c:25-60`) and, for that description, asks the GPU service only for the zcull figures (line 73). So the console should report the same:

| Item | Value |
|---|---|
| Adapter | NVIDIA Tegra X1 (GM20B) (NVK GM20B), Vulkan |
| Driver | NVK Mesa 26.2.3 (git-d4a00ea0ab) |
| `INDIRECT_FIRST_INSTANCE`, `IMMEDIATES` | Yes, so Bevy builds mesh data and culls on the GPU (its default) |
| `MULTI_DRAW_INDIRECT_COUNT` | No, so Bevy issues one indirect draw per batch |
| Storage textures and buffers per stage | 524288 each |
| Compute workgroup storage | 49152 bytes |

**NVK caches compiled shaders on the SD card.** `scripts/build_nvk.sh` builds NVK with Mesa's disk cache (`-Dshader-cache=enabled`, line 145). With `HOME` unset, as bevy_horizon leaves it, the cache goes to `sdmc:/.mesa/mesa_shader_cache_sf/<driver build ID>/nvk_012b/`, in Mesa's single-file format: `foz_cache.foz` and an index (mesa-switch `src/util/disk_cache_os.c:968-986`, `src/util/disk_cache.c:221-231`). Every app shares it. An app's first launch compiles its shaders and later launches reuse them. Deleting the folder is safe; Mesa makes it again. In the harness it's `~/ryujinx-bh/data/sdcard/.mesa`, about 320 KB after the runs below.

## Indirect-call validation

With `InstanceFlags::VALIDATION_INDIRECT_CALL`, wgpu checks every indirect draw before the GPU runs it. A compute pass copies each draw's arguments and zeroes those that break wgpu's rules: indices past the end of the index buffer, instances past the end of an instance-rate vertex buffer, or a first instance the device can't use. "Behavior is undefined if this validation is disabled and the rules above are not satisfied" (wgpu-types 29.0.4, `src/instance.rs:194-218`). Bevy draws each batch with an indirect call by default, so the pass runs every frame.

- **On Horizon it's off in release builds and on in debug builds.** bevy_horizon's wgpu-core patch drops the flag unless `InstanceFlags::VALIDATION` is set too, which debug builds set (`patches/wgpu-core/src/instance.rs:106-118`).
- **That is Bevy's own rule, applied to the backends the Switch has.** In release builds bevy_render removes the flag unless its backends include DX12, where "wgpu executes additional necessary logic during validation passes"; elsewhere "Removing this flag improves performance" (`bevy_render-0.19.1/src/settings.rs:130-139`). But Bevy's default backends are `Backends::all()`, which names DX12 on every platform, so the flag stays on regardless. wgpu on Horizon has only the Vulkan backend.
- **In the emulator, with the validation on, every indirect draw read zeros as its arguments,** so nothing drew (rc2, rc3, rc6). With it off, the same draws read Bevy's arguments and drew (rc4, rc5).
- **The zeros were Ryujinx's.** The validation pass reads the arguments from a storage buffer bound with a dynamic offset. NVK keeps such a buffer's descriptor in its root table split into words: for dynamic buffer *i*, the address's low word at 0x300 + 4*i*, its high word at 0x400 + 4*i*, the size at 0x500 + 4*i* (Mesa, `nvk_cmd_buffer.h`, "swizzled form of nvk_buffer_descriptor"). Ryujinx reads the 16 bytes from the low word on, as Nintendo's NVN lays them out, so it took the next buffer's low word for the high word. The address it got wasn't mapped, the pass read zeros, and zeros pass the checks as empty draws. With `gpu-nvk-dynamic-ssbo.patch` the arguments come through as Bevy wrote them (in rc8, the same values rc5 read with the validation off), and all 24 cubes draw (rc8b). Nothing points at NVK or wgpu; the hardware run with `indirect_validation` checks that ([Next](#next)).
- **Bevy writes the arguments itself, on the GPU,** so they should always be valid. A game that writes its own indirect draws should test them in a debug build.

To turn it on in a release build, insert both `VALIDATION` and `VALIDATION_INDIRECT_CALL` into `WgpuSettings::instance_flags`, as render_check's `indirect_validation` option does. `VALIDATION` on its own only asks Vulkan for the Khronos validation layer, which the Switch doesn't have, and wgpu carries on without it.

## Buffer robustness

wgpu turns on Vulkan's buffer robustness whenever the device has it: `robustBufferAccess`, and `robustBufferAccess2` where `VK_EXT_robustness2` has it. NVK has both (mesa-switch `src/nouveau/vulkan/nvk_physical_device.c:372`, `:778`). With them, a shader's access past the end of a buffer can't reach other memory: a read returns zero or a value from within the buffer, and a write changes at most the buffer (`robustBufferAccess2` makes the reads zero and drops the writes). Without them it's undefined behaviour, which can read other memory or lose the device.

- **On the Switch's GPU, robustness costs shader code.** With it on, NVK gives storage-buffer accesses `nir_address_format_64bit_bounded_global` (mesa-switch `src/nouveau/vulkan/nvk_shader.c:121-139`), and NIR then splits each load and store into single components and checks each against the buffer's size (`src/compiler/nir/nir_lower_explicit_io.c:1480-1495`). NAK can fold a load's check into the load itself only on SM 7.3 and later (`has_load_global_bounded`, `src/nouveau/compiler/nak/api.rs:190`). The Switch's GM20B is SM 5.3 (`src/nouveau/horizon/nouveau_horizon_device.c:39`), so each component's check is a branch around its access (`nir_lower_explicit_io.c:839-860`).
- **Bevy's shaders have no other bounds checks.** By default Bevy creates shader modules with `ShaderRuntimeChecks::unchecked()`, so naga adds none (bevy_shader 0.18.1 `src/shader.rs:58-71`, the `ValidateShader::Disabled` default; bevy_render 0.18.1 `src/render_resource/pipeline_cache.rs:183-192`, `src/renderer/render_device.rs:83-93`).
- **`BH_ROBUST_BUFFER_ACCESS` picks what keeps a shader's buffer accesses in bounds on Horizon,** before the device is made. Other platforms always get the driver's robustness, as upstream (`HorizonBufferChecks`, `patches/wgpu-hal/src/vulkan/adapter.rs:15-71`, used at `:2318-2330` and `:2365`, with naga's checks kept at `device.rs:718-729` and `:1820-1830`; for Bevy 0.18's wgpu 27, `patches/bevy-0.18/wgpu-hal/src/vulkan/adapter.rs:13-69`, `:1822-1825`, `:1859` and `:1866-1871`, and `device.rs:1139-1150` and `:2248-2258`).

| `BH_ROBUST_BUFFER_ACCESS` | Driver robustness | naga's bounds checks | Logs (target `wgpu_hal::vulkan::adapter`) |
|---|---|---|---|
| Unset, empty or `clamp` (the default) | Off | `Restrict` in every shader, Bevy's unchecked ones included | Info: `BH_ROBUST_BUFFER_ACCESS: naga's clamping bounds checks in every shader, no driver buffer robustness` |
| `1` or `driver` | On | Upstream's, so none in Bevy's shaders | Info: `BH_ROBUST_BUFFER_ACCESS: the driver's buffer robustness` |
| `0` or `off` | Off | None in Bevy's shaders | Warning: `BH_ROBUST_BUFFER_ACCESS=0: buffer robustness is off` |

Any other value logs a warning and clamps. A game's log filter can hide the info lines; Skate 3's (`warn,skate3rust=info,bevy_horizon=info`) does. `NAK_DEBUG=stats` shows what each mode did to each shader ([logging-and-crash-reports.md, Part 4](logging-and-crash-reports.md#part-4-what-a-shader-costs-nak_debugstats)).

**How clamp keeps accesses in bounds.** naga's `Restrict` policy turns an index into an array in a buffer into `min(index, length - 1)`, taking a runtime-sized array's length from `arrayLength`: an instruction or two, and no branch (naga 27.0.3 and 29.0.4 `src/back/spv/index.rs:295-320` and `:341-377`, `write_restricted_index`). An access past the end reads or writes the array's last element instead. That's what wgpu has naga do on any device without `robustBufferAccess2` (`naga_options` in `adapter.rs`), and it's within what core `robustBufferAccess` allows, which is any value from within the buffer. The last element is always inside the binding, because wgpu-core checks every buffer binding against the size the shader needs, counting a runtime-sized array as one element (naga 27.0.3 `src/proc/type_methods.rs:216`, 29.0.4 `:249`): when the bind group is made, or, for a layout without `min_binding_size`, at the draw or dispatch (wgpu-core 27.0.3 `src/device/resource.rs:2627`, `BindingSizeTooSmall`, and `src/command/bind.rs:449`, `check_late_buffer_bindings`, called from `render.rs:523` and `compute.rs:276`; 29.0.4 `resource.rs:2956` and `bind.rs:482`). Image loads keep the driver's image robustness (`robustImageAccess2`), and binding arrays are unchecked in every mode, as upstream.

**What each costs.** On the Switch, in Skate 3's perf4 runs at 1280×720, the driver's checks made each world fragment take 54.2 ns against 41.8 ns without them, 30% longer, and the game ran at 7.7 fps against 9.5. Clamp hasn't run on hardware yet. In the nakhost harness (the findings' `tools/nakhost`: NAK built from mesa-switch with this repo's patches, `NAK_DEBUG=xbar,stats`, SPIR-V from the same naga as the game), clamp adds a few instructions and no loads or branches, where the driver's checks add both:

| Shader | Off | Clamp | Driver |
|---|---|---|---|
| Skate 3's world fragment | 3,595 instructions (62 loads, 120 branches) | 3,708 (62, 120) | 4,118 (74, 163) |
| The same with the cutout test | 3,604 (63, 120) | 3,717 (63, 120) | 4,136 (75, 164) |
| Skate 3's water fragment | 8,318 (128, 297) | 8,353 (128, 297) | 9,735 (173, 408) |
| Skate 3's cutout depth fragment | 245 (2, 9) | 254 (2, 9) | 267 (2, 11) |
| Bevy's mesh vertex shader (bevy_pbr 0.18.1 `src/render/mesh.wgsl`) | 151 (7, 2) | 156 (7, 2) | 472 (22, 24) |
| The same, skinned | 227 (17, 0) | 239 (17, 0) | 1,206 (65, 65) |

Register counts are the same in all three modes for these shaders. NAK's own estimate, `static_cycles`, rises 3% with clamp for the world fragment and 11% for the two vertex shaders, against 14% and 4 to 10 times with the driver's checks.

**Clamp in the game.** Skate 3 with clamp (the perf5 NRO, Ryujinx run sk8zm) compiled its world, cutout and water fragments to exactly the counts above, and drew what it drew before clamp (run sk8zl). Its own two variants of Bevy's mesh vertex shader, for characters and props, took 191 and 293 instructions with 17 and 18 loads and 0 and 1 branches. Off, in sk8u below, they took 185 and 284 with the same loads and branches; with the driver's checks, in sk8t, 1,188 and 1,287 with 65 and 66 loads and 65 and 67 branches. Unlike the harness's variant, they took 8 more registers with clamp than in either other mode, 56 against 48 and 64 against 56, so fewer of their warps fit at once: 36 against 40, and 32 against 36.

**Off against the driver's checks in the emulator.** Skate 3 in the Ryujinx runs sk8t and sk8u: the same NRO, from before clamp, the first with the driver's robustness (the default then) and the second with `BH_ROBUST_BUFFER_ACCESS=0`. Shaders with the same `source` hash came from the same SPIR-V, so the difference is NVK's. Over the 52 shader modules both runs compiled, instructions fell from 41,611 to 30,786 (26%), branches from 1,694 to 912 and loads from 939 to 619. Register counts didn't change, except one compute shader's (48 to 40). Both runs drew the same 162 world draws with no errors. Only sk8u lived long enough for a screenshot. In it the emulator draws the skater, plants, rocks and buildings but not the ground. It doesn't draw the ground for a build from before these changes either, with robustness on (run sk8zc), and both builds stretch some triangles across the screen, sk8zc many more. On hardware, perf4's runs with robustness off drew correctly.

| Shader (`source`) | Instructions | Loads | Branches |
|---|---|---|---|
| Skate 3's world fragment, opaque materials (`3375de10`) | 3,884 → 3,371 | 70 → 49 | 155 → 116 |
| The same with the cutout test's `discard`, cutout and blended materials (`790c0d3c`) | 3,948 → 3,374 | 71 → 49 | 157 → 117 |
| Skate 3's water fragment (`309bc5fb`) | 9,269 → 8,049 | 153 → 100 | 382 → 291 |
| Bevy's mesh vertex shader, characters and props (`658c9d8f`) | 1,188 → 185 | 65 → 17 | 65 → 0 |
| The same, skinned (`0cbee040`) | 1,287 → 284 | 66 → 18 | 67 → 1 |
| Bevy's mesh preprocessing, compute (`411368b2`) | 2,365 → 763 | 70 → 32 | 129 → 12 |

The vertex and compute shaders read Bevy's per-mesh data from storage buffers, so nearly all their branches were bounds checks, and their loads were the same data a component at a time. The fragment shaders keep most of theirs: they're the material's own.

**What off risks.** Nothing keeps a Bevy shader's buffer accesses in bounds then. That isn't a security hole: the shaders are the game's own, and the GPU address space they run in is the game's alone (NVK makes it, mesa-switch `src/nouveau/horizon/nouveau_horizon_device.c:243`, `nvAddressSpaceCreate`). The risk is a bug that robustness hides. A shader that indexes past a buffer reads zero with the driver's checks and the last element with clamp, and the frame usually still looks right. With them off, it reads whatever the address holds: another of the game's buffers, which shows as wrong pixels or stretched triangles, or nothing mapped, which faults the GPU (what Horizon then does hasn't been tried; on other Vulkan drivers a fault loses the device). A write past the end, from a compute shader, changes another buffer. The indices most at risk come from data: instance and mesh indices into Bevy's per-mesh arrays, material slots, clustered lights and skin joints. Skate 3's perf4 runs with robustness off drew correctly for 30 s at a time, which doesn't cover every mesh, material and effect. Clamp keeps every access inside its binding for about 5% more instructions than off at most, in the shaders above, which is why it's the default. Use `off` to measure what clamp costs, and `driver` to see whether a rendering bug comes from an index past a buffer.

## Render-target compression

The Switch's Maxwell GPU can keep a render target compressed, which cuts the memory traffic of its clears, depth tests and blending. NVK compresses an image only when the image's memory was allocated for it alone, with `VkMemoryDedicatedAllocateInfo` naming it. On the Switch, mesa-switch's `nvk_device_memory.c` then maps that allocation with the image's compressed PTE kind from NIL, and `nvk_image_plane_bind` marks the image compressed. wgpu never allocates memory like that for a texture: it places every texture inside a larger allocation. deko3d, the Switch homebrew graphics library, does the same thing a different way: it maps its image memory with compressible kinds as well as plain ones, and decompresses an image only before presenting it (`source/dk_memblock.cpp:93-103`, `source/maxwell/image_formats.cpp`, `source/dk_swapchain.cpp:213-217`).

- **On Horizon, `BH_RT_COMPRESSION=1` gives render targets memory of their own,** for NVK to compress. That applies only to single-sampled colour attachments of 4, 8 or 16 bytes a texel and `Depth32Float` depth attachments, each over 1 MiB, as NVK prefers (`nvk_GetImageMemoryRequirements2`); NIL has no compressed kind for smaller texels. Storage textures are left out. Every other texture is placed as upstream (`horizon_rt_compression`, `patches/wgpu-hal/src/vulkan/device.rs:18-71`, used at `:1125-1130` and `:1144`). Bevy 0.18's wgpu 27 allocates through gpu-alloc, which can't name the image, so that copy allocates the memory itself (`patches/bevy-0.18/wgpu-hal/src/vulkan/device.rs:18-67`, `:842-843`, `:1018-1083`, `:1548-1552` and `:1615-1621`).
- **It's read when wgpu makes its first texture,** so set it before the app starts: Skate 3's `options.txt` takes `env BH_RT_COMPRESSION=1`. Swapchain images come from NVK's window code and aren't affected.
- **What it logs.** A warning when it's on (`BH_RT_COMPRESSION=1: render targets get memory of their own, for NVK to compress`), then one for each of the first 32 textures it applies to (`BH_RT_COMPRESSION: "label" (1280x720x1 Depth32Float, … KiB) gets memory of its own`). mesa-switch patch 0013 has NVK report each of the first 32 such images: `[NVK] compression: a 1280x720x1 VK_FORMAT_D32_SFLOAT image with memory of its own is compressed, PTE kind 0x..`. That is what NVK asked for, not what the GPU did.
- **Off by default.** mesa-switch's compressed path hadn't run on a Switch before this. Whether Horizon keeps the compressed kinds, and has compression tags for every target, is for a hardware run to show. Skate 3's perf17 is the first.

## On the Switch

Run 5 was the first Vulkan run on hardware: a Switch Lite under Atmosphère, each app started from sphaira with a game's memory (findings 18 §7 lists the steps, findings 19 the results).

| App | What happened |
|---|---|
| nvk_probe | NVK's own triangle, without wgpu or Bevy: 60.7 and 60.1 fps over two 5 s spans, the worst frame 16.9 ms. (+) exited cleanly |
| hello_switch, Vulkan build | The lit cube and the FPS overlay. Once started it held 60 fps: its heartbeats came 600 frames and 10.0 to 10.1 s apart. (+) exited with `[Exit] PASS` |
| render_check | Two squares, then a black screen (the tester's description). 4.4 s after the first frame, wgpu reported `Caught DeviceLost error: Unknown Unexpected error variant (driver implementation is at fault)`. The window's next image failed (`Couldn't get swap chain texture: Validation`), and the app panicked in `Buffer::get_mapped_range`. Then its exit stopped inside `nvExit`. Both render_check runs panicked the same way (`panic.txt` holds two identical reports); the uploaded log is from a run without `options.txt` |
| Breakout | Played for two minutes, to `[Game] score 56, 0 bricks left`; 59.1 fps on average by frame 7200, start-up included. (+) exited with `[Exit] PASS` (`breakoutlog.txt:97`, `:99`, `:116`) |

**What the run-5 log can't say** is which call lost the device, or why. Each wgpu-hal call maps the Vulkan errors it expects there (out of memory, and for some calls device lost) and turns any other into `DeviceError::Unexpected`, dropping the VkResult (`get_unexpected_err`, `patches/wgpu-hal/src/vulkan/mod.rs`), and wgpu-core then loses the device without saying which call returned the error. NVK's own messages, which might explain it, go only to the app's debug messengers: a release build of Mesa drops them when there are none (`src/vulkan/runtime/vk_log.c:113-119` in mesa-switch), and stock wgpu makes one only when its `DEBUG` flag is set, which Bevy's release builds don't set (wgpu-types 29.0.4 `InstanceFlags::from_build_config`, `src/instance.rs:258-264`). The run-5 build logs frames 1 and 60: the emulator run rc5, of the same NRO (SHA-256 `da3fcd72…`), has both lines. The Switch's log has frame 1 and then, 4.4 s later, the loss, with no frame 60 between them. stdout and stderr both go to `log.txt` in the order they're written (`crates/bevy_horizon/src/logging.rs`, the `dup2` calls), so a frame-60 line would have come first. If the card had that NRO, render_check made fewer than 60 updates in 4.4 s, under 14 fps, and lost the device before frame 60, well before its frame-240 screenshot. Why the first frames were that slow isn't known; NVK compiling the app's shaders on first launch is one possible reason (findings 19 §2.2).

**The panic is a consequence of the loss, not a second bug.** Once a device is lost, wgpu-core refuses to create any buffer (`Device::create_buffer` starts with `check_is_valid`, `patches/wgpu-core/src/device/resource.rs:1010-1014`, `:673-679`). wgpu's `create_buffer_init`, which Bevy uses to make buffers with data in them, creates the buffer mapped and writes through `get_mapped_range_mut` (wgpu 29.0.4 `src/util/device.rs:60-74`); for a buffer that failed, wgpu treats that as fatal (`handle_error_fatal`, `patches/wgpu/src/backend/wgpu_core.rs:2239-2256`; line 2256 is the location in run 5's panic). The buffer in the panic has no label, so it isn't the screenshot's, which Bevy names `screenshot-transfer-buffer` (bevy_render 0.19.1 `src/view/window/screenshot.rs:382`). Without the panic, Bevy's default `RenderErrorHandler` would have quit the app at its next update, logging `Quitting the application due to DeviceLost RenderError` (`src/error_handler.rs:69-84`); the panic came first, in the same frame.

**The exit after the loss** stopped inside `nvExit`, with Mesa still holding GPU memory. bevy_horizon now skips `nvExit` in that state and exits through AM to HOME ([The `userAppExit` hook](lifecycle-and-exit.md#the-userappexit-hook), step 7).

**What run 6 adds.** Each of these is on in every Horizon build:

| Change | Where | What it logs |
|---|---|---|
| wgpu-hal always registers a `VK_EXT_debug_utils` messenger on Horizon | `patches/wgpu-hal/src/vulkan/instance.rs`, `// bevy-horizon:` comment | NVK's own error messages (Mesa's `vk_errorf`, sent at WARNING severity through `__vk_log`, `src/vulkan/runtime/vk_log.c` in mesa-switch), as `WARN wgpu_hal::vulkan::instance: GENERAL [<file>:<line> (0x0)]`, with the message on the next line |
| wgpu-hal logs the VkResult behind an unexpected error | `patches/wgpu-hal/src/vulkan/mod.rs:1466-1469` | `Vulkan returned ERROR_…, which wgpu reports as an unexpected driver error` |
| wgpu-core logs the call that lost the device | `patches/wgpu-core/src/device/resource.rs:702-710` | `wgpu-core: the device is lost to … returned to the call at <file>:<line>` |
| render_check checks the messenger at startup | `messenger_self_test` in `examples/render_check/src/main.rs` | Asks NVK about a D3D11 texture handle, which it doesn't support. The Ryujinx run rc9 logged NVK's answer, `nvk_image.c:603 … (VK_ERROR_FORMAT_NOT_SUPPORTED)`, and nothing else from NVK in the whole run |
| render_check logs frames 1 to 5, 10, 20, 30, 60, 120, 180, 200 to 202, 240 to 243, 300, 480 and 600 | `LOGGED_FRAMES` | `[Engine] frame N updated, T ms after frame 1`, each flushed to the SD card, so a cut-short log shows how far it got |
| render_check's readback test at frame 200 | `READBACK_FRAME`, `readback_test` | `[Check] readback test at frame 200: …`: it copies a 64x64 texture to a buffer and maps the buffer, before the first screenshot, so a failing copy shows apart from a failing screenshot |
| render_check's `no_screenshot` option | `options.txt` | No readback test at frame 200 and no screenshot at frame 240; (A) still takes one. If the tiles keep drawing until (A) and go black after it, reading back is what loses the device |

## What has been seen

All runs below are in the Ryujinx harness ([testing.md](testing.md#the-ryujinx-harness)). Each problem found was fixed before the next run.

| Run | App | What happened | Cause and fix |
|---|---|---|---|
| hsvk1 | hello_switch, first Vulkan build | NVK made its instance; Bevy panicked "Unable to find a GPU!" | wgpu-hal hid NVK for reporting conformance version 0; the adapter patch above |
| hsvk2 | hello_switch | Adapter found; Ryujinx crashed at startup | Ryujinx bug with NROs that have no RomFS; hello_switch got a RomFS then, and Ryujinx was fixed at rc2 |
| hsvk3 | hello_switch | The Vulkan device was made after about 46 s; the first frame crashed in Bevy's light clustering | Ryujinx's JIT lacks two vector instructions; patched |
| hsvk4, hsvk5 | hello_switch | The first frame ran, then `OUT_OF_DEVICE_MEMORY` | Ryujinx read buffer sizes from where NVK keeps push constants; patched |
| hsvk6 | hello_switch | 1200 frames of the clear colour and the UI panel, with no text and no cube; clean exit | Bevy's tonemapping LUT was missing (a real bug, fixed in the app), and Ryujinx's shader translator lacked `PRMT` (patched) |
| hsvk7 | hello_switch with the LUT and the FPS overlay | Text and the FPS overlay draw, the LUT error is gone, + exits. Still no cube | render_check was written to find out why |
| rc1 | render_check | Ryujinx crashed at startup: render_check has no RomFS | The hsvk2 bug, now fixed in Ryujinx itself |
| rc2 | render_check | The direct-draw tiles, 2 and 4, drew all four cubes. The four indirect-draw tiles drew only their clear colour. render_check's own count, 4/24, also missed tile 4, because the emulator's screenshot readback is wrong below about row 205 | Indirect draws; rc3 and rc4 narrowed it down |
| rc3 | render_check, with Ryujinx made to sync host writes back to guest memory before every semaphore acquire | The same as rc2. Ryujinx read every indirect draw's arguments as zeros, from the guest memory NVK's no-prefetch GPFIFO entries point at | Not a sync problem |
| rc4 | render_check with `options.txt` = `no_indirect_validation` | All 24 cubes drew in all six tiles (the harness screenshot at 300 s). The arguments became real draws, for example 36 indices, 1 instance, first index 42, base vertex 24. Then, at frame 240, making the screenshot's texture failed with `Out of Memory`, and Bevy quit | With the validation off the arguments came through, so the zeros came from the validation pass (rc8 found why). It's now off by default on Horizon in release builds ([Indirect-call validation](#indirect-call-validation)). The out-of-memory error is unexplained: Ryujinx refused a fixed 3.75 MiB address-space reservation (`AllocSpace: Failed to allocate size 0x3c0000`). Looking for the cause found the allocator bug in [What the emulator gets wrong](#what-the-emulator-gets-wrong), but in rc5 and rc6 the app freed almost nothing before it exited, which leaves that bug nothing to act on |
| rc5 | render_check rebuilt with the Horizon default, no `options.txt` | `[Init] wgpu indirect-call validation: off`. All 24 cubes drew in all six tiles, in every harness screenshot from 300 s to 630 s. The screenshot at frame 240 worked, and (+) exited cleanly (`[Exit] PASS`). render_check counted 12/24 because of the readback problem below | The default needs no options. Ryujinx's allocator patch was installed. Its merge ran only at exit, when Bevy freed everything, and never while the app ran |
| rc6 | render_check with `options.txt` = `indirect_validation`, and Ryujinx counting its address-space reservations and frees | As rc2: only the direct-draw tiles drew. The arguments were zeros in guest memory and in Ryujinx's host copy of it. The screenshot worked and (+) exited cleanly. Of 2,741 reservations, one was freed while the app ran, the screenshot's texture; the rest were freed at exit | Not stale memory; rc8 found the cause |
| hsvk8 | hello_switch rebuilt with the Horizon default | The lit cube draws. Holding (−) and pressing (+) showed the FPS overlay, at 13 fps, without quitting; doing it again hid it. (+) exited cleanly (`[Exit] PASS`) | hsvk7's missing cube was the validation zeros |
| rc8 | render_check with `options.txt` = `indirect_validation`, and Ryujinx reading NVK's layout for dynamic storage buffers | The arguments the GPU read were real draws, all of them values rc5 read with the validation off. Then Ryujinx died at 175 s with a segmentation fault (status 139), before the first screenshot, with nothing in its log | The zeros' cause found: [Indirect-call validation](#indirect-call-validation). The crash didn't come back in rc8b and is unexplained |
| rc8b | rc8 again, with .NET crash reports on | All 24 cubes drew in all six tiles with the validation on (`[Init] wgpu indirect-call validation: on`), from 200 s to 630 s. No crash; (+) exited cleanly (`[Exit] PASS`) | The validation works in the emulator once Ryujinx reads NVK's layout |
| bo1 | `examples/breakout`, Bevy's Breakout with gamepad input and the FPS overlay | The paddle followed the D-pad to both walls; the ball cleared all 56 bricks (`[Game] score 56, 0 bricks left`). Holding (−) and pressing (+) showed the overlay, over the score in the top-left corner, then hid it, without quitting. (+) exited cleanly (`[Exit] PASS`). 16.7 fps on average by frame 9000, and 19 fps from frame 600 on. Bevy's main schedule took 8 to 10 ms of each frame; the rest, the render step and the emulator, isn't timed ([Heartbeat lines](logging-and-crash-reports.md#heartbeat-lines)). The guest heap held at 221 MiB | Nothing to fix. Where the overlay sits is Bevy's choice ([The FPS overlay](#the-fps-overlay)) |
| rc9 | render_check with run 6's diagnostics, no `options.txt` | The messenger self-test's warning, naming `nvk_image.c:603`, reached `log.txt`, and NVK returned `ERROR_FORMAT_NOT_SUPPORTED`. No other NVK message in the whole run. All 24 cubes drew (the harness screenshot at 280 s). (+) exited: `[Exit] nvdrv: already closed (the GPU driver released it)`, then PASS | The messenger works and adds no noise |
| rc9p, rc9p2 | render_check with `options.txt` = `panic`, which panics at frame 120 with the GPU driver running | rc9p's exit looked only for DeviceMapped pages, which Ryujinx never sets, so it called `nvExit`, which returned. rc9p2 also counts Uncached pages: it found 1830 blocks, 182.31 MiB, held for the GPU, skipped `nvExit` and exited through AM (`[Exit] WARN`) | The exit after a panic; Ryujinx can't show the hardware hang ([lifecycle-and-exit.md](lifecycle-and-exit.md#the-userappexit-hook), step 7) |

## What the emulator gets wrong

The harness runs Ryubing 1.3.3 from nixpkgs on lavapipe, a software Vulkan driver on the host CPU. Device creation takes about 46 s, frames take 50 to 200 ms, and the container peaks near 8 GB. These Ryubing bugs stopped NVK or Skate 3; the patches are in `~/ryujinx-bh/ryupatch/`, outside this repo, and are only for the harness.

| Ryubing bug | What it did to NVK or Bevy | Patch |
|---|---|---|
| `AllocSpace` with a fixed offset records the reservation at the start of the free block, not at the address asked for | NVK's next fixed `MapBufferEx` fails, so there's no device | `hle-allocspace-romfs.patch` (it replaces the earlier same-length IL patch, `patch_allocspace.py`) |
| An NRO without a RomFS: `OpenDataStorageByCurrentProcess` throws `KeyNotFoundException`. Ryujinx starts NROs without a homebrew loader, so libnx's `romfsMountSelf` asks the file system service, which a real loader never makes it do | Ryujinx aborts at startup | `hle-allocspace-romfs.patch`: returns 2002-1002 (`TargetNotFound`), and bevy_horizon logs that and carries on |
| The JIT has no vector `FCVTPU` or `FCVTMU` | Crash in Bevy's light clustering on the first frame | `armeilleure-fcvt.patch` |
| Storage-buffer sizes are read from constant buffer 0 where NVN, Nintendo's API, puts them | NVK keeps push constants there, and NVK's own copy shader puts addresses there, so Ryujinx made enormous buffers | `gpu-nvk-cb0-ssbo.patch`: only trusts a size whose padding word is 0 and which isn't 0 |
| The shader translator doesn't implement `PRMT` with an immediate | Shaders that permute bytes, among them text's, drew nothing | `shader-prmt.patch` |
| The GPU address-space allocator (`NvMemoryAllocator.DeallocateRange`) never merges a freed range with the free range directly to its left, and drops the free altogether when there's no free range below it. `IsRegionInUse` checks a single free range, so a fixed `AllocSpace` across the split fails even though all of it is free | NVK chooses each address from its own heap, which merges properly, so once a freed range has met a free one on its left, a later reservation across the join fails and the app gets `Out of Memory`. No run has been seen to hit this: render_check frees almost nothing (below) | `hle-nvalloc-coalesce.patch`, on top of `hle-allocspace-romfs.patch` |
| The shader translator finds a storage buffer's address and size in a constant buffer by reading 16 bytes from where the address starts, as NVN lays them out | NVK keeps the descriptors of buffers bound with a dynamic offset in its root table split into words, 0x100 bytes apart. Ryujinx built an address that wasn't mapped, so wgpu's indirect-call validation read zeros and every indirect draw was empty (rc2, rc3, rc6) | `gpu-nvk-dynamic-ssbo.patch`, on top of `gpu-nvk-cb0-ssbo.patch` and `gpu-acquire-sync.patch`: for constant buffer 0 at 0x300 to 0x3FF, reads the high word 0x100 on, the size 0x200 on and a zero word 0x300 on, and uses them when the zero word is 0 and the size and high word aren't. `BH_RYU_NVK_DYNSB=0` turns it off |
| When a new buffer in the GPU buffer cache covers older ones, `MultiRegionHandle`'s constructor assumes the memory-tracking handles it takes over are in order and inside the new buffer. Skate 3 hit a case where they weren't; why is still open | `IndexOutOfRangeException`, and Ryujinx aborts (sk8j, 90 s after Skate 3's first frame) | `mem-multiregion-misfit.patch`: sets a handle that doesn't fit aside, passes its state to the slots it covers, and logs `[bh-diag] MultiRegionHandle misfit #n` |
| The Vulkan backend binds a sparse buffer's memory (`vkQueueBindSparse`, in `BufferManager.CreateSparse`) without the queue lock and without waiting for it, and the buffer can be destroyed right after it's made | lavapipe ran the bind after the buffer was gone, and Ryujinx crashed with a segmentation fault (sk8k, sk8l) | `gpu-vk-sparse-bind-sync.patch`: binds under `QueueLock` and waits for the queue to go idle |
| When a guest keeps waiting on the same fence, nvservices waits for it on the CPU's side instead, but only for 1 s (`SynchronizationManager.WaitOnSyncpoint`), and `NvHostEvent.Wait` then reports the fence as passed whether the GPU reached it or not | NVK, and wgpu above it, can reuse memory, commands included, that the emulated GPU hasn't read yet. The likely cause of sk8n's abort, an `IndexOutOfRangeException` in `GPFifoClass.LoadMmeInstructionRam` | `hle-nv-fence-wait.patch`: reports the fence as passed only when it was, and otherwise answers "try again", as for any fence that hasn't passed, and logs `[bh-diag] fence wait #n` |
| When the shader translator's optimiser removes an instruction whose result is unused, it also queues the instruction that wrote each of its sources once nothing else reads that source (`Optimizer.RemoveNode`), without asking whether that instruction has side effects. An atomic is never removed, but its sources lose their last reader, so the instructions that work out its address are removed under it | NAK gives a shared-memory atomic whose result is unused a register as its destination, where NVIDIA's compiler uses RZ, and Ryujinx's copy out of that register is the first thing removed. Skate 3's triangle cull (`triangle_cull_v2.wgsl` in its vendored bevy_pbr) then counted and marked at an index that was never set, kept no triangles and drew almost none of the world (p26a). On the Switch a write to a dead register is harmless | `shader-dce-side-effects.patch`: queues the instruction only when it has no side effects |

`~/ryujinx-bh/allocrepro` shows the allocator bug outside the emulator. It compiles Ryubing's `NvMemoryAllocator.cs` as shipped and as patched, and drives both the way NVK does. Of 5,000 random NVK-like operations, the original rejected 3,604 reservations of spans that were entirely free; the patched one rejected none and agreed with a page-by-page model on every check. Neither ever called a used span free, so the bug causes false out-of-memory errors, not overlapping mappings.

In the harness, though, render_check frees almost nothing while it runs. In rc6 Ryujinx counted 2,741 reservations: one, the screenshot's texture, was freed while the app ran, and the rest at exit. In rc5 the patched merge ran only at exit. With nothing freed, the original code behaves exactly like the patched one. So the patch matters to apps that free GPU memory as they go, a game loading levels for example, and it doesn't explain rc4's out-of-memory error.

Whichever patch you rebuild, pass `-p:Version=1.3.3` to `dotnet build`, or Ryubing can't load the DLL and aborts with a `FileNotFoundException`.

`Ryujinx.ShaderTools`, in the same source tree, translates a shader binary to GLSL without running the emulator, which is how the atomic bug was found. With `NAK_DEBUG=print`, NAK prints each shader's code after "Encoded shader:"; write those words out little-endian and run `dotnet Ryujinx.ShaderTools.dll --compute --target-api Vulkan in.bin out.glsl`. The harness's copy prints the translator's warnings to stderr and assumes 64 threads in X. Only compute shaders have been tried this way.

Two more emulator problems are known but not patched:

| Problem | Effect | What to do |
|---|---|---|
| Ryujinx died with a segmentation fault (status 139) 175 s into rc8, with no managed exception in its log and the container under its memory limit (6.7 of 7.6 GiB) | The run was lost before its first screenshot | Run it again: rc8b, the same run, didn't crash. Unexplained. One guess: Ryujinx turns on Vulkan's `robustBufferAccess` only on NVIDIA hosts (`VulkanInitialization.cs:294`), so on lavapipe an out-of-bounds access is undefined behaviour |
| Reading back a texture larger than about 1 MiB returns wrong data past the first MiB | render_check's results for the bottom row of tiles (4 to 6) are wrong in the emulator | Judge the emulator's runs from the harness screenshots (`out/<run>/shot_t*.png`), not from render_check's count |

The harness also carries `gpu-acquire-sync.patch`, a diagnostic from rc3 and rc4. It makes Ryujinx sync host writes back before each semaphore acquire, and logs whether the host's copy of an indirect draw's arguments matches guest memory. It changed nothing, so it isn't needed to run NVK.

Emulator differences that don't involve rendering are in [testing.md](testing.md#what-the-emulator-gets-wrong).

## Bevy features a game needs

A game turns these on in its own `bevy` dependency; `examples/hello_switch/Cargo.toml` and `examples/render_check/Cargo.toml` show a working list.

| Feature | Why |
|---|---|
| `tonemapping_luts`, `ktx2`, `zstd_rust` | Bevy's default tonemapping, TonyMcMapface, reads a lookup table that only ships with `tonemapping_luts`. The table is a KTX2 file compressed with Zstandard, so it needs `ktx2` and a decoder; `zstd_rust` is the pure-Rust one, which builds for Horizon without a C toolchain. Without them Bevy logs `TonyMcMapFace tonemapping requires the tonemapping_luts feature` and the camera draws only its clear colour (hsvk6). The alternative is a different `Tonemapping` on every camera |
| `bevy_ui_render`, `bevy_text`, `default_font` | UI and text, the FPS overlay included |

## The FPS overlay

bevy_horizon's `fps_overlay` feature adds `HorizonFpsOverlayPlugin`, which wraps Bevy's own `FpsOverlayPlugin` from bevy_dev_tools: the FPS as text, with a frame-time graph under it (`crates/bevy_horizon/src/fps_overlay.rs`).

- **It's hidden at launch.** Hold (−) and press (+) to show or hide it; pressing both together works too. `HorizonFpsOverlayPlugin { visible_at_launch: true }` shows it from the first frame.
- **A game should ignore (+) while (−) is held,** because many games use (+) to pause or quit. `fps_chord_held` tells it when; hello_switch and render_check both use it.
- **The graph's colour is frame time.** Each bar mixes green to red by its frame time over 1/30 s, Bevy's default `min_fps` of 30. At the emulator's 5 to 11 fps every bar is fully red; that's correct, not a rendering fault.
- **It needs the UI and text features** above, or a font of the game's own in `FpsOverlayConfig::text_config`.
- **It sits in the top-left corner, over whatever the game draws there.** Bevy 0.19.1 places it there, above everything else (`GlobalZIndex(FPS_OVERLAY_ZINDEX)`), and `FpsOverlayConfig` has no setting to move it (`bevy_dev_tools-0.19.1/src/fps_overlay.rs:110-123`, `:192-201`). While it's shown it covers Breakout's score (bo1).
- **Status Monitor Overlay can't show a Bevy game's FPS,** going by its code; it hasn't been tried. Its FPS line comes from NX-FPS in masagrator's SaltyNX, which counts frames by replacing a game's dynamic imports of the present calls (`vkQueuePresentKHR`, `eglSwapBuffers`, `nvnBootstrapLoader`) in the modules loaded at launch (`saltysd_core/source/NX-FPS.cpp:36-98`). A Bevy NRO is loaded later, by hbl, and calls NVK, which is linked into it, directly, so there's no import to replace. SaltyNX also skips system titles, homebrew title IDs and a process named `hbloader` (`saltysd_proc/source/hijack.c`; both at <https://github.com/masagrator/SaltyNX> `af894141`). Status Monitor's CPU, GPU load, RAM and temperature lines don't need SaltyNX, so they work alongside a Bevy game.

## GPU timings

NVK reports `timestampPeriod = 1.0`, which is right for desktop GPUs, whose timer counts nanoseconds. The Tegra X1's GPU timer is built for a 31.25 MHz reference, adding 32 a tick, but runs from 19.2 MHz, so each unit is 1.6276 ns. Every GPU time read from timestamp queries came out 1.63 times too short: wgpu's, and Bevy's `RenderDiagnosticsPlugin`'s. UnleashedRecomp-NX measured the same tick against wall-clock time on hardware. `patches/mesa-switch/0002-nvk-switch-timestamp-period.patch` sets the right period on the Switch; its header gives the sources. `scripts/build_nvk.sh` applies it, and then `scripts/localize_nvk.sh` must remake `libvulkan_bh.a`, the library games link.

## render_check

`examples/render_check` draws the same four cubes through six cameras, each changing one thing about Bevy's 3D path. At frame 240 it takes a screenshot, looks for each cube where it should be, and writes the answer to its log. It exists because hello_switch's cube didn't draw in the emulator while everything 2D did.

| Tile | Camera |
|---|---|
| 1 | Bevy's defaults: GPU culling and indirect draws, MSAA ×4 |
| 2 | `NoIndirectDrawing`: direct draws, no GPU culling, MSAA ×4 |
| 3 | Indirect draws, MSAA off |
| 4 | Direct draws, MSAA off |
| 5 | HDR |
| 6 | Orthographic projection |

The cubes, left to right: lit cyan (`StandardMaterial`); unlit magenta; unlit yellow with alpha blending; unlit green with depth testing off (an `ExtendedMaterial` whose pipeline compares with `Always`). Each tile shows its name, its result and this legend.

Run it like any example:

```text
JOBS=4 scripts/build_example.sh render_check "Render check"
```

Copy `render_check.nro` to `sdmc:/switch/render_check/`, start it, and wait for each tile's second line to change from `checks at frame 240` to its result, such as `lit ok, unlit ok, blend ok, no-depth ok` (a cube it can't find shows `MISSING`). (A) checks again, and (+) quits. It writes, next to the NRO:

| File | What it holds |
|---|---|
| `log.txt` | `[Render]` lines: the adapter, the three features and the limits above. The messenger self-test's `[Check]` lines and NVK's warning. `[Engine] frame N updated` lines for the first frames. Then one `[Check]` line per tile, for example `T2 NoIndirectDrawing, MSAA x4: lit DRAWN (625/625 px at 498,180), …`, and a summary: `[Check] screenshot 1: 24/24 cubes drawn, mesh preprocessing on the GPU` |
| `render_check_1.png` | The screenshot it judged; (A) adds `_2`, `_3`… |
| `options.txt` | Optional, written by you; any of these words, separated by spaces or new lines. `cpu_preprocessing`: Bevy builds mesh data on the CPU instead (`PbrPlugin { use_gpu_instance_buffer_builder: false }`). `indirect_validation`: turns on wgpu's indirect-call validation, which is off by default in release builds ([Indirect-call validation](#indirect-call-validation)). `no_indirect_validation`: turns it off in a debug build too. `no_screenshot`: no screenshot at frame 240, so the tiles keep drawing without one; (A) still takes one. `panic`: panics at frame 120, to test the exit with the GPU driver still running. The `[Init]` lines say which are in effect |

A cube counts as DRAWN when at least half of a 25×25 pixel box at its projected centre differs from the tile's background. How to read a result:

| Pattern | Points to |
|---|---|
| Direct-draw tiles (2, 4) draw and indirect ones (1, 3, 5, 6) don't | Indirect draws or GPU culling |
| Only the green, depth-test-off cube draws | Depth: the clear, the compare or the viewport's depth range |
| MSAA-off tiles (3, 4) draw and the others don't | Multisampling or its resolve |
| Unlit cubes draw and the lit one doesn't | Lighting data (lights, clusters, shadows) |
| Nothing draws, and also nothing with `cpu_preprocessing` | Vertex input, the view uniform or dynamic offsets |

## The OpenGL ES path (captest)

captest still checks the OpenGL ES driver directly, without wgpu: devkitPro's portlibs Mesa (20.1, nouveau), through EGL, drawing a spinning triangle (`examples/switch_captest/src/gpu.rs`). On the console the triangle draws and you confirm it with `gpu-seen` (findings 16). Bevy doesn't use this path. wgpu's GL backend was the plan before NVK; the Vulkan path replaced it, and NVK is a far newer Mesa (26.2).

## Next

1. **Hardware, run 6:** render_check as it is, and with `no_screenshot`. The new log lines should name the Vulkan error and the call that lost the device, and `no_screenshot` tells whether the screenshot sets it off.
2. **The validation on hardware:** render_check with `options.txt` = `indirect_validation`, once render_check runs without losing the device. The emulator's zeros are explained and patched (rc8b), so all 24 cubes should draw; if they do, a game that wants the check in a release build can turn it on. If only tiles 2 and 4 draw, NVK has a problem of its own, and the validation stays off on the Switch.
3. **More of Bevy:** Breakout, the first small game, runs in the emulator (bo1) and, by the tester's report, on hardware. Next are more of Bevy's examples and small games, before sk8 and IW4L.
