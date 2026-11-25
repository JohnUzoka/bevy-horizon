use core::{any::Any, ffi::c_void, fmt, ops::Range, ptr::NonNull};
use std::sync::Arc;

use wgpu_hal as hal;
use wgpu_types as wgt;

use super::{
    conv, dk, ffi, Api, BindGroup, Buffer, ComputePipeline, Context, EntrySlots, Kind, Pipeline, PipelineLayout,
    QuerySet, Resource, StageTextures, Texture, TextureView, Unsupported, COMPUTE, STAGE_FLAGS,
};

pub struct CommandBuffer {
    raw: NonNull<c_void>,
}

// The native command list has one owner. Recording requires exclusive access;
// shared references are only used for submission, which the bridge serializes.
unsafe impl Send for CommandBuffer {}
unsafe impl Sync for CommandBuffer {}

impl CommandBuffer {
    pub(super) fn ptr(&self) -> *mut c_void {
        self.raw.as_ptr()
    }
}

impl fmt::Debug for CommandBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CommandBuffer").field(&self.raw).finish()
    }
}

impl Drop for CommandBuffer {
    fn drop(&mut self) {
        unsafe { ffi::dkp_cmd_free(self.ptr()) };
    }
}

impl hal::DynResource for CommandBuffer {
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

impl hal::DynCommandBuffer for CommandBuffer {}

const MAX_GROUPS: usize = 8;
/// Pipeline kinds whose state is tracked separately: graphics stages, then compute.
const GRAPHICS: usize = 0;
const DISPATCH: usize = 1;

/// Immediate data words (`max_immediate_size` bytes).
struct Immediates([u32; 64]);

impl Default for Immediates {
    fn default() -> Self {
        Self([0; 64])
    }
}

#[derive(Default)]
struct State {
    /// Bound groups' resources by group index. Textures resolve at draw or dispatch time,
    /// when the pipeline says which sampler each one is combined with.
    groups: [Option<Arc<[(u32, Resource)]>>; MAX_GROUPS],
    /// Combined textures of the bound graphics and compute pipelines.
    textures: [Option<StageTextures>; 2],
    textures_dirty: [bool; 2],
    /// Texture handles last bound per stage, and how many slots they cover.
    bound: [([u32; 32], usize); 3],
    immediates: Immediates,
    /// Immediates' size and uniform-buffer slot per stage, from the last set_immediates layout.
    immediates_layout: (u32, [Option<u8>; 3]),
    immediates_dirty: [bool; 2],
    /// Query report written when the current pass ends.
    pass_end_timestamp: Option<u64>,
}

pub struct Encoder {
    ctx: Context,
    recording: Option<CommandBuffer>,
    state: State,
}

impl Encoder {
    pub(super) fn new(ctx: Context) -> Self {
        Self { ctx, recording: None, state: State::default() }
    }

    fn ptr(&self) -> *mut c_void {
        self.recording.as_ref().expect("deko3d encoder is not recording").ptr()
    }

    fn resource(&self, binding: naga::ResourceBinding) -> Option<Resource> {
        let group = self.state.groups.get(binding.group as usize)?.as_ref()?;
        group.binary_search_by_key(&binding.binding, |(b, _)| *b).ok().map(|i| group[i].1)
    }

    /// Binds what draws (`GRAPHICS`) or dispatches (`DISPATCH`) read that depends on more than
    /// one call: textures combined with samplers, and immediates.
    fn flush(&mut self, kind: usize) {
        let stages = if kind == GRAPHICS { 0..2 } else { COMPUTE..COMPUTE + 1 };
        if self.state.textures_dirty[kind] {
            self.state.textures_dirty[kind] = false;
            if let Some(textures) = self.state.textures[kind].clone() {
                for stage in stages.clone() {
                    self.bind_textures(stage, &textures[stage]);
                }
            }
        }
        let (size, slots) = self.state.immediates_layout;
        if self.state.immediates_dirty[kind] && size != 0 {
            self.state.immediates_dirty[kind] = false;
            let address = unsafe { ffi::dkp_upload(self.ptr(), self.state.immediates.0.as_ptr().cast(), size) };
            for stage in stages {
                if let Some(slot) = slots[stage] {
                    unsafe { ffi::dkp_uniform_buffer(self.ptr(), STAGE_FLAGS[stage], u32::from(slot), address, size.next_multiple_of(16)) };
                }
            }
        }
    }

