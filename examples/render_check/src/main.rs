//! Render check: which of Bevy's 3D render paths draw on this GPU stack.
//!
//! Six tiles, each a `Camera3d` viewport with one rendering setting changed, look at the same four
//! cubes, which differ in one thing each: lit (cyan), unlit (magenta), unlit and alpha-blended
//! (yellow), and unlit with a pipeline whose depth test always passes (green). At frame
//! `CHECK_FRAME` the app screenshots itself, checks whether each cube's pixels differ from its
//! tile's background, prints one `[Check]` line per tile, shows the result in the tile and saves
//! the screenshot as `render_check_N.png` beside `log.txt`. (A) checks again; hold (−) and press
//! (+) for the FPS overlay; (+) exits.
//!
//! An `options.txt` beside the NRO containing `cpu_preprocessing` turns off Bevy's GPU mesh
//! preprocessing (its compute shaders) for the whole run. wgpu's indirect-call validation, the
//! compute pass that checks each indirect draw's arguments, is off on Horizon in release builds
//! (bevy-horizon's wgpu-core patch); `indirect_validation` turns it on, and
//! `no_indirect_validation` turns it off in a debug build too. `panic` makes the app panic at
//! frame 120, to test how bevy_horizon exits while the GPU driver is still running.
//! At frame `READBACK_FRAME`, before the first screenshot, it copies a small texture to a buffer
//! and reads it back (`[Check] readback test` lines), so that if the GPU stops at the screenshot,
//! the log tells a failing copy from a failing screenshot. `no_screenshot` skips both, to tell
//! whether reading back is what stops the GPU; (A) still takes a screenshot.
//!
//! At startup it checks that NVK's error messages reach log.txt (`messenger_self_test`), and it
//! logs the first frames one by one, so a log cut short shows how far rendering got.
//!
//! The viewport tiles with per-camera UI follow Bevy's `split_screen` example, and the depth-test
//! material follows its `extended_material` and `clustered_decals` examples
//! (<https://github.com/bevyengine/bevy>, MIT OR Apache-2.0).

// AsBindGroup's derive makes rustc prove `Send` through wgpu's `ContextWgpuCore`, deeper than the
// default limit of 128 (rust-lang/rust#159228, a future-incompatibility warning).
#![recursion_limit = "256"]

use bevy::camera::{ClearColorConfig, Hdr, MsaaWriteback, ScalingMode, Viewport};
use bevy::color::Srgba;
use bevy::ecs::message::MessageWriter;
use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline, PbrPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice, RenderQueue};
use bevy::render::RenderPlugin;
use bevy::render::settings::{InstanceFlags, WgpuFeatures, WgpuSettings};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::render::view::{Msaa, NoIndirectDrawing};
use bevy::text::FontSize;

#[cfg(target_os = "horizon")]
const APP_DIR: &str = "sdmc:/switch/render_check";
#[cfg(not(target_os = "horizon"))]
const APP_DIR: &str = ".";

/// The frame of the first check, late enough that every pipeline has been built.
const CHECK_FRAME: u32 = 240;
/// The frame of the readback test, which copies a small texture to a buffer and maps the buffer:
/// the two GPU steps a screenshot ends with, without Bevy's offscreen window texture and its blit.
const READBACK_FRAME: u32 = 200;
/// The readback test's texture is this many pixels square.
const READBACK_SIDE: u32 = 64;
/// Half the side of the square of pixels checked around each cube's centre.
const PROBE_HALF: i32 = 12;
/// How far, in any sRGB channel, a pixel must be from the background to count as drawn.
const DRAWN_THRESHOLD: f32 = 0.08;

struct TileSpec {
    label: &'static str,
    indirect: bool,
    msaa_off: bool,
    hdr: bool,
    orthographic: bool,
    background: Color,
}

