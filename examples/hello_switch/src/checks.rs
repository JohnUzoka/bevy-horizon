//! Checks for things build A never proved. Each writes `[Check]` lines to the log.
//!
//! - asset: writes a text file into the asset folder, loads it through Bevy's `AssetServer`
//!   and compares the text. That proves `BEVY_ASSET_ROOT`, the SD card and Bevy's file reader
//!   work together, the path every game's textures, models and sounds take.
//! - fs: reads that file's details back through `std::fs` (`stat`, `fstat`, `read_dir`). Those
//!   hand C structs across, so Rust's `libc` must lay them out exactly as devkitA64's newlib does.
//! - thread: at frame 60, spawns a `std::thread` and joins it, checking that the new thread
//!   gets its own copy of a `thread_local!`. Bevy's multi-threaded mode, audio and most crates
//!   that do background work need this.
//! - romfs: reads `romfs:/hello_switch.txt`, which build_example.sh packs from `romfs/`, and
//!   compares it with the copy compiled in. That proves bevy_horizon mounted the NRO's RomFS.
//!   It also keeps Ryujinx alive: there libnx asks for the process's data storage, and Ryujinx
//!   throws instead of failing the call when the NRO has no RomFS.
//! - upload: at frame 90 of the render world, 6000 `queue.write_texture` calls go to the GPU in
//!   one submission, then the texture is read back and compared. Each write has its own staging
//!   copy and barriers, as when a game uploads a large texture array layer by layer.
//! - draws: at frame 150 of the render world, 20 frames go to the GPU back to back, nothing
//!   pacing them (as with VSync off). Each starts with a compute pass that keeps the GPU busy, so
//!   the CPU runs ahead, then 5000 `draw_indirect` calls in 20 submissions, then a
//!   `multi_draw_indirect` and a `multi_draw_indexed_indirect` of over 5000 draws each. Every
//!   draw covers its own texel; the last frame's target is read back and compared.

use std::cell::Cell;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::asset::io::Reader;
use bevy::asset::io::file::FileAssetReader;
use bevy::asset::{AssetLoader, LoadContext, LoadState};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    BindGroupDescriptor, BindGroupEntry, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites,
    CommandEncoderDescriptor, ComputePassDescriptor, Extent3d, IndexFormat, LoadOp, MapMode, MultisampleState,
    Operations, Origin3d, PollType, PrimitiveState, RawComputePipelineDescriptor, RawFragmentState,
    RawRenderPipelineDescriptor, RawVertexState, RenderPassColorAttachment, RenderPassDescriptor, ShaderModuleDescriptor,
    ShaderSource, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, WgpuTextureView,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp};

pub struct ChecksPlugin;

impl Plugin for ChecksPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<CheckText>()
            .init_asset_loader::<CheckTextLoader>()
            .add_systems(Startup, (start_asset_check, romfs_check))
            .add_systems(Update, (finish_asset_check, thread_check));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(Render, (upload_check, draws_check));
        }
    }
}

const CHECK_FILE: &str = "a2_check.txt";
const ASSET_TIMEOUT_FRAMES: u32 = 600;
const THREAD_CHECK_FRAME: u32 = 60;
const THREAD_NAME: &str = "a2-thread-check";

#[derive(Asset, TypePath)]
struct CheckText(String);

#[derive(Default, TypePath)]
struct CheckTextLoader;

impl AssetLoader for CheckTextLoader {
    type Asset = CheckText;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<CheckText, std::io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        String::from_utf8(bytes)
            .map(CheckText)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    }

    fn extensions(&self) -> &[&str] {
        &["txt"]
    }
}

#[derive(Resource)]
struct AssetCheck {
    handle: Handle<CheckText>,
    expected: String,
    requested: Instant,
    frame: u32,
    done: bool,
}

