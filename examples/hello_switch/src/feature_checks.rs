//! The `features` check: at frame 120 of the render world, proves the device features a game's
//! occlusion culling and terrain spill take, where the device offers them. Each writes a
//! `[Check]` line.
//!
//! - early depth: a quad writes depth 0.5, then fragment shaders that count their invocations
//!   in a storage buffer draw behind it (0.75) and in front (0.25). With `@early_depth_test(force)`
//!   the hidden one must count nothing and the visible one every pixel. The same hidden quad
//!   without the attribute is the control: its count shows whether the test ran late without it.
//! - vertex storage: a vertex shader writes each point's index into a storage buffer.
//! - mappable primary: a VERTEX | MAP_WRITE buffer, written through its mapping, feeds a vertex
//!   shader that writes into a STORAGE | MAP_READ buffer, read through its mapping. Three rounds
//!   rewrite the same buffers, so stale GPU cache lines from the round before would show.

use std::borrow::Cow;

use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroupDescriptor, BindGroupEntry, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, CompareFunction,
    DepthStencilState, Extent3d, LoadOp, MapMode, Operations, PollType, PrimitiveState, PrimitiveTopology,
    RawFragmentState, RawRenderPipelineDescriptor, RawVertexBufferLayout, RawVertexState, RenderPassDepthStencilAttachment,
    RenderPassDescriptor, ShaderModule, ShaderModuleDescriptor, ShaderSource, StoreOp, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureViewDescriptor, VertexAttribute, VertexFormat, VertexStepMode, WgpuFeatures,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

const FEATURES_CHECK_FRAME: u32 = 120;
const SIZE: u32 = 64;
const POINTS: u32 = 256;
const ROUNDS: u32 = 3;

const EARLY_DEPTH: &str = "
@group(0) @binding(0) var<storage, read_write> counts: array<atomic<u32>, 4>;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) slot: u32,
}

// A full-screen triangle. Instance 0 is the occluder (depth 0.5); 1, 2 and 3 count into slots
// 0, 1 and 2 at depths 0.75 (hidden), 0.25 (visible) and 0.75 (hidden).
@vertex
fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> Out {
    var z = 0.75;
    if i == 0u { z = 0.5; } else if i == 2u { z = 0.25; }
    let uv = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u));
    return Out(vec4<f32>(uv * 2.0 - 1.0, z, 1.0), max(i, 1u) - 1u);
}

@fragment
@early_depth_test(force)
fn fs_early(in: Out) {
    atomicAdd(&counts[in.slot], 1u);
}

@fragment
fn fs_late(in: Out) {
    atomicAdd(&counts[in.slot], 1u);
}
";

const VERTEX_WRITE: &str = "
@group(0) @binding(0) var<storage, read_write> written: array<u32>;

@vertex
fn vs(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    written[v] = v * 7u + 3u;
    return vec4<f32>(0.0, 0.0, 0.5, 1.0);
}
";

const MAPPED: &str = "
@group(0) @binding(0) var<storage, read_write> copied: array<u32>;

@vertex
fn vs(@builtin(vertex_index) v: u32, @location(0) value: u32) -> @builtin(position) vec4<f32> {
    copied[v] = value * 2u + 1u;
    return vec4<f32>(0.0, 0.0, 0.5, 1.0);
}
";

/// What round `round` writes for point `i` through the vertex buffer's mapping.
fn mapped_value(round: u32, i: u32) -> u32 {
    i ^ (round.wrapping_mul(0x0101_0101) + 0x5a00)
}