const TILES: [TileSpec; 6] = [
    TileSpec {
        label: "T1 default: indirect draws, MSAA x4",
        indirect: true,
        msaa_off: false,
        hdr: false,
        orthographic: false,
        background: Color::srgb(0.16, 0.05, 0.05),
    },
    TileSpec {
        label: "T2 NoIndirectDrawing, MSAA x4",
        indirect: false,
        msaa_off: false,
        hdr: false,
        orthographic: false,
        background: Color::srgb(0.05, 0.16, 0.05),
    },
    TileSpec {
        label: "T3 indirect draws, MSAA off",
        indirect: true,
        msaa_off: true,
        hdr: false,
        orthographic: false,
        background: Color::srgb(0.05, 0.05, 0.18),
    },
    TileSpec {
        label: "T4 NoIndirectDrawing, MSAA off",
        indirect: false,
        msaa_off: true,
        hdr: false,
        orthographic: false,
        background: Color::srgb(0.15, 0.05, 0.15),
    },
    TileSpec {
        label: "T5 Hdr (tonemapping pass)",
        indirect: true,
        msaa_off: false,
        hdr: true,
        orthographic: false,
        background: Color::srgb(0.05, 0.15, 0.15),
    },
    TileSpec {
        label: "T6 orthographic projection",
        indirect: true,
        msaa_off: false,
        hdr: false,
        orthographic: true,
        background: Color::srgb(0.15, 0.15, 0.05),
    },
];

/// The cubes, left to right: name and x position.
const CUBES: [(&str, f32); 4] = [("lit", -3.0), ("unlit", -1.0), ("blend", 1.0), ("no-depth", 3.0)];

#[derive(Component)]
struct Tile(usize);

#[derive(Component)]
struct TileStatus(usize);

#[derive(Component)]
struct Probe(usize);

#[derive(Resource, Default)]
struct CheckState {
    frame: u32,
    checks: u32,
    cpu_preprocessing: bool,
    /// From `options.txt`: panic at this frame, to test the exit after a panic.
    panic_at: Option<u32>,
    /// From `options.txt`: skip the readback test and the screenshot, so the tiles keep drawing
    /// without either.
    no_screenshot: bool,
}

/// The frame `options.txt`'s `panic` stops the app at.
const PANIC_FRAME: u32 = 120;
/// Leaves `StandardMaterial` as it is, except that the pipeline's depth test always passes.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
struct NoDepthTest {}

impl MaterialExtension for NoDepthTest {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(depth_stencil) = descriptor.depth_stencil.as_mut() {
            depth_stencil.depth_compare = Some(CompareFunction::Always);
        }
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "horizon")]
    bevy_horizon::init_switch_logging("render_check");

    let options = std::fs::read_to_string(format!("{APP_DIR}/options.txt")).unwrap_or_default();
    let option = |name: &str| options.split_whitespace().any(|word| word == name);
    let cpu_preprocessing = option("cpu_preprocessing");
    println!("[Init] render_check starting; options.txt: {:?}", options.trim());

    // bevy-horizon's wgpu-core patch drops `VALIDATION_INDIRECT_CALL` on Horizon unless
    // `VALIDATION` is also set (Bevy's release rule for a platform without DX12). So to turn the
    // validation on, ask for both; `VALIDATION` alone only looks for the Khronos layer, which the
    // Switch doesn't have.
    let mut wgpu_settings = WgpuSettings::default();
    let flags = &mut wgpu_settings.instance_flags;
    if option("indirect_validation") {
        flags.insert(InstanceFlags::VALIDATION | InstanceFlags::VALIDATION_INDIRECT_CALL);
    }
    if option("no_indirect_validation") {
        flags.remove(InstanceFlags::VALIDATION_INDIRECT_CALL);
    }
    let validation_on = flags.contains(InstanceFlags::VALIDATION_INDIRECT_CALL)
        && (cfg!(not(target_os = "horizon")) || flags.contains(InstanceFlags::VALIDATION));
    println!(
        "[Init] wgpu indirect-call validation: {}",
        if validation_on { "on" } else { "off" }
    );

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .set(bevy::log::LogPlugin {
                filter: "wgpu=warn,naga=warn".into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bevy Horizon - Render Check".into(),
                    resolution: (1280u32, 720u32).into(),
                    ..default()
                }),
                ..default()
            })
            .set(PbrPlugin {
                use_gpu_instance_buffer_builder: !cpu_preprocessing,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: wgpu_settings.into(),
                ..default()
            }),
    );
    app.add_plugins(MaterialPlugin::<ExtendedMaterial<StandardMaterial, NoDepthTest>>::default());
    app.add_plugins(bevy_horizon::HorizonFpsOverlayPlugin::default());

    #[cfg(target_os = "horizon")]
    {
        use bevy::ecs::schedule::SingleThreadedExecutor;
        app.add_plugins(bevy_horizon::HorizonPlugin);
        // As in hello_switch: every primary schedule runs single-threaded on Horizon.
        macro_rules! single_threaded {
            ($($label:expr),*) => {$(
                app.edit_schedule($label, |s| {
                    s.set_executor(SingleThreadedExecutor::new());
                });
            )*};
        }
        single_threaded!(Main, Startup, First, PreUpdate, Update, PostUpdate, Last);
    }

    app.insert_resource(CheckState {
        cpu_preprocessing,
        panic_at: option("panic").then_some(PANIC_FRAME),
        no_screenshot: option("no_screenshot"),
        ..default()
    })
    .add_systems(Startup, (setup, log_renderer))
    .add_systems(Update, (run_checks, handle_input));

    #[cfg(target_os = "horizon")]
    bevy_horizon::flush_switch_logs();
    let exit = app.run();
    println!("[Exit] app.run() returned {exit:?}");
    #[cfg(target_os = "horizon")]
    bevy_horizon::flush_switch_logs();
}