fn start_asset_check(mut commands: Commands, server: Res<AssetServer>) {
    // The folder AssetPlugin reads: <BEVY_ASSET_ROOT>/assets, with the default `file_path`.
    let dir = FileAssetReader::get_base_path().join("assets");
    let path = dir.join(CHECK_FILE);
    // Different every run, so a file left by an earlier run can't pass.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .unwrap_or(0);
    let expected = format!("bevy-horizon asset check {nanos}\n");
    // `exists` only looks at whether `stat` succeeded, not at what it filled in.
    let folder = if dir.exists() { "the existing folder" } else { "a new folder" };
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &expected)) {
        println!("[Check] asset: FAIL, couldn't write {}: {err}", path.display());
        return;
    }
    println!(
        "[Check] asset: wrote {} ({} bytes) into {folder}; loading it through the AssetServer",
        path.display(),
        expected.len()
    );
    fs_check(&dir, &path, expected.len());
    // Taken before `load`: without Bevy's `multi_threaded` feature, `load` itself reads the file.
    let requested = Instant::now();
    let handle = server.load(CHECK_FILE);
    commands.insert_resource(AssetCheck {
        handle,
        expected,
        requested,
        frame: 0,
        done: false,
    });
}

/// `stat`, `fstat` and `readdir` fill in C structs that Rust then reads at fixed offsets, so a
/// layout that differs from newlib's shows up here as wrong values, not as an error.
fn fs_check(dir: &Path, path: &Path, expected_len: usize) {
    let expected_len = expected_len as u64;
    let mut problems = Vec::new();
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() == expected_len => {}
        Ok(meta) => problems.push(format!(
            "stat says {CHECK_FILE} has is_file {} and {} bytes",
            meta.is_file(),
            meta.len()
        )),
        Err(err) => problems.push(format!("stat of {CHECK_FILE} failed: {err}")),
    }
    // `File::read_to_end`, which Bevy's file reader uses, sizes its buffer from this one.
    match std::fs::File::open(path).and_then(|file| file.metadata()) {
        Ok(meta) if meta.is_file() && meta.len() == expected_len => {}
        Ok(meta) => problems.push(format!(
            "fstat says the open {CHECK_FILE} has is_file {} and {} bytes",
            meta.is_file(),
            meta.len()
        )),
        Err(err) => problems.push(format!("open and fstat of {CHECK_FILE} failed: {err}")),
    }
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(meta) => problems.push(format!(
            "stat says the folder has is_dir false and is_file {}",
            meta.is_file()
        )),
        Err(err) => problems.push(format!("stat of the folder failed: {err}")),
    }
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut names = Vec::new();
            let mut listed_as_file = None;
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let name = entry.file_name();
                        if name == CHECK_FILE {
                            listed_as_file = Some(entry.file_type().map(|kind| kind.is_file()));
                        }
                        if names.len() < 16 {
                            names.push(name.to_string_lossy().into_owned());
                        }
                    }
                    Err(err) => {
                        problems.push(format!("read_dir failed part way: {err}"));
                        break;
                    }
                }
            }
            match listed_as_file {
                Some(Ok(true)) => {}
                Some(Ok(false)) => problems.push(format!("read_dir lists {CHECK_FILE}, but not as a file")),
                Some(Err(err)) => {
                    problems.push(format!("read_dir lists {CHECK_FILE}, but its file_type failed: {err}"))
                }
                None => problems.push(format!("read_dir doesn't list {CHECK_FILE}; it lists {names:?}")),
            }
        }
        Err(err) => problems.push(format!("read_dir failed: {err}")),
    }
    if problems.is_empty() {
        println!(
            "[Check] fs: PASS, stat and fstat say {CHECK_FILE} is a {expected_len}-byte file, stat says its folder is a folder, and read_dir lists it as a file"
        );
    } else {
        println!("[Check] fs: FAIL, {}", problems.join("; "));
    }
}