    fn bind_textures(&mut self, stage: usize, textures: &[super::shader::CombinedTexture]) {
        let mut handles = [0u32; 32];
        let mut count = 0;
        for combined in textures {
            let Some(Resource::Texture(image)) = self.resource(combined.texture) else { continue };
            let sampler = match combined.sampler.and_then(|b| self.resource(b)) {
                Some(Resource::Sampler(slot)) => slot,
                _ => 0, // texelFetch-only textures take the default sampler
            };
            let slot = combined.slot as usize;
            handles[slot] = (image as u32 & 0xfffff) | ((sampler as u32) << 20);
            count = count.max(slot + 1);
        }
        if count != 0 && (count, &handles[..count]) != (self.state.bound[stage].1, &self.state.bound[stage].0[..count]) {
            unsafe { ffi::dkp_textures(self.ptr(), STAGE_FLAGS[stage], 0, handles.as_ptr(), count as u32) };
            self.state.bound[stage] = (handles, count);
        }
    }

    fn query_address(set: &QuerySet, index: u32) -> u64 {
        set.buffer.address(u64::from(index) * 16)
    }

    fn begin_pass_timestamps(&mut self, writes: Option<&hal::PassTimestampWrites<'_, QuerySet>>) {
        self.state.pass_end_timestamp = None;
        if let Some(writes) = writes {
            if let Some(index) = writes.beginning_of_pass_write_index {
                unsafe { ffi::dkp_report(self.ptr(), ffi::COUNTER_TIMESTAMP, Self::query_address(writes.query_set, index)) };
            }
            self.state.pass_end_timestamp = writes.end_of_pass_write_index.map(|i| Self::query_address(writes.query_set, i));
        }
    }

    fn end_pass_timestamp(&mut self) {
        if let Some(address) = self.state.pass_end_timestamp.take() {
            unsafe { ffi::dkp_report(self.ptr(), ffi::COUNTER_TIMESTAMP, address) };
        }
    }
}

impl fmt::Debug for Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encoder").field("recording", &self.recording).finish_non_exhaustive()
    }
}

impl hal::DynResource for Encoder {
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

macro_rules! unsupported {
    ($(fn $name:ident $(<$($generic:lifetime),+>)? ($($args:tt)*);)*) => {
        $(unsafe fn $name $(<$($generic),+>)? ($($args)*) {
            panic!(concat!("unsupported deko3d HAL command: ", stringify!($name)));
        })*
    };
}

// wgpu-hal 30 renamed dispatch and dispatch_indirect to dispatch_workgroups(_indirect).
macro_rules! dispatches {
    ($dispatch:ident, $indirect:ident) => {
        unsafe fn $dispatch(&mut self, count: [u32; 3]) {
            self.flush(DISPATCH);
            unsafe { ffi::dkp_dispatch(self.ptr(), count[0], count[1], count[2]) };
        }

        unsafe fn $indirect(&mut self, buffer: &Buffer, offset: wgt::BufferAddress) {
            self.flush(DISPATCH);
            unsafe { ffi::dkp_dispatch_indirect(self.ptr(), buffer.address(offset)) };
        }
    };
}

/// A texture copy region in deko3d's terms: layers (or 3D slices) are depth.
fn region(texture: &Texture, base: &hal::TextureCopyBase, size: &hal::CopyExtent) -> ffi::Region {
    let z = if texture.dimension() == wgt::TextureDimension::D3 { base.origin.z } else { base.array_layer };
    ffi::Region { mip: base.mip_level, x: base.origin.x, y: base.origin.y, z, width: size.width, height: size.height, depth: size.depth }
}

/// Buffer row and layer strides in bytes for deko3d (0: tightly packed).
fn buffer_strides(texture: &Texture, layout: &wgt::TexelCopyBufferLayout) -> (u32, u32) {
    let row = layout.bytes_per_row.unwrap_or(0);
    let (_, block_height) = texture.format().block_dimensions();
    let layer = match (layout.bytes_per_row, layout.rows_per_image) {
        (Some(row), Some(rows)) => rows / block_height * row,
        _ => 0,
    };
    (row, layer)
}

fn invalidations(from: u64, to: u64, copy: u64) -> u32 {
    // Image and shader caches hold whatever the next commands sample or read as uniforms.
    // Copy and mapping traffic also goes around the GPU's L2.
    let mut flags = dk::DkInvalidateFlags_Image | dk::DkInvalidateFlags_Shader;
    if (from | to) & copy != 0 {
        flags |= dk::DkInvalidateFlags_L2Cache;
    }
    flags
}

impl hal::CommandEncoder for Encoder {
    type A = Api;

