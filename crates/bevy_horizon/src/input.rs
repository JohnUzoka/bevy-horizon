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

/// Player 1's controller as this frame's read left it, for a game with its own input layer
/// instead of Bevy's gamepad (Skate 3 reads XInput state on Windows, for example).
/// The plugin's PreUpdate system refreshes it before `InputSystems`; until then, off Horizon, and
/// with no controller connected it is all zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawPad {
    /// [`npad_button`] bits held down. A mapped button pressed and released since the last
    /// frame's read (a tap shorter than the frame) is held here for this frame and let go in the
    /// next, as it is for Bevy's gamepad.
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
    /// The buttons Bevy has been told are held.
    #[cfg(target_os = "horizon")]
    last_buttons: u64,
    /// Buttons pressed and released within the last frame's read, held for that frame; their
    /// release goes to Bevy first thing this frame.
    #[cfg(target_os = "horizon")]
    deferred_releases: u64,
    /// This frame's reads from the sampler, and the button changes they make; kept for reuse.
    #[cfg(target_os = "horizon")]
    reads: Vec<RawPad>,
    #[cfg(target_os = "horizon")]
    changes: Vec<Option<(GamepadButton, f32)>>,
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
        pub fn svcSetThreadPriority(handle: u32, priority: u32) -> u32;
    }

    /// libnx `HidNpadStyleSet_NpadStandard` (switch/services/hid.h): Pro Controller, handheld,
    /// dual Joy-Con, and a single left or right Joy-Con.
    pub const NPAD_STYLE_SET_STANDARD: u32 = 0x1F;
    pub const CUR_THREAD_HANDLE: u32 = 0xFFFF_8000;
}

