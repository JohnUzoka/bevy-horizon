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

use std::cell::Cell;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::asset::io::Reader;
use bevy::asset::io::file::FileAssetReader;
use bevy::asset::{AssetLoader, LoadContext, LoadState};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, MapMode, Origin3d, PollType, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages,
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
            render_app.add_systems(Render, upload_check);
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