fn finish_asset_check(
    check: Option<ResMut<AssetCheck>>,
    server: Res<AssetServer>,
    texts: Res<Assets<CheckText>>,
) {
    let Some(mut check) = check else {
        return;
    };
    if check.done {
        return;
    }
    let frame = check.frame;
    check.frame += 1;
    let ms = check.requested.elapsed().as_secs_f64() * 1000.0;
    match server.load_state(check.handle.id()) {
        LoadState::Loaded => {
            check.done = true;
            match texts.get(&check.handle) {
                Some(text) if text.0 == check.expected => println!(
                    "[Check] asset: PASS, loaded by the Update of frame {frame}, {ms:.1} ms after the request; the text matches"
                ),
                Some(text) => println!(
                    "[Check] asset: FAIL, loaded but the text differs: {:?} instead of {:?}",
                    text.0, check.expected
                ),
                None => println!("[Check] asset: FAIL, the state is Loaded but the asset is missing"),
            }
        }
        LoadState::Failed(err) => {
            check.done = true;
            println!("[Check] asset: FAIL in frame {frame}, {ms:.1} ms after the request: {err}");
        }
        LoadState::NotLoaded | LoadState::Loading => {
            if check.frame >= ASSET_TIMEOUT_FRAMES {
                check.done = true;
                println!("[Check] asset: FAIL, still not loaded after {ASSET_TIMEOUT_FRAMES} frames");
            }
        }
    }
}

thread_local! {
    static CHECK_VALUE: Cell<u32> = const { Cell::new(1) };
}

fn thread_check(mut frame: Local<u32>) {
    let this_frame = *frame;
    *frame += 1;
    if this_frame != THREAD_CHECK_FRAME {
        return;
    }
    CHECK_VALUE.set(7);
    let started = Instant::now();
    let spawned = std::thread::Builder::new().name(THREAD_NAME.into()).spawn(|| {
        let first = CHECK_VALUE.get();
        CHECK_VALUE.set(2);
        let second = CHECK_VALUE.get();
        (first, second, std::thread::current().name().map(str::to_owned))
    });
    let handle = match spawned {
        Ok(handle) => handle,
        Err(err) => {
            println!("[Check] thread: FAIL in frame {this_frame}, spawn returned: {err}");
            return;
        }
    };
    let joined = handle.join();
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let main_value = CHECK_VALUE.get();
    match joined {
        Ok((1, 2, Some(name))) if name == THREAD_NAME && main_value == 7 => println!(
            "[Check] thread: PASS in frame {this_frame}, spawned and joined \"{name}\" in {ms:.2} ms; it read its own thread_local (1, then 2) while main's stayed 7"
        ),
        Ok((first, second, name)) => println!(
            "[Check] thread: FAIL in frame {this_frame}: the thread read {first} then {second} and was named {name:?}; main read {main_value} (expected 1, 2, \"{THREAD_NAME}\", 7)"
        ),
        Err(_) => println!("[Check] thread: FAIL in frame {this_frame}, the thread panicked"),
    }
}

const ROMFS_FILE: &str = "romfs:/hello_switch.txt";

fn romfs_check() {
    let expected = include_str!("../romfs/hello_switch.txt");
    match std::fs::read_to_string(ROMFS_FILE) {
        Ok(text) if text == expected => {
            println!("[Check] romfs: PASS, read {ROMFS_FILE} ({} bytes) and it matches", text.len())
        }
        Ok(text) => println!("[Check] romfs: FAIL, {ROMFS_FILE} reads {text:?}, not {expected:?}"),
        Err(err) => println!("[Check] romfs: FAIL, can't read {ROMFS_FILE}: {err}"),
    }
}

const UPLOAD_CHECK_FRAME: u32 = 90;
const UPLOAD_WRITES: u32 = 6000;
const UPLOAD_LAYERS: u32 = 64;
const UPLOAD_SIZE: u32 = 16;

/// The texel bytes write `index` puts in its layer.
fn upload_texel(index: u32, layer: u32) -> [u8; 4] {
    [index as u8, (index >> 8) as u8, layer as u8, 0xa5]
}