/// Reads the pad on a thread of its own every `SAMPLE_PERIOD` and queues each change.
///
/// libnx's pad API only gives the pad's latest state, so reading it once a frame missed a press
/// and release that both fell between two frames: a 50 ms tap at 10 fps, or every tap during a
/// long frame. `poll_horizon_input` drains the queue each frame instead. A full queue (nobody
/// reading) holds new changes back, so they fold into one, as a once-a-frame read would.
#[cfg(target_os = "horizon")]
mod sampler {
    use super::{RawPad, hid_ffi};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, PoisonError};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    /// How often the thread reads the pad: well under the shortest press a person makes.
    pub const SAMPLE_PERIOD: Duration = Duration::from_millis(4);
    /// Changes queued before the thread waits for a frame to take them.
    const QUEUE_LEN: usize = 256;
    /// The thread's priority: above the main thread's 0x2C, so a long frame on its core doesn't
    /// hold the reads back. Each read takes microseconds.
    const PRIORITY: u32 = 0x2B;
    /// How long the exit hook waits for the thread to stop; it looks every `SAMPLE_PERIOD`.
    const EXIT_WAIT: Duration = Duration::from_millis(100);

    struct Queue {
        /// Reads whose buttons or controller style differ from the read queued before, oldest first.
        changes: VecDeque<RawPad>,
        /// The latest read, for the sticks and for a change a full queue held back.
        latest: RawPad,
    }

    static QUEUE: Mutex<Queue> = Mutex::new(Queue {
        changes: VecDeque::new(),
        latest: RawPad { buttons: 0, sticks: [[0; 2]; 2], style_set: 0 },
    });
    static STOP: AtomicBool = AtomicBool::new(false);
    static STOPPED: AtomicBool = AtomicBool::new(false);
    static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

    /// Starts the thread on `pad`, which libnx has set up, and has the exit hook join it.
    pub fn start(pad: hid_ffi::PadState) -> std::io::Result<()> {
        QUEUE.lock().unwrap_or_else(PoisonError::into_inner).changes.reserve(QUEUE_LEN);
        let thread = std::thread::Builder::new()
            .name("bh pad sampler".into())
            .stack_size(64 << 10)
            .spawn(move || run(pad))?;
        *THREAD.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread);
        if !crate::lifecycle::register_release_hook(stop_for_exit) {
            tracing::warn!(
                "[Input] all release hook slots are taken, so the pad sampler won't be stopped at exit and the heap check will list its stack"
            );
        }
        Ok(())
    }

    fn run(mut pad: hid_ffi::PadState) {
        // SAFETY: changes only this thread's priority. If it fails, the thread stays at std's
        // 0x3B, behind the main thread.
        unsafe { hid_ffi::svcSetThreadPriority(hid_ffi::CUR_THREAD_HANDLE, PRIORITY) };
        let mut queued = RawPad::default();
        while !STOP.load(Ordering::Acquire) {
            // SAFETY: libnx set `pad` up (padInitializeWithMask); padUpdate only writes it.
            unsafe { hid_ffi::padUpdate(&mut pad) };
            let read = RawPad {
                buttons: pad.buttons_cur,
                sticks: [[pad.sticks[0].x, pad.sticks[0].y], [pad.sticks[1].x, pad.sticks[1].y]],
                style_set: pad.style_set,
            };
            {
                let mut queue = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
                queue.latest = read;
                let changed = (read.buttons, read.style_set) != (queued.buttons, queued.style_set);
                if changed && queue.changes.len() < QUEUE_LEN {
                    queue.changes.push_back(read);
                    queued = read;
                }
            }
            std::thread::sleep(SAMPLE_PERIOD);
        }
        STOPPED.store(true, Ordering::Release);
    }

    /// Moves the queued changes to `reads`, oldest first, then the latest read if a full queue
    /// held it back. Returns the latest read.
    pub fn drain(reads: &mut Vec<RawPad>) -> RawPad {
        let mut queue = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
        let Queue { changes, latest } = &mut *queue;
        let latest = *latest;
        reads.clear();
        reads.extend(changes.drain(..));
        if reads.last().is_some_and(|last| (last.buttons, last.style_set) != (latest.buttons, latest.style_set)) {
            reads.push(latest);
        }
        latest
    }

    /// Release hook: stops the thread and joins it, so its stack is back in the heap before the
    /// homebrew loader reads the next program into it. Allocates nothing.
    fn stop_for_exit() {
        STOP.store(true, Ordering::Release);
        let start = Instant::now();
        while !STOPPED.load(Ordering::Acquire) && start.elapsed() < EXIT_WAIT {
            std::thread::sleep(Duration::from_millis(1));
        }
        // try_lock: exit could come from a thread that holds it, which would deadlock here.
        let thread = THREAD.try_lock().ok().and_then(|mut thread| thread.take());
        match thread {
            Some(thread) if STOPPED.load(Ordering::Acquire) => {
                let _ = thread.join();
                crate::logging::write_log_line(format_args!(
                    "[Input] exit: the pad sampler stopped and was joined in {} ms",
                    start.elapsed().as_millis()
                ));
            }
            // Dropped, the handle detaches the thread, and bevy_horizon closes it once it exits.
            _ => crate::logging::write_log_line(format_args!(
                "[Input] exit: the pad sampler didn't stop within {} ms",
                EXIT_WAIT.as_millis()
            )),
        }
    }
}

/// Bevy's button for each mapped [`npad_button`] bit.
#[cfg(target_os = "horizon")]
const BUTTON_MAP: [(u64, GamepadButton); 16] = {
    use npad_button as nb;
    [
        (nb::A, GamepadButton::East),  // Nintendo A is East
        (nb::B, GamepadButton::South), // Nintendo B is South
        (nb::X, GamepadButton::North), // Nintendo X is North
        (nb::Y, GamepadButton::West),  // Nintendo Y is West
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
    ]
};

