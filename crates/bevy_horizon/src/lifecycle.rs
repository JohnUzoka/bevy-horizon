use bevy_app::{App, AppExit, Plugin, PreUpdate};
use bevy_ecs::prelude::*;

pub struct HorizonLifecyclePlugin;

impl Plugin for HorizonLifecyclePlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_os = "horizon")]
        {
            init_horizon_services();
            app.add_systems(PreUpdate, check_applet_loop);
        }
    }
}

#[cfg(target_os = "horizon")]
mod ffi {
    use core::ffi::c_int;

    extern "C" {
        pub fn romfsInit() -> c_int;
        pub fn romfsExit() -> c_int;
        pub fn socketInitializeDefault() -> c_int;
        pub fn socketExit();
        pub fn appletMainLoop() -> bool;
    }
}

#[cfg(target_os = "horizon")]
fn init_horizon_services() {
    unsafe {
        let romfs_res = ffi::romfsInit();
        if romfs_res != 0 {
            tracing::warn!("romfsInit returned error code: {romfs_res}");
        } else {
            tracing::info!("romfs initialized successfully");
        }

        let socket_res = ffi::socketInitializeDefault();
        if socket_res != 0 {
            tracing::warn!("socketInitializeDefault returned error code: {socket_res}");
        } else {
            tracing::info!("BSD socket layer initialized successfully");
        }
    }
}

#[cfg(target_os = "horizon")]
fn check_applet_loop(mut exit_writer: MessageWriter<AppExit>) {
    if !unsafe { ffi::appletMainLoop() } {
        tracing::info!("Switch applet requested exit; shutting down Bevy app.");
        unsafe {
            ffi::socketExit();
            ffi::romfsExit();
        }
        exit_writer.send(AppExit::Success);
    }
}
