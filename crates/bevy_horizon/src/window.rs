//! Gives Bevy's renderer the Switch's window.
//!
//! On desktop, bevy_winit creates an OS window for each `Window` entity and attaches a
//! [`RawHandleWrapper`]; bevy_render then makes a wgpu surface for every window that has one. The
//! Switch has a single screen, which libnx sets up before `main` as its default `NWindow`. This
//! module attaches that window to Bevy's primary window, wrapped in raw-window-handle's Horizon
//! handles (`patches/raw-window-handle`), which wgpu's Vulkan backend (`patches/wgpu-hal`) turns
//! into a `VK_NN_vi_surface`. Other `Window` entities get nothing, so they aren't drawn.

use core::ffi::c_void;
use core::ptr::NonNull;

use bevy_app::{App, First, Plugin};
use bevy_ecs::prelude::*;
use bevy_window::{PrimaryWindow, RawHandleWrapper, RawHandleWrapperHolder, WindowWrapper};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, HorizonWindowHandle,
    RawWindowHandle, WindowHandle,
};

unsafe extern "C" {
    /// libnx: the window for the application's default display layer.
    fn nwindowGetDefault() -> *mut c_void;
}

/// libnx's default `NWindow`. libnx creates it before `main` and never frees it.
struct DefaultNWindow(NonNull<c_void>);

// SAFETY: a pointer to an object that lives for the whole process; the renderer is its only user.
unsafe impl Send for DefaultNWindow {}
// SAFETY: as above.
unsafe impl Sync for DefaultNWindow {}

impl HasWindowHandle for DefaultNWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Horizon(HorizonWindowHandle::new(self.0));
        // SAFETY: the NWindow outlives every borrow (see DefaultNWindow).
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for DefaultNWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::horizon())
    }
}

/// Attaches the Switch's screen to the primary window, so bevy_render draws to it.
pub struct HorizonWindowPlugin;

impl Plugin for HorizonWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(First, attach_default_nwindow);
    }
}

fn attach_default_nwindow(
    mut commands: Commands,
    windows: Query<(Entity, Option<&RawHandleWrapperHolder>), (With<PrimaryWindow>, Without<RawHandleWrapper>)>,
) {
    let Some((entity, holder)) = windows.iter().next() else {
        return;
    };
    let Some(n_window) = NonNull::new(unsafe { nwindowGetDefault() }) else {
        tracing::error!("nwindowGetDefault returned null; nothing can be drawn");
        return;
    };
    let wrapper = match RawHandleWrapper::new(&WindowWrapper::new(DefaultNWindow(n_window))) {
        Ok(wrapper) => wrapper,
        Err(err) => {
            tracing::error!("can't wrap the default NWindow: {err}");
            return;
        }
    };
    // bevy_render only reads the holder when RenderPlugin builds, which is before any system
    // runs, so this matters only to code that looks later; bevy_winit fills it too.
    if let Some(holder) = holder {
        *holder.0.lock().unwrap() = Some(wrapper.clone());
    }
    commands.entity(entity).insert(wrapper);
    tracing::info!("primary window attached to libnx's default NWindow");
}