fn setup(
    mut commands: Commands,
    windows: Query<&Window>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut no_depth_materials: ResMut<Assets<ExtendedMaterial<StandardMaterial, NoDepthTest>>>,
) {
    let cube = meshes.add(Cuboid::new(1.2, 1.2, 1.2));
    let rotation = Quat::from_euler(EulerRot::YXZ, 0.6, 0.45, 0.0);
    let at = |index: usize| Transform::from_xyz(CUBES[index].1, 0.0, 0.0).with_rotation(rotation);

    commands.spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 0.75, 0.95),
            perceptual_roughness: 0.4,
            ..default()
        })),
        at(0),
        Probe(0),
    ));
    commands.spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.2, 0.7),
            unlit: true,
            ..default()
        })),
        at(1),
        Probe(1),
    ));
    commands.spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.85, 0.1, 0.7),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })),
        at(2),
        Probe(2),
    ));
    commands.spawn((
        Mesh3d(cube),
        MeshMaterial3d(no_depth_materials.add(ExtendedMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(0.2, 0.9, 0.3),
                unlit: true,
                ..default()
            },
            extension: NoDepthTest {},
        })),
        at(3),
        Probe(3),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 3500.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let screen = windows
        .iter()
        .next()
        .map(|window| window.physical_size())
        .unwrap_or(UVec2::new(1280, 720));
    let tile_size = UVec2::new(screen.x / 3, screen.y / 2);
    println!("[Scene] screen {}x{}, tiles {}x{}", screen.x, screen.y, tile_size.x, tile_size.y);

    for (index, spec) in TILES.iter().enumerate() {
        let position = UVec2::new(index as u32 % 3, index as u32 / 3) * tile_size;
        let mut camera = commands.spawn((
            Camera3d::default(),
            Camera {
                order: index as isize,
                viewport: Some(Viewport {
                    physical_position: position,
                    physical_size: tile_size,
                    ..default()
                }),
                clear_color: ClearColorConfig::Custom(spec.background),
                msaa_writeback: MsaaWriteback::Off,
                ..default()
            },
            if spec.msaa_off { Msaa::Off } else { Msaa::Sample4 },
            Transform::from_xyz(0.0, 2.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
            Tile(index),
        ));
        // Bevy reads NoIndirectDrawing when the camera is first extracted, so it goes on at spawn.
        if !spec.indirect {
            camera.insert(NoIndirectDrawing);
        }
        if spec.hdr {
            camera.insert(Hdr);
        }
        if spec.orthographic {
            camera.insert(Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical {
                    viewport_height: 7.0,
                },
                ..OrthographicProjection::default_3d()
            }));
        }
        let camera = camera.id();

        commands.spawn((
            UiTargetCamera(camera),
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(8)),
                row_gap: px(4),
                ..default()
            },
            children![
                (
                    Text::new(spec.label),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                ),
                (
                    Text::new(format!("checks at frame {CHECK_FRAME}")),
                    TextFont {
                        font_size: FontSize::Px(14.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.85, 0.85, 0.85)),
                    TileStatus(index),
                ),
                (
                    Text::new("cyan lit | magenta unlit | yellow blend | green no depth test"),
                    TextFont {
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.7, 0.7, 0.7)),
                    Node {
                        position_type: PositionType::Absolute,
                        bottom: px(8),
                        left: px(8),
                        ..default()
                    },
                ),
            ],
        ));
    }
    println!("[Scene] 4 cubes and {} tiles spawned", TILES.len());
}