/// Thousands of `queue.write_texture` calls land in one submission (wgpu records them all in its
/// pending-writes command buffer), each with its own staging copy and barriers. A game's texture
/// atlas upload does the same. Then reads every layer back and checks it holds its last write.
fn upload_check(mut frame: Local<u32>, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    let this_frame = *frame;
    *frame += 1;
    if this_frame != UPLOAD_CHECK_FRAME {
        return;
    }
    let device = device.wgpu_device();
    let size = Extent3d { width: UPLOAD_SIZE, height: UPLOAD_SIZE, depth_or_array_layers: UPLOAD_LAYERS };
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("upload check"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::COPY_DST | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let started = Instant::now();
    let layer_bytes = (UPLOAD_SIZE * UPLOAD_SIZE * 4) as usize;
    for index in 0..UPLOAD_WRITES {
        let layer = index % UPLOAD_LAYERS;
        let data = upload_texel(index, layer).repeat(layer_bytes / 4);
        queue.write_texture(
            TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: Origin3d { x: 0, y: 0, z: layer }, aspect: TextureAspect::All },
            &data,
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(UPLOAD_SIZE * 4), rows_per_image: Some(UPLOAD_SIZE) },
            Extent3d { width: UPLOAD_SIZE, height: UPLOAD_SIZE, depth_or_array_layers: 1 },
        );
    }
    // Rows of a texture-to-buffer copy are 256-byte aligned.
    let row = 256u32;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("upload check readback"),
        size: u64::from(row * UPLOAD_SIZE * UPLOAD_LAYERS),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("upload check") });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(UPLOAD_SIZE) },
        },
        size,
    );
    // The pending writes go to the GPU in this submission, ahead of the copy.
    queue.submit([encoder.finish()]);
    readback.slice(..).map_async(MapMode::Read, |_| {});
    if let Err(err) = device.poll(PollType::wait_indefinitely()) {
        println!("[Check] upload: FAIL in frame {this_frame}, waiting for the GPU failed: {err}");
        return;
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let bytes = readback.slice(..).get_mapped_range();
    let mut wrong = 0;
    for layer in 0..UPLOAD_LAYERS {
        let last = layer + UPLOAD_LAYERS * ((UPLOAD_WRITES - 1 - layer) / UPLOAD_LAYERS);
        let expected = upload_texel(last, layer);
        for y in 0..UPLOAD_SIZE {
            let start = ((layer * UPLOAD_SIZE + y) * row) as usize;
            if bytes[start..start + (UPLOAD_SIZE * 4) as usize].chunks(4).any(|texel| texel != expected) {
                wrong += 1;
            }
        }
    }
    if wrong == 0 {
        println!(
            "[Check] upload: PASS in frame {this_frame}, {UPLOAD_WRITES} write_texture calls into {UPLOAD_LAYERS} layers went in one submission and read back right ({ms:.1} ms)"
        );
    } else {
        println!("[Check] upload: FAIL in frame {this_frame}, {wrong} of {} rows read back wrong ({ms:.1} ms)", UPLOAD_SIZE * UPLOAD_LAYERS);
    }
}

const DRAWS_CHECK_FRAME: u32 = 150;
/// Frames of the draws check, sent back to back.
const DRAWS_FRAMES: u32 = 20;
/// Single draws per frame, and draws in each multi-draw besides the few that test instances.
const DRAWS_PER_KIND: u32 = 5000;
/// The single draws go in submissions of this many, as a game's passes go in several.
const DRAWS_PER_SUBMIT: u32 = 250;
/// Iterations of the compute shader that starts each frame, so the GPU falls behind the CPU.
const DRAWS_SPIN: u32 = 10_000_000;
/// The target is DRAWS_SIZE texels square; the grids of draws are DRAWS_COLUMNS wide.
const DRAWS_SIZE: u32 = 128;
const DRAWS_COLUMNS: u32 = 125;

const DRAWS_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read_write> spin_data: array<u32>;

@compute @workgroup_size(1)
fn spin() {
    var x = spin_data[1];
    for (var i = 0u; i < spin_data[0]; i++) {
        x = x * 1664525u + 1013904223u;
    }
    spin_data[2] = x;
}

// Vertex v of a triangle list is a corner of a triangle in column v / 3 and row instance_index,
// placed 0.1 into its texel with legs 1.5 long: it covers that texel's center and no other.
@vertex
fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) row: u32) -> @builtin(position) vec4<f32> {
    var p = vec2<f32>(f32(v / 3u) + 0.1, f32(row) + 0.1);
    let corner = v % 3u;
    if corner == 1u {
        p.x += 1.5;
    }
    if corner == 2u {
        p.y += 1.5;
    }
    return vec4<f32>(p.x / 64.0 - 1.0, 1.0 - p.y / 64.0, 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0);
}
"#;

