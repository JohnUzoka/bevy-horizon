//! The Bevy half: DefaultPlugins (no renderer, no window) plus HorizonPlugin, run through stages
//! that check the executor, frame pacing, assets, bevy_audio, the gamepad and HOME/resume. Each
//! stage is a guarded test (report::begin), so a crash in one is skipped on the next run.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bevy::app::{AppExit, PluginsState, ScheduleRunnerPlugin};
use bevy::asset::LoadState;
use bevy::asset::io::file::FileAssetReader;
use bevy::asset::io::{AssetSourceBuilder, AssetSourceId, ErasedAssetReader};
use bevy::audio::{
    AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings, Volume,
};
use bevy::ecs::message::MessageWriter;
use bevy::image::{CompressedImageFormats, ImageLoader};
use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;

use crate::interactive::Answer;
use crate::nx;
use crate::platform::{calibrate, spin};
use crate::report::{self, check, check_warn, fail, info, pass, skip};

const STAGES: [(&str, u32); 7] = [
    ("bevy-executor", 30),
    ("bevy-assets", 30),
    ("bevy-audio", 120),
    ("bevy-buttons", 420),
    ("bevy-sticks", 120),
    ("bevy-suspend", 150),
    ("bevy-exit", 150),
];

/// A reader rooted at romfs:/. FileAssetReader joins its path onto BEVY_ASSET_ROOT, and joining
/// "romfs:/" onto "sdmc:/..." gives "sdmc:/.../romfs:/" (neither counts as absolute), so the root
/// is swapped while the reader is made. AssetPlugin calls this once, on the main thread.
fn romfs_reader() -> Box<dyn ErasedAssetReader> {
    let saved = std::env::var_os("BEVY_ASSET_ROOT");
    unsafe { std::env::set_var("BEVY_ASSET_ROOT", "romfs:/") };
    let reader = FileAssetReader::new("");
    match saved {
        Some(v) => unsafe { std::env::set_var("BEVY_ASSET_ROOT", v) },
        None => unsafe { std::env::remove_var("BEVY_ASSET_ROOT") },
    }
    Box::new(reader)
}

pub fn build_app(single_threaded: bool) -> App {
    let mut app = App::new();
    app.register_asset_source(
        AssetSourceId::from("romfs"),
        AssetSourceBuilder::new(romfs_reader),
    );
    // Without bevy_window, DefaultPlugins already holds a ScheduleRunnerPlugin that loops with no
    // wait (so HorizonPlugin doesn't add its 60 Hz one): replace it, or the loop spins a core.
    // Bevy's own TaskPoolOptions are kept: patches/bevy_tasks sizes the pools from the core mask.
    let plugins =
        DefaultPlugins
            .build()
            .set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
                1.0 / 60.0,
            )));
    app.add_plugins(plugins);
    // bevy_image 0.19 only pre-registers ImageLoader; bevy_render registers the real one, with the
    // GPU's compressed formats (bevy_render texture/mod.rs:59). Without a renderer every image
    // load, even of a missing file, waits for it and stays Loading forever, with no error.
    app.register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()));
    app.add_plugins(bevy_horizon::HorizonPlugin);
    // The messages a game gets about HOME, as WindowPlugin would register them, and a primary
    // window for WindowFocused to name. With no renderer, the window is only an entity.
    app.add_message::<bevy_window::AppLifecycle>();
    app.add_message::<bevy_window::WindowFocused>();
    app.world_mut()
        .spawn((bevy_window::Window::default(), bevy_window::PrimaryWindow));
    if single_threaded {
        macro_rules! single {
            ($($label:expr),*) => {$(
                app.edit_schedule($label, |s| {
                    s.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
                });
            )*};
        }
        single!(
            Main,
            Startup,
            First,
            PreUpdate,
            Update,
            PostUpdate,
            Last,
            FixedUpdate
        );
    }
    app.insert_resource(Probe {
        iters: AtomicU64::new(calibrate(1)),
        ..default()
    });
    app.init_resource::<Run>();
    app.init_resource::<FixedCount>();
    app.add_systems(FixedUpdate, |mut c: ResMut<FixedCount>| c.0 += 1);
    app.add_systems(
        Update,
        (
            probe::<0>,
            probe::<1>,
            probe::<2>,
            (record_lifecycle, drive).chain(),
        ),
    );
    app
}

