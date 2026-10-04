//! Bevy Engine runtime support for Nintendo Switch (Horizon OS).
//!
//! Provides libnx lifecycle initialization, Joy-Con/Pro Controller gamepad integration,
//! and dynamic handheld/docked resolution handling.

pub mod display;
pub mod input;
pub mod lifecycle;

use bevy_app::{App, Plugin};
pub use display::{HorizonDisplayMode, HorizonDisplayPlugin};
pub use input::{HorizonGamepadState, HorizonInputPlugin};
pub use lifecycle::HorizonLifecyclePlugin;

/// Unified plugin for running Bevy applications on Nintendo Switch.
#[derive(Default)]
pub struct HorizonPlugin;

impl Plugin for HorizonPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            HorizonLifecyclePlugin,
            HorizonInputPlugin,
            HorizonDisplayPlugin,
        ));
    }
}