#[cfg(target_os = "horizon")]
fn poll_horizon_input(
    mut state: ResMut<HorizonGamepadState>,
    mut events: MessageWriter<RawGamepadEvent>,
    mut button_events: MessageWriter<RawGamepadButtonChangedEvent>,
    mut axis_events: MessageWriter<RawGamepadAxisChangedEvent>,
) {
    let Some(gamepad) = state.entity else { return };

    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        // libnx's pad API needs this first: it runs hidInitializeNpad and tells HID which
        // players and controller styles the app supports. Every libnx example that reads a pad
        // calls it before padInitialize*.
        hid_ffi::padConfigureInput(1, hid_ffi::NPAD_STYLE_SET_STANDARD);
        // Player 1 (bit 0) and the handheld Joy-Con (bit 32), the same set as padInitializeDefault.
        let mut pad = hid_ffi::PadState::default();
        hid_ffi::padInitializeWithMask(&mut pad, (1u64 << 0) | (1u64 << 32));
        match sampler::start(pad) {
            Ok(()) => tracing::info!(
                "[Input] HID configured: 1 player, standard controllers; reading player 1 and handheld every {} ms on a thread of its own",
                sampler::SAMPLE_PERIOD.as_millis()
            ),
            Err(error) => tracing::error!("[Input] couldn't start the pad sampler's thread ({error}); no input"),
        }
    });

    let state = &mut *state;
    let latest = sampler::drain(&mut state.reads);

    if latest.style_set != state.last_style_set {
        tracing::info!(
            "[Input] controller styles now {:#x} (0 = none, 0x1 = Pro Controller, 0x2 = handheld, 0x4 = dual Joy-Con)",
            latest.style_set
        );
        state.last_style_set = latest.style_set;
    }
    if !state.seen_buttons {
        if let Some(read) = state.reads.iter().find(|read| read.buttons != 0) {
            tracing::info!("[Input] first button press read from HID (buttons {:#x})", read.buttons);
            state.seen_buttons = true;
        }
    }

    // Every change since the last frame, in order; the releases held back last frame first.
    state.changes.clear();
    for (mask, button) in BUTTON_MAP {
        if state.deferred_releases & mask != 0 {
            state.changes.push(Some((button, 0.0)));
        }
    }
    let mut held = state.last_buttons & !state.deferred_releases;
    let mut pressed = 0;
    for read in &state.reads {
        let changed = read.buttons ^ held;
        for (mask, button) in BUTTON_MAP {
            if changed & mask != 0 {
                let down = read.buttons & mask != 0;
                if down {
                    pressed |= mask;
                }
                state.changes.push(Some((button, if down { 1.0 } else { 0.0 })));
            }
        }
        held = read.buttons;
    }
    // A button pressed and released within this frame stays held until the next, so a game that
    // asks `pressed` sees the tap as well as one that asks `just_pressed`: its last release waits.
    let deferred = pressed & !held;
    for (mask, button) in BUTTON_MAP {
        if deferred & mask != 0 {
            if let Some(last) = state.changes.iter_mut().rev().find(|change| **change == Some((button, 0.0))) {
                *last = None;
            }
        }
    }
    for &(button, value) in state.changes.iter().flatten() {
        let ev = RawGamepadButtonChangedEvent::new(gamepad, button, value);
        events.write(ev.into());
        button_events.write(ev);
    }
    state.last_buttons = held | deferred;
    state.deferred_releases = deferred;

    let sticks = latest.sticks;
    *RAW_PAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        RawPad { buttons: held | deferred, sticks, style_set: latest.style_set };

    // Analog stick normalization: clamp and scale [-32768, 32767] -> [-1.0, 1.0]
    let lx = (sticks[0][0] as f32 / 32767.0).clamp(-1.0, 1.0);
    let ly = (sticks[0][1] as f32 / 32767.0).clamp(-1.0, 1.0);
    let rx = (sticks[1][0] as f32 / 32767.0).clamp(-1.0, 1.0);
    let ry = (sticks[1][1] as f32 / 32767.0).clamp(-1.0, 1.0);

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