    unsafe fn begin_encoding(&mut self, _label: hal::Label) -> Result<(), hal::DeviceError> {
        assert!(self.recording.is_none());
        let raw = NonNull::new(unsafe { ffi::dkp_cmd(self.ctx.ptr()) }).ok_or(hal::DeviceError::OutOfMemory)?;
        self.recording = Some(CommandBuffer { raw });
        self.state = State::default();
        Ok(())
    }

    unsafe fn discard_encoding(&mut self) {
        drop(self.recording.take());
    }

    unsafe fn end_encoding(&mut self) -> Result<CommandBuffer, hal::DeviceError> {
        Ok(self.recording.take().expect("deko3d encoder is not recording"))
    }

    unsafe fn reset_all<I: Iterator<Item = CommandBuffer>>(&mut self, command_buffers: I) {
        command_buffers.for_each(drop);
    }

    unsafe fn transition_buffers<'a, T: Iterator<Item = hal::BufferBarrier<'a, Buffer>>>(&mut self, barriers: T) {
        use wgt::BufferUses as U;
        let copy = (U::COPY_SRC | U::COPY_DST | U::MAP_READ | U::MAP_WRITE | U::QUERY_RESOLVE).bits() as u64;
        // deko3d barriers are global, so one covers the whole batch.
        let flags = barriers.fold(None, |acc: Option<u32>, b| {
            Some(acc.unwrap_or(0) | invalidations(b.usage.from.bits() as u64, b.usage.to.bits() as u64, copy))
        });
        if let Some(flags) = flags {
            unsafe { ffi::dkp_barrier(self.ptr(), flags) };
        }
    }

    unsafe fn transition_textures<'a, T: Iterator<Item = hal::TextureBarrier<'a, Texture>>>(&mut self, barriers: T) {
        use wgt::TextureUses as U;
        let copy = (U::COPY_SRC | U::COPY_DST).bits() as u64;
        let flags = barriers.fold(None, |acc: Option<u32>, b| {
            Some(acc.unwrap_or(0) | invalidations(b.usage.from.bits() as u64, b.usage.to.bits() as u64, copy))
        });
        if let Some(flags) = flags {
            unsafe { ffi::dkp_barrier(self.ptr(), flags) };
        }
    }

    unsafe fn clear_buffer(&mut self, buffer: &Buffer, range: hal::MemoryRange) {
        if !range.is_empty() {
            unsafe { ffi::dkp_clear_buffer(self.ptr(), buffer.address(range.start), range.end - range.start) };
        }
    }

