# How a bevy_horizon program starts, runs and exits

This page follows a `bevy_horizon` program through its life on the Switch: startup, each frame, HOME, and every way it can end. Read it when you decide where your game pauses, saves and releases what it opened, or when a log ends somewhere you didn't expect. [Inside the exit path](#inside-the-exit-path) is for contributors, and for reading an exit log line by line. For symptoms and fixes, see [troubleshooting.md](troubleshooting.md).

## In short

- **Before `main`, `bevy_horizon` makes Rust work.** libnx calls its [`userAppInit`](glossary.md#userappinit) hook, which sets up Rust's thread-locals and opens `log.txt`; you write nothing for it.
- **In `main`, call `init_switch_logging`, set any environment variables, then add `DefaultPlugins` and then `HorizonPlugin`,** which handles RomFS, sockets, HOME, the controller, the dock state, the screen and the 60 Hz loop.
- **HOME arrives as Bevy's usual `WindowFocused` and `AppLifecycle` messages,** with two frames to react before the system suspends the game. See [HOME and focus](#home-and-focus).
- **Quit by sending `AppExit` and returning from `main`.** The App, with its GPU device and audio stream, is dropped inside `app.run()`. `std::process::exit` skips that drop, so a game that draws ends on HOME instead of the homebrew menu. Never call `svcExitProcess`.
- **Save before you send `AppExit`, and on `AppLifecycle::WillSuspend`:** the World is gone once `app.run()` returns, and closing the game from the HOME menu runs none of your code.
- **Release what you open, and join your threads.** Anything on [transfer memory](glossary.md#transfer-memory), or a thread still running, keeps heap pages locked at exit. See [Rules for services you add later](#rules-for-services-you-add-later).
- **The last `[Exit]` line in `log.txt` says how the exit went:** after `[Exit] PASS` the console goes back to the homebrew menu; after `[Exit] WARN` something still held heap pages, and it goes to the [HOME menu](glossary.md#home-menu) instead.

## Startup, in order

A call made too early can fail before there is a screen, a panic hook, or even thread-local storage. [A good startup](#a-good-startup) shows what each step logs.

### Before `main`: `userAppInit`

The homebrew [loader](glossary.md#loader) jumps into the [NRO](glossary.md#nro)'s startup code, where [libnx](glossary.md#libnx) sets up [newlib](glossary.md#newlib), the [heap](glossary.md#heap), its default [services](glossary.md#service) and the SD card, then calls `userAppInit`, before any C constructor or Rust code runs. `bevy_horizon`'s hook:

1. **Sets the thread pointer.** It copies libnx's thread pointer into [`TPIDR_EL0`](glossary.md#tpidr_el0), where Rust looks for [thread-local storage](glossary.md#thread-local-storage); without it, every `std` program would crash before `main`. A wrapper around libnx's `threadCreate` does the same for later threads.
2. **Opens the log** at `sdmc:/switch/<name>/log.txt`, where `<name>` is the NRO's file name without `.nro` (or at `sdmc:/log.txt` if that folder can't be made), points stdout and stderr at it, and writes the boot report.
3. **Sets `BEVY_ASSET_ROOT`** to the NRO's folder, unless it is already set, so Bevy loads assets from `assets/` beside the NRO. Otherwise Bevy's `AssetPlugin` asks for `current_exe`, which Rust doesn't have on Horizon, and panics.

### In `main`: logging first, then the App

Set any environment variables first, with `std::env::set_var`, while no other thread can read the environment: `HorizonPlugin`, `DefaultPlugins` and the drivers they start read them ([porting-a-bevy-game.md](porting-a-bevy-game.md#reference-environment-variables) lists them). Then call `init_switch_logging`. It sets `NO_COLOR=1`, so log lines carry no colour codes, starts the log writer thread, and installs the panic hook that writes `panic.txt`. If `userAppInit` couldn't open the log, it opens `sdmc:/switch/<app_name>/log.txt`. Add `HorizonPlugin` after `DefaultPlugins`:

```rust
fn main() {
    #[cfg(target_os = "horizon")]
    {
        // Optional: environment variables, before any other thread starts.
        unsafe { std::env::set_var("BEVY_HORIZON_MEMORY_EVERY", "0") };
        bevy_horizon::init_switch_logging("my_game");
    }

    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    #[cfg(target_os = "horizon")]
    app.add_plugins(bevy_horizon::HorizonPlugin);
    // Your plugins and systems.
    let exit = app.run();
    println!("[Exit] app.run() returned {exit:?}");
}
```

### What `HorizonPlugin` adds

| Part | What it does |
|---|---|
| `HorizonLifecyclePlugin` | Mounts the NRO's [RomFS](glossary.md#romfs) as `romfs:`, sets focus handling so HOME is reported, and starts sockets, which lock 2.20 MiB of the heap until exit. Each frame: applet messages, frame timing, the heartbeat, syncing `log.txt` |
| `HorizonInputPlugin` | Reads the controller into one Bevy gamepad |
| `HorizonDisplayPlugin` | Follows the dock state in the `HorizonDisplayMode` resource |
| `HorizonWindowPlugin` | Attaches the primary window to libnx's default [NWindow](glossary.md#nwindow) |
| `HorizonUploadPlugin` | With the `render` or `render_0_18` feature: [paces big GPU uploads](platform-notes.md#pacing-uploads-the-render-feature) |
| `ScheduleRunnerPlugin::run_loop(1/60 s)` | Unless one is already added: runs `App::update`, then sleeps out the rest of 16.67 ms |

Something has to run the frames: winit, which does on desktop, has no Switch backend, and with no runner `App::run` runs one update and returns. With `bevy_window` off, `DefaultPlugins` adds a [`ScheduleRunnerPlugin`](glossary.md#schedulerunnerplugin) that never waits, so `HorizonPlugin` adds none and a CPU core stays busy. Set it to 60 Hz with `DefaultPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 60.0)))`, as [captest](glossary.md#captest)'s `build_app` does, and add `HorizonPlugin` after it: added before, `HorizonPlugin` adds its own runner, and Bevy panics on the duplicate.

## Every frame

`bevy_horizon`'s systems run at fixed points in Bevy's `Main` schedule. Bevy's render sub-app runs after it, and the runner then sleeps out the rest of the 1/60 s.

| Schedule | System | What it does |
|---|---|---|
| `FrameStart` and `FrameEnd`, around `First` to `Last` | frame timer | Times the `Main` schedule |
| `First` | `attach_default_nwindow` | Attaches the screen, once |
| `PreUpdate` | `check_applet_loop` | Applet messages: exit requests, HOME and focus |
| `PreUpdate`, before `InputSystems` | `poll_horizon_input` | Reads the controller |
| `PreUpdate` | `monitor_dock_status` | Follows the dock state |
| `PostUpdate` | `flush_logs_system` | Heartbeat, closing exited threads, syncing `log.txt` |

### Applet messages and exit requests

`check_applet_loop` handles up to 16 waiting [applet](glossary.md#applet) messages a frame. When the system asks the app to exit (`ExitRequest`), it logs `Switch applet requested exit; shutting down Bevy app.`, syncs `log.txt` and sends `AppExit::Success`.

Closing the game from the HOME menu sends `ExitRequest` only to a game that holds an [exit lock](glossary.md#exit-lock) (`appletLockExit`, which `switch_captest` takes and `bevy_horizon` doesn't); otherwise the system just ends the process. With a lock, an app suspended behind HOME first logs `[Applet] the system asked the app to exit ... s into the background`. Neither case has been tested on the console.

### HOME and focus

On HOME or sleep, `bevy_horizon` sends the messages bevy_winit sends on other platforms, lets your game run two more frames, then has the system suspend it until the player is back:

| Frame | What Bevy gets |
|---|---|
| The first two frames | `AppLifecycle::WillResume`, then `Running`, as winit sends at startup |
| N: HOME pressed | `WindowFocused { focused: false }` (and `Window::focused` false), then `AppLifecycle::WillSuspend` |
| N+1 | `AppLifecycle::Suspended`, the last update before the suspension |
| N+2 | Suspended in `PreUpdate` until the player is back; then `WindowFocused { focused: true }` and `AppLifecycle::WillResume` |
| N+3 | `AppLifecycle::Running` |

**In your game,** pause on `WillSuspend` or `Suspended`, and resume on `WillResume` or `Running`; a game that handles `AppLifecycle` for Android or iOS needs no change. Save on `WillSuspend` if progress matters, because the player may close the game from the HOME menu. For example:

```rust
use bevy::prelude::*;
use bevy::window::AppLifecycle;

fn pause_for_home(mut lifecycle: MessageReader<AppLifecycle>, mut time: ResMut<Time<Virtual>>) {
    for event in lifecycle.read() {
        match event {
            AppLifecycle::WillSuspend => time.pause(), // and save here
            AppLifecycle::WillResume => time.unpause(),
            _ => {}
        }
    }
}
```

- **The messages need Bevy's `WindowPlugin`.** Without it, the game is suspended without notice, unless you add them with `add_message`, and a `PrimaryWindow` for `WindowFocused`, as captest's `build_app` does.
- **A library applet over the game,** such as the software keyboard, only sends `WindowFocused { focused: false }`; the game keeps running.
- **Game time jumps by at most 250 ms.** `Time<Virtual>`, and so `FixedUpdate`, clamps the time away to Bevy's default maximum.
- **In [applet mode](glossary.md#applet-mode)** (started from the Album, for example), focus handling can't be changed, so HOME isn't reported. The log says `[Applet] focus handling left as it was (...)`.

HOME and back logs this:

```text
[Applet] focus: in focus -> in the background (HOME menu or sleep)
[Applet] AppLifecycle::WillSuspend
[Applet] AppLifecycle::Suspended
[Applet] the last frame before the background has run; switching to SuspendHomeSleepNotify so the system suspends the app
[Applet] in focus again ... s later: ... s inside SetFocusHandlingMode, then ... wait(s) for the focus message
[Applet] focus: in the background (HOME menu or sleep) -> in focus
[Applet] AppLifecycle::WillResume
[Applet] AppLifecycle::Running
```

**How it works.** In libnx's default [focus-handling mode](glossary.md#focus-handling-mode), and on the console in `SuspendHomeSleepNotify` too, HOME suspends the app without telling it, while the system still reports it in [focus](glossary.md#focus). Only `NoSuspend` reports the background. So `bevy_horizon` runs in `NoSuspend`, turns `FocusStateChanged` into Bevy's messages, and after the last frame switches to `SuspendHomeSleepNotify` so the system suspends the app, then switches back. The pattern comes from switchbrew's deko3d samples, the message order from bevy_winit ([CREDITS.md](../CREDITS.md)).

If focus comes back within 100 ms of that switch, the system didn't suspend the app, which hasn't been seen. `bevy_horizon` then logs a WARN with `too soon for anyone to have come back from HOME`, and from then on waits without running frames.

### Input before Bevy input systems

`poll_horizon_input` reads [HID](glossary.md#hid) in `PreUpdate`, before Bevy's `InputSystems`, as bevy_gilrs does on desktop, so a press reaches Bevy's gamepad state in the same frame. The first poll logs `[Input] HID configured: 1 player, standard controllers; reading player 1 and handheld`, and each controller change logs `[Input] controller styles now ...`. The mapping is in [input-and-audio.md](input-and-audio.md).

### Dock-status check

`monitor_dock_status` checks libnx's operation mode. When it changes, it updates `HorizonDisplayMode`, resizes the primary window to 1920×1080 docked or 1280×720 handheld, and logs `Switch display mode changed to Docked: 1920x1080` (or `Handheld: 1280x720`). [Docked mode](glossary.md#docked-mode) is untested: a Switch Lite can't dock.

### Frame timing and the heartbeat

`flush_logs_system` goes by time, not frames:

- **Every second,** it closes detached threads that have exited, so their stacks go back to the heap, and has `log.txt` synced ([the cost of flushing](logging-and-crash-reports.md#flushing-and-the-cost-of-flushing)).
- **Every 10 seconds,** it writes the [heartbeat](logging-and-crash-reports.md#heartbeat-lines): `[Heartbeat] frame 600, 10.0 s after the first frame, 60.0 fps on average`, then the frame timer's work per frame. The frame timer covers only the `Main` schedule, not the render sub-app, the runner's sleep or time suspended.
- **Every 60 seconds by default,** it adds a `[Memory]` line from newlib's `mallinfo`, which holds malloc's lock while it walks the free blocks: about 41 ms in a full 3D game. `BEVY_HORIZON_MEMORY_EVERY` sets the interval in seconds, or `0` for none.

## Every way the program can end

| How it ends | `userAppExit` runs? | The console goes to | Read |
|---|---|---|---|
| **Your game sends `AppExit`** and `main` returns | Yes | The homebrew menu after `[Exit] PASS`, HOME after `[Exit] WARN` | The last `[Exit]` lines |
| **The system asks the app to exit** (`ExitRequest`) | Yes: `bevy_horizon` sends `AppExit` | As above | `Switch applet requested exit; ...`, then the `[Exit]` lines |
| **`std::process::exit`** | Yes, after the `atexit` handlers, which close the audio | HOME, in a game that draws: the GPU device is never dropped | `log.txt` |
| **A Rust [panic](glossary.md#panic)** | Yes, but without `atexit` handlers: the target aborts on panic | HOME, in a game that draws or has Bevy's audio: neither is closed | `panic.txt`, `log.txt` |
| **Closing from the HOME menu,** with no exit lock | No: the system ends the process | HOME | `log.txt`, synced before the suspension |
| **A CPU exception** | No | HOME, with an Atmosphère [crash report](glossary.md#crash-report) | `crash.log`, the crash report |
| **A libnx fatal error** (`diagAbortWithResult`) | No | HOME, with a crash report | `abort.log`, the crash report |
| **The loader crashes after exit** | It already ran | HOME, with a crash report in `hbl` | [A loader crash after exit](#a-loader-crash-after-exit) |

[logging-and-crash-reports.md](logging-and-crash-reports.md) describes these files. **Never call `svcExitProcess` yourself:** it ends the process without telling the system's applet manager ([AM](glossary.md#am)), which may then report an error. When `bevy_horizon` must end the process, it asks AM.

## Quitting: the App drop

`App::run` hands the App to the runner (`bevy_app-0.19.1/src/app.rs:192-194`), whose loop drops it on `AppExit`, still inside `run()`. Everything the App owns drops with it:

- **The GPU device.** When the last Vulkan device goes, Mesa closes [nvdrv](glossary.md#nv), the GPU's service, and the GPU's memory goes back to the heap.
- **The audio stream, on Bevy 0.19,** kept in bevy_audio's `AudioOutput` resource (`bevy_audio-0.19.1/src/audio_output.rs:16-31`). The [cpal](glossary.md#cpal) host closes it as [the audio close at exit](#the-audio-close-at-exit) shows. Bevy 0.18 leaks its stream on purpose (`bevy_audio-0.18.1/src/audio_output.rs:33-35`), so it is closed at exit instead.

So save before you send `AppExit`, while the World exists; `app.run()` returns only after the drop. captest's `run_app` and `drop_app` time the drop on its own, under a [watchdog](glossary.md#watchdog) (`examples/switch_captest/src/bevy_phase.rs`).

### The audio close at exit

An open stream keeps [audren](glossary.md#audren)'s transfer memory, which sends the exit through AM to HOME. Bevy 0.18 never drops its stream, and `std::process::exit` skips the App drop, so both cpal hosts register an `atexit` handler, `close_at_exit`, when the first stream opens. It runs after `main` returns, before the exit hook, and closes any stream still open the way `Stream::drop` does:

```text
Stream::drop (the App drop)  or  close_at_exit (atexit)
└─ Shared::close         sets `stop`, waits up to 1 s (CLOSE_WAIT), joins the audio thread
   └─ the audio thread   sees `stop` within 10 ms (FRAME_WAIT_NS)
      └─ Output drop     audrvClose, audrenExit, frees the buffers
```

For each stream it closes at exit, it logs one of these:

```text
[Audio] exit: closed the audio renderer in 19.0 ms, which gives audren its memory back
[Audio] exit: the audio thread didn't stop within 1000 ms; audren keeps its memory
```

The second, for example with a callback stuck in your code, sends the exit through AM; in the App drop the same case logs `[Audio] the audio thread didn't stop within 1000 ms; audren stays open`. If the handler couldn't be registered, the first stream logs `[Audio] atexit failed: the renderer stays open at exit`. A panic skips the close, because an abort doesn't run `atexit` handlers.

On the console, the close took 19 ms in a full 3D Bevy 0.18 game, whose exit passed. The Bevy 0.19 host (`patches/cpal`, the same code on a newer cpal) is tested only in the emulator.

## Rules for services you add later

When the NRO exits, the homebrew loader reads the next NRO into the same heap, which fails while any heap page is locked, so `bevy_horizon` exits to HOME instead. Before your game opens anything that locks heap pages, decide how it gets released:

| What | What locks heap pages | Who releases it |
|---|---|---|
| Sockets ([BSD](glossary.md#bsd)) | 2.20 MiB of transfer memory | `bevy_horizon`'s exit hook (`socketExit`) |
| GPU (nvdrv) | 8 MiB of transfer memory, and every NVK allocation ([GPU memory](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap)) | Mesa (`nvExit`), when the App drop destroys the last Vulkan device; otherwise the hook exits through AM ([step 7](#the-userappexit-hook)) |
| Audio (audren) | Its work buffer, in transfer memory | The cpal host (`audrenExit`), when a stream closes or at exit ([the audio host](input-and-audio.md#the-audio-host)) |
| The six-axis sensor | `hidInitializeSevenSixAxisSensor`'s transfer memory | You: `hidFinalizeSevenSixAxisSensor`. Ordinary input doesn't use it |
| Threads you spawn | Each thread's stack, 2 MiB by default | You: join them before `main` returns. Detached threads are closed once they exit |
| Bevy's [task pools](glossary.md#task-pool), with `multi_threaded` | Their threads' stacks | The exit hook, within 500 ms, through the patched `bevy_tasks` ([the crate patches](toolchain.md#the-crate-patches)); a task that never yields keeps its thread |
| Anything else on transfer memory (`tmemCreate`), such as libnx's console | Its transfer memory | You: its exit function, such as `consoleExit`, before `main` returns, or a release hook |

For anything you can't close before `main` returns, register a release hook. The exit hook runs it after the task pools stop, before `bevy_horizon` closes its own services and checks the heap:

```rust
fn close_my_service() {
    // Call the service's exit function here.
}

// After opening the service:
if !bevy_horizon::register_release_hook(close_my_service) {
    // All 8 slots are taken: close the service before main returns instead.
}
```

Hooks run newest first, once each, and each logs `[Exit] release hook N: done`; on other platforms they never run. Keep them quick, don't allocate in them ([why](#why-it-allocates-nothing)), and never panic. They don't run when `userAppExit` doesn't, but then the system ends the process and closes everything.

## Logs to recognize

Lines logged through `tracing` also carry Bevy's level and module prefix, left out here; `...` stands for values or lines that vary.

### A good startup

hello_switch's log:

```text
==== bevy_horizon log: hello_switch ====
...
[Boot] launched from sdmc:/switch/hello_switch/hello_switch.nro
...
[Boot] TLS template at nro+0x..., align 8: Rust and libnx agree on thread-local addresses
[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
...
[Boot] next: C constructors, Rust's runtime setup (std::rt::lang_start), then main
[Main] init_switch_logging("hello_switch"): Rust's runtime is up and main is running
...
romfs initialized successfully
[Applet] focus handling: NoSuspend, so HOME sends FocusStateChanged; the app is suspended after its last frame
BSD socket layer initialized successfully
...
primary window attached to libnx's default NWindow
```

No `[Boot]` line means `userAppInit` didn't run or couldn't open the log. A log that stops at `[Boot] next:` means a crash before `main`; look for `crash.log` or `abort.log` beside it. `MISALIGNED` in the TLS line means a broken build ([platform-notes.md](platform-notes.md)). Applet type 2, `LibraryApplet`, means applet mode, with far less memory. `romfsMountSelf: 0x1559 = 2345-0010 (libnx IoError): this NRO has no RomFS, ...` is harmless unless you meant to pack one.

### A clean exit

hello_switch, quit with (+):

```text
[Exit] app.run() returned Success
[Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
[Exit] focus handling back to SuspendHomeSleep: done
[Exit] heap check, before releasing services: ... would fail the loader's read:
[Exit]   0x...-0x..., 2.20 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, as an open transfer memory or a thread stack leaves it
...
[Exit] Bevy's task pools: no threads of their own (bevy_tasks without multi_threaded runs tasks on the main thread)
[Exit] detached threads: closed ... now and ... while the app ran (... threads were created in all); 0 still running after ... ms
[Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
[Exit] romfsUnmount: done
[Exit] nvdrv: already closed (the GPU driver released it), so there is nothing for nvExit to do
[Exit] the heap was unlocked ... ms later, once the services had let go of their transfer memory
[Exit] heap check, after releasing services: the loader can read into all ... MiB of the malloc heap; every page is read/write and unlocked
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

The first check's blocks are normal: the sockets' transfer memory, and threads not yet closed. Only the second check counts.

### A Bevy app exit

With `multi_threaded`, the hook logs `[Exit] Bevy's task pools: stopped and joined their ... thread(s) in ... ms`. A task that never yields keeps its thread's 2 MiB stack locked, and the console goes to HOME, with no crash report:

```text
[Exit] Bevy's task pools: 1 of ... thread(s) still running after 500 ms, in a task that doesn't yield (or exit was called on a pool thread)
...
[Exit] heap check, after releasing services: 1 block, 2.00 MiB, would fail the loader's read:
[Exit]   0x...-0x..., 2.00 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, ...
[Exit] WARN: heap pages are still locked, so the loader would crash reading the next NRO (2001-0106); exiting through AM to HOME instead
```

A thread of your own still running at exit ends the same way; the `[Exit] detached threads:` line counts the detached ones.

### An exit after a panic

After a panic, or `std::process::exit` in a game that draws, the GPU device is never dropped. The hook skips `nvExit`, which could hang, and exits through AM to HOME, with no crash report:

```text
[Exit] nvdrv: still open, and ... heap blocks (... MiB) are still held for the GPU, so the GPU driver never shut down; skipping nvExit, which can wait forever in this state, and exiting through AM to HOME
```

A panic skips [the audio close](#the-audio-close-at-exit) too, so the heap check can also list audren's memory and the audio thread's stack.

### A loader crash after exit

The homebrew loader reads the next NRO into the heap in one read. If any heap page is still locked, the kernel rejects it ([result code](glossary.md#result-code) 2001-0106, `0xD401`) and the loader crashes: the console goes to HOME, with an Atmosphère crash report in `hbl`, the loader, not your NRO. Open sockets and Bevy's pool threads both did this on the console, before the exit hook released them.

`bevy_horizon` prevents it by exiting through AM whenever anything is still locked. A game could still cause it if the heap check itself fails (`[Exit] heap check failed: svcQueryMemory returned an error; returning to the loader`), if a thread of yours opens a service or a thread after the check, or if something holds pages in a way the check doesn't model; none of these has been seen. With a crash report in `hbl`, read the last `[Exit]` lines in `log.txt`, and see [troubleshooting.md](troubleshooting.md).

## What is proven on hardware and what is not

Tested on a Switch Lite, so in [handheld mode](glossary.md#handheld-mode) only, under Atmosphère, launched from [sphaira](glossary.md#sphaira) with a game's memory (about 3.1 GiB of malloc heap):

| Area | Status |
|---|---|
| Startup, thread-locals, files, Bevy's AssetServer, the 60 Hz loop, controller input | Works on the console |
| HOME and sleep, with Bevy's focus and lifecycle messages | Works on the console |
| Exit to the homebrew menu with `[Exit] PASS` | Works on the console: hello_switch, Breakout, render_check, a bare NVK triangle test, and a full 3D Bevy 0.18 game |
| Stopping the task pools at exit (`multi_threaded`) | Works on the console in that game, whose exit passed |
| The Bevy 0.18 audio close at exit | Works on the console: 19 ms in that game |
| The Bevy 0.19 audio close | Emulator only |
| The AM exit (`[Exit] WARN`), and skipping `nvExit` | Emulator only. On the console, before the skip existed, such an exit hung in `nvExit` |
| Closing from the HOME menu; docked mode | Not tested on the console |

The full 3D game is [Skate 3 Rust Engine](https://github.com/JohnUzoka/skate-3-rust-engine) (branch `horizon-port`, Switch setup in `crates/skate-game/src/horizon.rs`).

## Inside the exit path

This part is for contributors, and for reading an exit log line by line.

### libnx's exit chain

The normal exit runs like this (libnx v4.12.0, `nx/source/runtime/init.c`):

```text
main returns
└─ exit                        newlib
   ├─ atexit handlers          including the cpal hosts' close_at_exit
   ├─ stdio cleanup
   └─ _exit -> __libnx_exit
      ├─ __appExit
      │  ├─ userAppExit        bevy_horizon's hook
      │  ├─ __nx_win_exit      closes libnx's default NWindow, the screen's layer
      │  ├─ fsdevUnmountAll, fsExit, timeExit, hidExit
      │  ├─ appletExit         reads __nx_applet_exit_mode
      │  └─ smExit
      └─ __nx_exit             back into the loader, or AM ends the process
```

- **libnx's services close after the hook,** so the hook can still write to the SD card. Sockets aren't among them: whoever calls `socketInitialize` must call `socketExit`.
- **`log.txt` survives stdio cleanup,** because `bevy_horizon` keeps its own descriptor for it and devkitPro counts the `dup2` references from stdout and stderr ([why writes take turns](logging-and-crash-reports.md#why-writes-take-turns)).
- **Exit mode 1 ends the process through AM.** When `__nx_applet_exit_mode` is 1, `appletExit` has the last step ask AM to end the process (`ISelfController::Exit`) instead of returning to the loader.

### The `userAppExit` hook

`crates/bevy_horizon/src/compat.rs` defines libnx's weak [`userAppExit`](glossary.md#userappexit) symbol, which calls `release_horizon_services` in `lifecycle.rs`. It runs whenever the NRO ends itself (`main` returning, `std::process::exit`, or an abort), and logs the lines in [a clean exit](#a-clean-exit). In order:

1. **Run once,** even if something aborts inside it.
2. **Log the start and stop the log writer,** so each later line reaches the card before the next step runs. Put focus handling back to `SuspendHomeSleep`, as the deko3d sample does.
3. **Check the heap** (`check_heap_for_exit`), listing every block the loader couldn't read into ([how to read heap-check lines](#how-to-read-heap-check-lines)).
4. **Stop threads and run your hooks.** `stop_task_pools` stops Bevy's global task pools and joins their threads, for up to 500 ms. The release hooks run, newest first. `close_detached_threads` closes detached threads that have exited, and waits up to 1 s for the rest, such as the log writer and the `blocking` crate's threads behind Bevy's file reads.
5. **Close sockets** (`socketExit`), if they were started.
6. **Unmount RomFS,** if it was mounted.
7. **Close nvdrv, unless the GPU driver still uses it** (`release_nvdrv`). On a clean exit, Mesa has already called `nvExit`. If the device was never dropped, `nvExit` would wait with no time limit for nvdrv to give back its 8 MiB of transfer memory (libnx v4.12.0 `nv.c:102-106`, `tmem.c:128-136`); on the console, such an exit hung there. So the hook checks first:

   | nvdrv | Heap pages held for the GPU | What happens |
   |---|---|---|
   | Closed | Any | Nothing: `[Exit] nvdrv: already closed ...` |
   | Open | None | `[Exit] nvdrv: still open, but no heap pages are held for the GPU; calling nvExit`, then `[Exit] nvExit: done` |
   | Open | Some | Skips `nvExit` and exits through AM: `[Exit] nvdrv: still open, and ... heap blocks ...` |
   | Open | Unknown | Skips `nvExit` and exits through AM: `[Exit] nvdrv: still open, and svcQueryMemory failed, ...` |

   "Held for the GPU" means pages with the DeviceMapped attribute, or Uncached, which libnx's `nvMapCreate` sets on uncached GPU objects. Shutting Mesa down here wouldn't help: only `nvExit` frees nvdrv's transfer memory.
8. **Wait for the heap to unlock,** if the first check found blocks: up to 1 s, checking every millisecond. If time runs out, it logs `[Exit] the heap was still locked 1000 ms later: a service kept its transfer memory, or something else still holds heap pages`.
9. **Check again, log the verdict and sync `log.txt`.** `[Exit] PASS` means the loader can read the heap; `[Exit] WARN` means something still holds pages, and the blocks listed above it say what. After `[Exit] heap check failed: ...` the hook returns to the loader, which may [crash](#a-loader-crash-after-exit).
10. **Exit through AM if anything is still locked.** On WARN it sets libnx's `__nx_applet_exit_mode` to 1 (step 7's skip has already), so AM ends the process and the console goes to HOME, instead of the loader crashing with 2001-0106.

#### Why it allocates nothing

Exit can follow a failed allocation or a panic, and libnx's `tmemClose` can give memory back to malloc before the service has unmapped it, so allocating at exit could fail or fault. The heap scanner keeps up to eight bad blocks in a fixed array on the stack, and `write_log_line` formats into a fixed 256-byte buffer (`FixedBuf<256>`). `bevy_horizon`'s own exit steps don't allocate; keep your release hooks the same.

#### How to read heap-check lines

Each listed block gets a line like the 2.20 MiB one in [a clean exit](#a-clean-exit). With more than one block, the line above the list reads `N blocks, X MiB in all`, and only the first 8 are listed.

| Field | Meaning |
|---|---|
| Address range, MiB | The heap block and its size. Typical: 2.20 MiB for sockets, about 2 MiB for a Rust thread's stack, 64 KiB for the log writer's, 8 MiB for nvdrv |
| `memory type 0x5` | Heap memory; locking doesn't change it |
| `permission 0x0` | No access now; the loader needs read/write (0x3) |
| `attributes 0x1` | Locked: lent out, as transfer memory and thread stacks are |
| `attributes` with 0x8 | Uncached: mapped for the GPU by nvmap, usually with DeviceMapped (0x4) |

The check applies the kernel's rule for [IPC](glossary.md#ipc) buffers (Atmosphère's `KPageTableBase::SetupForIpcClient`) to what [`svcQueryMemory`](glossary.md#svc) reports: a page passes if it is read/write with no attribute other than IpcMapped (0x2), DeviceMapped (0x4) or PermissionLocked (0x10). That can't prove the loader's read will succeed, but it catches what makes it fail.

## Source map

| Area | Where |
|---|---|
| `HorizonPlugin` and the runner | `crates/bevy_horizon/src/lib.rs` (`HorizonPlugin`, `FRAME_TIME`) |
| libnx hooks: thread pointer, threads, aborts, exceptions | `crates/bevy_horizon/src/compat.rs` (`userAppInit`, `set_thread_pointer`, `__wrap_threadCreate`, `close_detached_threads`, `userAppExit`, `diagAbortWithResult`, `__libnx_exception_handler`) |
| Boot log, `log.txt`, the log writer, the panic hook | `crates/bevy_horizon/src/logging.rs` (`start_boot_log`, `init_switch_logging`, `set_asset_root`, `stop_log_writer`, `write_log_line`, `FixedBuf`, `record_panic`) |
| Startup services, HOME, the heartbeat, the exit hook | `crates/bevy_horizon/src/lifecycle.rs` (`init_horizon_services`, `check_applet_loop`, `suspend_until_out_of_background`, `frame_timer`, `flush_logs_system`, `register_release_hook`, `release_horizon_services`, `stop_task_pools`, `release_nvdrv`) |
| The heap scan | `crates/bevy_horizon/src/memory.rs` (`scan_heap`, `check_heap_for_exit`, `wait_for_heap_unlock`, `nvmap_heap`) |
| Screen, input, dock status | `crates/bevy_horizon/src/window.rs` (`attach_default_nwindow`), `input.rs` (`poll_horizon_input`), `display.rs` (`monitor_dock_status`) |
| Stopping the task pools | `patches/bevy_tasks/src/usages.rs`, `patches/bevy-0.18/bevy_tasks/src/usages.rs` (`shut_down_global_task_pools`) |
| Closing the audio | `patches/cpal/src/host/horizon/mod.rs`, `patches/bevy-0.18/cpal/src/host/horizon/mod.rs` (`Stream::drop`, `Shared::close`, `close_at_exit`) |
| Examples | `examples/hello_switch/src/main.rs` (`main`), `checks.rs` (`thread_check`, `fs_check`); `examples/switch_captest/src/bevy_phase.rs` (`build_app`, `run_app`, `drop_app`) |
