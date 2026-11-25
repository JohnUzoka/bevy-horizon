# deko3d

This page is for anyone who wants a Bevy game to draw through [deko3d](https://github.com/devkitPro/deko3d), the Switch's own GPU API, instead of Vulkan and [NVK](glossary.md#nvk), and for contributors to the backend. It is an alternative to the default renderer on the `deko3d` branch; [Rendering](rendering.md) describes the default.

## In short

- **Turn on bevy_horizon's `deko3d` feature instead of `vulkan`, and give Bevy's `RenderPlugin` `bevy_horizon::deko3d::render_creation()`.** [hello_switch](glossary.md#hello_switch) does this with its own `deko3d` feature: `FEATURES=deko3d scripts/build_example.sh`.
- **Build UAM first:** `scripts/build_uam.sh` leaves `deko3d/lib/libuam.a`, deko3d's shader compiler as a library. Shaders are compiled on the console, from the GLSL naga writes for each pipeline, and kept for the run.
- **The game's code doesn't change.** `crates/wgpu_deko3d` is a [wgpu](glossary.md#wgpu)-hal backend, so Bevy and the game use wgpu as usual.
- **What doesn't work yet** is listed under [Limits](#limits): above all, a depth texture that a shader both compares against and reads directly.

## How it fits together

| Piece | What it does |
|---|---|
| `crates/wgpu_deko3d/native/bridge.c` | Thin C over deko3d with plain structs: devices, the queue and fences, memory, images, samplers, pipelines, command recording, swapchains. deko3d's state structs are bitfields, which Rust can't build directly |
| `crates/wgpu_deko3d/src` | The `wgpu_hal::Api`: bind group slots, texture/sampler pairing, immediates, queries, surface |
| `patches/uam/console-library.patch` | UAM v1.1.0 as a static library with a C API, and three scheduling fixes (below) |
| `crates/bevy_horizon/src/deko3d.rs` | Creates the device and queue Bevy's automatic setup would, on deko3d's one adapter |
| `patches/wgpu-hal` | Two accessors an out-of-tree backend needs: `BufferBinding::buffer` and `InstanceError::new` |
| `patches/naga` | naga 29.0.4's GLSL writer, changed for the depth reads below |

**Shaders.** wgpu hands the backend naga IR it has validated. The backend writes GLSL 4.50 with naga, numbering each resource class per stage across bind groups, so a bind group's slots depend only on the groups before it, and compiles it with UAM. GLSL combines each texture with its sampler; the backend binds a texture handle made from both at draw time, from the pairs naga reports. Immediates (push constants) become a uniform buffer that the encoder fills before each draw or dispatch that follows a change.

**Depth reads.** Upstream naga declares every depth texture `sampler2DShadow`, which only comparisons can use, so `textureLoad`, gathers and plain samples of depth failed (SSAO's depth prepass). The patched writer declares a depth texture no shader in the module compares against as a plain sampler and takes the depth from `.x`. Shadow lookups at level 0 on cube arrays (point lights) used `textureLod`, which needs `GL_EXT_texture_shadow_lod` and UAM lacks it; they use core `texture` instead, which reads level 0 outside fragment shaders and, inside them, as long as the shadow map has one mip level.

**Memory.** Buffers up to 1 MiB are slots in shared 64 KiB+ blocks: wgpu-core makes a staging buffer for each `queue.write_buffer`, and mapping a block per buffer cost about 0.3 ms. Command memory is pooled the same way. CPU-written memory is uncached on both sides, so neither needs cache maintenance.

**UAM fixes.** Upstream UAM read an instruction's scheduling data before setting it, so stall counts depended on heap contents and the console's output differed from the host tool's; ended a block with a stall of 1 after an instruction that sets a barrier, where Mesa uses 2; and dual-issued instructions, which Mesa never does on GM107 and which has been reported to corrupt lighting. With the patch, the console and a host build of the same source produce identical bytes.

## Limits

- **A depth texture one shader both compares against and reads directly fails pipeline creation,** as does reading depth in a module that passes a depth texture to a function: GLSL would need two sampler types for one binding. None of hello_switch's pipelines do either.
- **UAM emulates non-constant integer division in floating point** and warns about it in `log.txt` (`program uses non-constant integer division/modulo`): one of hello_switch's shaders divides integers this way, and results lose precision past 2^24.
- **One texture with two samplers in one shader fails pipeline creation:** GLSL combines them at the texture's binding.
- **Not supported:** atomic storage textures, binding arrays, pipeline-statistics queries, multiview, conservative rasterization, rendering into a 3D texture's slice, gaps in a pass's color attachments, dual-source blending, `multi_draw_indirect_count`.
- **deko3d's blend state writes the alpha destination factor from the color one,** so blend modes whose two differ come out wrong. Alpha and premultiplied blending are unaffected.

## Testing

`~/wgpu-deko-proof` (outside this repo) runs the same wgpu program on deko3d and NVK and compares their output: a lit, textured scene, and twelve feature tests (compute, storage textures, MSAA, immediates, queries, cube/array/3D textures, depth comparison, stencil, copies, indirect draws). Every one matched NVK byte for byte on the console.

**When the GPU faults,** the bridge appends deko3d's report to `sdmc:/switch/deko3d.log`: the engine class and the method it rejected. `BEVY_HORIZON_DEKO3D_DEBUG=1` at build time links deko3d's debug library, which also reports API misuse there.