/// Keeps every AppLifecycle and WindowFocused message, in order, for the HOME stage.
fn record_lifecycle(
    mut run: ResMut<Run>,
    real: Res<Time<Real>>,
    mut focused: MessageReader<bevy_window::WindowFocused>,
    mut lifecycle: MessageReader<bevy_window::AppLifecycle>,
) {
    use bevy_window::AppLifecycle;
    let now = real.elapsed_secs_f64();
    for message in focused.read() {
        let name = if message.focused {
            "focused"
        } else {
            "unfocused"
        };
        println!("[Captest] Bevy message WindowFocused: {name} at {now:.2} s");
        run.messages.push(name);
    }
    for message in lifecycle.read() {
        let name = match message {
            AppLifecycle::Idle => "Idle",
            AppLifecycle::Running => "Running",
            AppLifecycle::WillSuspend => "WillSuspend",
            AppLifecycle::Suspended => "Suspended",
            AppLifecycle::WillResume => "WillResume",
        };
        println!("[Captest] Bevy message AppLifecycle::{name} at {now:.2} s");
        run.messages.push(name);
    }
}

/// True if `want` appears in `seen` in order, with anything in between.
fn in_order(seen: &[&str], want: &[&str]) -> bool {
    let mut seen = seen.iter();
    want.iter().all(|w| seen.any(|s| s == w))
}

#[derive(Resource, Default)]
struct Probe {
    on: AtomicBool,
    iters: AtomicU64,
    active: AtomicU32,
    max_active: AtomicU32,
    cores: AtomicU32,
}

/// Three systems that touch no shared data, so a multi-threaded executor can run them at once.
fn probe<const N: usize>(p: Res<Probe>) {
    if !p.on.load(Ordering::Relaxed) {
        return;
    }
    let now = p.active.fetch_add(1, Ordering::SeqCst) + 1;
    p.max_active.fetch_max(now, Ordering::SeqCst);
    p.cores.fetch_or(1 << nx::core_now(), Ordering::Relaxed);
    spin(p.iters.load(Ordering::Relaxed));
    p.active.fetch_sub(1, Ordering::SeqCst);
}

#[derive(Resource, Default)]
struct FixedCount(u32);

#[derive(Component)]
struct CaptestSound;

#[derive(Resource, Default)]
struct Run {
    stage: usize,
    started: bool,
    t0: f64,
    step: usize,
    step_t0: f64,
    prompted: bool,
    frames: u32,
    deltas: Vec<f32>,
    fixed0: u32,
    image: Option<Handle<Image>>,
    wav: Option<Handle<AudioSource>>,
    rom: Option<Handle<Image>>,
    missing: Option<Handle<Image>>,
    reported: [bool; 4],
    sound: Option<Entity>,
    sink_seen: bool,
    paused_ok: bool,
    wrong: u32,
    timeouts: u32,
    range: [f32; 8],
    max_len: f32,
    focus0: (u32, u32, u32),
    /// AppLifecycle and WindowFocused messages so far (`record_lifecycle`).
    messages: Vec<&'static str>,
    /// How many there were when the HOME prompt went up.
    messages0: usize,
    max_real: f32,
    max_virtual: f32,
    last_status: f64,
    last_heap_log: f64,
}

impl Run {
    fn next(&mut self, now: f64) {
        self.step += 1;
        self.step_t0 = now;
        self.prompted = false;
    }
    /// True the first time it's called in a step: print the step's prompt then.
    fn prompt(&mut self) -> bool {
        !std::mem::replace(&mut self.prompted, true)
    }
}

fn answer(pad: Option<&Gamepad>) -> Option<Answer> {
    let p = pad?;
    if p.just_pressed(GamepadButton::East) {
        Some(Answer::Yes)
    } else if p.just_pressed(GamepadButton::South) {
        Some(Answer::No)
    } else if p.just_pressed(GamepadButton::North) {
        Some(Answer::Unsure)
    } else {
        None
    }
}

fn ask_prompt(q: &str) {
    report::screen(&format!(
        "\n\x1b[1;33m> {q}\x1b[0m\n  A = yes   B = no   X = not sure   (30 s)\n"
    ));
}