/// Logs what Bevy's GPU preprocessing decision depends on.
fn log_renderer(
    device: Option<Res<RenderDevice>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    state: Res<CheckState>,
) {
    if let Some(adapter) = adapter {
        println!(
            "[Render] adapter {} ({:?}), driver {} {}",
            adapter.name, adapter.backend, adapter.driver, adapter.driver_info
        );
    }
    match device {
        Some(ref device) => {
            let features = device.features();
            let limits = device.limits();
            println!(
                "[Render] features: INDIRECT_FIRST_INSTANCE {}, IMMEDIATES {}, MULTI_DRAW_INDIRECT_COUNT {}",
                features.contains(WgpuFeatures::INDIRECT_FIRST_INSTANCE),
                features.contains(WgpuFeatures::IMMEDIATES),
                features.contains(WgpuFeatures::MULTI_DRAW_INDIRECT_COUNT),
            );
            println!(
                "[Render] limits: {} storage textures and {} storage buffers per stage, {} B compute workgroup storage",
                limits.max_storage_textures_per_shader_stage,
                limits.max_storage_buffers_per_shader_stage,
                limits.max_compute_workgroup_storage_size,
            );
        }
        None => println!("[Render] no RenderDevice in the main world"),
    }
    println!(
        "[Render] mesh preprocessing: {}",
        if state.cpu_preprocessing {
            "CPU (options.txt says cpu_preprocessing)"
        } else {
            "GPU compute (Bevy's default)"
        }
    );
    #[cfg(target_os = "horizon")]
    if let Some(device) = device {
        messenger_self_test(&device);
    }
}

/// Shows that NVK's own error messages reach log.txt, so that a run whose log has none before a
/// lost device means NVK reported none. bevy-horizon's wgpu-hal patch registers a
/// VK_EXT_debug_utils messenger on Horizon, and NVK reports why it returns an error through it
/// (`vk_errorf`). Asking about an external memory handle type NVK doesn't know, a D3D11 texture,
/// gets one such error (mesa `src/nouveau/vulkan/nvk_image.c`,
/// `nvk_GetPhysicalDeviceImageFormatProperties2`) and changes nothing.
#[cfg(target_os = "horizon")]
fn messenger_self_test(device: &RenderDevice) {
    use ash::vk;

    // SAFETY: only reads the instance and physical device handles, and the guard is dropped here.
    let Some(hal) = (unsafe { device.wgpu_device().as_hal::<wgpu::hal::api::Vulkan>() }) else {
        println!("[Check] debug messenger self-test: skipped, the device isn't Vulkan");
        return;
    };
    let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE);
    let info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(vk::Format::R8G8B8A8_UNORM)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .push_next(&mut external);
    let mut properties = vk::ImageFormatProperties2::default();
    println!(
        "[Check] debug messenger self-test: asking NVK about a D3D11 texture handle, which it doesn't support; the WARN from wgpu_hal just below, naming nvk_image.c, is expected"
    );
    let result = unsafe {
        hal.shared_instance()
            .raw_instance()
            .get_physical_device_image_format_properties2(
                hal.raw_physical_device(),
                &info,
                &mut properties,
            )
    };
    println!(
        "[Check] debug messenger self-test: NVK returned {}",
        match result {
            Ok(()) => "success, so it didn't reject the handle type and sent no message".to_string(),
            Err(error) => format!("{error:?}"),
        }
    );
}

/// Frames whose start is logged and flushed to log.txt, so that a log cut short by a lost GPU
/// device or a hang still shows how far rendering got. 201 and 202 follow the readback test at
/// `READBACK_FRAME`, 241 to 243 the screenshot taken at `CHECK_FRAME`; the later ones place a loss
/// that comes without either.
const LOGGED_FRAMES: [u32; 21] = [
    1, 2, 3, 4, 5, 10, 20, 30, 60, 120, 180, 200, 201, 202, 240, 241, 242, 243, 300, 480, 600,
];

