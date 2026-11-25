//! Declarations for bridge.c. Struct layouts mirror the C definitions field for field.
#![allow(dead_code)]

use core::ffi::{c_char, c_void};

#[repr(C)]
pub struct TextureDesc {
    pub ty: u32,
    pub format: u32,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mips: u32,
    pub samples: u32,
}

#[repr(C)]
pub struct ViewDesc {
    pub ty: u32,
    pub format: u32,
    pub ds_source: u32,
    pub layer_offset: u32,
    pub layer_count: u32,
    pub mip_offset: u32,
    pub mip_count: u32,
    pub swizzle: [u32; 4],
    /// 0: none, 1: sampled, 2: storage.
    pub descriptor: u32,
}

#[repr(C)]
pub struct SamplerDesc {
    pub min_filter: u32,
    pub mag_filter: u32,
    pub mip_filter: u32,
    pub wrap: [u32; 3],
    pub lod_min: f32,
    pub lod_max: f32,
    pub lod_bias: f32,
    pub compare_enable: u32,
    pub compare_op: u32,
    pub border: [f32; 4],
    pub max_anisotropy: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Attrib {
    pub buffer: u32,
    pub offset: u32,
    pub size: u32,
    pub ty: u32,
    pub bgra: u32,
    pub fixed: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Blend {
    pub color_op: u32,
    pub src_color: u32,
    pub dst_color: u32,
    pub alpha_op: u32,
    pub src_alpha: u32,
    pub dst_alpha: u32,
}

#[repr(C)]
pub struct PipelineDesc {
    pub vert: *const u8,
    pub vert_size: usize,
    pub frag: *const u8,
    pub frag_size: usize,
    pub primitive: u32,
    pub restart_enable: u32,
    pub restart_index: u32,
    pub cull_mode: u32,
    pub front_face: u32,
    pub polygon_mode: u32,
    pub depth_clamp: u32,
    pub depth_bias_enable: u32,
    pub bias_constant: f32,
    pub bias_slope: f32,
    pub bias_clamp: f32,
    pub depth_test: u32,
    pub depth_write: u32,
    pub depth_compare: u32,
    pub stencil_test: u32,
    /// fail, pass, depth fail, compare.
    pub stencil_front: [u32; 4],
    pub stencil_back: [u32; 4],
    pub stencil_read_mask: u32,
    pub stencil_write_mask: u32,
    pub color_count: u32,
    pub blend_enable_mask: u32,
    pub write_masks: u32,
    pub blend: [Blend; 8],
    pub alpha_to_coverage: u32,
    pub samples: u32,
    pub sample_mask: u32,
    pub attrib_count: u32,
    pub attribs: [Attrib; 32],
    pub buffer_count: u32,
    pub strides: [u32; 16],
    pub divisors: [u32; 16],
}

#[repr(C)]
pub struct PassDesc {
    pub colors: [*mut c_void; 8],
    pub color_count: u32,
    pub clear_mask: u32,
    pub discard_mask: u32,
    pub clear_values: [[u32; 4]; 8],
    pub depth: *mut c_void,
    pub depth_clear: u32,
    pub stencil_clear: u32,
    pub depth_discard: u32,
    pub depth_value: f32,
    pub stencil_value: u32,
    pub width: u32,
    pub height: u32,
    /// Resolve target per color target, or null.
    pub resolves: [*mut c_void; 8],
}

#[repr(C)]
pub struct Region {
    pub mip: u32,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

unsafe extern "C" {
    pub fn dkp_open() -> *mut c_void;
    pub fn dkp_close(ctx: *mut c_void);

    pub fn dkp_buffer(ctx: *mut c_void, size: u64, mappable: bool) -> *mut c_void;
    pub fn dkp_buffer_free(buffer: *mut c_void);
    pub fn dkp_buffer_data(buffer: *mut c_void) -> *mut c_void;
    pub fn dkp_buffer_address(buffer: *mut c_void) -> u64;

    pub fn dkp_texture(ctx: *mut c_void, desc: *const TextureDesc) -> *mut c_void;
    pub fn dkp_texture_free(texture: *mut c_void);
    pub fn dkp_view(texture: *mut c_void, desc: *const ViewDesc) -> *mut c_void;
    pub fn dkp_view_slots(view: *mut c_void, sampled: *mut i32, storage: *mut i32);
    pub fn dkp_view_free(view: *mut c_void);
    pub fn dkp_sampler(ctx: *mut c_void, desc: *const SamplerDesc) -> i32;
    pub fn dkp_sampler_free(ctx: *mut c_void, slot: i32);

    pub fn dkp_pipeline(ctx: *mut c_void, desc: *const PipelineDesc) -> *mut c_void;
    pub fn dkp_pipeline_free(pipeline: *mut c_void);
    pub fn dkp_compute_pipeline(ctx: *mut c_void, dksh: *const u8, size: usize) -> *mut c_void;
    pub fn dkp_compute_pipeline_free(pipeline: *mut c_void);

    pub fn dkp_cmd(ctx: *mut c_void) -> *mut c_void;
    pub fn dkp_cmd_free(cmd: *mut c_void);

    pub fn dkp_fence_create(ctx: *mut c_void) -> *mut c_void;
    pub fn dkp_fence_destroy(fence: *mut c_void);
    pub fn dkp_fence_completed(fence: *mut c_void, completed: *mut u64) -> bool;
    pub fn dkp_fence_wait(fence: *mut c_void, value: u64, timeout_ns: i64) -> i32;
    pub fn dkp_submit(ctx: *mut c_void, commands: *const *const c_void, count: usize, fence: *mut c_void, value: u64) -> bool;

    pub fn dkp_pass(cmd: *mut c_void, desc: *const PassDesc);
    pub fn dkp_end_pass(cmd: *mut c_void);
    pub fn dkp_bind(cmd: *mut c_void, pipeline: *mut c_void);
    pub fn dkp_stencil_reference(cmd: *mut c_void, reference: u32);
    pub fn dkp_blend_constant(cmd: *mut c_void, rgba: *const f32);
    pub fn dkp_viewport(cmd: *mut c_void, x: f32, y: f32, w: f32, h: f32, min: f32, max: f32);
    pub fn dkp_scissor(cmd: *mut c_void, x: u32, y: u32, w: u32, h: u32);
    pub fn dkp_vertex_buffer(cmd: *mut c_void, slot: u32, address: u64, size: u32);
    pub fn dkp_index_buffer(cmd: *mut c_void, format: u32, address: u64);
    pub fn dkp_uniform_buffer(cmd: *mut c_void, stages: u32, slot: u32, address: u64, size: u32);
    pub fn dkp_storage_buffer(cmd: *mut c_void, stages: u32, slot: u32, address: u64, size: u32);
    pub fn dkp_textures(cmd: *mut c_void, stages: u32, first: u32, handles: *const u32, count: u32);
    pub fn dkp_images(cmd: *mut c_void, stages: u32, first: u32, handles: *const u32, count: u32);
    pub fn dkp_upload(cmd: *mut c_void, data: *const c_void, size: u32) -> u64;
    pub fn dkp_draw(cmd: *mut c_void, vertices: u32, instances: u32, first_vertex: u32, first_instance: u32);
    pub fn dkp_draw_indexed(cmd: *mut c_void, indices: u32, instances: u32, first_index: u32, base_vertex: i32, first_instance: u32);
    pub fn dkp_draw_indirect(cmd: *mut c_void, indexed: bool, address: u64, count: u32, stride: u32);
    pub fn dkp_bind_compute(cmd: *mut c_void, pipeline: *mut c_void);
    pub fn dkp_dispatch(cmd: *mut c_void, x: u32, y: u32, z: u32);
    pub fn dkp_dispatch_indirect(cmd: *mut c_void, address: u64);
    pub fn dkp_report(cmd: *mut c_void, counter: u32, address: u64);
    pub fn dkp_reset_counter(cmd: *mut c_void, counter: u32);

    pub fn dkp_copy_buffer(cmd: *mut c_void, from: u64, to: u64, size: u64);
    pub fn dkp_clear_buffer(cmd: *mut c_void, to: u64, size: u64);
    pub fn dkp_copy_buffer_image(cmd: *mut c_void, to_image: bool, address: u64, row_bytes: u32, layer_bytes: u32,
                                 texture: *mut c_void, region: *const Region);
    pub fn dkp_copy_image(cmd: *mut c_void, src: *mut c_void, src_region: *const Region, dst: *mut c_void, dst_region: *const Region);
    pub fn dkp_barrier(cmd: *mut c_void, invalidate: u32);

    pub fn dkp_swapchain(ctx: *mut c_void, native_window: *mut c_void, textures: *const *mut c_void, count: u32) -> *mut c_void;
    pub fn dkp_swapchain_free(swapchain: *mut c_void);
    pub fn dkp_swapchain_interval(swapchain: *mut c_void, interval: u32);
    pub fn dkp_acquire(swapchain: *mut c_void) -> i32;
    pub fn dkp_present(swapchain: *mut c_void, slot: i32) -> bool;

    pub fn dkp_chunk_pool_limit() -> u32;
    pub fn dkp_stats(out: *mut u64);

    pub fn dkp_uam_compile(stage: i32, glsl: *const c_char, size: *mut usize) -> *mut u8;
    pub fn dkp_uam_free(dksh: *mut c_void);

    // libnx
    pub fn nwindowGetDimensions(window: *mut c_void, width: *mut u32, height: *mut u32) -> u32;
}

/// UAM's pipeline_stage values.
pub const UAM_VERTEX: i32 = 0;
pub const UAM_FRAGMENT: i32 = 4;
pub const UAM_COMPUTE: i32 = 5;

// DkCounter values.
pub const COUNTER_TIMESTAMP: u32 = 1;
pub const COUNTER_SAMPLES_PASSED: u32 = 2;