/// Waits for an answer to the current step's question; true once answered or timed out.
fn await_answer(
    run: &mut Run,
    now: f64,
    pad: Option<&Gamepad>,
    id: &str,
    q: &str,
    what: &str,
) -> bool {
    if run.prompt() {
        ask_prompt(q);
    }
    let a = answer(pad).or_else(|| (now - run.step_t0 > 30.0).then_some(Answer::Timeout));
    match a {
        Some(a) => {
            crate::interactive::record_answer(id, a, what);
            true
        }
        None => false,
    }
}

const BUTTONS: [(&str, &str, GamepadButton); 16] = [
    ("btn-A", "A", GamepadButton::East),
    ("btn-B", "B", GamepadButton::South),
    ("btn-X", "X", GamepadButton::North),
    ("btn-Y", "Y", GamepadButton::West),
    ("btn-L", "L", GamepadButton::LeftTrigger),
    ("btn-R", "R", GamepadButton::RightTrigger),
    ("btn-ZL", "ZL", GamepadButton::LeftTrigger2),
    ("btn-ZR", "ZR", GamepadButton::RightTrigger2),
    ("btn-plus", "+ (plus)", GamepadButton::Start),
    ("btn-minus", "- (minus)", GamepadButton::Select),
    ("btn-dpad-up", "D-pad UP", GamepadButton::DPadUp),
    ("btn-dpad-down", "D-pad DOWN", GamepadButton::DPadDown),
    ("btn-dpad-left", "D-pad LEFT", GamepadButton::DPadLeft),
    ("btn-dpad-right", "D-pad RIGHT", GamepadButton::DPadRight),
    (
        "btn-lstick",
        "the LEFT stick (click it in)",
        GamepadButton::LeftThumb,
    ),
    (
        "btn-rstick",
        "the RIGHT stick (click it in)",
        GamepadButton::RightThumb,
    ),
];

