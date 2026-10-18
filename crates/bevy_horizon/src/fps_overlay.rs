//! Bevy's own FPS overlay, `bevy_dev_tools::fps_overlay::FpsOverlayPlugin` (the FPS text and the
//! frame-time graph under it), hidden at launch and shown or hidden by a button chord: hold (−)
//! and press (+). Pressing both together also works.
//!
//! Bevy gives the overlay no button of its own; a game shows or hides it through
//! `FpsOverlayConfig`. This plugin only adds the chord. Many games use (+) alone for pause or
//! exit, so a game should ignore (+) while (−) is held ([`fps_chord_held`] tells it when), as
//! hello_switch does.
//!
//! The overlay draws through bevy_ui and bevy_text, so a game needs Bevy's `bevy_ui_render`,
//! `bevy_text` and `default_font` features (or its own font in `FpsOverlayConfig::text_config`).

use bevy_app::{App, Plugin, Update};
use bevy_dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin, FpsOverlaySystems};
use bevy_ecs::prelude::*;
use bevy_input::gamepad::{Gamepad, GamepadButton};

/// Adds Bevy's `FpsOverlayPlugin` and the (−) + (+) chord that shows or hides it.
#[derive(Default)]
pub struct HorizonFpsOverlayPlugin {
    /// Show the overlay from the first frame instead of waiting for the chord.
    pub visible_at_launch: bool,
}

impl Plugin for HorizonFpsOverlayPlugin {
    fn build(&self, app: &mut App) {
        let mut config = FpsOverlayConfig::default();
        set_visible(&mut config, self.visible_at_launch);
        app.add_plugins(FpsOverlayPlugin { config }).add_systems(
            Update,
            toggle_on_chord.before(FpsOverlaySystems::Customize),
        );
    }
}

/// True while (−) is held, so a game can tell a press of (+) that belongs to the chord from a
/// press of (+) on its own.
pub fn fps_chord_held(gamepads: &Query<&Gamepad>) -> bool {
    gamepads
        .iter()
        .any(|gamepad| gamepad.pressed(GamepadButton::Select))
}

fn set_visible(config: &mut FpsOverlayConfig, visible: bool) {
    // `enabled` only covers the text; the graph has its own switch.
    config.enabled = visible;
    config.frame_time_graph_config.enabled = visible;
}

fn toggle_on_chord(gamepads: Query<&Gamepad>, mut config: ResMut<FpsOverlayConfig>) {
    let chord = gamepads.iter().any(|gamepad| {
        gamepad.pressed(GamepadButton::Select)
            && gamepad.pressed(GamepadButton::Start)
            && (gamepad.just_pressed(GamepadButton::Select)
                || gamepad.just_pressed(GamepadButton::Start))
    });
    if chord {
        let visible = !config.enabled;
        set_visible(&mut config, visible);
        tracing::info!(
            "[FPS] (−) + (+): overlay {}",
            if visible { "shown" } else { "hidden" }
        );
    }
}