/// A frame's indirect records and the texels they cover.
struct DrawRecords {
    /// {vertex count, instance count, first vertex, first instance}, one `draw_indirect` each.
    single: Vec<[u32; 4]>,
    /// The same, for one `multi_draw_indirect`.
    multi: Vec<[u32; 4]>,
    /// {index count, instance count, first index, base vertex, first instance}, for one
    /// `multi_draw_indexed_indirect` with an index buffer that holds 0, 1, 2, ...
    indexed: Vec<[u32; 5]>,
    covered: Vec<bool>,
}

impl DrawRecords {
    fn new() -> Self {
        let mut records = DrawRecords {
            single: Vec::new(),
            multi: Vec::new(),
            indexed: Vec::new(),
            covered: vec![false; (DRAWS_SIZE * DRAWS_SIZE) as usize],
        };
        // Each multi-draw starts with draws of no instances (rows 120 and 121 stay clear) between
        // draws of several, which must each start again at their first instance.
        for column in 0..DRAWS_SIZE {
            let (instances, row) = if column % 2 == 0 { (0, 120) } else { (2, 125) };
            records.multi.push([3, instances, 3 * column, row]);
            records.cover(column, row, instances);
            let (instances, row) = if column % 2 == 0 { (0, 121) } else { (3, 122) };
            records.indexed.push(Self::indexed(column, instances, row));
            records.cover(column, row, instances);
        }
        // Then a grid of draws of one instance each, rows 0-39, 40-79 and 80-119.
        for i in 0..DRAWS_PER_KIND {
            let (column, row) = (i % DRAWS_COLUMNS, i / DRAWS_COLUMNS);
            records.single.push([3, 1, 3 * column, row]);
            records.multi.push([3, 1, 3 * column, row + 40]);
            records.indexed.push(Self::indexed(column, 1, row + 80));
            records.cover(column, row, 1);
            records.cover(column, row + 40, 1);
            records.cover(column, row + 80, 1);
        }
        records
    }

    /// An indexed draw of a column: half its first vertex comes from the first index, the rest
    /// from the base vertex.
    fn indexed(column: u32, instances: u32, row: u32) -> [u32; 5] {
        [3, instances, 3 * (column / 2), 3 * (column - column / 2), row]
    }

    fn cover(&mut self, column: u32, row: u32, instances: u32) {
        for row in row..row + instances {
            self.covered[(row * DRAWS_SIZE + column) as usize] = true;
        }
    }
}

fn draws_attachment(view: &WgpuTextureView, clear: bool) -> [Option<RenderPassColorAttachment<'_>>; 1] {
    let load = if clear { LoadOp::Clear(Default::default()) } else { LoadOp::Load };
    [Some(RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: Operations { load, store: StoreOp::Store },
    })]
}

