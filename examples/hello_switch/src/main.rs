use bevy::ecs::message::{MessageReader, MessageWriter};
use bevy::input::gamepad::{
    Gamepad, GamepadAxis, GamepadButton, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent,
};
use bevy::prelude::*;

mod checks;

#[derive(Component)]
struct RotatingCube {
    speed_x: f32,
    speed_y: f32,
    color_idx: usize,
}

#[derive(Component)]
struct CubeMaterialHandle(Handle<StandardMaterial>);

fn main() {
    #[cfg(target_os = "horizon")]
    bevy_horizon::init_switch_logging("hello_switch");

    println!("============================================================");
    println!("Starting Bevy Horizon Switch application (Release)...");
    println!("============================================================");

    let mut app = App::new();
    println!("[Init] Step 1: Bevy App instance created");

    // Don't `.disable::<PipelinedRenderingPlugin>()` here: DefaultPlugins only contains it with
    // Bevy's `multi_threaded` feature, which this app doesn't enable, and `disable` panics
    // ("Cannot disable a plugin that does not exist.") for a plugin that isn't in the group.
    let plugins = DefaultPlugins
        .build()
        // Bevy's default filter is "wgpu=error,naga=warn"; wgpu's warnings say why an adapter
        // or surface was refused, which is what matters while the Switch's renderer is new.
        .set(bevy::log::LogPlugin {
            filter: "wgpu=warn,naga=warn".into(),
            ..default()
        })
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy Horizon - Switch Demo".into(),
                resolution: (1280u32, 720u32).into(),
                ..default()
            }),
            ..default()
        });
    // RenderPlugin keeps its defaults: on Horizon, wgpu's only backend is Vulkan
    // (patches/wgpu-hal), drawing through NVK, which bevy_horizon's `vulkan` feature links.
    app.add_plugins(plugins);
    println!("[Init] Step 2: DefaultPlugins added");

    // Bevy's FPS overlay, hidden until (−) is held and (+) pressed.
    app.add_plugins(bevy_horizon::HorizonFpsOverlayPlugin::default());

    #[cfg(target_os = "horizon")]
    {
        app.add_plugins(bevy_horizon::HorizonPlugin);
        println!("[Init] Step 3: HorizonPlugin added");

        // Horizon OS kernel scheduler requires single-threaded execution on all schedules
        app.edit_schedule(Main, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(Startup, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(First, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(PreUpdate, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(Update, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(PostUpdate, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(Last, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        println!("[Init] Step 4: SingleThreadedExecutor configured on all primary schedules");
    }

    app.add_systems(Startup, setup_scene)
        .add_systems(Update, (rotate_cube, handle_input))
        .add_plugins(checks::ChecksPlugin);
    println!("[Init] Step 5: Startup and Update systems registered, plus the [Check] asset and thread checks");

    println!("[Init] Step 6: Calling app.run()...");
    #[cfg(target_os = "horizon")]
    bevy_horizon::flush_switch_logs();

    let exit = app.run();
    println!("[Exit] app.run() returned {exit:?}");
    #[cfg(target_os = "horizon")]
    bevy_horizon::flush_switch_logs();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    println!("[Scene] setup_scene starting...");
    // 3D Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.5, 3.5).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Directional Sunlight
    commands.spawn((
        DirectionalLight {
            illuminance: 3500.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Rotating 3D Cube
    let initial_color = Color::srgb(0.2, 0.75, 0.95);
    let mat_handle = materials.add(StandardMaterial {
        base_color: initial_color,
        metallic: 0.1,
        perceptual_roughness: 0.35,
        ..default()
    });

    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.2, 1.2, 1.2))),
        MeshMaterial3d(mat_handle.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
        RotatingCube {
            speed_x: 0.5,
            speed_y: 0.8,
            color_idx: 0,
        },
        CubeMaterialHandle(mat_handle),
    ));

    // UI Overlay with Instructions
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(24.0),
                left: Val::Px(24.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(16.0)),
                row_gap: Val::Px(8.0),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.05, 0.08, 0.8)),
        ))
        .with_children(|ui| {
            ui.spawn((
                Text::new("Nintendo Switch | Bevy Horizon Demo"),
                TextColor(Color::srgb(1.0, 0.85, 0.2)),
            ));
            ui.spawn((
                Text::new("L-Stick / Arrows: Adjust Rotation\n(A) Button / Space: Cycle Color\n(+) Button / Esc: Exit\nHold (-), press (+): FPS overlay"),
                TextColor(Color::srgb(0.85, 0.85, 0.85)),
            ));
        });
    println!("[Scene] setup_scene completed successfully!");
}

fn rotate_cube(
    time: Res<Time>,
    mut query: Query<(&RotatingCube, &mut Transform)>,
) {
    static FRAME_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let count = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if count == 0 {
        println!("[Engine] First frame's Update ran");
        #[cfg(target_os = "horizon")]
        bevy_horizon::flush_switch_logs();
    } else if count == 60 {
        println!("[Engine] 60 frames updated");
        #[cfg(target_os = "horizon")]
        bevy_horizon::flush_switch_logs();
    }

    for (cube, mut transform) in &mut query {
        transform.rotate_x(cube.speed_x * time.delta_secs());
        transform.rotate_y(cube.speed_y * time.delta_secs());
    }
}

fn handle_input(
    mut cube_query: Query<(&mut RotatingCube, &CubeMaterialHandle)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut axis_events: MessageReader<RawGamepadAxisChangedEvent>,
    mut button_events: MessageReader<RawGamepadButtonChangedEvent>,
    mut exit: MessageWriter<AppExit>,
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    gamepads: Query<&Gamepad>,
) {
    const COLORS: [Color; 4] = [
        Color::srgb(0.2, 0.75, 0.95), // Cyan
        Color::srgb(0.95, 0.25, 0.65), // Magenta
        Color::srgb(0.95, 0.85, 0.2),  // Yellow
        Color::srgb(0.25, 0.9, 0.4),  // Green
    ];

    let mut cycle_color = false;

    // Check keyboard for desktop testing
    if let Some(keys) = keyboard {
        if keys.just_pressed(KeyCode::Space) {
            cycle_color = true;
        }
        if keys.just_pressed(KeyCode::Escape) {
            exit.write(AppExit::Success);
        }
    }

    // Check Gamepad buttons (Horizon Joy-Con / Pro Controller)
    for ev in button_events.read() {
        if ev.value > 0.5 {
            match ev.button {
                // East is (A) button in Nintendo layout
                GamepadButton::East => {
                    println!("[Input] (A) pressed: cycling the cube's colour");
                    cycle_color = true;
                }
                // Start is (+) button; with (-) held it belongs to the FPS overlay's chord
                GamepadButton::Start if bevy_horizon::fps_chord_held(&gamepads) => {}
                GamepadButton::Start => {
                    println!("[Input] (+) pressed: exiting");
                    exit.write(AppExit::Success);
                }
                _ => {}
            }
        }
    }

    // Check Analog Stick input
    for ev in axis_events.read() {
        if let Ok((mut cube, _)) = cube_query.single_mut() {
            match ev.axis {
                GamepadAxis::LeftStickX => {
                    cube.speed_y = 0.8 + ev.value * 2.5;
                }
                GamepadAxis::LeftStickY => {
                    cube.speed_x = 0.5 + ev.value * 2.5;
                }
                _ => {}
            }
        }
    }

    // Apply color change if triggered
    if cycle_color {
        if let Ok((mut cube, mat_handle)) = cube_query.single_mut() {
            cube.color_idx = (cube.color_idx + 1) % COLORS.len();
            if let Some(mut material) = materials.get_mut(&mat_handle.0) {
                material.base_color = COLORS[cube.color_idx];
            }
        }
    }
}
