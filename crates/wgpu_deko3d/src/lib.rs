//! A wgpu-hal backend on deko3d, the Switch's own GPU API, for wgpu without Vulkan or NVK.
//!
//! [`instance`] makes a `wgpu::Instance` on it; everything after that is ordinary wgpu. Shaders
//! are compiled on the console: naga writes GLSL from the validated WGSL and UAM, deko3d's shader
//! compiler, linked from `deko3d/lib/libuam.a` (scripts/build_uam.sh), turns it into DKSH.
//! native/bridge.c wraps deko3d in plain C structs; docs/deko3d.md has the design and limits.
#![cfg(target_os = "horizon")]
use std::{
    any::Any,
    borrow::Borrow,
    ffi::c_void,
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use naga::back::glsl;
use wgpu_hal as hal;
use wgpu_types as wgt;

mod commands;
mod conv;
mod dk;
mod ffi;
mod shader;
mod shader_cache;
mod surface;

use commands::{CommandBuffer, Encoder};
pub use surface::{Surface, SurfaceTexture};

const LIMITS: wgt::Limits = wgt::Limits {
    max_texture_dimension_1d: 16384,
    max_texture_dimension_2d: 16384,
    max_texture_dimension_3d: 2048,
    max_texture_array_layers: 2048,
    max_bind_groups: 8,
    max_bindings_per_bind_group: 1000,
    max_dynamic_uniform_buffers_per_pipeline_layout: 8,
    max_dynamic_storage_buffers_per_pipeline_layout: 4,
    max_sampled_textures_per_shader_stage: 32,
    max_samplers_per_shader_stage: 16,
    max_storage_buffers_per_shader_stage: 16,
    max_storage_textures_per_shader_stage: 8,
    max_uniform_buffers_per_shader_stage: 16,
    max_binding_array_elements_per_shader_stage: 0,
    max_binding_array_acceleration_structure_elements_per_shader_stage: 0,
    max_binding_array_sampler_elements_per_shader_stage: 0,
    max_uniform_buffer_binding_size: 0x10000,
    max_storage_buffer_binding_size: 1 << 27,
    max_vertex_buffers: 16,
    max_buffer_size: 1 << 29,
    max_vertex_attributes: 32,
    max_vertex_buffer_array_stride: 2048,
    max_inter_stage_shader_variables: 16,
    min_uniform_buffer_offset_alignment: 256,
    min_storage_buffer_offset_alignment: 32,
    max_color_attachments: 8,
    max_color_attachment_bytes_per_sample: 32,
    max_compute_workgroup_storage_size: 16384,
    max_compute_invocations_per_workgroup: 1024,
    max_compute_workgroup_size_x: 1024,
    max_compute_workgroup_size_y: 1024,
    max_compute_workgroup_size_z: 64,
    max_compute_workgroups_per_dimension: 65535,
    // Immediates are a uniform buffer filled per draw (see shader::IMMEDIATES).
    max_immediate_size: 256,
    max_non_sampler_bindings: 1_000_000,
    ..wgt::Limits::defaults()
};

// Early depth tests: naga writes `layout(early_fragment_tests) in;`, UAM sets the DKSH header's
// flag and deko3d forces the hardware's early Z for the program. Storage writes from vertex
// shaders are GLSL `buffer` blocks bound per stage like the fragment stage's. Mappable buffers of
// any usage: CPU and GPU share the memory, and every block is mapped (create_buffer).
const FEATURES: wgt::Features = wgt::Features::TEXTURE_COMPRESSION_BC
    .union(wgt::Features::DEPTH_CLIP_CONTROL)
    .union(wgt::Features::TEXTURE_FORMAT_16BIT_NORM)
    .union(wgt::Features::INDIRECT_FIRST_INSTANCE)
    .union(wgt::Features::DEPTH32FLOAT_STENCIL8)
    .union(wgt::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    .union(wgt::Features::IMMEDIATES)
    .union(wgt::Features::TIMESTAMP_QUERY)
    .union(wgt::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
    .union(wgt::Features::TIMESTAMP_QUERY_INSIDE_PASSES)
    .union(wgt::Features::SHADER_EARLY_DEPTH_TEST)
    .union(wgt::Features::VERTEX_WRITABLE_STORAGE)
    .union(wgt::Features::MAPPABLE_PRIMARY_BUFFERS);

/// deko3d's DkStageFlag bits for the stages this backend compiles, in `Stage` order.
const STAGE_FLAGS: [u32; 3] = [dk::DkStageFlag_Vertex, dk::DkStageFlag_Fragment, dk::DkStageFlag_Compute];
const STAGES: [wgt::ShaderStages; 3] = [wgt::ShaderStages::VERTEX, wgt::ShaderStages::FRAGMENT, wgt::ShaderStages::COMPUTE];
const COMPUTE: usize = 2;

#[derive(Clone, Debug)]
pub struct Api;
pub struct Instance;
#[derive(Debug)]
pub struct Adapter;

#[derive(Debug)]
pub(crate) struct Handle(pub(crate) NonNull<c_void>);

// These opaque handles are never dereferenced in Rust. The bridge locks its own shared state
// (queue, allocators); wgpu-core guarantees resource lifetimes and mapping access ordering.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    fn new(ptr: *mut c_void) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
    fn ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

#[derive(Debug)]
struct NativeContext {
    handle: Handle,
    lost: AtomicBool,
    shaders: shader::Cache,
}

impl Drop for NativeContext {
    fn drop(&mut self) {
        unsafe { ffi::dkp_close(self.handle.ptr()) };
    }
}

#[derive(Debug, Clone)]
pub struct Context {
    raw: Arc<NativeContext>,
}

impl Context {
    fn open() -> DeviceResult<Self> {
        let handle = Handle::new(unsafe { ffi::dkp_open() }).ok_or(hal::DeviceError::Lost)?;
        Ok(Self { raw: Arc::new(NativeContext { handle, lost: AtomicBool::new(false), shaders: Default::default() }) })
    }

    pub(crate) fn ptr(&self) -> *mut c_void {
        self.raw.handle.ptr()
    }

    fn check_alive(&self) -> DeviceResult<()> {
        if self.raw.lost.load(Ordering::Acquire) { Err(hal::DeviceError::Lost) } else { Ok(()) }
    }

    fn lose(&self) -> hal::DeviceError {
        self.raw.lost.store(true, Ordering::Release);
        hal::DeviceError::Lost
    }

    /// Distinct shader programs loaded so far, compiled on the console or read from the shader cache.
    pub fn compiled_shaders(&self) -> usize {
        self.raw.shaders.len()
    }
}

#[derive(Debug)]
pub struct Buffer {
    handle: Handle,
    size: u64,
    address: u64,
    mappable: bool,
    mapped: AtomicBool,
}

impl Buffer {
    pub(crate) fn address(&self, offset: u64) -> u64 {
        self.address + offset
    }
    pub(crate) fn bound_size(&self, offset: u64, size: Option<wgt::BufferSize>) -> u32 {
        size.map_or(self.size - offset, |s| s.get()).min(u32::MAX as u64) as u32
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe { ffi::dkp_buffer_free(self.handle.ptr()) };
    }
}

#[derive(Debug)]
pub(crate) struct NativeTexture {
    handle: Handle,
    format: wgt::TextureFormat,
    dimension: wgt::TextureDimension,
    size: wgt::Extent3d,
    mips: u32,
    samples: u32,
}

impl Drop for NativeTexture {
    fn drop(&mut self) {
        unsafe { ffi::dkp_texture_free(self.handle.ptr()) };
    }
}

#[derive(Debug, Clone)]
pub struct Texture(Arc<NativeTexture>);

impl Texture {
    pub(crate) fn ptr(&self) -> *mut c_void {
        self.0.handle.ptr()
    }
    pub(crate) fn format(&self) -> wgt::TextureFormat {
        self.0.format
    }
    pub(crate) fn dimension(&self) -> wgt::TextureDimension {
        self.0.dimension
    }
}

#[derive(Debug)]
pub struct TextureView {
    handle: Handle,
    /// Image descriptor ids for sampling and for load/store, or -1.
    slot: i32,
    storage_slot: i32,
    format: wgt::TextureFormat,
    _texture: Texture,
}

impl TextureView {
    pub(crate) fn ptr(&self) -> *mut c_void {
        self.handle.ptr()
    }
}

impl Drop for TextureView {
    fn drop(&mut self) {
        unsafe { ffi::dkp_view_free(self.handle.ptr()) };
    }
}

#[derive(Debug)]
pub struct Sampler {
    slot: i32,
    ctx: Context,
}

impl Drop for Sampler {
    fn drop(&mut self) {
        unsafe { ffi::dkp_sampler_free(self.ctx.ptr(), self.slot) };
    }
}

#[derive(Debug)]
pub struct BindGroupLayout {
    entries: Arc<[wgt::BindGroupLayoutEntry]>,
}

/// A layout entry's deko3d slot in each compiled stage it is visible to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EntrySlots {
    binding: u32,
    kind: Kind,
    slots: [Option<u8>; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Uniform,
    Storage,
    Texture,
    Sampler,
    Image,
}

#[derive(Debug)]
pub struct PipelineLayout {
    /// Per bind group: each entry's slots, sorted by binding.
    groups: Vec<Arc<[EntrySlots]>>,
    binding_maps: [glsl::BindingMap; 3],
    /// Immediates' size and uniform-buffer slot per stage.
    immediates: (u32, [Option<u8>; 3]),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Resource {
    Buffer { address: u64, size: u32, dynamic: bool },
    Texture(i32),
    Sampler(i32),
    Image(i32),
}

/// Resources by binding number, sorted.
#[derive(Debug)]
pub struct BindGroup(Arc<[(u32, Resource)]>);

/// Combined textures per compiled stage (vertex, fragment, compute).
pub(crate) type StageTextures = Arc<[Vec<shader::CombinedTexture>; 3]>;

#[derive(Debug)]
pub struct Pipeline {
    handle: Handle,
    textures: StageTextures,
}

#[derive(Debug)]
pub struct ComputePipeline {
    handle: Handle,
    textures: StageTextures,
}

impl ComputePipeline {
    pub(crate) fn ptr(&self) -> *mut c_void {
        self.handle.ptr()
    }
}

impl Drop for ComputePipeline {
    fn drop(&mut self) {
        unsafe { ffi::dkp_compute_pipeline_free(self.ptr()) };
    }
}

/// Query reports are 16 bytes: {u64 counter value, u64 timestamp}.
#[derive(Debug)]
pub struct QuerySet {
    buffer: Buffer,
    ty: wgt::QueryType,
}

impl Pipeline {
    pub(crate) fn ptr(&self) -> *mut c_void {
        self.handle.ptr()
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        unsafe { ffi::dkp_pipeline_free(self.ptr()) };
    }
}

#[derive(Debug)]
pub struct Fence(Handle);

impl Drop for Fence {
    fn drop(&mut self) {
        unsafe { ffi::dkp_fence_destroy(self.0.ptr()) };
    }
}

#[derive(Debug)]
pub struct ShaderModule(hal::NagaShader);
#[derive(Debug)]
pub enum Unsupported {}

type DeviceResult<T> = Result<T, hal::DeviceError>;

impl hal::Api for Api {
    // External HAL implementations cannot add an enum variant. This is only
    // the identity Core uses to route to this injected instance, not a backend.
    const VARIANT: wgt::Backend = wgt::Backend::Noop;
    type Instance = Instance;
    type Surface = Surface;
    type Adapter = Adapter;
    type Device = Context;
    type Queue = Context;
    type CommandEncoder = Encoder;
    type CommandBuffer = CommandBuffer;
    type Buffer = Buffer;
    type Texture = Texture;
    type SurfaceTexture = SurfaceTexture;
    type TextureView = TextureView;
    type Sampler = Sampler;
    type QuerySet = QuerySet;
    type Fence = Fence;
    type BindGroupLayout = BindGroupLayout;
    type BindGroup = BindGroup;
    type PipelineLayout = PipelineLayout;
    type ShaderModule = ShaderModule;
    type RenderPipeline = Pipeline;
    type ComputePipeline = ComputePipeline;
    type PipelineCache = Unsupported;
    type AccelerationStructure = Unsupported;
    #[cfg(feature = "wgpu-30")]
    type RayTracingPipeline = Unsupported;
}

macro_rules! dyn_resources {
    ($($ty:ty $(=> $kind:ident)?),+ $(,)?) => {
        $(
            impl hal::DynResource for $ty {
                fn as_any(&self) -> &dyn Any { self }
                fn as_any_mut(&mut self) -> &mut dyn Any { self }
            }
            $(impl hal::$kind for $ty {})?
        )+
    };
}

dyn_resources! {
    Buffer => DynBuffer, Texture => DynTexture, TextureView => DynTextureView, Sampler => DynSampler,
    Fence => DynFence, BindGroupLayout => DynBindGroupLayout, BindGroup => DynBindGroup,
    PipelineLayout => DynPipelineLayout, ShaderModule => DynShaderModule, Pipeline => DynRenderPipeline,
    ComputePipeline => DynComputePipeline, QuerySet => DynQuerySet,
    // Device, queue, adapter, instance and surface get their Dyn traits from wgpu-hal's blanket impls.
    Unsupported, Context, Adapter, Instance, Surface,
}

impl hal::DynPipelineCache for Unsupported {}
impl hal::DynAccelerationStructure for Unsupported {}
#[cfg(feature = "wgpu-30")]
impl hal::DynRayTracingPipeline for Unsupported {}

impl hal::Instance for Instance {
    type A = Api;

    unsafe fn init(_desc: &hal::InstanceDescriptor<'_>) -> Result<Self, hal::InstanceError> {
        Ok(Self)
    }

    unsafe fn create_surface(
        &self,
        _display: raw_window_handle::RawDisplayHandle,
        window: raw_window_handle::RawWindowHandle,
    ) -> Result<Surface, hal::InstanceError> {
        match window {
            raw_window_handle::RawWindowHandle::Horizon(handle) => Ok(Surface::new(handle.n_window)),
            _ => Err(hal::InstanceError::new("deko3d presents only to a Horizon NWindow".into())),
        }
    }

    unsafe fn enumerate_adapters(&self, _surface_hint: Option<&Surface>) -> Vec<hal::ExposedAdapter<Api>> {
        use wgt::DownlevelFlags as D;
        let downlevel = D::COMPARISON_SAMPLERS | D::INDEPENDENT_BLEND | D::VERTEX_STORAGE | D::FRAGMENT_STORAGE
            | D::FRAGMENT_WRITABLE_STORAGE | D::INDIRECT_EXECUTION | D::BASE_VERTEX
            | D::NON_POWER_OF_TWO_MIPMAPPED_TEXTURES | D::CUBE_ARRAY_TEXTURES | D::ANISOTROPIC_FILTERING
            | D::BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED | D::UNRESTRICTED_INDEX_BUFFER | D::FULL_DRAW_INDEX_UINT32
            | D::DEPTH_BIAS_CLAMP | D::VIEW_FORMATS | D::SURFACE_VIEW_FORMATS | D::READ_ONLY_DEPTH_STENCIL
            | D::DEPTH_TEXTURE_AND_BUFFER_COPIES | D::COMPUTE_SHADERS;
        vec![hal::ExposedAdapter {
            adapter: Adapter,
            info: wgt::AdapterInfo {
                name: "Nintendo Switch Tegra X1 (deko3d)".into(),
                vendor: 0x10de,
                device: 0,
                device_type: wgt::DeviceType::IntegratedGpu,
                device_pci_bus_id: String::new(),
                driver: "deko3d".into(),
                driver_info: "wgpu-hal on deko3d, shaders compiled by UAM on the console".into(),
                backend: <Api as hal::Api>::VARIANT,
                subgroup_min_size: 0,
                subgroup_max_size: 0,
                #[cfg(not(feature = "wgpu-30"))]
                transient_saves_memory: false,
                #[cfg(feature = "wgpu-30")]
                transient_saves_memory: Some(false),
                #[cfg(feature = "wgpu-30")]
                limit_bucket: None,
            },
            features: FEATURES,
            capabilities: hal::Capabilities {
                limits: LIMITS,
                alignments: hal::Alignments {
                    buffer_copy_offset: wgt::BufferSize::new(4).unwrap(),
                    buffer_copy_pitch: wgt::BufferSize::new(4).unwrap(),
                    uniform_bounds_check_alignment: wgt::BufferSize::MIN,
                    raw_tlas_instance_size: 0,
                    ray_tracing_scratch_buffer_alignment: 1,
                    #[cfg(feature = "wgpu-30")]
                    ray_tracing_pipeline_group_data_size: 0,
                    #[cfg(feature = "wgpu-30")]
                    ray_tracing_pipeline_group_data_alignment: 0,
                    #[cfg(feature = "wgpu-30")]
                    ray_tracing_pipeline_data_offset_alignment: 0,
                },
                downlevel: wgt::DownlevelCapabilities {
                    flags: downlevel,
                    limits: wgt::DownlevelLimits::default(),
                    shader_model: wgt::ShaderModel::Sm5,
                },
                cooperative_matrix_properties: Vec::new(),
            },
        }]
    }
}

impl hal::Adapter for Adapter {
    type A = Api;

    unsafe fn open(&self, features: wgt::Features, limits: &wgt::Limits, _: &wgt::MemoryHints) -> DeviceResult<hal::OpenDevice<Api>> {
        if !FEATURES.contains(features) || !limits.check_limits(&LIMITS) {
            return Err(unsupported("requested device features or limits"));
        }
        let device = Context::open()?;
        let queue = device.clone();
        Ok(hal::OpenDevice { device, queue })
    }

    unsafe fn texture_format_capabilities(&self, format: wgt::TextureFormat) -> hal::TextureFormatCapabilities {
        conv::format_capabilities(format)
    }

    unsafe fn surface_capabilities(&self, surface: &Surface) -> Option<hal::SurfaceCapabilities> {
        Some(surface.capabilities())
    }

    unsafe fn get_presentation_timestamp(&self) -> wgt::PresentationTimestamp {
        wgt::PresentationTimestamp::INVALID_TIMESTAMP
    }

    fn get_ordered_buffer_usages(&self) -> wgt::BufferUses {
        wgt::BufferUses::empty()
    }

    fn get_ordered_texture_usages(&self) -> wgt::TextureUses {
        wgt::TextureUses::empty()
    }
}

fn synchronize_device_memory() {
    // The Tegra GPU is outside the CPU's inner-shareable domain; an ordinary
    // Rust fence is not a system-scope barrier for these uncached buffers.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    #[cfg(not(target_arch = "aarch64"))]
    std::sync::atomic::fence(Ordering::SeqCst);
}

/// The fence `Queue::submit` signals: wgpu-hal 30 takes it shared (fences synchronize themselves).
#[cfg(not(feature = "wgpu-30"))]
type SignalFence<'a> = &'a mut Fence;
#[cfg(feature = "wgpu-30")]
type SignalFence<'a> = &'a Fence;

impl hal::Queue for Context {
    type A = Api;

    unsafe fn submit(&self, commands: &[&CommandBuffer], _surfaces: &[&SurfaceTexture], (fence, value): (SignalFence<'_>, u64)) -> DeviceResult<()> {
        self.check_alive()?;
        let raw: Vec<*const c_void> = commands.iter().map(|cmd| cmd.ptr().cast_const()).collect();
        // The native queue signals its fence and flushes, but does not wait. Core retains all
        // submitted command buffers and resources until get_fence_value/wait report completion.
        // Acquired surface images were already waited for on the queue (Surface::acquire_texture).
        synchronize_device_memory();
        if !unsafe { ffi::dkp_submit(self.ptr(), raw.as_ptr(), raw.len(), fence.0.ptr(), value) } {
            return Err(self.lose());
        }
        Ok(())
    }

    unsafe fn present(&self, surface: &Surface, texture: SurfaceTexture) -> Result<(), hal::SurfaceError> {
        surface.present(texture)
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        // deko3d's GPU timestamps: dkTimestampToNs(ts) = ts * 625 / 384.
        625.0 / 384.0
    }

    #[cfg(feature = "wgpu-30")]
    unsafe fn wait_for_idle(&self) -> DeviceResult<()> {
        self.check_alive()?;
        if !unsafe { ffi::dkp_wait_idle(self.ptr()) } {
            return Err(self.lose());
        }
        Ok(())
    }
}

fn unsupported(operation: &str) -> hal::DeviceError {
    log::error!("unsupported deko3d HAL operation: {operation}");
    hal::DeviceError::Unexpected
}

fn pipeline_error(stage: wgt::ShaderStages, error: impl Into<String>) -> hal::PipelineError {
    hal::PipelineError::Linkage(stage, error.into())
}

impl hal::Device for Context {
    type A = Api;

    unsafe fn create_buffer(&self, desc: &hal::BufferDescriptor<'_>) -> DeviceResult<Buffer> {
        self.check_alive()?;
        if desc.size > LIMITS.max_buffer_size
            || desc.usage.intersects(wgt::BufferUses::ACCELERATION_STRUCTURE_SCRATCH)
        {
            return Err(unsupported("buffer descriptor"));
        }
        let host = wgt::BufferUses::MAP_READ | wgt::BufferUses::MAP_WRITE;
        let copies = wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST;
        let mappable = desc.usage.intersects(host);
        // Mapped buffers that only copies touch (staging, readback) bypass the GPU's L2. Mapped
        // buffers the GPU also draws or computes with (MAPPABLE_PRIMARY_BUFFERS) keep it: the GPU
        // memory is CPU mapped too, the CPU writes them only between submissions, each of which
        // starts by invalidating L2 (dkQueueFlush), and the fence wgpu-core waits on before a
        // MAP_READ mapping flushes L2 (dkp_submit).
        let uncached = mappable && (desc.usage - host - copies).is_empty();
        let handle = Handle::new(unsafe { ffi::dkp_buffer(self.ptr(), desc.size, uncached) })
            .ok_or(hal::DeviceError::OutOfMemory)?;
        let address = unsafe { ffi::dkp_buffer_address(handle.ptr()) };
        Ok(Buffer { handle, size: desc.size, address, mappable, mapped: AtomicBool::new(false) })
    }

    unsafe fn destroy_buffer(&self, buffer: Buffer) {
        drop(buffer);
    }

    unsafe fn add_raw_buffer(&self, _: &Buffer) {}

    unsafe fn map_buffer(&self, buffer: &Buffer, range: hal::MemoryRange) -> DeviceResult<hal::BufferMapping> {
        self.check_alive()?;
        if !buffer.mappable || range.start > range.end || range.end > buffer.size {
            return Err(unsupported("buffer mapping range or usage"));
        }
        let base = NonNull::new(unsafe { ffi::dkp_buffer_data(buffer.handle.ptr()) }.cast::<u8>())
            .ok_or(hal::DeviceError::Unexpected)?;
        assert!(!buffer.mapped.swap(true, Ordering::Relaxed), "deko3d buffer is already mapped");
        // Every buffer's memory is CPU mapped and CPU uncached: this maps the allocation itself, no shadow copy.
        Ok(hal::BufferMapping { ptr: unsafe { base.add(range.start as usize) }, is_coherent: true })
    }

    unsafe fn unmap_buffer(&self, buffer: &Buffer) {
        assert!(buffer.mapped.swap(false, Ordering::Relaxed), "deko3d buffer is not mapped");
    }

    unsafe fn flush_mapped_ranges<I: Iterator<Item = hal::MemoryRange>>(&self, _: &Buffer, _: I) {}

    unsafe fn invalidate_mapped_ranges<I: Iterator<Item = hal::MemoryRange>>(&self, _: &Buffer, _: I) {}

    unsafe fn create_texture(&self, desc: &hal::TextureDescriptor<'_>) -> DeviceResult<Texture> {
        use wgt::TextureUses as Use;
        self.check_alive()?;
        let Some((format, _)) = conv::texture_format(desc.format) else {
            return Err(unsupported("texture format"));
        };
        let storage = desc.usage.intersects(Use::STORAGE_READ_ONLY | Use::STORAGE_WRITE_ONLY | Use::STORAGE_READ_WRITE | Use::STORAGE_ATOMIC);
        let traits = dk::FORMAT_TRAIT_FLAGS[format as usize];
        if (storage && traits & dk::FormatTraitFlags_CanLoadStore == 0) || ![1, 2, 4, 8].contains(&desc.sample_count) {
            return Err(unsupported("storage texture format or sample count"));
        }
        let multisampled = desc.sample_count > 1;
        let size = desc.size;
        let (ty, width, height, depth) = match desc.dimension {
            wgt::TextureDimension::D1 if size.depth_or_array_layers > 1 => (dk::DkImageType_1DArray, size.width, size.depth_or_array_layers, 1),
            wgt::TextureDimension::D1 => (dk::DkImageType_1D, size.width, 1, 1),
            wgt::TextureDimension::D2 if size.depth_or_array_layers > 1 => (if multisampled { dk::DkImageType_2DMSArray } else { dk::DkImageType_2DArray }, size.width, size.height, size.depth_or_array_layers),
            wgt::TextureDimension::D2 => (if multisampled { dk::DkImageType_2DMS } else { dk::DkImageType_2D }, size.width, size.height, 1),
            wgt::TextureDimension::D3 => (dk::DkImageType_3D, size.width, size.height, size.depth_or_array_layers),
        };
        let render = desc.usage.intersects(Use::COLOR_TARGET | Use::DEPTH_STENCIL_READ | Use::DEPTH_STENCIL_WRITE);
        let mut flags = if render { dk::DkImageFlags_UsageRender } else { 0 };
        if storage {
            flags |= dk::DkImageFlags_UsageLoadStore;
        }
        // MSAA resolves run on the 2D engine, from multisampled targets into resolve targets.
        if desc.usage.contains(Use::COLOR_TARGET) && traits & dk::FormatTraitFlags_CanUse2DEngine != 0
            && desc.dimension == wgt::TextureDimension::D2
        {
            flags |= dk::DkImageFlags_Usage2DEngine;
        }
        let native = ffi::TextureDesc {
            ty, format, flags, width, height, depth, mips: desc.mip_level_count, samples: desc.sample_count,
        };
        let handle = Handle::new(unsafe { ffi::dkp_texture(self.ptr(), &native) }).ok_or(hal::DeviceError::OutOfMemory)?;
        Ok(Texture(Arc::new(NativeTexture {
            handle, format: desc.format, dimension: desc.dimension, size, mips: desc.mip_level_count, samples: desc.sample_count,
        })))
    }

    unsafe fn destroy_texture(&self, texture: Texture) {
        drop(texture);
    }

    unsafe fn add_raw_texture(&self, _: &Texture) {}

    unsafe fn create_texture_view(&self, texture: &Texture, desc: &hal::TextureViewDescriptor<'_>) -> DeviceResult<TextureView> {
        use wgt::TextureUses as Use;
        self.check_alive()?;
        let native = &texture.0;
        let Some((format, _)) = conv::texture_format(desc.format) else {
            return Err(unsupported("texture view format"));
        };
        let range = &desc.range;
        let layers = range.array_layer_count.unwrap_or(native.size.depth_or_array_layers.max(1) - range.base_array_layer);
        let mips = range.mip_level_count.unwrap_or(native.mips - range.base_mip_level);
        let three_d = desc.dimension == wgt::TextureViewDimension::D3;
        let view = ffi::ViewDesc {
            ty: conv::view_type(desc.dimension, native.samples > 1),
            format: if desc.format == native.format { 0 } else { format },
            ds_source: if range.aspect == wgt::TextureAspect::StencilOnly { dk::DkDsSource_Stencil } else { dk::DkDsSource_Depth },
            layer_offset: if three_d { 0 } else { range.base_array_layer },
            layer_count: if three_d { 0 } else { layers },
            mip_offset: range.base_mip_level,
            mip_count: mips,
            swizzle: [dk::DkImageSwizzle_Red, dk::DkImageSwizzle_Green, dk::DkImageSwizzle_Blue, dk::DkImageSwizzle_Alpha],
            descriptor: u32::from(desc.usage.contains(Use::RESOURCE))
                | if desc.usage.intersects(Use::STORAGE_READ_ONLY | Use::STORAGE_WRITE_ONLY | Use::STORAGE_READ_WRITE) { 2 } else { 0 },
        };
        let handle = Handle::new(unsafe { ffi::dkp_view(texture.ptr(), &view) }).ok_or(hal::DeviceError::OutOfMemory)?;
        let (mut slot, mut storage_slot) = (-1, -1);
        unsafe { ffi::dkp_view_slots(handle.ptr(), &mut slot, &mut storage_slot) };
        Ok(TextureView { handle, slot, storage_slot, format: desc.format, _texture: texture.clone() })
    }

    unsafe fn destroy_texture_view(&self, view: TextureView) {
        drop(view);
    }

    unsafe fn create_sampler(&self, desc: &hal::SamplerDescriptor<'_>) -> DeviceResult<Sampler> {
        self.check_alive()?;
        let native = ffi::SamplerDesc {
            min_filter: conv::filter(desc.min_filter),
            mag_filter: conv::filter(desc.mag_filter),
            mip_filter: conv::mip_filter(desc.mipmap_filter),
            wrap: desc.address_modes.map(conv::wrap),
            lod_min: desc.lod_clamp.start,
            lod_max: desc.lod_clamp.end,
            lod_bias: 0.0,
            compare_enable: u32::from(desc.compare.is_some()),
            compare_op: desc.compare.map_or(dk::DkCompareOp_Less, conv::compare),
            border: conv::border(desc.border_color),
            max_anisotropy: f32::from(desc.anisotropy_clamp),
        };
        let slot = unsafe { ffi::dkp_sampler(self.ptr(), &native) };
        if slot < 0 {
            return Err(hal::DeviceError::OutOfMemory);
        }
        Ok(Sampler { slot, ctx: self.clone() })
    }

    unsafe fn destroy_sampler(&self, sampler: Sampler) {
        drop(sampler);
    }

    unsafe fn create_command_encoder(&self, desc: &hal::CommandEncoderDescriptor<'_, Context>) -> DeviceResult<Encoder> {
        self.check_alive()?;
        assert!(Arc::ptr_eq(&self.raw, &desc.queue.raw));
        Ok(Encoder::new(self.clone()))
    }

    unsafe fn create_bind_group_layout(&self, desc: &hal::BindGroupLayoutDescriptor<'_>) -> DeviceResult<BindGroupLayout> {
        for entry in desc.entries {
            let supported = entry.count.is_none() && match entry.ty {
                wgt::BindingType::Buffer { .. } | wgt::BindingType::Sampler(_) | wgt::BindingType::Texture { .. } => true,
                wgt::BindingType::StorageTexture { access, .. } => access != wgt::StorageTextureAccess::Atomic,
                _ => false,
            };
            if !supported {
                return Err(unsupported("bind group layout entry (binding arrays, atomic storage, external textures or acceleration structures)"));
            }
        }
        Ok(BindGroupLayout { entries: desc.entries.into() })
    }

    unsafe fn destroy_bind_group_layout(&self, layout: BindGroupLayout) {
        drop(layout);
    }

    unsafe fn create_pipeline_layout(&self, desc: &hal::PipelineLayoutDescriptor<'_, BindGroupLayout>) -> DeviceResult<PipelineLayout> {
        // Slots count up per stage and resource class across groups 0..n, so group i's slots
        // depend only on groups 0..=i: bind groups stay valid across compatible layouts.
        let mut counters = [[0u8; 5]; 3];
        let mut binding_maps: [glsl::BindingMap; 3] = Default::default();
        let mut groups = Vec::with_capacity(desc.bind_group_layouts.len());
        for (group, layout) in desc.bind_group_layouts.iter().enumerate() {
            let Some(layout) = layout else {
                groups.push(Arc::from([]));
                continue;
            };
            let mut entries: Vec<EntrySlots> = layout.entries.iter().map(|entry| {
                let kind = match entry.ty {
                    wgt::BindingType::Buffer { ty: wgt::BufferBindingType::Uniform, .. } => Kind::Uniform,
                    wgt::BindingType::Buffer { .. } => Kind::Storage,
                    wgt::BindingType::Texture { .. } => Kind::Texture,
                    wgt::BindingType::StorageTexture { .. } => Kind::Image,
                    _ => Kind::Sampler,
                };
                let mut slots = [None; 3];
                for (stage, mask) in STAGES.iter().enumerate() {
                    if entry.visibility.contains(*mask) {
                        let counter = &mut counters[stage][kind as usize];
                        slots[stage] = Some(*counter);
                        binding_maps[stage].insert(naga::ResourceBinding { group: group as u32, binding: entry.binding }, *counter);
                        *counter += 1;
                    }
                }
                EntrySlots { binding: entry.binding, kind, slots }
            }).collect();
            entries.sort_by_key(|e| e.binding);
            groups.push(Arc::from(entries));
        }
        // Immediates take the next uniform-buffer slot in every stage.
        let mut immediates = (desc.immediate_size, [None; 3]);
        if desc.immediate_size != 0 {
            for (stage, counter) in counters.iter_mut().enumerate() {
                let uniform = &mut counter[Kind::Uniform as usize];
                immediates.1[stage] = Some(*uniform);
                binding_maps[stage].insert(shader::IMMEDIATES, *uniform);
                *uniform += 1;
            }
        }
        let limits = [LIMITS.max_uniform_buffers_per_shader_stage, LIMITS.max_storage_buffers_per_shader_stage,
                      LIMITS.max_sampled_textures_per_shader_stage, 255, LIMITS.max_storage_textures_per_shader_stage];
        if counters.iter().any(|stage| stage.iter().zip(limits).any(|(&n, limit)| u32::from(n) > limit)) {
            return Err(unsupported("pipeline layout exceeds deko3d's per-stage bindings"));
        }
        Ok(PipelineLayout { groups, binding_maps, immediates })
    }

    unsafe fn destroy_pipeline_layout(&self, layout: PipelineLayout) {
        drop(layout);
    }

    unsafe fn create_bind_group(&self, desc: &hal::BindGroupDescriptor<'_, BindGroupLayout, Buffer, Sampler, TextureView, Unsupported>) -> DeviceResult<BindGroup> {
        let mut resources = Vec::with_capacity(desc.entries.len());
        for entry in desc.entries {
            let layout = desc.layout.entries.iter().find(|e| e.binding == entry.binding)
                .expect("bind group entry without a layout entry");
            let index = entry.resource_index as usize;
            let resource = match layout.ty {
                wgt::BindingType::Buffer { has_dynamic_offset, .. } => {
                    let binding = &desc.buffers[index];
                    let buffer = binding.buffer();
                    Resource::Buffer {
                        address: buffer.address(binding.offset),
                        size: buffer.bound_size(binding.offset, binding.size),
                        dynamic: has_dynamic_offset,
                    }
                }
                wgt::BindingType::Sampler(_) => Resource::Sampler(desc.samplers[index].slot),
                wgt::BindingType::Texture { .. } => {
                    let view = desc.textures[index].view;
                    if view.slot < 0 {
                        return Err(unsupported("texture binding of a view created without RESOURCE usage"));
                    }
                    Resource::Texture(view.slot)
                }
                wgt::BindingType::StorageTexture { .. } => {
                    let view = desc.textures[index].view;
                    if view.storage_slot < 0 {
                        return Err(unsupported("storage binding of a view created without STORAGE usage"));
                    }
                    Resource::Image(view.storage_slot)
                }
                _ => return Err(unsupported("bind group resource")),
            };
            resources.push((entry.binding, resource));
        }
        resources.sort_by_key(|(binding, _)| *binding);
        Ok(BindGroup(Arc::from(resources)))
    }

    unsafe fn destroy_bind_group(&self, group: BindGroup) {
        drop(group);
    }

    unsafe fn create_shader_module(&self, _: &hal::ShaderModuleDescriptor<'_>, input: hal::ShaderInput<'_>) -> Result<ShaderModule, hal::ShaderError> {
        self.check_alive()?;
        match input {
            hal::ShaderInput::Naga(shader) => Ok(ShaderModule(shader)),
            _ => Err(hal::ShaderError::Compilation("deko3d accepts only validated Naga shaders".into())),
        }
    }

    unsafe fn destroy_shader_module(&self, module: ShaderModule) {
        drop(module);
    }

    unsafe fn create_render_pipeline(&self, desc: &hal::RenderPipelineDescriptor<'_, PipelineLayout, ShaderModule, Unsupported>) -> Result<Pipeline, hal::PipelineError> {
        use wgt::ShaderStages as Stage;
        self.check_alive()?;
        let hal::VertexProcessor::Standard { vertex_buffers, vertex_stage } = &desc.vertex_processor else {
            return Err(pipeline_error(Stage::VERTEX, "deko3d does not support mesh shaders"));
        };
        if desc.multiview_mask.is_some() || desc.primitive.conservative {
            return Err(pipeline_error(Stage::VERTEX | Stage::FRAGMENT, "multiview and conservative rasterization are not supported"));
        }
        let shaders = &self.raw.shaders;
        let vertex = shader::compile(shaders, vertex_stage, naga::ShaderStage::Vertex, &desc.layout.binding_maps[0])?;
        let fragment = match &desc.fragment_stage {
            Some(stage) => shader::compile(shaders, stage, naga::ShaderStage::Fragment, &desc.layout.binding_maps[1])?,
            None => shader::Stage {
                dksh: shaders.compile(ffi::UAM_FRAGMENT, shader::EMPTY_FRAGMENT).map_err(|e| pipeline_error(Stage::FRAGMENT, e))?,
                textures: Vec::new(),
            },
        };

        let mut native = ffi::PipelineDesc {
            vert: vertex.dksh.as_ptr(), vert_size: vertex.dksh.len(),
            frag: fragment.dksh.as_ptr(), frag_size: fragment.dksh.len(),
            primitive: conv::primitive(desc.primitive.topology),
            restart_enable: 0, restart_index: 0,
            cull_mode: match desc.primitive.cull_mode {
                None => dk::DkFace_None,
                Some(wgt::Face::Front) => dk::DkFace_Front,
                Some(wgt::Face::Back) => dk::DkFace_Back,
            },
            front_face: match desc.primitive.front_face {
                wgt::FrontFace::Ccw => dk::DkFrontFace_CCW,
                wgt::FrontFace::Cw => dk::DkFrontFace_CW,
            },
            polygon_mode: match desc.primitive.polygon_mode {
                wgt::PolygonMode::Fill => dk::DkPolygonMode_Fill,
                wgt::PolygonMode::Line => dk::DkPolygonMode_Line,
                wgt::PolygonMode::Point => dk::DkPolygonMode_Point,
            },
            depth_clamp: u32::from(desc.primitive.unclipped_depth),
            depth_bias_enable: 0, bias_constant: 0.0, bias_slope: 0.0, bias_clamp: 0.0,
            depth_test: 0, depth_write: 0, depth_compare: dk::DkCompareOp_Always,
            stencil_test: 0, stencil_front: [0; 4], stencil_back: [0; 4], stencil_read_mask: 0, stencil_write_mask: 0,
            color_count: 0, blend_enable_mask: 0, write_masks: 0, blend: Default::default(),
            alpha_to_coverage: u32::from(desc.multisample.alpha_to_coverage_enabled),
            samples: desc.multisample.count,
            sample_mask: desc.multisample.mask as u32,
            attrib_count: 0, attribs: [ffi::Attrib::default(); 32],
            buffer_count: vertex_buffers.len() as u32, strides: [0; 16], divisors: [0; 16],
        };
        if let Some(format) = desc.primitive.strip_index_format {
            native.restart_enable = 1;
            native.restart_index = match format {
                wgt::IndexFormat::Uint16 => 0xffff,
                wgt::IndexFormat::Uint32 => 0xffff_ffff,
            };
        }

        for (index, buffer) in vertex_buffers.iter().enumerate() {
            // wgpu-hal 30 leaves unused slots as gaps: `None`, which reads nothing.
            #[cfg(feature = "wgpu-30")]
            let Some(buffer) = buffer else { continue };
            native.strides[index] = buffer.array_stride as u32;
            native.divisors[index] = u32::from(buffer.step_mode == wgt::VertexStepMode::Instance);
            for attribute in buffer.attributes {
                let Some((size, ty, bgra)) = conv::vertex_format(attribute.format) else {
                    return Err(pipeline_error(Stage::VERTEX, format!("vertex format {:?}", attribute.format)));
                };
                let location = attribute.shader_location as usize;
                native.attribs[location] = ffi::Attrib { buffer: index as u32, offset: attribute.offset as u32, size, ty, bgra: u32::from(bgra), fixed: 0 };
                native.attrib_count = native.attrib_count.max(location as u32 + 1);
            }
        }
        // deko3d matches attributes to shader inputs by index: unused locations read a constant.
        for attrib in &mut native.attribs[..native.attrib_count as usize] {
            if attrib.size == 0 {
                *attrib = ffi::Attrib { buffer: 0, offset: 0, size: dk::DkVtxAttribSize_1x32, ty: dk::DkVtxAttribType_Float, bgra: 0, fixed: 1 };
            }
        }

        if let Some(ds) = &desc.depth_stencil {
            native.depth_test = 1;
            native.depth_write = u32::from(ds.depth_write_enabled.unwrap_or(false));
            native.depth_compare = conv::compare(ds.depth_compare.unwrap_or(wgt::CompareFunction::Always));
            if ds.stencil.is_enabled() {
                native.stencil_test = 1;
                native.stencil_front = conv::stencil_face(&ds.stencil.front);
                native.stencil_back = conv::stencil_face(&ds.stencil.back);
                native.stencil_read_mask = ds.stencil.read_mask;
                native.stencil_write_mask = ds.stencil.write_mask;
            }
            if ds.bias.is_enabled() {
                native.depth_bias_enable = 1;
                native.bias_constant = ds.bias.constant as f32;
                native.bias_slope = ds.bias.slope_scale;
                native.bias_clamp = ds.bias.clamp;
            }
        }

        native.color_count = desc.color_targets.len() as u32;
        for (index, target) in desc.color_targets.iter().enumerate() {
            let Some(target) = target else { continue };
            native.write_masks |= target.write_mask.bits() << (4 * index);
            if let Some(blend) = target.blend {
                let factor = |f| conv::blend_factor(f).ok_or_else(|| pipeline_error(Stage::FRAGMENT, "dual-source blending"));
                native.blend_enable_mask |= 1 << index;
                native.blend[index] = ffi::Blend {
                    color_op: conv::blend_op(blend.color.operation),
                    src_color: factor(blend.color.src_factor)?,
                    dst_color: factor(blend.color.dst_factor)?,
                    alpha_op: conv::blend_op(blend.alpha.operation),
                    src_alpha: factor(blend.alpha.src_factor)?,
                    dst_alpha: factor(blend.alpha.dst_factor)?,
                };
            }
        }

        let handle = Handle::new(unsafe { ffi::dkp_pipeline(self.ptr(), &native) })
            .ok_or_else(|| pipeline_error(Stage::VERTEX | Stage::FRAGMENT, "deko3d rejected the compiled programs"))?;
        Ok(Pipeline { handle, textures: Arc::new([vertex.textures, fragment.textures, Vec::new()]) })
    }

    unsafe fn destroy_render_pipeline(&self, pipeline: Pipeline) {
        drop(pipeline);
    }

    unsafe fn create_compute_pipeline(&self, desc: &hal::ComputePipelineDescriptor<'_, PipelineLayout, ShaderModule, Unsupported>) -> Result<ComputePipeline, hal::PipelineError> {
        self.check_alive()?;
        let compiled = shader::compile(&self.raw.shaders, &desc.stage, naga::ShaderStage::Compute, &desc.layout.binding_maps[COMPUTE])?;
        let handle = Handle::new(unsafe { ffi::dkp_compute_pipeline(self.ptr(), compiled.dksh.as_ptr(), compiled.dksh.len()) })
            .ok_or_else(|| pipeline_error(wgt::ShaderStages::COMPUTE, "deko3d rejected the compiled program"))?;
        Ok(ComputePipeline { handle, textures: Arc::new([Vec::new(), Vec::new(), compiled.textures]) })
    }
    unsafe fn destroy_compute_pipeline(&self, pipeline: ComputePipeline) {
        drop(pipeline);
    }

    unsafe fn create_pipeline_cache(&self, _: &hal::PipelineCacheDescriptor<'_>) -> Result<Unsupported, hal::PipelineCacheError> {
        Err(unsupported("pipeline cache creation").into())
    }
    unsafe fn destroy_pipeline_cache(&self, cache: Unsupported) {
        match cache {}
    }

    #[cfg(feature = "wgpu-30")]
    unsafe fn create_ray_tracing_pipeline(&self, _: &hal::RayTracingPipelineDescriptor<'_, PipelineLayout, ShaderModule, Unsupported>) -> Result<Unsupported, hal::PipelineError> {
        Err(unsupported("ray tracing pipeline creation").into())
    }
    #[cfg(feature = "wgpu-30")]
    unsafe fn destroy_ray_tracing_pipeline(&self, pipeline: Unsupported) {
        match pipeline {}
    }
    #[cfg(feature = "wgpu-30")]
    unsafe fn get_raytracing_pipeline_group_data(&self, pipeline: &Unsupported, _: std::ops::Range<u32>) -> DeviceResult<Vec<u8>> {
        match *pipeline {}
    }

    unsafe fn create_query_set(&self, desc: &wgt::QuerySetDescriptor<hal::Label<'_>>) -> DeviceResult<QuerySet> {
        if matches!(desc.ty, wgt::QueryType::PipelineStatistics(_)) {
            return Err(unsupported("pipeline statistics queries"));
        }
        let buffer = unsafe { self.create_buffer(&hal::BufferDescriptor {
            label: None, size: u64::from(desc.count.max(1)) * 16, usage: wgt::BufferUses::COPY_SRC,
            memory_flags: hal::MemoryFlags::empty(),
        }) }?;
        Ok(QuerySet { buffer, ty: desc.ty })
    }
    unsafe fn destroy_query_set(&self, set: QuerySet) {
        drop(set);
    }

    unsafe fn create_fence(&self) -> DeviceResult<Fence> {
        self.check_alive()?;
        Handle::new(unsafe { ffi::dkp_fence_create(self.ptr()) }).map(Fence).ok_or(hal::DeviceError::OutOfMemory)
    }
    unsafe fn destroy_fence(&self, fence: Fence) {
        drop(fence);
    }

    unsafe fn get_fence_value(&self, fence: &Fence) -> DeviceResult<u64> {
        self.check_alive()?;
        let mut completed = 0;
        if !unsafe { ffi::dkp_fence_completed(fence.0.ptr(), &mut completed) } {
            return Err(self.lose());
        }
        // A completed native fence makes uncached GPU writes visible to the CPU before Core
        // retires a readback or calls a map callback.
        synchronize_device_memory();
        Ok(completed)
    }

    unsafe fn wait(&self, fence: &Fence, value: u64, timeout: Option<Duration>) -> DeviceResult<bool> {
        self.check_alive()?;
        let nanos = timeout.map_or(-1, |limit| limit.as_nanos().min(i64::MAX as u128) as i64);
        match unsafe { ffi::dkp_fence_wait(fence.0.ptr(), value, nanos) } {
            1 => {
                synchronize_device_memory();
                Ok(true)
            }
            0 => Ok(false),
            _ => Err(self.lose()),
        }
    }

    unsafe fn start_graphics_debugger_capture(&self) -> bool {
        false
    }
    unsafe fn stop_graphics_debugger_capture(&self) {}

    unsafe fn create_acceleration_structure(&self, _: &hal::AccelerationStructureDescriptor<'_>) -> DeviceResult<Unsupported> {
        Err(unsupported("acceleration structure creation"))
    }
    unsafe fn get_acceleration_structure_build_sizes(&self, _: &hal::GetAccelerationStructureBuildSizesDescriptor<'_, Buffer>) -> hal::AccelerationStructureBuildSizes {
        Default::default()
    }
    unsafe fn get_acceleration_structure_device_address(&self, structure: &Unsupported) -> u64 {
        match *structure {}
    }
    unsafe fn destroy_acceleration_structure(&self, structure: Unsupported) {
        match structure {}
    }
    fn tlas_instance_to_bytes(&self, _: hal::TlasInstance) -> Vec<u8> {
        Vec::new()
    }

    fn get_internal_counters(&self) -> wgt::HalCounters {
        wgt::HalCounters::default()
    }
    fn check_if_oom(&self) -> DeviceResult<()> {
        // Native allocation failures are reported at the allocation site.
        self.check_alive()
    }
}

pub use wgpu;

/// A wgpu instance whose one adapter is the Switch's GPU through deko3d.
pub fn instance() -> wgpu::Instance {
    unsafe { wgpu::Instance::from_hal::<Api>(Instance) }
}

/// Native operations timed by the bridge, in its stat_* order.
pub const STAT_NAMES: [&str; 7] =
    ["block_create", "block_destroy", "cmdbuf_create", "cmd_free", "submit", "fence_poll", "fence_wait"];

pub fn chunk_pool_limit() -> u32 {
    unsafe { ffi::dkp_chunk_pool_limit() }
}

/// Returns `(calls, ns)` per native operation since the previous call, then resets them.
pub fn take_stats() -> [(u64, u64); STAT_NAMES.len()] {
    let mut raw = [0u64; 2 * STAT_NAMES.len()];
    unsafe { ffi::dkp_stats(raw.as_mut_ptr()) };
    std::array::from_fn(|i| (raw[2 * i], raw[2 * i + 1]))
}

/// Distinct programs the device has loaded, compiled on the console or read from the shader cache.
pub fn compiled_programs(device: &wgpu::Device) -> usize {
    unsafe { device.as_hal::<Api>() }.map_or(0, |device| device.compiled_shaders())
}

impl Borrow<dyn hal::DynTexture> for Texture {
    fn borrow(&self) -> &dyn hal::DynTexture {
        self
    }
}