/// Copies a `READBACK_SIDE`-pixel square texture with known bytes to a buffer on the GPU, maps
/// the buffer and compares, and says what happened.
fn readback_test(device: &RenderDevice, queue: &RenderQueue) -> String {
    use bevy::render::render_resource::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, MapMode, PollType,
        TexelCopyBufferInfo, TexelCopyBufferLayout, TextureDescriptor, TextureDimension,
        TextureFormat, TextureUsages,
    };

    // 64 RGBA8 pixels make a 256-byte row, wgpu's alignment for a copy's rows.
    let row = READBACK_SIDE * 4;
    let size = Extent3d {
        width: READBACK_SIDE,
        height: READBACK_SIDE,
        depth_or_array_layers: 1,
    };
    let layout = TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(row),
        rows_per_image: Some(READBACK_SIDE),
    };
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("render_check readback"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::COPY_SRC | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let written: Vec<u8> = (0..row * READBACK_SIDE).map(|i| (i % 251) as u8).collect();
    queue.write_texture(texture.as_image_copy(), &written, layout, size);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("render_check readback"),
        size: u64::from(row * READBACK_SIDE),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("render_check readback"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &buffer,
            layout,
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    if let Err(error) = device.poll(PollType::Wait {
        submission_index: None,
        timeout: Some(std::time::Duration::from_secs(5)),
    }) {
        return format!("waiting for the GPU failed: {error}");
    }
    match receiver.try_recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return format!("mapping the buffer failed: {error}"),
        Err(_) => return "the buffer still wasn't mapped after the GPU finished".into(),
    }
    let differing = slice
        .get_mapped_range()
        .iter()
        .zip(&written)
        .filter(|(read, wrote)| read != wrote)
        .count();
    buffer.unmap();
    if differing == 0 {
        format!("all {} bytes came back as written", written.len())
    } else {
        format!(
            "{differing} of {} bytes came back different from what was written",
            written.len()
        )
    }
}

fn run_checks(
    mut commands: Commands,
    mut state: ResMut<CheckState>,
    mut started: Local<Option<std::time::Instant>>,
    gamepads: Query<&Gamepad>,
    mut statuses: Query<&mut Text, With<TileStatus>>,
    device: Option<Res<RenderDevice>>,
    queue: Option<Res<RenderQueue>>,
) {
    state.frame += 1;
    let started = *started.get_or_insert_with(std::time::Instant::now);
    if LOGGED_FRAMES.contains(&state.frame) {
        println!(
            "[Engine] frame {} updated, {:.0} ms after frame 1",
            state.frame,
            started.elapsed().as_secs_f64() * 1000.0
        );
        #[cfg(target_os = "horizon")]
        bevy_horizon::flush_switch_logs();
    }
    if state.panic_at == Some(state.frame) {
        panic!("options.txt says panic: stopping at frame {PANIC_FRAME} to test the exit after a panic, with the GPU driver still running");
    }
    if state.frame == READBACK_FRAME && !state.no_screenshot {
        if let (Some(device), Some(queue)) = (&device, &queue) {
            println!(
                "[Check] readback test at frame {READBACK_FRAME}: copying a {READBACK_SIDE}x{READBACK_SIDE} texture to a buffer on the GPU and mapping the buffer"
            );
            #[cfg(target_os = "horizon")]
            bevy_horizon::flush_switch_logs();
            let outcome = readback_test(device, queue);
            println!("[Check] readback test: {outcome}");
        } else {
            println!("[Check] readback test: skipped, no RenderDevice or RenderQueue in the main world");
        }
        #[cfg(target_os = "horizon")]
        bevy_horizon::flush_switch_logs();
    }
    let again = state.frame > CHECK_FRAME
        && gamepads
            .iter()
            .any(|gamepad| gamepad.just_pressed(GamepadButton::East));
    if state.frame == CHECK_FRAME && state.no_screenshot {
        println!(
            "[Check] frame {CHECK_FRAME}: no screenshot, as options.txt says no_screenshot; the tiles keep drawing (the heartbeat lines show for how long), and (A) takes one"
        );
        for mut text in &mut statuses {
            text.0 = "no screenshot yet: (A) takes one".into();
        }
        #[cfg(target_os = "horizon")]
        bevy_horizon::flush_switch_logs();
        return;
    }
    if state.frame != CHECK_FRAME && !again {
        return;
    }
    state.checks += 1;
    println!("[Check] screenshot {} requested at frame {}", state.checks, state.frame);
    for mut text in &mut statuses {
        text.0 = format!("checking (screenshot {})", state.checks);
    }
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(format!("{APP_DIR}/render_check_{}.png", state.checks)))
        .observe(analyze_screenshot);
}

