use bevy_app::{App, Plugin, PreStartup, PreUpdate};
use bevy_ecs::message::MessageWriter;
use bevy_ecs::prelude::*;
use bevy_input::gamepad::{
    GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent,
    RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};

pub struct HorizonInputPlugin;

#[derive(Resource, Default)]
pub struct HorizonGamepadState {
    pub entity: Option<Entity>,
    #[cfg(target_os = "horizon")]
    last_buttons: u64,
    #[cfg(target_os = "horizon")]
    last_left_x: f32,
    #[cfg(target_os = "horizon")]
    last_left_y: f32,
    #[cfg(target_os = "horizon")]
    last_right_x: f32,
    #[cfg(target_os = "horizon")]
    last_right_y: f32,
}

impl Plugin for HorizonInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HorizonGamepadState>();
        app.add_systems(PreStartup, setup_horizon_gamepad);
        #[cfg(target_os = "horizon")]
        app.add_systems(PreUpdate, poll_horizon_input);
    }
}

fn setup_horizon_gamepad(
    mut commands: Commands,
    mut state: ResMut<HorizonGamepadState>,
    mut events: MessageWriter<RawGamepadEvent>,
    mut conn_events: MessageWriter<GamepadConnectionEvent>,
) {
    let entity = commands.spawn_empty().id();
    state.entity = Some(entity);

    let event = GamepadConnectionEvent::new(
        entity,
        GamepadConnection::Connected {
            name: "Nintendo Switch Controller".to_string(),
            vendor_id: Some(0x057e),
            product_id: Some(0x2009),
        },
    );
    events.write(event.clone().into());
    conn_events.write(event);
}

#[cfg(target_os = "horizon")]
mod hid_ffi {
    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    pub struct HidAnalogStickState {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    pub struct PadState {
        pub active_handheld: bool,
        pub active_id: u32,
        pub buttons: u64,
        pub sticks: [HidAnalogStickState; 2],
    }

    extern "C" {
        pub fn padInitializeDefault(state: *mut PadState);
        pub fn padUpdate(state: *mut PadState);
    }
}

#[cfg(target_os = "horizon")]
fn poll_horizon_input(
    mut state: ResMut<HorizonGamepadState>,
    mut events: MessageWriter<RawGamepadEvent>,
    mut button_events: MessageWriter<RawGamepadButtonChangedEvent>,
    mut axis_events: MessageWriter<RawGamepadAxisChangedEvent>,
) {
    let Some(gamepad) = state.entity else { return };

    // Standard Switch Joy-Con / Pro Controller button masks from libnx
    const BUTTON_A: u64 = 1 << 0;
    const BUTTON_B: u64 = 1 << 1;
    const BUTTON_X: u64 = 1 << 2;
    const BUTTON_Y: u64 = 1 << 3;
    const BUTTON_STICK_L: u64 = 1 << 4;
    const BUTTON_STICK_R: u64 = 1 << 5;
    const BUTTON_L: u64 = 1 << 6;
    const BUTTON_R: u64 = 1 << 7;
    const BUTTON_ZL: u64 = 1 << 8;
    const BUTTON_ZR: u64 = 1 << 9;
    const BUTTON_PLUS: u64 = 1 << 10;
    const BUTTON_MINUS: u64 = 1 << 11;
    const BUTTON_DLEFT: u64 = 1 << 12;
    const BUTTON_DUP: u64 = 1 << 13;
    const BUTTON_DRIGHT: u64 = 1 << 14;
    const BUTTON_DDOWN: u64 = 1 << 15;

    static mut NX_PAD: hid_ffi::PadState = hid_ffi::PadState {
        active_handheld: false,
        active_id: 0,
        buttons: 0,
        sticks: [hid_ffi::HidAnalogStickState { x: 0, y: 0 }; 2],
    };
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        hid_ffi::padInitializeDefault(std::ptr::addr_of_mut!(NX_PAD));
    });

    unsafe {
        hid_ffi::padUpdate(std::ptr::addr_of_mut!(NX_PAD));
    }

    let buttons = unsafe { NX_PAD.buttons };
    let changed_buttons = buttons ^ state.last_buttons;

    let button_mappings = [
        (BUTTON_A, GamepadButton::East),       // Nintendo A is East
        (BUTTON_B, GamepadButton::South),      // Nintendo B is South
        (BUTTON_X, GamepadButton::North),      // Nintendo X is North
        (BUTTON_Y, GamepadButton::West),       // Nintendo Y is West
        (BUTTON_L, GamepadButton::LeftTrigger),
        (BUTTON_R, GamepadButton::RightTrigger),
        (BUTTON_ZL, GamepadButton::LeftTrigger2),
        (BUTTON_ZR, GamepadButton::RightTrigger2),
        (BUTTON_PLUS, GamepadButton::Start),
        (BUTTON_MINUS, GamepadButton::Select),
        (BUTTON_DUP, GamepadButton::DPadUp),
        (BUTTON_DDOWN, GamepadButton::DPadDown),
        (BUTTON_DLEFT, GamepadButton::DPadLeft),
        (BUTTON_DRIGHT, GamepadButton::DPadRight),
        (BUTTON_STICK_L, GamepadButton::LeftThumb),
        (BUTTON_STICK_R, GamepadButton::RightThumb),
    ];

    for (mask, button) in button_mappings {
        if changed_buttons & mask != 0 {
            let pressed = (buttons & mask) != 0;
            let value = if pressed { 1.0 } else { 0.0 };
            let ev = RawGamepadButtonChangedEvent::new(gamepad, button, value);
            events.write(ev.into());
            button_events.write(ev);
        }
    }
    state.last_buttons = buttons;

    // Analog stick normalization: clamp and scale [-32768, 32767] -> [-1.0, 1.0]
    let lx = (unsafe { NX_PAD.sticks[0].x } as f32 / 32767.0).clamp(-1.0, 1.0);
    let ly = (unsafe { NX_PAD.sticks[0].y } as f32 / 32767.0).clamp(-1.0, 1.0);
    let rx = (unsafe { NX_PAD.sticks[1].x } as f32 / 32767.0).clamp(-1.0, 1.0);
    let ry = (unsafe { NX_PAD.sticks[1].y } as f32 / 32767.0).clamp(-1.0, 1.0);

    if (lx - state.last_left_x).abs() > 0.01 {
        let ev = RawGamepadAxisChangedEvent::new(gamepad, GamepadAxis::LeftStickX, lx);
        events.write(ev.into());
        axis_events.write(ev);
        state.last_left_x = lx;
    }
    if (ly - state.last_left_y).abs() > 0.01 {
        let ev = RawGamepadAxisChangedEvent::new(gamepad, GamepadAxis::LeftStickY, ly);
        events.write(ev.into());
        axis_events.write(ev);
        state.last_left_y = ly;
    }
    if (rx - state.last_right_x).abs() > 0.01 {
        let ev = RawGamepadAxisChangedEvent::new(gamepad, GamepadAxis::RightStickX, rx);
        events.write(ev.into());
        axis_events.write(ev);
        state.last_right_x = rx;
    }
    if (ry - state.last_right_y).abs() > 0.01 {
        let ev = RawGamepadAxisChangedEvent::new(gamepad, GamepadAxis::RightStickY, ry);
        events.write(ev.into());
        axis_events.write(ev);
        state.last_right_y = ry;
    }
}
