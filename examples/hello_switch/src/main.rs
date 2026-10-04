use bevy::ecs::message::{MessageReader, MessageWriter};
use bevy::input::gamepad::{
    GamepadAxis, GamepadButton, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent,
};
use bevy::prelude::*;

#[derive(Component)]
struct RotatingCube {
    speed_x: f32,
    speed_y: f32,
    color_idx: usize,
}

#[derive(Component)]
struct CubeMaterialHandle(Handle<StandardMaterial>);

fn main() {
    let mut app = App::new();

    app.add_plugins(
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy Horizon - Switch Demo".into(),
                resolution: (1280.0, 720.0).into(),
                ..default()
            }),
            ..default()
        }),
    );

    #[cfg(target_os = "horizon")]
    {
        app.add_plugins(bevy_horizon::HorizonPlugin);

        // Horizon OS kernel scheduler works best with single-threaded execution
        app.edit_schedule(Update, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(PreUpdate, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.edit_schedule(PostUpdate, |s| {
            s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
    }

    app.add_systems(Startup, setup_scene)
        .add_systems(Update, (rotate_cube, handle_input));

    app.run();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // 3D Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.5, 3.5).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Directional Sunlight
    commands.spawn((
        DirectionalLight {
            illuminance: 3500.0,
            shadows_enabled: false,
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
                Text::new("L-Stick / Arrows: Adjust Rotation\n(A) Button / Space: Cycle Color\n(+) Button / Esc: Exit"),
                TextColor(Color::srgb(0.85, 0.85, 0.85)),
            ));
        });
}

fn rotate_cube(
    time: Res<Time>,
    mut query: Query<(&RotatingCube, &mut Transform)>,
) {
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
                    cycle_color = true;
                }
                // Start is (+) button
                GamepadButton::Start => {
                    exit.write(AppExit::Success);
                }
                _ => {}
            }
        }
    }

    // Check Analog Stick input
    for ev in axis_events.read() {
        if let Ok((mut cube, _)) = cube_query.get_single_mut() {
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
        if let Ok((mut cube, mat_handle)) = cube_query.get_single_mut() {
            cube.color_idx = (cube.color_idx + 1) % COLORS.len();
            if let Some(material) = materials.get_mut(&mat_handle.0) {
                material.base_color = COLORS[cube.color_idx];
            }
        }
    }
}