#[allow(clippy::too_many_arguments)]
fn drive(
    mut run: ResMut<Run>,
    probe: Res<Probe>,
    fixed: Res<FixedCount>,
    real: Res<Time<Real>>,
    virt: Res<Time<Virtual>>,
    gamepads: Query<(Entity, &Gamepad, Option<&Name>)>,
    asset_server: Res<AssetServer>,
    images: Res<Assets<Image>>,
    sounds: Res<Assets<AudioSource>>,
    sinks: Query<&AudioSink>,
    alive: Query<(), With<CaptestSound>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    report::kick_frame(15);
    let run = &mut *run;
    let now = real.elapsed_secs_f64();
    run.max_real = run.max_real.max(real.delta_secs());
    run.max_virtual = run.max_virtual.max(virt.delta_secs());
    // log.txt only: shows whether the app's memory grows while each stage runs, and whether
    // frames still come after AppExit. "process" is svcGetInfo's used memory (stacks, code and
    // the whole heap region), which malloc's count leaves out.
    if now - run.last_heap_log >= 1.0 {
        run.last_heap_log = now;
        let mib = 1024.0 * 1024.0;
        println!(
            "[Captest] heap {:.1} MiB in use, process {:.1} MiB, at {now:.1} s, stage {}, step {}",
            nx::heap_in_use() as f64 / mib,
            nx::info(7, 0).unwrap_or(0) as f64 / mib,
            STAGES.get(run.stage).map_or("(all done)", |s| s.0),
            run.step
        );
    }
    if run.stage >= STAGES.len() {
        return;
    }
    if !run.started {
        let (id, limit) = STAGES[run.stage];
        if !report::begin(id, limit) {
            run.stage += 1;
            // A skipped bevy-exit must still end the app, or it would run on with nothing to do.
            if run.stage == STAGES.len() {
                exit.write(AppExit::Success);
            }
            return;
        }
        run.started = true;
        run.t0 = now;
        run.step = 0;
        run.step_t0 = now;
        run.prompted = false;
    }
    let pad = gamepads.iter().next().map(|(_, g, _)| g);
    let done = match run.stage {
        // ---- Executor, frame pacing, fixed timestep, the gamepad entity.
        0 => {
            if run.prompt() {
                report::heading("Bevy");
                probe.on.store(true, Ordering::SeqCst);
                run.fixed0 = fixed.0;
                run.frames = 0;
                run.deltas.clear();
            }
            run.frames += 1;
            if run.frames > 2 {
                run.deltas.push(real.delta_secs());
            }
            if run.frames < 182 {
                false
            } else {
                probe.on.store(false, Ordering::SeqCst);
                let secs = now - run.t0;
                let mut d = run.deltas.clone();
                d.sort_by(|a, b| a.total_cmp(b));
                let avg = d.len() as f64 / d.iter().map(|x| *x as f64).sum::<f64>().max(1e-6);
                let p95 = d[d.len() * 95 / 100] * 1e3;
                let worst = d.last().copied().unwrap_or(0.0) * 1e3;
                let slow = d.iter().filter(|x| **x > 0.020).count();
                check_warn(
                    "bevy-frame-pacing",
                    (57.0..63.0).contains(&avg) && slow <= 3,
                    format!(
                        "{avg:.1} fps over {} frames; 95% under {p95:.1} ms, worst {worst:.1} ms, {slow} over 20 ms",
                        d.len()
                    ),
                );
                let max_active = probe.max_active.load(Ordering::SeqCst);
                check_warn(
                    "bevy-parallel-systems",
                    max_active >= 2,
                    format!(
                        "up to {max_active} of 3 independent systems ran at once, on cores {:#05b}",
                        probe.cores.load(Ordering::SeqCst)
                    ),
                );
                let hz = (fixed.0 - run.fixed0) as f64 / secs.max(1e-6);
                check_warn(
                    "bevy-fixed-update",
                    (60.0..68.0).contains(&hz),
                    format!("FixedUpdate ran {hz:.1} times a second (64 expected)"),
                );
                let n = gamepads.iter().count();
                let desc: Vec<String> = gamepads
                    .iter()
                    .map(|(e, g, name)| {
                        format!(
                            "{e} {:?} {:04x?}:{:04x?}",
                            name.map(|n| n.as_str()),
                            g.vendor_id(),
                            g.product_id()
                        )
                    })
                    .collect();
                check(
                    "bevy-gamepad-entity",
                    n == 1,
                    format!("{n} Gamepad entities: {desc:?}"),
                );
                let startup: Vec<&str> = run
                    .messages
                    .iter()
                    .copied()
                    .filter(|m| !m.ends_with("focused"))
                    .take(2)
                    .collect();
                check(
                    "lifecycle-startup",
                    startup == ["WillResume", "Running"],
                    format!(
                        "first AppLifecycle messages: {} (winit sends WillResume, Running at startup)",
                        startup.join(", ")
                    ),
                );
                true
            }
        }
        // ---- Assets from the SD card and romfs, and a missing file.
        1 => {
            if run.prompt() {
                run.image = Some(asset_server.load("captest.png"));
                run.wav = Some(asset_server.load("captest.wav"));
                run.rom = Some(asset_server.load("romfs://captest.png"));
                run.missing = Some(asset_server.load("missing.png"));
                run.reported = [false; 4];
            }
            let ms = (now - run.t0) * 1e3;
            let states = [
                run.image.as_ref().map(|h| asset_server.load_state(h.id())),
                run.wav.as_ref().map(|h| asset_server.load_state(h.id())),
                run.rom.as_ref().map(|h| asset_server.load_state(h.id())),
                run.missing
                    .as_ref()
                    .map(|h| asset_server.load_state(h.id())),
            ];
            for (i, st) in states.iter().enumerate() {
                let Some(st) = st else { continue };
                if run.reported[i] || matches!(st, LoadState::NotLoaded | LoadState::Loading) {
                    continue;
                }
                run.reported[i] = true;
                let err = |st: &LoadState| match st {
                    LoadState::Failed(e) => e.to_string(),
                    other => format!("{other:?}"),
                };
                match i {
                    0 => match (st, run.image.as_ref().and_then(|h| images.get(h))) {
                        (LoadState::Loaded, Some(img)) => {
                            let px = img
                                .data
                                .as_ref()
                                .and_then(|d| d.get((20 * 64 + 10) * 4..(20 * 64 + 10) * 4 + 4))
                                .map(|p| p.to_vec());
                            check(
                                "bevy-asset-png",
                                img.width() == 64 && px.as_deref() == Some(&[40, 80, 128, 255][..]),
                                format!(
                                    "assets/captest.png from the SD card in {ms:.0} ms: {}x{}, pixel {px:?}",
                                    img.width(),
                                    img.height()
                                ),
                            );
                        }
                        _ => fail("bevy-asset-png", format!("assets/captest.png: {}", err(st))),
                    },
                    1 => match (st, run.wav.as_ref().and_then(|h| sounds.get(h))) {
                        (LoadState::Loaded, Some(s)) => {
                            check(
                                "bevy-asset-wav",
                                s.bytes.len() == 57644,
                                format!(
                                    "assets/captest.wav in {ms:.0} ms, {} bytes",
                                    s.bytes.len()
                                ),
                            );
                        }
                        _ => fail("bevy-asset-wav", format!("assets/captest.wav: {}", err(st))),
                    },
                    2 => match (st, run.rom.as_ref().and_then(|h| images.get(h))) {
                        (LoadState::Loaded, Some(img)) => {
                            check(
                                "bevy-asset-romfs",
                                img.width() == 64,
                                format!(
                                    "romfs://captest.png through a second asset source in {ms:.0} ms"
                                ),
                            );
                        }
                        _ => fail(
                            "bevy-asset-romfs",
                            format!("romfs://captest.png: {}", err(st)),
                        ),
                    },
                    _ => match st {
                        LoadState::Failed(e) => pass(
                            "bevy-asset-missing",
                            format!(
                                "a missing file fails cleanly: {}",
                                e.to_string().lines().next().unwrap_or("")
                            ),
                        ),
                        other => fail("bevy-asset-missing", format!("missing.png ended {other:?}")),
                    },
                };
            }
            if run.reported.iter().all(|r| *r) {
                true
            } else if ms > 10_000.0 {
                for (i, name) in [
                    "bevy-asset-png",
                    "bevy-asset-wav",
                    "bevy-asset-romfs",
                    "bevy-asset-missing",
                ]
                .iter()
                .enumerate()
                {
                    if !run.reported[i] {
                        fail(name, format!("still {:?} after 10 s", states[i]));
                    }
                }
                true
            } else {
                false
            }
        }
        // ---- bevy_audio: a one-shot that despawns itself, then a loop that pauses and stops.
        2 => {
            let since = now - run.step_t0;
            let sink = run.sound.and_then(|e| sinks.get(e).ok());
            match run.step {
                0 => {
                    let wav_ok = run.wav.as_ref().is_some_and(|h| sounds.get(h).is_some());
                    if !wav_ok {
                        skip(
                            "bevy-audio",
                            "captest.wav didn't load, so there's nothing to play",
                        );
                        run.step = 99;
                    } else {
                        report::screen(
                            "\n\x1b[1;33m> Listen for a short chime (bevy_audio).\x1b[0m\n",
                        );
                        run.sound = play(&mut commands, &run.wav, PlaybackSettings::DESPAWN);
                        run.sink_seen = false;
                        run.next(now);
                    }
                    run.step == 99
                }
                1 => {
                    if sink.is_some() {
                        pass(
                            "bevy-audio-sink",
                            format!(
                                "AudioSink attached {:.0} ms after spawning AudioPlayer",
                                since * 1e3
                            ),
                        );
                        run.sink_seen = true;
                        run.next(now);
                    } else if since > 5.0 {
                        fail(
                            "bevy-audio-sink",
                            "no AudioSink after 5 s: rodio couldn't open an output stream (log.txt says why)",
                        );
                        run.step = 3;
                        run.step_t0 = now;
                    }
                    false
                }
                2 => {
                    let gone = run.sound.is_none_or(|e| alive.get(e).is_err());
                    if gone {
                        check(
                            "bevy-audio-despawn",
                            since > 0.3,
                            format!(
                                "PlaybackMode::Despawn removed the entity {:.2} s after the sink appeared (the chime is 0.6 s)",
                                since
                            ),
                        );
                        run.next(now);
                    } else if since > 5.0 {
                        fail(
                            "bevy-audio-despawn",
                            "the one-shot entity was still there 5 s later",
                        );
                        run.next(now);
                    }
                    false
                }
                3 => {
                    if await_answer(
                        run,
                        now,
                        pad,
                        "bevy-audio-heard",
                        "Did you hear the chime?",
                        "the bevy_audio chime",
                    ) {
                        if !run.sink_seen {
                            return finish_stage(run);
                        }
                        report::screen(
                            "\n\x1b[1;33m> A repeating chime: it should pause once, play again, then stop.\x1b[0m\n",
                        );
                        run.sound = play(
                            &mut commands,
                            &run.wav,
                            PlaybackSettings::LOOP.with_volume(Volume::Linear(0.6)),
                        );
                        run.paused_ok = false;
                        run.next(now);
                    }
                    false
                }
                4 => {
                    if let Some(s) = sink {
                        if since > 1.5 && !s.is_paused() && !run.paused_ok {
                            s.pause();
                            run.paused_ok = s.is_paused();
                            run.step_t0 = now;
                            run.step = 5;
                        }
                    } else if since > 5.0 {
                        fail(
                            "bevy-audio-loop",
                            "the looping sound never got an AudioSink",
                        );
                        run.step = 99;
                    }
                    run.step == 99
                }
                5 => {
                    if since > 1.0 {
                        if let Some(s) = sink {
                            s.play();
                        }
                        check(
                            "bevy-audio-pause",
                            run.paused_ok,
                            "AudioSink::pause took effect and play() resumed",
                        );
                        run.next(now);
                    }
                    false
                }
                6 => {
                    if since > 1.5 {
                        if let Some(e) = run.sound.take() {
                            commands.entity(e).despawn();
                        }
                        run.next(now);
                    }
                    false
                }
                7 => await_answer(
                    run,
                    now,
                    pad,
                    "bevy-audio-loop",
                    "Did the repeating chime pause once, then stop when despawned?",
                    "looping, pausing and stopping",
                ),
                _ => true,
            }
        }
        // ---- Every button, through bevy_horizon's mapping into Bevy's GamepadButton.
        3 => {
            if run.step >= BUTTONS.len() {
                true
            } else {
                let (id, label, want) = BUTTONS[run.step];
                if run.prompt() {
                    report::screen(&format!(
                        "\n\x1b[1;33m> Press {label}\x1b[0m  ({}/16, 20 s)\n",
                        run.step + 1
                    ));
                    run.wrong = 0;
                }
                let pressed: Vec<GamepadButton> = pad
                    .map(|p| p.digital().get_just_pressed().copied().collect())
                    .unwrap_or_default();
                if pressed.contains(&want) {
                    pass(id, format!("{label} arrived as GamepadButton::{want:?}"));
                    run.timeouts = 0;
                    run.next(now);
                } else if !pressed.is_empty() {
                    run.wrong += 1;
                    info(
                        id,
                        format!("pressed while waiting for {label}: {pressed:?}"),
                    );
                    if run.wrong >= 3 {
                        fail(
                            id,
                            format!("{label} never arrived as {want:?}; got {pressed:?} instead"),
                        );
                        run.next(now);
                    }
                } else if now - run.step_t0 > 20.0 {
                    skip(id, format!("{label} not pressed within 20 s"));
                    run.timeouts += 1;
                    run.next(now);
                    if run.timeouts >= 2 {
                        skip(
                            "bevy-buttons",
                            "two prompts in a row went unanswered; skipping the rest",
                        );
                        run.step = BUTTONS.len();
                    }
                }
                false
            }
        }
        // ---- Sticks: direction, full range, centring.
        4 => {
            let (l, r) = pad
                .map(|p| (p.left_stick(), p.right_stick()))
                .unwrap_or_default();
            let since = now - run.step_t0;
            if now - run.last_status > 0.25 {
                run.last_status = now;
                report::status_line(&format!(
                    "  left ({:+.2}, {:+.2})   right ({:+.2}, {:+.2})",
                    l.x, l.y, r.x, r.y
                ));
            }
            match run.step {
                0 => {
                    if run.prompt() {
                        report::screen("\n\x1b[1;33m> Push the LEFT stick all the way UP\x1b[0m\n");
                    }
                    if l.y > 0.5 {
                        pass(
                            "stick-left-up",
                            format!("left stick up reads y = {:+.2}", l.y),
                        );
                        run.next(now);
                    } else if l.y < -0.5 {
                        fail(
                            "stick-left-up",
                            format!("left stick up reads y = {:+.2}: Y is inverted", l.y),
                        );
                        run.next(now);
                    } else if since > 15.0 {
                        skip("stick-left-up", "no input in 15 s");
                        run.step = 99;
                    }
                }
                1 => {
                    if run.prompt() {
                        report::screen(
                            "\n\x1b[1;33m> Push the RIGHT stick all the way RIGHT\x1b[0m\n",
                        );
                    }
                    if r.x > 0.5 {
                        pass(
                            "stick-right-right",
                            format!("right stick right reads x = {:+.2}", r.x),
                        );
                        run.next(now);
                    } else if r.x < -0.5 {
                        fail(
                            "stick-right-right",
                            format!("right stick right reads x = {:+.2}: X is inverted", r.x),
                        );
                        run.next(now);
                    } else if since > 15.0 {
                        skip("stick-right-right", "no input in 15 s");
                        run.step = 99;
                    }
                }
                2 => {
                    if run.prompt() {
                        report::screen(
                            "\n\x1b[1;33m> Spin BOTH sticks in big circles for 5 seconds\x1b[0m\n",
                        );
                        run.range = [0.0; 8];
                        run.max_len = 0.0;
                    }
                    for (i, v) in [l.x, l.y, r.x, r.y].into_iter().enumerate() {
                        run.range[i * 2] = run.range[i * 2].min(v);
                        run.range[i * 2 + 1] = run.range[i * 2 + 1].max(v);
                    }
                    run.max_len = run.max_len.max(l.length()).max(r.length());
                    if since > 5.0 {
                        let reach = run.range.iter().map(|v| v.abs()).fold(1.0f32, f32::min);
                        check_warn(
                            "stick-range",
                            reach >= 0.9,
                            format!(
                                "every direction reached at least {reach:.2} (range {:?})",
                                run.range.map(|v| (v * 100.0).round() / 100.0)
                            ),
                        );
                        info(
                            "stick-magnitude",
                            format!(
                                "longest stick vector {:.2} (above 1.0 means diagonals aren't clamped to a circle)",
                                run.max_len
                            ),
                        );
                        run.next(now);
                    }
                }
                3 => {
                    if run.prompt() {
                        report::screen("\n\x1b[1;33m> Let go of both sticks\x1b[0m\n");
                    }
                    if since > 2.0 {
                        let drift = l.length().max(r.length());
                        check_warn(
                            "stick-center",
                            drift < 0.08,
                            format!(
                                "released sticks read {drift:.3} (Bevy's default dead zone is 0.05)"
                            ),
                        );
                        run.step = 99;
                    }
                }
                _ => {}
            }
            run.step == 99
        }
        // ---- HOME and back: focus hooks, the time gap Bevy sees, audio afterwards.
        5 => {
            let since = now - run.step_t0;
            let focus = (
                nx::FOCUS_LOST.load(Ordering::SeqCst),
                nx::FOCUS_GAINED.load(Ordering::SeqCst),
                nx::RESUMED.load(Ordering::SeqCst),
            );
            match run.step {
                0 => {
                    if run.prompt() {
                        report::screen(
                            "\n\x1b[1;33m> Press HOME, wait 3 seconds, then go back into the app.\x1b[0m\n  (B skips; 60 s)\n",
                        );
                        run.focus0 = focus;
                        run.messages0 = run.messages.len();
                        run.max_real = 0.0;
                        run.max_virtual = 0.0;
                    }
                    let resumed = (focus.0 > run.focus0.0 && focus.1 > run.focus0.1)
                        || focus.2 > run.focus0.2
                        || run.max_real > 2.0;
                    if resumed {
                        run.next(now);
                    } else if pad.is_some_and(|p| p.just_pressed(GamepadButton::South))
                        || since > 60.0
                    {
                        skip("suspend", "HOME wasn't pressed");
                        run.step = 99;
                    }
                    run.step == 99
                }
                1 => {
                    if since > 0.5 {
                        let (lost, gained, resumed) = (
                            focus.0 - run.focus0.0,
                            focus.1 - run.focus0.1,
                            focus.2 - run.focus0.2,
                        );
                        // bevy_horizon puts the app in NoSuspend, so HOME sends FocusStateChanged
                        // both ways. Resume needs SetRestartMessageEnabled, which nothing sets.
                        check(
                            "suspend-hooks",
                            lost > 0 && gained > 0,
                            format!(
                                "libnx's focus hook: focus lost {lost}x, regained {gained}x; resumed {resumed}x"
                            ),
                        );
                        let seen = &run.messages[run.messages0.min(run.messages.len())..];
                        check(
                            "suspend-messages",
                            in_order(
                                seen,
                                &[
                                    "unfocused",
                                    "WillSuspend",
                                    "Suspended",
                                    "focused",
                                    "WillResume",
                                    "Running",
                                ],
                            ),
                            format!(
                                "Bevy got: {} (want unfocused, WillSuspend, Suspended, focused, WillResume, Running)",
                                if seen.is_empty() {
                                    "nothing".to_string()
                                } else {
                                    seen.join(", ")
                                }
                            ),
                        );
                        info(
                            "suspend-gap",
                            format!("longest real frame gap {:.2} s", run.max_real),
                        );
                        check(
                            "suspend-virtual-clamp",
                            run.max_virtual <= 0.26,
                            format!(
                                "game time (Time<Virtual>) advanced at most {:.3} s in one frame",
                                run.max_virtual
                            ),
                        );
                        run.sound = play(&mut commands, &run.wav, PlaybackSettings::DESPAWN);
                        run.next(now);
                    }
                    false
                }
                2 => {
                    since > 2.0
                        && await_answer(
                            run,
                            now,
                            pad,
                            "suspend-audio",
                            "Did a chime play after you came back?",
                            "audio after HOME",
                        )
                }
                _ => true,
            }
        }
        // ---- Exit through AppExit on +.
        6 => {
            if run.prompt() {
                report::screen(&format!(
                    "\n\x1b[1;32mBevy tests done: {}\x1b[0m\n\x1b[1;33m> Press + to leave the Bevy app (closes itself in 2 minutes)\x1b[0m\n",
                    report::summary()
                ));
            }
            let plus = pad.is_some_and(|p| p.just_pressed(GamepadButton::Start));
            if plus || now - run.step_t0 > 120.0 {
                info(
                    "bevy-exit",
                    if plus {
                        "+ pressed: sending AppExit"
                    } else {
                        "no + in 2 minutes: sending AppExit"
                    },
                );
                exit.write(AppExit::Success);
                run.stage = STAGES.len();
            }
            // The marker stays set until main sees run_app return.
            return;
        }
        _ => true,
    };
    if done {
        finish_stage(run);
    }
}

