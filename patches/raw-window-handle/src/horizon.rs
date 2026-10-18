// bevy-horizon: this module is new. It follows haiku.rs in shape.

use core::ffi::c_void;
use core::ptr::NonNull;

use super::DisplayHandle;

/// Raw display handle for Horizon, the Nintendo Switch's OS, as homebrew sees it through libnx.
///
/// libnx opens the display service (vi) and the default layer itself, before `main`, so there is
/// no separate display connection to hand around.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HorizonDisplayHandle {}

impl HorizonDisplayHandle {
    /// Create a new empty display handle.
    ///
    ///
    /// # Example
    ///
    /// ```
    /// # use raw_window_handle::HorizonDisplayHandle;
    /// let handle = HorizonDisplayHandle::new();
    /// ```
    pub fn new() -> Self {
        Self {}
    }
}

impl DisplayHandle<'static> {
    /// Create a Horizon-based display handle.
    ///
    /// As no data is borrowed by this handle, it is completely safe to create.
    ///
    /// # Example
    ///
    /// ```
    /// # use raw_window_handle::{DisplayHandle, HasDisplayHandle};
    /// # fn do_something(rwh: impl HasDisplayHandle) { let _ = rwh; }
    /// let handle = DisplayHandle::horizon();
    /// do_something(handle);
    /// ```
    pub fn horizon() -> Self {
        // SAFETY: No data is borrowed.
        unsafe { Self::borrow_raw(HorizonDisplayHandle::new().into()) }
    }
}

/// Raw window handle for Horizon.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HorizonWindowHandle {
    /// A pointer to a libnx `NWindow`, such as the one `nwindowGetDefault()` returns. It is what
    /// `VkViSurfaceCreateInfoNN::window` (`VK_NN_vi_surface`) and `eglCreateWindowSurface` take.
    pub n_window: NonNull<c_void>,
}

impl HorizonWindowHandle {
    /// Create a new handle to a window.
    ///
    ///
    /// # Example
    ///
    /// ```
    /// # use core::ptr::NonNull;
    /// # use raw_window_handle::HorizonWindowHandle;
    /// # type NWindow = ();
    /// #
    /// let n_window: NonNull<NWindow>;
    /// # n_window = NonNull::from(&());
    /// let handle = HorizonWindowHandle::new(n_window.cast());
    /// ```
    pub fn new(n_window: NonNull<c_void>) -> Self {
        Self { n_window }
    }
}