    unsafe fn copy_buffer_to_buffer<T: Iterator<Item = hal::BufferCopy>>(&mut self, src: &Buffer, dst: &Buffer, regions: T) {
        for r in regions {
            unsafe { ffi::dkp_copy_buffer(self.ptr(), src.address(r.src_offset), dst.address(r.dst_offset), r.size.get()) };
        }
    }

    unsafe fn copy_texture_to_texture<T: Iterator<Item = hal::TextureCopy>>(&mut self, src: &Texture, _src_usage: wgt::TextureUses, dst: &Texture, regions: T) {
        for r in regions {
            let (from, to) = (region(src, &r.src_base, &r.size), region(dst, &r.dst_base, &r.size));
            unsafe { ffi::dkp_copy_image(self.ptr(), src.ptr(), &from, dst.ptr(), &to) };
        }
    }

    unsafe fn copy_buffer_to_texture<T: Iterator<Item = hal::BufferTextureCopy>>(&mut self, src: &Buffer, dst: &Texture, regions: T) {
        for r in regions {
            let (row, layer) = buffer_strides(dst, &r.buffer_layout);
            let target = region(dst, &r.texture_base, &r.size);
            unsafe {
                ffi::dkp_copy_buffer_image(self.ptr(), true, src.address(r.buffer_layout.offset), row, layer, dst.ptr(), &target)
            };
        }
    }

    unsafe fn copy_texture_to_buffer<T: Iterator<Item = hal::BufferTextureCopy>>(&mut self, src: &Texture, _src_usage: wgt::TextureUses, dst: &Buffer, regions: T) {
        for r in regions {
            let (row, layer) = buffer_strides(src, &r.buffer_layout);
            let source = region(src, &r.texture_base, &r.size);
            unsafe {
                ffi::dkp_copy_buffer_image(self.ptr(), false, dst.address(r.buffer_layout.offset), row, layer, src.ptr(), &source)
            };
        }
    }

    unsafe fn set_bind_group(&mut self, layout: &PipelineLayout, index: u32, group: &BindGroup, dynamic_offsets: &[wgt::DynamicOffset]) {
        let slots: &[EntrySlots] = &layout.groups[index as usize];
        let mut offsets = dynamic_offsets.iter();
        // Both are sorted by binding; dynamic offsets come in binding order too.
        for (&(binding, resource), entry) in group.0.iter().zip(slots) {
            debug_assert_eq!(binding, entry.binding);
            match resource {
                Resource::Buffer { address, size, dynamic } => {
                    let address = address + if dynamic { u64::from(*offsets.next().expect("dynamic offset")) } else { 0 };
                    let bind = if entry.kind == Kind::Uniform { ffi::dkp_uniform_buffer } else { ffi::dkp_storage_buffer };
                    match entry.slots {
                        [Some(v), Some(f), None] if v == f => unsafe {
                            bind(self.ptr(), STAGE_FLAGS[0] | STAGE_FLAGS[1], u32::from(v), address, size)
                        },
                        slots => {
                            for (stage, slot) in slots.iter().enumerate() {
                                if let Some(slot) = slot {
                                    unsafe { bind(self.ptr(), STAGE_FLAGS[stage], u32::from(*slot), address, size) };
                                }
                            }
                        }
                    }
                }
                Resource::Image(image) => {
                    let handle = image as u32 & 0xfffff;
                    for (stage, slot) in entry.slots.iter().enumerate() {
                        if let Some(slot) = slot {
                            unsafe { ffi::dkp_images(self.ptr(), STAGE_FLAGS[stage], u32::from(*slot), &handle, 1) };
                        }
                    }
                }
                Resource::Texture(_) | Resource::Sampler(_) => self.state.textures_dirty = [true; 2],
            }
        }
        self.state.groups[index as usize] = Some(group.0.clone());
    }

    unsafe fn set_immediates(&mut self, layout: &PipelineLayout, offset_bytes: u32, data: &[u32]) {
        let start = offset_bytes as usize / 4;
        self.state.immediates.0[start..start + data.len()].copy_from_slice(data);
        self.state.immediates_layout = layout.immediates;
        self.state.immediates_dirty = [true; 2];
    }

