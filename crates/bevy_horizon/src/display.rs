use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_window::{PrimaryWindow, Window, WindowResolution};

pub struct HorizonDisplayPlugin;

#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizonDisplayMode {
    Handheld,
    Docked,
}

impl Plugin for HorizonDisplayPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(HorizonDisplayMode::Handheld);
        #[cfg(target_os = "horizon")]
        app.add_systems(PreUpdate, monitor_dock_status);
    }
}

#[cfg(target_os = "horizon")]
mod applet_ffi {
    use core::ffi::c_int;

    unsafe extern "C" {
        // 0: Handheld, 1: Docked
        pub fn appletGetOperationMode() -> c_int;
    }
}

#[cfg(target_os = "horizon")]
fn monitor_dock_status(
    mut current_mode: ResMut<HorizonDisplayMode>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let mode = unsafe { applet_ffi::appletGetOperationMode() };
    let new_mode = if mode == 1 {
        HorizonDisplayMode::Docked
    } else {
        HorizonDisplayMode::Handheld
    };

    if *current_mode != new_mode {
        *current_mode = new_mode;
        let (width, height) = match new_mode {
            HorizonDisplayMode::Docked => (1920, 1080),
            HorizonDisplayMode::Handheld => (1280, 720),
        };
        tracing::info!("Switch display mode changed to {new_mode:?}: {width}x{height}");

        for mut window in &mut windows {
            window.resolution = WindowResolution::new(width, height);
        }
    }
}