fn analyze_screenshot(
    captured: On<ScreenshotCaptured>,
    state: Res<CheckState>,
    tiles: Query<(&Tile, &Camera, &GlobalTransform)>,
    probes: Query<(&Probe, &GlobalTransform)>,
    mut statuses: Query<(&TileStatus, &mut Text)>,
) {
    let image = &captured.image;
    let (width, height) = (image.width() as i32, image.height() as i32);
    println!(
        "[Check] screenshot {}: {}x{}, {:?}",
        state.checks, width, height, image.texture_descriptor.format
    );
    let pixel = |x: i32, y: i32| -> Option<Srgba> {
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        image.get_color_at(x as u32, y as u32).ok().map(|color| color.to_srgba())
    };

    let mut tiles: Vec<_> = tiles.iter().collect();
    tiles.sort_by_key(|(tile, _, _)| tile.0);
    let mut probes: Vec<_> = probes.iter().collect();
    probes.sort_by_key(|(probe, _)| probe.0);
    let (mut drawn_total, mut checked_total) = (0, 0);

    for (tile, camera, camera_transform) in tiles {
        let label = TILES[tile.0].label;
        let (Some(physical), Some(logical)) =
            (camera.physical_viewport_rect(), camera.logical_viewport_rect())
        else {
            println!("[Check] {label}: no viewport");
            continue;
        };
        let scale = physical.size().as_vec2() / logical.size();
        // The tile's left edge, halfway down, is clear of the cubes and the text.
        let Some(background) =
            pixel(physical.min.x as i32 + 4, (physical.min.y + physical.height() / 2) as i32)
        else {
            println!("[Check] {label}: viewport outside the screenshot");
            continue;
        };

        let mut results = Vec::new();
        let mut short = Vec::new();
        for (probe, probe_transform) in &probes {
            let name = CUBES[probe.0].0;
            checked_total += 1;
            let Ok(at) = camera.world_to_viewport(camera_transform, probe_transform.translation())
            else {
                results.push(format!("{name} OFF-SCREEN"));
                short.push(format!("{name} off-screen"));
                continue;
            };
            let centre = ((at - logical.min) * scale + physical.min.as_vec2())
                .round()
                .as_ivec2();
            let (mut differ, mut total) = (0, 0);
            for dy in -PROBE_HALF..=PROBE_HALF {
                for dx in -PROBE_HALF..=PROBE_HALF {
                    if let Some(p) = pixel(centre.x + dx, centre.y + dy) {
                        total += 1;
                        let distance = (p.red - background.red)
                            .abs()
                            .max((p.green - background.green).abs())
                            .max((p.blue - background.blue).abs());
                        if distance > DRAWN_THRESHOLD {
                            differ += 1;
                        }
                    }
                }
            }
            let drawn = total > 0 && differ * 2 >= total;
            if drawn {
                drawn_total += 1;
            }
            results.push(format!(
                "{name} {} ({differ}/{total} px at {},{})",
                if drawn { "DRAWN" } else { "MISSING" },
                centre.x,
                centre.y
            ));
            short.push(format!("{name} {}", if drawn { "ok" } else { "MISSING" }));
        }
        println!(
            "[Check] {label}: {} | background {}",
            results.join(", "),
            background.to_hex()
        );
        for (status, mut text) in &mut statuses {
            if status.0 == tile.0 {
                text.0 = short.join(", ");
            }
        }
    }
    println!(
        "[Check] screenshot {}: {drawn_total}/{checked_total} cubes drawn, mesh preprocessing on the {}",
        state.checks,
        if state.cpu_preprocessing { "CPU" } else { "GPU" }
    );
    #[cfg(target_os = "horizon")]
    bevy_horizon::flush_switch_logs();
}

fn handle_input(gamepads: Query<&Gamepad>, mut exit: MessageWriter<AppExit>) {
    for gamepad in &gamepads {
        // With (−) held, (+) belongs to the FPS overlay's chord.
        if gamepad.just_pressed(GamepadButton::Start) && !bevy_horizon::fps_chord_held(&gamepads) {
            println!("[Input] (+) pressed: exiting");
            exit.write(AppExit::Success);
        }
    }
}