    unsafe fn insert_debug_marker(&mut self, _label: &str) {}
    unsafe fn begin_debug_marker(&mut self, _group_label: &str) {}
    unsafe fn end_debug_marker(&mut self) {}

    unsafe fn begin_query(&mut self, set: &QuerySet, _index: u32) {
        // Occlusion: count samples from here; end_query reports the count.
        if matches!(set.ty, wgt::QueryType::Occlusion) {
            unsafe { ffi::dkp_reset_counter(self.ptr(), ffi::COUNTER_SAMPLES_PASSED) };
        }
    }

    unsafe fn end_query(&mut self, set: &QuerySet, index: u32) {
        if matches!(set.ty, wgt::QueryType::Occlusion) {
            unsafe { ffi::dkp_report(self.ptr(), ffi::COUNTER_SAMPLES_PASSED, Self::query_address(set, index)) };
        }
    }

    unsafe fn write_timestamp(&mut self, set: &QuerySet, index: u32) {
        unsafe { ffi::dkp_report(self.ptr(), ffi::COUNTER_TIMESTAMP, Self::query_address(set, index)) };
    }

    unsafe fn reset_queries(&mut self, set: &QuerySet, range: Range<u32>) {
        if !range.is_empty() {
            let start = Self::query_address(set, range.start);
            unsafe { ffi::dkp_clear_buffer(self.ptr(), start, u64::from(range.end - range.start) * 16) };
        }
    }

    unsafe fn copy_query_results(&mut self, set: &QuerySet, range: Range<u32>, buffer: &Buffer, offset: u64, stride: wgt::BufferSize) {
        // Reports are {u64 value, u64 timestamp}; wgpu resolves one u64 per query.
        let field = if matches!(set.ty, wgt::QueryType::Timestamp) { 8 } else { 0 };
        unsafe { ffi::dkp_barrier(self.ptr(), dk::DkInvalidateFlags_L2Cache) };
        for (i, index) in range.enumerate() {
            let to = buffer.address(offset + i as u64 * stride.get());
            unsafe { ffi::dkp_copy_buffer(self.ptr(), Self::query_address(set, index) + field, to, 8) };
        }
    }

    unsafe fn begin_render_pass(&mut self, desc: &hal::RenderPassDescriptor<QuerySet, TextureView>) -> Result<(), hal::DeviceError> {
        if desc.extent.depth_or_array_layers != 1 || desc.multiview_mask.is_some() || desc.color_attachments.len() > 8 {
            return Err(super::unsupported("render pass with multiview or layered targets"));
        }
        let mut native = ffi::PassDesc {
            colors: [core::ptr::null_mut(); 8], color_count: desc.color_attachments.len() as u32,
            clear_mask: 0, discard_mask: 0, clear_values: [[0; 4]; 8],
            depth: core::ptr::null_mut(), depth_clear: 0, stencil_clear: 0, depth_discard: 0, depth_value: 0.0, stencil_value: 0,
            width: desc.extent.width, height: desc.extent.height, resolves: [core::ptr::null_mut(); 8],
        };
        for (index, attachment) in desc.color_attachments.iter().enumerate() {
            // deko3d binds render targets as a dense list; it has no empty slots.
            let Some(attachment) = attachment else {
                return Err(super::unsupported("render pass with an empty color attachment slot"));
            };
            if attachment.depth_slice.is_some() {
                return Err(super::unsupported("3D render targets"));
            }
            let view = attachment.target.view;
            native.colors[index] = view.ptr();
            if let Some(resolve) = &attachment.resolve_target {
                native.resolves[index] = resolve.view.ptr();
            }
            if attachment.ops.contains(hal::AttachmentOps::LOAD_CLEAR) {
                native.clear_mask |= 1 << index;
                native.clear_values[index] = conv::clear_bits(view.format, attachment.clear_value);
            }
            if attachment.ops.contains(hal::AttachmentOps::STORE_DISCARD) {
                native.discard_mask |= 1 << index;
            }
        }
        if let Some(ds) = &desc.depth_stencil_attachment {
            let view = ds.target.view;
            native.depth = view.ptr();
            if view.format.has_depth_aspect() && ds.depth_ops.contains(hal::AttachmentOps::LOAD_CLEAR) {
                native.depth_clear = 1;
                native.depth_value = ds.clear_value.0;
            }
            if view.format.has_stencil_aspect() && ds.stencil_ops.contains(hal::AttachmentOps::LOAD_CLEAR) {
                native.stencil_clear = 1;
                native.stencil_value = ds.clear_value.1;
            }
            // deko3d discards depth and stencil together.
            let discard = |ops: hal::AttachmentOps, present: bool| !present || ops.contains(hal::AttachmentOps::STORE_DISCARD);
            native.depth_discard = u32::from(discard(ds.depth_ops, view.format.has_depth_aspect())
                && discard(ds.stencil_ops, view.format.has_stencil_aspect()));
        }
        self.begin_pass_timestamps(desc.timestamp_writes.as_ref());
        unsafe { ffi::dkp_pass(self.ptr(), &native) };
        Ok(())
    }

