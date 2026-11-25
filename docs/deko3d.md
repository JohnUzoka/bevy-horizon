# deko3d

This page is for anyone who wants a Bevy game to draw through [deko3d](https://github.com/devkitPro/deko3d), the Switch's own GPU API, instead of Vulkan and [NVK](glossary.md#nvk), and for contributors to the backend. It is an alternative to the default renderer on the `deko3d` branch; [Rendering](rendering.md) describes the default.

## In short

- **Turn on bevy_horizon's `deko3d` feature instead of `vulkan`, and give Bevy's `RenderPlugin` `bevy_horizon::deko3d::render_creation()`.** [hello_switch](glossary.md#hello_switch) does this with its own `deko3d` feature: `FEATURES=deko3d scripts/build_example.sh`.
- **Build UAM and deko3d first:** `scripts/build_uam.sh` leaves `deko3d/lib/libuam.a`, deko3d's shader compiler as a library, and `deko3d/lib/libdeko3d_bh.a`, deko3d itself with a blend fix (below). Shaders are compiled on the console, from the GLSL naga writes for each pipeline, and kept for the run.
- **The game's code doesn't change.** `crates/wgpu_deko3d` is a [wgpu](glossary.md#wgpu)-hal backend, so Bevy and the game use wgpu as usual.
- **What doesn't work yet** is listed under [Limits](#limits): above all, a depth texture that a shader both compares against and reads directly.

## How it fits together

| Piece | What it does |
|---|---|
| `crates/wgpu_deko3d/native/bridge.c` | Thin C over deko3d with plain structs: devices, the queue and fences, memory, images, samplers, pipelines, command recording, swapchains. deko3d's state structs are bitfields, which Rust can't build directly |
| `crates/wgpu_deko3d/src` | The `wgpu_hal::Api`: bind group slots, texture/sampler pairing, immediates, queries, surface |
| `patches/uam/console-library.patch` | UAM v1.1.0 as a static library with a C API, and three scheduling fixes (below) |
| `patches/deko3d` | deko3d v0.5.0, the source of devkitPro's package, with its alpha blend factor fixed (below) |
| `crates/bevy_horizon/src/deko3d.rs` | Creates the device and queue Bevy's automatic setup would, on deko3d's one adapter |
| `patches/wgpu-hal` | Two accessors an out-of-tree backend needs: `BufferBinding::buffer` and `InstanceError::new` |
| `patches/naga` | naga 29.0.4's GLSL writer, changed for the depth reads below |

**Shaders.** wgpu hands the backend naga IR it has validated. The backend writes GLSL 4.50 with naga, numbering each resource class per stage across bind groups, so a bind group's slots depend only on the groups before it, and compiles it with UAM. GLSL combines each texture with its sampler; the backend binds a texture handle made from both at draw time, from the pairs naga reports. Immediates (push constants) become a uniform buffer that the encoder fills before each draw or dispatch that follows a change.

**Depth reads.** Upstream naga declares every depth texture `sampler2DShadow`, which only comparisons can use, so `textureLoad`, gathers and plain samples of depth failed (SSAO's depth prepass). The patched writer declares a depth texture no shader in the module compares against as a plain sampler and takes the depth from `.x`. Shadow lookups at level 0 on cube arrays (point lights) used `textureLod`, which needs `GL_EXT_texture_shadow_lod` and UAM lacks it; they use core `texture` instead, which reads level 0 outside fragment shaders and, inside them, as long as the shadow map has one mip level.

**Memory.** Buffers up to 1 MiB are slots in shared 64 KiB+ blocks: wgpu-core makes a staging buffer for each `queue.write_buffer`, and mapping a block per buffer cost about 0.3 ms. Command memory is pooled the same way. CPU-written memory is uncached on both sides, so neither needs cache maintenance.

**UAM fixes.** Upstream UAM read an instruction's scheduling data before setting it, so stall counts depended on heap contents and the console's output differed from the host tool's; ended a block with a stall of 1 after an instruction that sets a barrier, where Mesa uses 2; and dual-issued instructions, which Mesa never does on GM107 and which has been reported to corrupt lighting. With the patch, the console and a host build of the same source produce identical bytes.

**deko3d fix.** devkitPro's deko3d 0.5.0 writes a blend state's alpha destination factor from its color one ([devkitPro/deko3d#27](https://github.com/devkitPro/deko3d/issues/27), open and unfixed on master), so a blend whose two differ, such as color blended over while alpha keeps the destination's, came out wrong. `scripts/build_uam.sh` builds v0.5.0 with the one-line fix, and its debug library, under `_bh` names so the link can't pick devkitPro's copies in `libnx/lib`.

## wgpu versions

One backend source serves two wgpu releases. `crates/wgpu_deko3d/Cargo.toml`, which bevy-horizon builds, is on wgpu 29 (Bevy 0.19). `Cargo.wgpu-30.toml` is the same package on wgpu, wgpu-hal, naga and wgpu-types 30.0.1 with the `wgpu-30` feature on; that feature selects the few places where wgpu-hal 30 differs (renamed dispatches, ray-tracing pipelines this backend reports unsupported, a shared submit fence, `Queue::wait_for_idle`, per-format surface color spaces, gaps in a pipeline's vertex buffers). Cargo never reads that manifest here; enabling `wgpu-30` with the wgpu 29 manifest doesn't build.

A game on wgpu 30 vendors the crate: `scripts/vendor_deko3d.sh <game checkout>` replaces `<game>/third_party/wgpu_deko3d` with `src/`, `native/`, `build.rs` and the wgpu 30 manifest as its `Cargo.toml`, plus a README naming the commit it came from. Change the backend here and run the script again rather than editing the copy. The game patches crates.io's wgpu-hal 30 with the same two accessors as `patches/wgpu-hal`, and builds with `BEVY_HORIZON_UAM_LIB` pointing at this repository's `deko3d/lib`.

## Limits

- **A depth texture one shader both compares against and reads directly fails pipeline creation,** as does reading depth in a module that passes a depth texture to a function: GLSL would need two sampler types for one binding. None of hello_switch's pipelines do either.
- **UAM emulates non-constant integer division in floating point** and warns about it in `log.txt` (`program uses non-constant integer division/modulo`): one of hello_switch's shaders divides integers this way, and results lose precision past 2^24.
- **One texture with two samplers in one shader fails pipeline creation:** GLSL combines them at the texture's binding.
- **Not supported:** atomic storage textures, binding arrays, pipeline-statistics queries, multiview, conservative rasterization, rendering into a 3D texture's slice, gaps in a pass's color attachments, dual-source blending, `multi_draw_indirect_count`.

## Testing

`~/wgpu-deko-proof` (outside this repo) runs the same wgpu program on deko3d and NVK and compares their output: a lit, textured scene, and twelve feature tests (compute, storage textures, MSAA, immediates, queries, cube/array/3D textures, depth comparison, stencil, copies, indirect draws). Every one matched NVK byte for byte on the console.

**The blend fix** was checked with a small deko3d program (outside this repo) that blends with differing color and alpha destination factors over a known clear and reads the target back: devkitPro's library gave alpha 0 where the destination's 0.5 should survive and 0.75 where 0.25 was expected; the patched one gives 127 and 64 of 255, as expected, with the same color results.

**When the GPU faults,** the bridge appends deko3d's report to `sdmc:/switch/deko3d.log`: the engine class and the method it rejected. `BEVY_HORIZON_DEKO3D_DEBUG=1` at build time links deko3d's debug library, which also reports API misuse there.
