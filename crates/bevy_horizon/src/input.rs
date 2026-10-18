use bevy_app::{App, Plugin, PreStartup, PreUpdate};
use bevy_ecs::message::MessageWriter;
use bevy_ecs::prelude::*;
use bevy_input::gamepad::{
    GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent,
    RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};

pub struct HorizonInputPlugin;

/// libnx's `HidNpadButton` bits (switch/services/hid.h) for the buttons in [`RawPad::buttons`].
pub mod npad_button {
    pub const A: u64 = 1 << 0;
    pub const B: u64 = 1 << 1;
    pub const X: u64 = 1 << 2;
    pub const Y: u64 = 1 << 3;
    pub const STICK_L: u64 = 1 << 4;
    pub const STICK_R: u64 = 1 << 5;
    pub const L: u64 = 1 << 6;
    pub const R: u64 = 1 << 7;
    pub const ZL: u64 = 1 << 8;
    pub const ZR: u64 = 1 << 9;
    pub const PLUS: u64 = 1 << 10;
    pub const MINUS: u64 = 1 << 11;
    pub const LEFT: u64 = 1 << 12;
    pub const UP: u64 = 1 << 13;
    pub const RIGHT: u64 = 1 << 14;
    pub const DOWN: u64 = 1 << 15;
}

/// Player 1's controller as libnx read it this frame, for a game with its own input layer
/// instead of Bevy's gamepad (Skate 3 reads XInput state on Windows, for example).
/// The plugin's PreUpdate system refreshes it before `InputSystems`; until then, off Horizon, and
/// with no controller connected it is all zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawPad {
    /// [`npad_button`] bits held down.
    pub buttons: u64,
    /// Left then right stick, `[x, y]`, each -32767..=32767, with +y up.
    pub sticks: [[i32; 2]; 2],
    /// libnx's `HidNpadStyleTag` bits for the controller: 0 when none is connected,
    /// 0x1 Pro Controller, 0x2 handheld, 0x4 dual Joy-Con.
    pub style_set: u32,
}

static RAW_PAD: std::sync::Mutex<RawPad> =
    std::sync::Mutex::new(RawPad { buttons: 0, sticks: [[0; 2]; 2], style_set: 0 });

/// Returns [`RawPad`]: player 1's controller from this frame's read.
pub fn raw_pad() -> RawPad {
    *RAW_PAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    #[cfg(target_os = "horizon")]
    last_style_set: u32,
    #[cfg(target_os = "horizon")]
    seen_buttons: bool,
}

impl Plugin for HorizonInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HorizonGamepadState>();
        app.add_systems(PreStartup, setup_horizon_gamepad);
        // Like bevy_gilrs: raw events written before InputSystems are in this frame's input.
        #[cfg(target_os = "horizon")]
        app.add_systems(PreUpdate, poll_horizon_input.before(bevy_input::InputSystems));
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
    #[derive(Default, Copy, Clone, Debug)]
    pub struct HidAnalogStickState {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Default, Copy, Clone, Debug)]
    pub struct PadState {
        pub id_mask: u8,
        pub active_id_mask: u8,
        pub read_handheld: bool,
        pub active_handheld: bool,
        pub style_set: u32,
        pub attributes: u32,
        pub _pad: u32,
        pub buttons_cur: u64,
        pub buttons_old: u64,
        pub sticks: [HidAnalogStickState; 2],
        pub gc_triggers: [u32; 2],
    }

    const _: () = {
        assert!(core::mem::size_of::<PadState>() == 56);
        assert!(core::mem::align_of::<PadState>() == 8);
        assert!(core::mem::offset_of!(PadState, buttons_cur) == 16);
        assert!(core::mem::offset_of!(PadState, buttons_old) == 24);
        assert!(core::mem::offset_of!(PadState, sticks) == 32);
        assert!(core::mem::offset_of!(PadState, gc_triggers) == 48);
    };

    unsafe extern "C" {
        pub fn padConfigureInput(max_players: u32, style_set: u32);
        pub fn padInitializeWithMask(state: *mut PadState, pad_mask: u64);
        pub fn padUpdate(state: *mut PadState);
    }

    /// libnx `HidNpadStyleSet_NpadStandard` (switch/services/hid.h): Pro Controller, handheld,
    /// dual Joy-Con, and a single left or right Joy-Con.
    pub const NPAD_STYLE_SET_STANDARD: u32 = 0x1F;
}

