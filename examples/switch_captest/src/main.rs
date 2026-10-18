//! switch_captest: checks, on a real Switch, what Bevy games need from the platform: libnx and
//! std (threads, time, files, sockets), the native crates IW4L and the Skate 3 engine use, Bevy's
//! executor, assets, audio and input, HOME and exit, and EGL/OpenGL ES for a renderer.
//! Results go to captest_results.txt next to the NRO. docs/testing.md explains the run.

#[cfg(target_os = "horizon")]
mod bevy_phase;
#[cfg(target_os = "horizon")]
mod files;
#[cfg(all(target_os = "horizon", feature = "gpu"))]
mod gpu;
#[cfg(target_os = "horizon")]
mod interactive;
#[cfg(target_os = "horizon")]
mod memory;
#[cfg(target_os = "horizon")]
mod native;
#[cfg(target_os = "horizon")]
mod net;
#[cfg(target_os = "horizon")]
mod nx;
#[cfg(target_os = "horizon")]
mod platform;
#[cfg(target_os = "horizon")]
mod report;
#[cfg(target_os = "horizon")]
mod threads;

#[cfg(not(target_os = "horizon"))]
fn main() {
    println!(
        "switch_captest only runs on a Switch: build it with scripts/build_example.sh switch_captest"
    );
}

#[cfg(target_os = "horizon")]
fn main() {
    use std::time::{Duration, Instant};

    bevy_horizon::init_switch_logging("switch_captest");
    nx::console_init();
    report::init();
    report::install_panic_hook();
    nx::install_applet_hook();
    report::screen(
        "\x1b[1;36mswitch_captest\x1b[0m: what Bevy games need from the Switch.\n\
         Results: captest_results.txt in this NRO's folder. Prompts time out on their own.\n",
    );
    // With the exit locked, closing from HOME makes appletMainLoop return false and gives the
    // app 15 s to save results and clear the crash marker, instead of being ended at once.
    let lock_rc = unsafe { nx::appletLockExit() };
    let locked = lock_rc == 0;
    report::check_warn(
        "exit-lock",
        locked,
        if locked {
            "appletLockExit: closing from HOME now asks the app to exit (15 s to finish)"
                .to_string()
        } else {
            format!("appletLockExit failed: {}", nx::rc_text(lock_rc))
        },
    );
    // Before the watchdog: spawning a thread registers std's first thread-local destructor.
    threads::tls_layout();
    report::start_watchdog();

    // Runs each phase unless HOME has asked the app to close. The applet loop runs in between,
    // so HOME and sleep messages are handled at least once per phase.
    let closed = std::cell::Cell::new(false);
    let phase = |name: &str, f: &dyn Fn()| {
        if closed.get() {
            return;
        }
        if !nx::applet_loop() {
            closed.set(true);
            report::info(
                "closed",
                format!("HOME asked the app to close before {name}; skipping the rest"),
            );
            return;
        }
        println!("[Captest] phase: {name}");
        f();
    };

    phase("system", &|| {
        platform::system_info();
        platform::random();
        platform::time();
        platform::cpu();
    });
    phase("threads", &|| {
        threads::threads();
        threads::core_policies();
    });
    phase("memory", &memory::memory);
    phase("sd card", &files::sd_card);
    phase("hands-on", &interactive::interactive);

    let mut app = None;
    phase("bevy setup", &|| {});
    if !closed.get() && !nx::exit_requested() {
        // An earlier run that died in the executor stage gets single-threaded schedules.
        let single = report::skipped("bevy-executor");
        if single {
            report::warn(
                "bevy-single-threaded",
                "bevy-executor crashed before: this run uses SingleThreadedExecutor everywhere",
            );
        }
        app = Some(bevy_phase::build_app(single));
    }
    // romfs is mounted and the task pools exist once the plugins are built.
    phase("romfs", &|| {
        files::romfs();
        files::install_assets();
        files::memmap();
    });
    phase("network", &net::network);
    phase("native crates", &native::native);
    phase("bevy pools", &native::bevy_pools);

    // run_app hands the App back instead of dropping it inside app.run(), so a drop that blocks
    // can't stop the GPU phase. It's dropped last, as its own test.
    let mut bevy_app = None;
    if let Some(app) = app.take()
        && !closed.get()
        && !nx::exit_requested()
    {
        let (exit, app) = bevy_phase::run_app(app);
        bevy_phase::after_run(&exit);
        bevy_app = Some(app);
    }

    #[cfg(feature = "gpu")]
    phase("gpu", &|| {
        // EGL needs the default window, which the text console holds.
        nx::console_exit();
        gpu::gpu();
        // Guarded too: if the console can't take the window back after EGL, the next run skips
        // this and finishes with the screen off (results still go to the file).
        if report::begin("gpu-console-back", 15) {
            let ok = nx::console_init();
            report::end();
            report::check(
                "gpu-console-back",
                ok,
                "the text console took the default window back after EGL",
            );
        }
        if nx::console_on() {
            gpu::ask_seen();
        }
    });

    if let Some(app) = bevy_app.take() {
        bevy_phase::drop_app(app);
    }

    let closed = closed.get() || nx::exit_requested();
    let note = if closed {
        "closed from HOME"
    } else {
        "finished"
    };
    report::finish(note);

    if !closed {
        report::screen(&format!(
            "\n\x1b[1;32mDone: {}\x1b[0m\nResults: {}\n\x1b[1;33m> Press + to exit\x1b[0m (exits by itself in 2 minutes)\n",
            report::summary(),
            report::path("captest_results.txt"),
        ));
        let mut pad = nx::Pad::new();
        let start = Instant::now();
        loop {
            if !interactive::pump(&mut pad, 0) {
                break;
            }
            if pad.down() & nx::BTN_PLUS != 0 || start.elapsed() > Duration::from_secs(120) {
                break;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    report::stop_watchdog();
    nx::console_exit();
    bevy_horizon::flush_switch_logs();
    if locked {
        unsafe { nx::appletUnlockExit() };
    }
}