fn play(
    commands: &mut Commands,
    wav: &Option<Handle<AudioSource>>,
    settings: PlaybackSettings,
) -> Option<Entity> {
    let wav = wav.clone()?;
    Some(
        commands
            .spawn((AudioPlayer::new(wav), settings, CaptestSound))
            .id(),
    )
}

fn finish_stage(run: &mut Run) {
    report::end();
    run.stage += 1;
    run.started = false;
}

/// Called once run_app has returned.
pub fn after_run(exit: &AppExit) {
    report::stop_frame_watch();
    let running = report::running();
    if nx::exit_requested() {
        let during = if running.is_empty() {
            "between stages".to_string()
        } else {
            running
        };
        check(
            "bevy-exit-home",
            exit.is_success(),
            format!(
                "closed from HOME during {during}: bevy_horizon sent AppExit and the Bevy loop returned {exit:?}"
            ),
        );
    } else if !report::skipped("bevy-exit") {
        check(
            "bevy-exit",
            exit.is_success(),
            format!("AppExit ended the Bevy loop: {exit:?}"),
        );
    }
    report::end();
}

/// The loop of `ScheduleRunnerPlugin::run_loop` at 60 Hz (bevy_app 0.19.1 schedule_runner.rs:76-170),
/// but the App comes back. `App::run` moves the App into the runner and drops it there
/// (app.rs:192-194), so a drop that blocks would hang inside app.run() and stop captest before
/// the GPU phase. main drops the App last, in `drop_app`.
pub fn run_app(mut app: App) -> (AppExit, App) {
    while app.plugins_state() == PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    if app.plugins_state() != PluginsState::Cleaned {
        app.finish();
        app.cleanup();
    }
    let frame = Duration::from_secs_f64(1.0 / 60.0);
    loop {
        let start = Instant::now();
        app.update();
        if let Some(exit) = app.should_exit() {
            return (exit, app);
        }
        if let Some(rest) = frame.checked_sub(start.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

/// Drops the App: its resources, including bevy_audio's output, whose cpal stream stops its audio
/// thread, which closes the audio renderer; the close waits at most 1 s. With the SDL host before
/// it, this blocked forever in Ryujinx, so it is guarded and runs last.
pub fn drop_app(app: App) {
    // HOME is waiting for the app to close, or an earlier run hung here: the exit cleans up.
    if nx::exit_requested() || !report::begin("bevy-app-drop", 20) {
        std::mem::forget(app);
        return;
    }
    let t = Instant::now();
    drop(app);
    report::end();
    pass(
        "bevy-app-drop",
        format!(
            "dropping the App (its resources and the audio device) took {:.0} ms",
            t.elapsed().as_secs_f64() * 1000.0
        ),
    );
}