    unsafe fn end_render_pass(&mut self) {
        unsafe { ffi::dkp_end_pass(self.ptr()) };
        self.end_pass_timestamp();
    }

    unsafe fn set_render_pipeline(&mut self, pipeline: &Pipeline) {
        unsafe { ffi::dkp_bind(self.ptr(), pipeline.ptr()) };
        self.state.textures[GRAPHICS] = Some(pipeline.textures.clone());
        self.state.textures_dirty[GRAPHICS] = true;
    }

    unsafe fn set_index_buffer<'a>(&mut self, binding: hal::BufferBinding<'a, Buffer>, format: wgt::IndexFormat) {
        let address = binding.buffer().address(binding.offset);
        unsafe { ffi::dkp_index_buffer(self.ptr(), conv::index_format(format), address) };
    }

    unsafe fn set_vertex_buffer<'a>(&mut self, index: u32, binding: hal::BufferBinding<'a, Buffer>) {
        let buffer = binding.buffer();
        unsafe { ffi::dkp_vertex_buffer(self.ptr(), index, buffer.address(binding.offset), buffer.bound_size(binding.offset, binding.size)) };
    }

    unsafe fn set_viewport(&mut self, rect: &hal::Rect<f32>, depth_range: Range<f32>) {
        unsafe { ffi::dkp_viewport(self.ptr(), rect.x, rect.y, rect.w, rect.h, depth_range.start, depth_range.end) };
    }

    unsafe fn set_scissor_rect(&mut self, rect: &hal::Rect<u32>) {
        unsafe { ffi::dkp_scissor(self.ptr(), rect.x, rect.y, rect.w, rect.h) };
    }

    unsafe fn set_stencil_reference(&mut self, value: u32) {
        unsafe { ffi::dkp_stencil_reference(self.ptr(), value) };
    }

    unsafe fn set_blend_constants(&mut self, color: &[f32; 4]) {
        unsafe { ffi::dkp_blend_constant(self.ptr(), color.as_ptr()) };
    }

    unsafe fn draw(&mut self, first_vertex: u32, vertex_count: u32, first_instance: u32, instance_count: u32) {
        self.flush(GRAPHICS);
        unsafe { ffi::dkp_draw(self.ptr(), vertex_count, instance_count, first_vertex, first_instance) };
    }

    unsafe fn draw_indexed(&mut self, first_index: u32, index_count: u32, base_vertex: i32, first_instance: u32, instance_count: u32) {
        self.flush(GRAPHICS);
        unsafe { ffi::dkp_draw_indexed(self.ptr(), index_count, instance_count, first_index, base_vertex, first_instance) };
    }

    unsafe fn draw_indirect(&mut self, buffer: &Buffer, offset: wgt::BufferAddress, draw_count: u32) {
        self.flush(GRAPHICS);
        unsafe { ffi::dkp_draw_indirect(self.ptr(), false, buffer.address(offset), draw_count, 16) };
    }

    unsafe fn draw_indexed_indirect(&mut self, buffer: &Buffer, offset: wgt::BufferAddress, draw_count: u32) {
        self.flush(GRAPHICS);
        unsafe { ffi::dkp_draw_indirect(self.ptr(), true, buffer.address(offset), draw_count, 20) };
    }

    unsafe fn begin_compute_pass(&mut self, desc: &hal::ComputePassDescriptor<QuerySet>) {
        self.begin_pass_timestamps(desc.timestamp_writes.as_ref());
    }

    unsafe fn end_compute_pass(&mut self) {
        self.end_pass_timestamp();
    }

    unsafe fn set_compute_pipeline(&mut self, pipeline: &ComputePipeline) {
        unsafe { ffi::dkp_bind_compute(self.ptr(), pipeline.ptr()) };
        self.state.textures[DISPATCH] = Some(pipeline.textures.clone());
        self.state.textures_dirty[DISPATCH] = true;
    }

    #[cfg(not(feature = "wgpu-30"))]
    dispatches!(dispatch, dispatch_indirect);
    #[cfg(feature = "wgpu-30")]
    dispatches!(dispatch_workgroups, dispatch_workgroups_indirect);

    unsafe fn build_acceleration_structures<'a, T>(&mut self, _count: u32, _descriptors: T)
    where
        Api: 'a,
        T: IntoIterator<Item = hal::BuildAccelerationStructureDescriptor<'a, Buffer, Unsupported>>,
    {
        panic!("unsupported deko3d HAL command: build_acceleration_structures");
    }

    unsupported! {
        fn draw_indirect_count(&mut self, _buffer: &Buffer, _offset: u64, _count_buffer: &Buffer, _count_offset: u64, _max_count: u32);
        fn draw_indexed_indirect_count(&mut self, _buffer: &Buffer, _offset: u64, _count_buffer: &Buffer, _count_offset: u64, _max_count: u32);
        fn draw_mesh_tasks(&mut self, _x: u32, _y: u32, _z: u32);
        fn draw_mesh_tasks_indirect(&mut self, _buffer: &Buffer, _offset: u64, _draw_count: u32);
        fn draw_mesh_tasks_indirect_count(&mut self, _buffer: &Buffer, _offset: u64, _count_buffer: &Buffer, _count_offset: u64, _max_count: u32);
        fn place_acceleration_structure_barrier(&mut self, _barrier: hal::AccelerationStructureBarrier);
        fn read_acceleration_structure_compact_size(&mut self, _structure: &Unsupported, _buf: &Buffer);
        fn copy_acceleration_structure_to_acceleration_structure(&mut self, _src: &Unsupported, _dst: &Unsupported, _copy: wgt::AccelerationStructureCopy);
        fn set_acceleration_structure_dependencies(_command_buffers: &[&CommandBuffer], _dependencies: &[&Unsupported]);
    }

    #[cfg(feature = "wgpu-30")]
    unsupported! {
        fn begin_ray_tracing_pass(&mut self, _desc: &hal::RayTracingPassDescriptor);
        fn end_ray_tracing_pass(&mut self);
        fn set_ray_tracing_pipeline(&mut self, _pipeline: &Unsupported);
        fn trace_rays<'a>(&mut self, _count: [u32; 3], _ray_generation: hal::PipelineGroupData<'a, Buffer>, _miss: hal::PipelineGroupData<'a, Buffer>, _intersection: hal::PipelineGroupData<'a, Buffer>);
    }
}