pub fn features_check(mut frame: Local<u32>, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    let this_frame = *frame;
    *frame += 1;
    if this_frame != FEATURES_CHECK_FRAME {
        return;
    }
    let features = device.features();
    let d = device.wgpu_device();
    let depth = d.create_texture(&TextureDescriptor {
        label: Some("features check depth"),
        size: Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Depth32Float,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&TextureViewDescriptor::default());
    let depth_state = |compare, write| DepthStencilState {
        format: TextureFormat::Depth32Float,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias: Default::default(),
    };
    let module = |label, source: &'static str| {
        d.create_shader_module(ShaderModuleDescriptor { label: Some(label), source: ShaderSource::Wgsl(Cow::Borrowed(source)) })
    };
    let pipeline = |label: &str, module: &ShaderModule, fragment: Option<&str>, depth: DepthStencilState, topology, buffers: &[RawVertexBufferLayout]| {
        d.create_render_pipeline(&RawRenderPipelineDescriptor {
            label: Some(label),
            layout: None,
            vertex: RawVertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers },
            primitive: PrimitiveState { topology, ..Default::default() },
            depth_stencil: Some(depth),
            multisample: Default::default(),
            fragment: fragment.map(|entry| RawFragmentState {
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let storage = |label, size, usage| device.create_buffer(&BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
    // A pass on the depth texture alone, cleared to 1.
    macro_rules! pass {
        ($encoder:expr) => {
            $encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("features check"),
                color_attachments: &[],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(Operations { load: LoadOp::Clear(1.0), store: StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            })
        };
    }
    let read_words = |buffer: &bevy::render::render_resource::Buffer| -> Option<Vec<u32>> {
        buffer.slice(..).map_async(MapMode::Read, |_| {});
        if let Err(err) = d.poll(PollType::wait_indefinitely()) {
            println!("[Check] features: FAIL, waiting for the GPU failed: {err}");
            return None;
        }
        let words = buffer.slice(..).get_mapped_range().chunks(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        buffer.unmap();
        Some(words)
    };

    if !features.contains(WgpuFeatures::SHADER_EARLY_DEPTH_TEST) {
        println!("[Check] early depth: SKIP, the device has no SHADER_EARLY_DEPTH_TEST");
    } else {
        let m = module("features check early depth", EARLY_DEPTH);
        let occluder = pipeline("occluder", &m, None, depth_state(CompareFunction::Always, true), PrimitiveTopology::TriangleList, &[]);
        let early = pipeline("early", &m, Some("fs_early"), depth_state(CompareFunction::Less, false), PrimitiveTopology::TriangleList, &[]);
        let late = pipeline("late", &m, Some("fs_late"), depth_state(CompareFunction::Less, false), PrimitiveTopology::TriangleList, &[]);
        let counts = storage("counts", 16, BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST);
        let readback = storage("counts readback", 16, BufferUsages::MAP_READ | BufferUsages::COPY_DST);
        let group = |layout| {
            d.create_bind_group(&BindGroupDescriptor {
                label: None,
                layout,
                entries: &[BindGroupEntry { binding: 0, resource: counts.as_entire_binding() }],
            })
        };
        let (early_group, late_group) = (group(&early.get_bind_group_layout(0)), group(&late.get_bind_group_layout(0)));
        queue.write_buffer(&counts, 0, &[0; 16]);
        let mut encoder = d.create_command_encoder(&CommandEncoderDescriptor { label: Some("early depth check") });
        {
            let mut rp = pass!(encoder);
            rp.set_pipeline(&occluder);
            rp.draw(0..3, 0..1);
            rp.set_pipeline(&early);
            rp.set_bind_group(0, &early_group, &[]);
            rp.draw(0..3, 1..3);
            rp.set_pipeline(&late);
            rp.set_bind_group(0, &late_group, &[]);
            rp.draw(0..3, 3..4);
        }
        encoder.copy_buffer_to_buffer(&counts, 0, &readback, 0, 16);
        queue.submit([encoder.finish()]);
        if let Some(c) = read_words(&readback) {
            let pixels = SIZE * SIZE;
            let verdict = if c[0] == 0 && c[1] == pixels { "PASS" } else { "FAIL" };
            println!(
                "[Check] early depth: {verdict}, forced-early fragments behind the occluder ran {} times (want 0), in front {} (want {pixels}); without the attribute, behind: {} (control)",
                c[0], c[1], c[2]
            );
        }
    }

    if !features.contains(WgpuFeatures::VERTEX_WRITABLE_STORAGE) {
        println!("[Check] vertex storage: SKIP, the device has no VERTEX_WRITABLE_STORAGE");
    } else {
        let m = module("features check vertex storage", VERTEX_WRITE);
        let p = pipeline("vertex storage", &m, None, depth_state(CompareFunction::Always, false), PrimitiveTopology::PointList, &[]);
        let size = u64::from(POINTS * 4);
        let written = storage("written", size, BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST);
        let readback = storage("written readback", size, BufferUsages::MAP_READ | BufferUsages::COPY_DST);
        let group = d.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &p.get_bind_group_layout(0),
            entries: &[BindGroupEntry { binding: 0, resource: written.as_entire_binding() }],
        });
        queue.write_buffer(&written, 0, &vec![0; size as usize]);
        let mut encoder = d.create_command_encoder(&CommandEncoderDescriptor { label: Some("vertex storage check") });
        {
            let mut rp = pass!(encoder);
            rp.set_pipeline(&p);
            rp.set_bind_group(0, &group, &[]);
            rp.draw(0..POINTS, 0..1);
        }
        encoder.copy_buffer_to_buffer(&written, 0, &readback, 0, size);
        queue.submit([encoder.finish()]);
        if let Some(w) = read_words(&readback) {
            let wrong = (0..POINTS).filter(|&i| w[i as usize] != i * 7 + 3).count();
            let verdict = if wrong == 0 { "PASS" } else { "FAIL" };
            println!("[Check] vertex storage: {verdict}, {wrong} of {POINTS} vertex shader writes wrong");
        }
    }

    let mappable = WgpuFeatures::MAPPABLE_PRIMARY_BUFFERS | WgpuFeatures::VERTEX_WRITABLE_STORAGE;
    if !features.contains(mappable) {
        println!("[Check] mappable primary: SKIP, the device lacks MAPPABLE_PRIMARY_BUFFERS or VERTEX_WRITABLE_STORAGE");
        return;
    }
    let m = module("features check mappable primary", MAPPED);
    let attributes = [VertexAttribute { format: VertexFormat::Uint32, offset: 0, shader_location: 0 }];
    let layout = [RawVertexBufferLayout { array_stride: 4, step_mode: VertexStepMode::Vertex, attributes: &attributes }];
    let p = pipeline("mappable primary", &m, None, depth_state(CompareFunction::Always, false), PrimitiveTopology::PointList, &layout);
    let size = u64::from(POINTS * 4);
    let values = device.create_buffer(&BufferDescriptor {
        label: Some("mapped vertices"),
        size,
        usage: BufferUsages::VERTEX | BufferUsages::MAP_WRITE,
        mapped_at_creation: true,
    });
    let copied = storage("mapped copies", size, BufferUsages::STORAGE | BufferUsages::MAP_READ);
    let group = d.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &p.get_bind_group_layout(0),
        entries: &[BindGroupEntry { binding: 0, resource: copied.as_entire_binding() }],
    });
    let mut wrong_rounds = Vec::new();
    for round in 0..ROUNDS {
        if round > 0 {
            values.slice(..).map_async(MapMode::Write, |_| {});
            if let Err(err) = d.poll(PollType::wait_indefinitely()) {
                println!("[Check] mappable primary: FAIL, waiting for the GPU failed: {err}");
                return;
            }
        }
        let bytes: Vec<u8> = (0..POINTS).flat_map(|i| mapped_value(round, i).to_le_bytes()).collect();
        values.slice(..).get_mapped_range_mut().copy_from_slice(&bytes);
        values.unmap();
        let mut encoder = d.create_command_encoder(&CommandEncoderDescriptor { label: Some("mappable primary check") });
        {
            let mut rp = pass!(encoder);
            rp.set_pipeline(&p);
            rp.set_bind_group(0, &group, &[]);
            rp.set_vertex_buffer(0, (*values).slice(..));
            rp.draw(0..POINTS, 0..1);
        }
        queue.submit([encoder.finish()]);
        let Some(w) = read_words(&copied) else { return };
        let wrong = (0..POINTS).filter(|&i| w[i as usize] != mapped_value(round, i).wrapping_mul(2).wrapping_add(1)).count();
        wrong_rounds.push(wrong);
    }
    let verdict = if wrong_rounds.iter().all(|&w| w == 0) { "PASS" } else { "FAIL" };
    println!(
        "[Check] mappable primary: {verdict}, {ROUNDS} rounds of {POINTS} values written through a VERTEX | MAP_WRITE mapping and read back through a STORAGE | MAP_READ one, wrong per round: {wrong_rounds:?}"
    );
}