/// Frames of thousands of indirect draws and multi-draws of thousands, sent faster than the GPU
/// runs them, as a game with VSync off sends its frames. Every draw covers one texel of a target;
/// after the last frame the target is read back and compared with the texels the draws cover.
fn draws_check(mut frame: Local<u32>, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    let this_frame = *frame;
    *frame += 1;
    if this_frame != DRAWS_CHECK_FRAME {
        return;
    }
    let device = device.wgpu_device();
    let records = DrawRecords::new();
    let buffer = |label: &str, usage: BufferUsages, words: &[u32]| {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: (words.len() * 4) as u64,
            usage: usage | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        queue.write_buffer(&buffer, 0, &bytes);
        buffer
    };
    let spin_data = buffer("draws check spin", BufferUsages::STORAGE, &[DRAWS_SPIN, 1, 0, 0]);
    let single = buffer("draws check single", BufferUsages::INDIRECT, records.single.as_flattened());
    let multi = buffer("draws check multi", BufferUsages::INDIRECT, records.multi.as_flattened());
    let indexed = buffer("draws check indexed", BufferUsages::INDIRECT, records.indexed.as_flattened());
    let indices: Vec<u32> = (0..3 * DRAWS_SIZE).collect();
    let index = buffer("draws check indices", BufferUsages::INDEX, &indices);

    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("draws check"),
        source: ShaderSource::Wgsl(DRAWS_SHADER.into()),
    });
    let spin = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: Some("draws check spin"),
        layout: None,
        module: &module,
        entry_point: Some("spin"),
        compilation_options: Default::default(),
        cache: None,
    });
    let spin_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("draws check spin"),
        layout: &spin.get_bind_group_layout(0),
        entries: &[BindGroupEntry { binding: 0, resource: spin_data.as_entire_binding() }],
    });
    let format = TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&RawRenderPipelineDescriptor {
        label: Some("draws check"),
        layout: None,
        vertex: RawVertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(RawFragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let size = Extent3d { width: DRAWS_SIZE, height: DRAWS_SIZE, depth_or_array_layers: 1 };
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("draws check"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let row = DRAWS_SIZE * 4;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("draws check readback"),
        size: u64::from(row * DRAWS_SIZE),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let started = Instant::now();
    let mut submissions = 0;
    for _ in 0..DRAWS_FRAMES {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("draws check spin") });
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&spin);
            pass.set_bind_group(0, &spin_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        queue.submit([encoder.finish()]);
        submissions += 1;
        for first in (0..DRAWS_PER_KIND).step_by(DRAWS_PER_SUBMIT as usize) {
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("draws check single") });
            {
                let attachments = draws_attachment(&view, first == 0);
                let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: None,
                    color_attachments: &attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                for i in first..(first + DRAWS_PER_SUBMIT).min(DRAWS_PER_KIND) {
                    pass.draw_indirect(&single, u64::from(i) * 16);
                }
            }
            queue.submit([encoder.finish()]);
            submissions += 1;
        }
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("draws check multi") });
        {
            let attachments = draws_attachment(&view, false);
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.multi_draw_indirect(&multi, 0, records.multi.len() as u32);
            pass.set_index_buffer(index.slice(..), IndexFormat::Uint32);
            pass.multi_draw_indexed_indirect(&indexed, 0, records.indexed.len() as u32);
        }
        queue.submit([encoder.finish()]);
        submissions += 1;
    }
    let submitted_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("draws check readback") });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(DRAWS_SIZE) },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    readback.slice(..).map_async(MapMode::Read, |_| {});
    if let Err(err) = device.poll(PollType::wait_indefinitely()) {
        println!("[Check] draws: FAIL in frame {this_frame}, waiting for the GPU failed: {err}");
        return;
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let bytes = readback.slice(..).get_mapped_range();
    let wrong: Vec<usize> = (0..records.covered.len())
        .filter(|&texel| (bytes[texel * 4] == 0xff) != records.covered[texel] || ![0, 0xff].contains(&bytes[texel * 4]))
        .collect();
    let what = format!(
        "{DRAWS_FRAMES} frames, each a busy compute pass, {DRAWS_PER_KIND} draw_indirect calls in {} submissions, a multi_draw_indirect of {} and a multi_draw_indexed_indirect of {}",
        DRAWS_PER_KIND.div_ceil(DRAWS_PER_SUBMIT),
        records.multi.len(),
        records.indexed.len()
    );
    if wrong.is_empty() {
        println!(
            "[Check] draws: PASS in frame {this_frame}, {what}; {submissions} submissions took {submitted_ms:.1} ms, the GPU was done {ms:.1} ms after the first, and the target read back right"
        );
    } else {
        let first: Vec<String> = wrong
            .iter()
            .take(8)
            .map(|&texel| format!("({}, {})", texel % DRAWS_SIZE as usize, texel / DRAWS_SIZE as usize))
            .collect();
        println!(
            "[Check] draws: FAIL in frame {this_frame}, {what}; {} of {} texels read back wrong, first (column, row) {} ({ms:.1} ms)",
            wrong.len(),
            records.covered.len(),
            first.join(" ")
        );
    }
}
