//! Hands-on tests that need the person holding the Switch: sound through cpal, rumble, touch and
//! motion. They run before Bevy, on libnx directly, with prompts on screen. Every wait has a
//! timeout, so an unattended run skips them instead of stalling.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::nx::{self, Pad};
use crate::report::{self, check, check_warn, fail, info, pass, skip, warn};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Yes,
    No,
    Unsure,
    Timeout,
    Closing,
}

/// Runs the applet loop and reads the pad for `ms`, so HOME keeps working. False if HOME asked
/// the app to close.
pub fn pump(pad: &mut Pad, ms: u64) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        if !nx::applet_loop() {
            return false;
        }
        pad.update();
        if Instant::now() >= end {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Shows `question` and waits for A (yes), B (no) or X (not sure).
pub fn ask(pad: &mut Pad, question: &str, secs: u64) -> Answer {
    report::screen(&format!(
        "\n\x1b[1;33m> {question}\x1b[0m\n  A = yes   B = no   X = not sure   ({secs} s)\n"
    ));
    let start = Instant::now();
    pad.update();
    loop {
        if !pump(pad, 0) {
            return Answer::Closing;
        }
        let down = pad.down();
        if down & nx::BTN_A != 0 {
            return Answer::Yes;
        }
        if down & nx::BTN_B != 0 {
            return Answer::No;
        }
        if down & nx::BTN_X != 0 {
            return Answer::Unsure;
        }
        if start.elapsed().as_secs() >= secs {
            return Answer::Timeout;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Records a yes/no answer as PASS/FAIL, "not sure" as WARN and no answer as SKIP.
pub fn record_answer(id: &str, answer: Answer, what: &str) {
    match answer {
        Answer::Yes => pass(id, format!("{what}: you said yes")),
        Answer::No => fail(id, format!("{what}: you said no")),
        Answer::Unsure => warn(id, format!("{what}: you weren't sure")),
        Answer::Timeout => skip(id, format!("{what}: no answer")),
        Answer::Closing => skip(id, format!("{what}: the app was closed")),
    }
}

pub fn interactive() {
    report::heading("Hands-on tests");
    if !report::begin("interactive", 90) {
        return;
    }
    let mut pad = Pad::new();
    pump(&mut pad, 100);
    info(
        "pad-style",
        format!(
            "controller styles {:#x} (0x2 = handheld Joy-Con)",
            pad.style_set()
        ),
    );
    let go = ask(
        &mut pad,
        "Run the hands-on tests (sound, rumble, touch, motion)? About 2 minutes.",
        30,
    );
    report::end();
    if go != Answer::Yes {
        skip("interactive", format!("hands-on tests not run ({go:?})"));
        return;
    }
    for (id, limit, test) in [
        ("sound-cpal", 120, sound as fn(&mut Pad)),
        ("rumble", 60, rumble),
        ("touch", 60, touch),
        ("motion", 60, motion),
    ] {
        if nx::exit_requested() {
            return;
        }
        if report::begin(id, limit) {
            test(&mut pad);
            report::end();
        }
    }
}

struct ToneStats {
    callbacks: AtomicU32,
    frames: AtomicU64,
    max_gap_us: AtomicU64,
    last_tick: AtomicU64,
    core_mask: AtomicU32,
    bad_len: AtomicBool,
}

/// Plays `plan` (per-callback tone: frequency, left gain, right gain) through cpal.
fn play_tones(
    plan: Arc<Mutex<(f32, f32, f32)>>,
    stats: Arc<ToneStats>,
) -> Result<(cpal::Stream, cpal::StreamConfig), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let dev = host
        .default_output_device()
        .ok_or("no default output device")?;
    let supported = dev
        .default_output_config()
        .map_err(|e| format!("default_output_config: {e}"))?;
    let config: cpal::StreamConfig = supported.config();
    let rate = config.sample_rate as f32;
    let channels = config.channels as usize;
    let mut phase = 0f32;
    let s = stats.clone();
    let stream = dev
        .build_output_stream::<f32, _, _>(
            &config,
            move |data, _| {
                let now = nx::ticks();
                let prev = s.last_tick.swap(now, Ordering::Relaxed);
                if prev != 0 {
                    let gap = (now - prev) * 1_000_000 / nx::tick_freq();
                    s.max_gap_us.fetch_max(gap, Ordering::Relaxed);
                }
                s.callbacks.fetch_add(1, Ordering::Relaxed);
                s.core_mask.fetch_or(1 << nx::core_now(), Ordering::Relaxed);
                if channels == 0 || data.len() % channels != 0 {
                    s.bad_len.store(true, Ordering::Relaxed);
                    return;
                }
                s.frames
                    .fetch_add((data.len() / channels) as u64, Ordering::Relaxed);
                let (freq, left, right) = plan.lock().map(|p| *p).unwrap_or((0.0, 0.0, 0.0));
                for frame in data.chunks_mut(channels) {
                    let v = (phase * TAU).sin() * 0.25;
                    phase = (phase + freq / rate).fract();
                    for (c, out) in frame.iter_mut().enumerate() {
                        *out = v * if c == 0 { left } else { right };
                    }
                }
            },
            |e| eprintln!("[Captest] cpal stream error: {e}"),
            None,
        )
        .map_err(|e| format!("build_output_stream: {e}"))?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    Ok((stream, config))
}

fn sound(pad: &mut Pad) {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    info("audio-host", format!("cpal host {:?}", host.id()));
    match host.default_output_device() {
        Some(dev) => {
            let name = dev
                .description()
                .map(|d| format!("{d:?}"))
                .unwrap_or_else(|e| format!("({e})"));
            let configs: Vec<String> = dev
                .supported_output_configs()
                .map(|it| {
                    it.map(|c| {
                        format!(
                            "{}ch {}-{} Hz {:?} {:?}",
                            c.channels(),
                            c.min_sample_rate(),
                            c.max_sample_rate(),
                            c.sample_format(),
                            c.buffer_size()
                        )
                    })
                    .collect()
                })
                .unwrap_or_default();
            info("audio-device", format!("{name}; supports {configs:?}"));
        }
        None => return fail("sound-cpal", "cpal found no output device"),
    }
    report::screen(
        "\n\x1b[1;33m> Listen: a beep on the LEFT, then the RIGHT, then a higher beep on BOTH.\x1b[0m\n",
    );
    let plan = Arc::new(Mutex::new((0.0f32, 0.0f32, 0.0f32)));
    let stats = Arc::new(ToneStats {
        callbacks: AtomicU32::new(0),
        frames: AtomicU64::new(0),
        max_gap_us: AtomicU64::new(0),
        last_tick: AtomicU64::new(0),
        core_mask: AtomicU32::new(0),
        bad_len: AtomicBool::new(false),
    });
    let t = Instant::now();
    let (stream, config) = match play_tones(plan.clone(), stats.clone()) {
        Ok(s) => s,
        Err(e) => return fail("sound-cpal", e),
    };
    info(
        "audio-open",
        format!("stream open in {} ms: {config:?}", t.elapsed().as_millis()),
    );
    let t = Instant::now();
    let mut closing = false;
    for (step, ms) in [
        ((440.0, 1.0, 0.0), 1500),
        ((0.0, 0.0, 0.0), 300),
        ((440.0, 0.0, 1.0), 1500),
        ((0.0, 0.0, 0.0), 300),
        ((880.0, 1.0, 1.0), 1000),
    ] {
        if let Ok(mut p) = plan.lock() {
            *p = step;
        }
        if !pump(pad, ms) {
            closing = true;
            break;
        }
    }
    let secs = t.elapsed().as_secs_f64();
    drop(stream);
    let frames = stats.frames.load(Ordering::Relaxed);
    let callbacks = stats.callbacks.load(Ordering::Relaxed);
    let expected = config.sample_rate as f64 * secs;
    check(
        "audio-callbacks",
        callbacks > 10 && !stats.bad_len.load(Ordering::Relaxed),
        format!(
            "{callbacks} callbacks, {frames} frames in {secs:.1} s ({:.0}% of real time), longest gap {:.1} ms, audio thread on cores {:#05b}",
            frames as f64 * 100.0 / expected.max(1.0),
            stats.max_gap_us.load(Ordering::Relaxed) as f64 / 1e3,
            stats.core_mask.load(Ordering::Relaxed)
        ),
    );
    if closing {
        return;
    }
    let answer = ask(pad, "Did you hear left, then right, then both?", 30);
    record_answer(
        "sound-stereo",
        answer,
        "beeps on the left, right, then both speakers",
    );

    // Games open and close the device (menus, level loads): a second stream must work too.
    let stats2 = Arc::new(ToneStats {
        callbacks: AtomicU32::new(0),
        frames: AtomicU64::new(0),
        max_gap_us: AtomicU64::new(0),
        last_tick: AtomicU64::new(0),
        core_mask: AtomicU32::new(0),
        bad_len: AtomicBool::new(false),
    });
    match play_tones(Arc::new(Mutex::new((660.0, 0.5, 0.5))), stats2.clone()) {
        Ok((s, _)) => {
            pump(pad, 500);
            drop(s);
            check(
                "audio-reopen",
                stats2.callbacks.load(Ordering::Relaxed) > 2,
                "a second stream after closing the first one plays",
            );
        }
        Err(e) => fail("audio-reopen", e),
    }
}

fn rumble(pad: &mut Pad) {
    let mut handles = [0u32; 2];
    let rc = unsafe {
        nx::hidInitializeVibrationDevices(
            handles.as_mut_ptr(),
            2,
            nx::NPAD_ID_HANDHELD,
            nx::STYLE_HANDHELD,
        )
    };
    if rc != 0 {
        return fail(
            "rumble",
            format!("hidInitializeVibrationDevices: {}", nx::rc_text(rc)),
        );
    }
    // The Switch Lite has no rumble motors: send the values anyway, but don't ask.
    let model = nx::product_model();
    let lite = model == Ok(nx::PRODUCT_MODEL_HOAG);
    report::screen(if lite {
        "\n\x1b[1;33m> Switch Lite: no rumble motors, so only the API is tested.\x1b[0m\n"
    } else {
        "\n\x1b[1;33m> Hold the Switch: both Joy-Con should rumble for 1 second.\x1b[0m\n"
    });
    pump(pad, 800);
    let on = nx::VibrationValue {
        amp_low: 0.5,
        freq_low: 160.0,
        amp_high: 0.5,
        freq_high: 320.0,
    };
    let off = nx::VibrationValue {
        amp_low: 0.0,
        freq_low: 160.0,
        amp_high: 0.0,
        freq_high: 320.0,
    };
    let rc_on = unsafe { nx::hidSendVibrationValues(handles.as_ptr(), [on, on].as_ptr(), 2) };
    // Values decay unless re-sent, so keep sending for the whole second.
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1) {
        unsafe { nx::hidSendVibrationValues(handles.as_ptr(), [on, on].as_ptr(), 2) };
        if !pump(pad, 50) {
            break;
        }
    }
    let rc_off = unsafe { nx::hidSendVibrationValues(handles.as_ptr(), [off, off].as_ptr(), 2) };
    check(
        "rumble-api",
        rc_on == 0 && rc_off == 0,
        format!(
            "hidSendVibrationValues on {}, off {}",
            nx::rc_text(rc_on),
            nx::rc_text(rc_off)
        ),
    );
    if lite {
        return skip(
            "rumble-felt",
            "a Switch Lite (product model Hoag) has no rumble motors",
        );
    }
    if let Err(rc) = model {
        info("rumble-model", format!("set:sys product model: {}", nx::rc_text(rc)));
    }
    let answer = ask(pad, "Did both Joy-Con rumble, and stop?", 30);
    record_answer("rumble-felt", answer, "rumble on both Joy-Con");
}

fn touch(pad: &mut Pad) {
    unsafe { nx::hidInitializeTouchScreen() };
    report::screen(
        "\n\x1b[1;33m> Touch the screen: one finger, then two, and drag into the corners (8 s).\x1b[0m\n",
    );
    let (mut max_fingers, mut samples) = (0i32, 0u32);
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (u32::MAX, 0u32, u32::MAX, 0u32);
    let mut last_sampling = 0u64;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        if !pump(pad, 0) {
            return;
        }
        let mut st = nx::TouchScreenState::default();
        if unsafe { nx::hidGetTouchScreenStates(&mut st, 1) } == 1
            && st.sampling_number != last_sampling
        {
            last_sampling = st.sampling_number;
            samples += 1;
            let n = st.count.clamp(0, 16);
            max_fingers = max_fingers.max(n);
            for t in &st.touches[..n as usize] {
                min_x = min_x.min(t.x);
                max_x = max_x.max(t.x);
                min_y = min_y.min(t.y);
                max_y = max_y.max(t.y);
            }
            if samples % 12 == 0 {
                report::status_line(&format!(
                    "  fingers {n}, x {min_x}..{max_x}, y {min_y}..{max_y}"
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    report::screen("\n");
    if max_fingers == 0 {
        return skip(
            "touch",
            format!("no touches in 8 s ({samples} samples read)"),
        );
    }
    check(
        "touch",
        max_x < 1280 && max_y < 720,
        format!(
            "up to {max_fingers} fingers; x {min_x}..{max_x} of 1280, y {min_y}..{max_y} of 720"
        ),
    );
    check_warn(
        "touch-multi",
        max_fingers >= 2,
        format!("{max_fingers} fingers at once"),
    );
}

fn motion(pad: &mut Pad) {
    let mut handle = 0u32;
    let rc = unsafe {
        nx::hidGetSixAxisSensorHandles(&mut handle, 1, nx::NPAD_ID_HANDHELD, nx::STYLE_HANDHELD)
    };
    if rc != 0 {
        return fail(
            "motion",
            format!("hidGetSixAxisSensorHandles: {}", nx::rc_text(rc)),
        );
    }
    let rc = unsafe { nx::hidStartSixAxisSensor(handle) };
    if rc != 0 {
        return fail(
            "motion",
            format!("hidStartSixAxisSensor: {}", nx::rc_text(rc)),
        );
    }
    let read = || {
        let mut st = nx::SixAxisState::default();
        (unsafe { nx::hidGetSixAxisSensorStates(handle, &mut st, 1) } == 1).then_some(st)
    };
    report::screen("\n\x1b[1;33m> Hold the Switch still for 2 seconds...\x1b[0m\n");
    pump(pad, 500);
    let (mut sum, mut n) = (0f32, 0u32);
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(1500) {
        if let Some(st) = read() {
            sum += st.acceleration.len();
            n += 1;
        }
        if !pump(pad, 16) {
            break;
        }
    }
    let g = sum / n.max(1) as f32;
    check(
        "motion-accel",
        n > 0 && (0.85..1.15).contains(&g),
        format!("at rest |acceleration| = {g:.3} g over {n} samples (expected ~1)"),
    );
    report::screen("\x1b[1;33m> Now tilt and turn the Switch for 4 seconds.\x1b[0m\n");
    let mut max_rate = 0f32;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(4) {
        if let Some(st) = read() {
            max_rate = max_rate.max(st.angular_velocity.len());
        }
        if !pump(pad, 16) {
            break;
        }
    }
    unsafe { nx::hidStopSixAxisSensor(handle) };
    // HID reports angular velocity in turns per second (1.0 = 360 deg/s).
    if max_rate < 0.05 {
        skip(
            "motion-gyro",
            format!("no rotation seen (peak {:.0} deg/s)", max_rate * 360.0),
        );
    } else {
        pass(
            "motion-gyro",
            format!("peak rotation {:.0} deg/s", max_rate * 360.0),
        );
    }
}