#[cfg(target_os = "horizon")]
fn poll_horizon_input(
    mut state: ResMut<HorizonGamepadState>,
    mut events: MessageWriter<RawGamepadEvent>,
    mut button_events: MessageWriter<RawGamepadButtonChangedEvent>,
    mut axis_events: MessageWriter<RawGamepadAxisChangedEvent>,
) {
    let Some(gamepad) = state.entity else { return };

    static mut NX_PAD: hid_ffi::PadState = hid_ffi::PadState {
        id_mask: 0,
        active_id_mask: 0,
        read_handheld: false,
        active_handheld: false,
        style_set: 0,
        attributes: 0,
        _pad: 0,
        buttons_cur: 0,
        buttons_old: 0,
        sticks: [hid_ffi::HidAnalogStickState { x: 0, y: 0 }; 2],
        gc_triggers: [0; 2],
    };
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        // libnx's pad API needs this first: it runs hidInitializeNpad and tells HID which
        // players and controller styles the app supports. Every libnx example that reads a pad
        // calls it before padInitialize*.
        hid_ffi::padConfigureInput(1, hid_ffi::NPAD_STYLE_SET_STANDARD);
        // Player 1 (bit 0) and the handheld Joy-Con (bit 32), the same set as padInitializeDefault.
        hid_ffi::padInitializeWithMask(std::ptr::addr_of_mut!(NX_PAD), (1u64 << 0) | (1u64 << 32));
        tracing::info!("[Input] HID configured: 1 player, standard controllers; reading player 1 and handheld");
    });

    unsafe {
        hid_ffi::padUpdate(std::ptr::addr_of_mut!(NX_PAD));
    }

    let style_set = unsafe { NX_PAD.style_set };
    if style_set != state.last_style_set {
        tracing::info!(
            "[Input] controller styles now {style_set:#x} (0 = none, 0x1 = Pro Controller, 0x2 = handheld, 0x4 = dual Joy-Con)"
        );
        state.last_style_set = style_set;
    }

    let buttons = unsafe { NX_PAD.buttons_cur };
    if buttons != 0 && !state.seen_buttons {
        tracing::info!("[Input] first button press read from HID (buttons {buttons:#x})");
        state.seen_buttons = true;
    }
    let sticks = unsafe { NX_PAD.sticks };
    *RAW_PAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = RawPad {
        buttons,
        sticks: [[sticks[0].x, sticks[0].y], [sticks[1].x, sticks[1].y]],
        style_set,
    };
    let changed_buttons = buttons ^ state.last_buttons;

    use npad_button as nb;
    let button_mappings = [
        (nb::A, GamepadButton::East),       // Nintendo A is East
        (nb::B, GamepadButton::South),      // Nintendo B is South
        (nb::X, GamepadButton::North),      // Nintendo X is North
        (nb::Y, GamepadButton::West),       // Nintendo Y is West
        (nb::L, GamepadButton::LeftTrigger),
        (nb::R, GamepadButton::RightTrigger),
        (nb::ZL, GamepadButton::LeftTrigger2),
        (nb::ZR, GamepadButton::RightTrigger2),
        (nb::PLUS, GamepadButton::Start),
        (nb::MINUS, GamepadButton::Select),
        (nb::UP, GamepadButton::DPadUp),
        (nb::DOWN, GamepadButton::DPadDown),
        (nb::LEFT, GamepadButton::DPadLeft),
        (nb::RIGHT, GamepadButton::DPadRight),
        (nb::STICK_L, GamepadButton::LeftThumb),
        (nb::STICK_R, GamepadButton::RightThumb),
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
    let lx = (sticks[0].x as f32 / 32767.0).clamp(-1.0, 1.0);
    let ly = (sticks[0].y as f32 / 32767.0).clamp(-1.0, 1.0);
    let rx = (sticks[1].x as f32 / 32767.0).clamp(-1.0, 1.0);
    let ry = (sticks[1].y as f32 / 32767.0).clamp(-1.0, 1.0);

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
