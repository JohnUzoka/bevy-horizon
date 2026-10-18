# How a bevy_horizon program starts, runs and exits

This page explains the runtime path of a `bevy_horizon` program: how the loader reaches Rust, what this layer does before and during [`App::run`](glossary.md#bevy), and what must be released before control returns to the homebrew loader. It is written for you if you are porting a Bevy game and need to know where to put startup code, frame code, service shutdown, or diagnostics.

The first hardware proof was `hello_switch` build A2, measured on a Switch in handheld mode under Atmosphère, launched from sphaira 1.0.0. A3's stronger exit path has since passed on the console: in hardware run 5, hello_switch logged the heap check's PASS and exited with no new crash report (`LOGS/hello_switchlog.txt:84-95`), and in captest's run 4 the HOME checks passed (`LOGS/captest_results4.txt:215-219`).

## In short

- **The first project hook is libnx `userAppInit`.** It runs before C constructors, Rust runtime setup, and `main`. `bevy_horizon` sets `TPIDR_EL0` there and opens `log.txt` before Rust can touch thread-locals (`crates/bevy_horizon/src/compat.rs:28-64`, `logging.rs:120-155`).
- **`HorizonPlugin` adds three plugins.** `HorizonLifecyclePlugin`, `HorizonInputPlugin`, and `HorizonDisplayPlugin` are added together (`lib.rs:31-33`). On Horizon it also adds `ScheduleRunnerPlugin::run_loop(1/60 s)` if no runner is present (`lib.rs:22`, `lib.rs:42-43`).
- **Startup initializes RomFS and sockets.** `HorizonLifecyclePlugin` calls `romfsMountSelf`, then `socketInitialize(NULL)` before Bevy systems run (`lifecycle.rs:6-12`, `lifecycle.rs:51-70`). The missing-RomFS result 2345-0010 is expected for the current NRO.
- **Every frame polls system state before ordinary Bevy work.** It handles applet messages (exit and focus), polls input before Bevy's `InputSystems`, checks dock status, times the frame, and flushes logs once a second (`lifecycle.rs:419-485`, `input.rs:30-36`, `display.rs:13-18`, `lifecycle.rs:703`).
- **HOME is reported to the game, then the app is suspended.** `bevy_horizon` puts the app in libnx's `NoSuspend` [focus-handling mode](glossary.md#focus-handling-mode), so HOME sends a message instead of freezing the app unannounced. Bevy gets `WindowFocused` and `AppLifecycle` as bevy_winit sends them on other platforms, with two frames to react, and then the system suspends the app until the player comes back. See [HOME and focus](#home-and-focus).
- **The heartbeat is every 10 seconds.** It logs frame count, average fps, average and worst work time and the worst frame (`lifecycle.rs:737-770`), and every sixth one, once a minute, the malloc figures. Those come from newlib's `mallinfo`, which walks every free block with malloc's lock held: about 41 ms a call once Skate 3 had loaded its map, a frame stalled each time, so every 10 seconds was too often (sk8 findings 21). A3 also attributes how much of the worst frame was spent on `log.txt` (`lifecycle.rs:754-763`). Builds before findings 20 counted 600 frames instead: the same at 60 Hz, but minutes apart in a game running at 1-2 fps.
- **The exit hook is [`userAppExit`](glossary.md#userappexit).** It runs when the NRO ends itself: main return, `std::process::exit`, or panic-abort. It does not run when the user closes the software from HOME without an exit lock (`compat.rs:66-75`, `lifecycle.rs:78-88`).
- **The exit path stops Bevy's task pools, releases sockets and RomFS, closes nvdrv if the GPU driver left it open and holds no memory, waits, checks the heap, and logs PASS or WARN** (`lifecycle.rs:219-280`, `release_nvdrv` at `:293`, `memory.rs:203-277`).
- **Clean CRT return to loader:** Calling raw `svcExitProcess` bypasses Horizon's applet manager, which triggers the OS dialog "The software was closed because an error occurred" (seen on hardware in Run 1). Instead, releasing all services (especially `nvExit` for GPU transfer memory) allows CRT `exit(0)` to branch through `__nx_exit`, returning gracefully to sphaira.
- **A Bevy app is dropped inside `App::run`, and with it a Bevy 0.19 game's audio stream.** Closing it stops the cpal host's audio thread, which closes the audio renderer, and waits at most 1 s. With SDL, which the host used before, that close never returned in Ryujinx. See [Quitting: the App drop](#quitting-the-app-drop).

## Startup, in order

Startup order matters because the first wrong call can fail before you have a screen, a panic hook, or even Rust thread-local storage.

```text
HOME icon / title override
  -> sphaira hbl loader
    -> libnx crt0 in the NRO
      -> __libnx_init
        -> newlib setup
        -> __appInit
          -> userAppInit              bevy_horizon hook
            -> set TPIDR_EL0
            -> open log.txt
            -> write boot memory report
        -> C constructors
      -> Rust std startup
      -> main
        -> init_switch_logging
        -> App::new
        -> DefaultPlugins, with no renderer on Horizon
        -> HorizonPlugin
        -> App::run
```

### 1. The loader jumps to crt0

The [loader](glossary.md#loader) maps the [NRO](glossary.md#nro), prepares arguments, and jumps to the NRO's crt0. The linked [ELF](glossary.md#elf) is position-independent, and `logging.rs` treats libnx `_start` as the runtime base for crash address math (`logging.rs:65-75`, `logging.rs:293-304`).

When the NRO later exits normally, libnx jumps back to the loader's saved exit function. That is why a normal exit should return to sphaira's menu, not to HOME.

### 2. libnx init runs and calls `userAppInit`

`userAppInit` is the earliest useful hook in this layer. It does two things:

1. `set_thread_pointer()` reads libnx's thread pointer from `TPIDRRO_EL0 + 0x1f8` and writes it to `TPIDR_EL0` (`compat.rs:28-40`).
2. `start_boot_log(tp)` opens `log.txt`, redirects output, records TLS and memory information, sets `BEVY_ASSET_ROOT`, and fsyncs (`compat.rs:62-64`, `logging.rs:120-155`).

This must happen before Rust's startup. Rust reads a thread-local while setting the main thread id. Without `TPIDR_EL0`, every `std` program can crash before `main`.

The boot log from A2 shows it worked:

```text
[Boot] TPIDR_EL0 = 0x2a6e7a60 (Rust's thread-local base, copied from libnx ThreadVars.tls_tp)
[Boot] TLS template at nro+..., align 8: Rust and libnx agree on thread-local addresses
```

`LOGS/log2.txt:10-11` has those lines.

### 3. Rust startup reaches `main`

`hello_switch` calls `init_switch_logging("hello_switch")` first (`examples/hello_switch/src/main.rs:20-21`). On Horizon, the log is usually already open, so this records that Rust's runtime is up and `main` is running (`logging.rs:91-118`). It also installs the panic hook.

Then the example creates the Bevy app. `RenderPlugin` keeps its defaults: on Horizon, wgpu's only backend is Vulkan, drawing through NVK, which bevy_horizon's `vulkan` feature links (`examples/hello_switch/src/main.rs:49-50`; [rendering.md](rendering.md)).

### 4. `HorizonPlugin` adds lifecycle, input, display, and the runner

The plugin setup is small and explicit:

```rust
app.add_plugins((
    HorizonLifecyclePlugin,
    HorizonInputPlugin,
    HorizonDisplayPlugin,
));
```

That is `crates/bevy_horizon/src/lib.rs:31-33`.

On Horizon, if the app has not already added a runner, `HorizonPlugin` adds `ScheduleRunnerPlugin::run_loop(FRAME_TIME)`, where `FRAME_TIME` is `1_000_000_000 / 60` ns (`lib.rs:22`, `lib.rs:42-43`). This is the 60 Hz loop. It is not presentation; today there is no renderer. It only means Bevy's schedules update every 16.67 ms when the work finishes early.

`HorizonLifecyclePlugin` runs `init_horizon_services()` immediately during plugin build (`lifecycle.rs:6-12`). That function:

1. calls `romfsMountSelf("romfs")`;
2. treats libnx `IoError` 2345-0010 as a warning that the NRO has no [RomFS](glossary.md#romfs), which is normal without `elf2nro --romfsdir`;
3. calls `socketInitialize(NULL)`;
4. records whether RomFS and sockets need release later (`lifecycle.rs:23-70`).

A2 logged both outcomes:

```text
romfsMountSelf: 0x1559 = 2345-0010 (libnx IoError): this NRO has no RomFS, which is normal unless elf2nro was given --romfsdir
BSD socket layer initialized successfully
```

Those are `LOGS/log2.txt:24-25`.

### 5. `main` enters `App::run`

`hello_switch` prints its last startup line, flushes logs on Horizon, and calls `app.run()` (`examples/hello_switch/src/main.rs:97-104`). After (+), A2 returned from `app.run()` and printed:

```text
[Exit] app.run() returned Success
```

That line is in `LOGS/log2.txt:70`. It proves Bevy and this layer ended their own work. It does not prove the loader survived afterwards.

## Every frame

Frame order matters because input latency, exit requests, display changes, and logging stalls all depend on which system runs first.

### Applet messages and exit requests

At PreUpdate, `check_applet_loop` handles every applet message that has arrived: `process_applet_messages` calls `appletGetMessage` and `appletProcessMessage` until none is left, at most 16 a frame (`lifecycle.rs:419`, `:490`). libnx's `appletMainLoop` handles one message per call, so a frame could fall behind when several arrive together. `appletProcessMessage` returns false only for `ExitRequest`. Then the system writes a Bevy `AppExit::Success`, flushes logs, and lets `App::run()` return. Service release happens later in `userAppExit`, not inside the frame.

Closing from HOME without an exit lock usually does not take this path. The system can end the whole process. In that case the log may simply stop at the last once-per-second sync. With an [exit lock](glossary.md#exit-lock), which `switch_captest` takes and `bevy_horizon` doesn't, closing from HOME sends `ExitRequest` instead. In eden's AM an app whose exit was requested is always allowed to run (`LifecycleManager::IsRunnable`), so even an app suspended in the background would get to exit; not yet seen on hardware.

### HOME and focus

**What was wrong.** libnx puts every application in the `SuspendHomeSleep` focus-handling mode at startup (`appletInitialize`). In that mode HOME suspends the app and never tells it. Hardware run 3 of `switch_captest` tried `SuspendHomeSleepNotify`, which libnx describes as still calling the focus hook. The mode was set (`[Applet] focus handling: SuspendHomeSleepNotify`), yet HOME gave the same result: 0 focus hooks and one 5.13 s gap between frames. That evidence is in the workshop findings, `LOGS/switch_captest/log.txt:641` and `LOGS/captest_results3.txt:213-214`. eden's AM explains why: in both `SuspendHomeSleep` modes, an application behind HOME is still given `InFocus` (`LifecycleManager::GetFocusStateWhileBackground`), so there is no change to report. Only in `NoSuspend` does it get `Background`.

**What `bevy_horizon` does now.**

1. `init_horizon_services` sets `NoSuspend` (`lifecycle.rs:164`). An NRO started as a library applet, from the album, can't change the mode; it logs `focus handling left as it was` and HOME stays unreported.
2. HOME sends `FocusStateChanged`, and libnx's focus state becomes `Background` (3). That frame sets the primary window's `Window::focused` to false and writes `WindowFocused { focused: false }` and `AppLifecycle::WillSuspend`.
3. The next frame writes `AppLifecycle::Suspended`. It is the game's last update. bevy_winit does the same, and Bevy's docs give an app "one frame to react to this event before being paused".
4. The frame after that, `suspend_until_out_of_background` switches to `SuspendHomeSleepNotify` (`lifecycle.rs:520`). In the background, that mode makes the system suspend the app: eden suspends the process inside the `SetFocusHandlingMode` call, and in run 3 the same mode froze the app for the 5.13 s it was away.
5. When the player comes back, a focus message is waiting. That frame sets `NoSuspend` again, then writes `WindowFocused { focused: true }` and `AppLifecycle::WillResume`. The next frame writes `Running`.

| Frame | What Bevy gets |
|---|---|
| The app's first two frames | `WillResume`, then `Running`; winit sends these at startup on every platform |
| N, HOME pressed | `WindowFocused(false)`, `WillSuspend` |
| N+1 | `Suspended`: the last update |
| N+2 | Suspended in PreUpdate until the player is back; then `WindowFocused(true)`, `WillResume` |
| N+3 | `Running` |

This is the pattern of switchbrew's deko3d sample framework (`CApplication::run`): `NoSuspend` while in focus, `SuspendHomeSleepNotify` and no frames while not. The message order is bevy_winit 0.19.1's (`src/state.rs`). Both are credited in [CREDITS.md](../CREDITS.md).

The messages exist only with Bevy's `WindowPlugin` (the `bevy_window` feature); without it the app is still suspended. A library applet over the app, such as the keyboard, gives `OutOfFocus` (2): that only changes `WindowFocused`, and the app keeps running.

After the suspension, `Time<Real>` sees the whole time away in one frame, and `Time<Virtual>` clamps it to 250 ms, so game time and `FixedUpdate` move by at most 250 ms. The frame timer doesn't count the time as work, and the heartbeat leaves it out of its fps average (`not counting X s suspended`).

**In a game.** Pause gameplay and music, and save if needed, on `AppLifecycle::WillSuspend` or `Suspended`; resume on `WillResume` or `Running`. A game that already handles `AppLifecycle` for Android or iOS needs no change.

**If the system doesn't suspend the app.** Neither case below has been seen; eden and the deko3d sample both rely on the suspension.

- If the switch returns and the app keeps running in the background, `suspend_until_out_of_background` waits on the applet message event, with no frames and no busy loop, until focus comes back.
- If `InFocus` comes back within 100 ms of the switch, that is the mode change, not the player: HOME reports `InFocus` in the `SuspendHomeSleep` modes. The app logs a WARN, goes back to `NoSuspend`, and from then on only waits (`lifecycle.rs:530`).

**At exit.** `release_horizon_services` sets `SuspendHomeSleep` back before anything else (`lifecycle.rs:217`), as the deko3d sample's destructor does: the loader runs in this process until the next NRO's libnx sets the mode again.

The log after HOME should read as below. These lines are what the code logs, not copied from a console; on hardware, captest's run 4 saw the same order of Bevy messages (unfocused, WillSuspend, Suspended, focused, WillResume, Running) and libnx's focus hook saw focus lost once and regained once (`LOGS/captest_results4.txt:215-216`):

```text
[Applet] focus: in focus -> in the background (HOME menu or sleep)
[Applet] AppLifecycle::WillSuspend
[Applet] AppLifecycle::Suspended
[Applet] the last frame before the background has run; switching to SuspendHomeSleepNotify so the system suspends the app
[Applet] in focus again X s later: Y s inside SetFocusHandlingMode, then N wait(s) for the focus message
[Applet] focus: in the background (HOME menu or sleep) -> in focus
[Applet] AppLifecycle::WillResume
[Applet] AppLifecycle::Running
```

The split between Y and the waits shows where the suspension happened: inside the call, as in eden, or just after it.

### Input before Bevy input systems

`HorizonInputPlugin` creates one gamepad entity during PreStartup and sends Bevy connection events (`input.rs:30-59`). On Horizon it polls HID in PreUpdate **before** Bevy's `InputSystems` (`input.rs:34-36`). That makes button changes visible in the same frame.

The first hardware log showed why the order matters. A2 logs HID configured, controller style `0x2` for handheld, then Bevy's gamepad connection and the checks (`LOGS/log2.txt:35-38`). Later, A and + came through as expected (`LOGS/log2.txt:67-70`).

### Dock-status check

`HorizonDisplayPlugin` inserts `HorizonDisplayMode::Handheld` and runs `monitor_dock_status` in PreUpdate on Horizon (`display.rs:13-18`). The system calls `appletGetOperationMode()`, maps 1 to docked, and updates the primary window to 1920x1080 or 1280x720 when the value changes (`display.rs:27-52`).

No docked hardware run is current. Treat this as code coverage to test later, not as a proven mode.

### Frame timing and the heartbeat

The frame timer inserts a `FrameStart` schedule before Bevy `First` and a `FrameEnd` schedule after Bevy `Last` (`lifecycle.rs:141-225`). It measures the work Bevy did. It does not count the runner's sleep to reach 60 Hz.

Every 10 seconds, from the first frame on, `flush_logs_system` writes:

1. frame number, seconds, and average fps;
2. work per frame over the frames since the last heartbeat, with average and worst values;
3. malloc use from `memory::malloc_report()` (`lifecycle.rs:737-770`).

A2's build counted 600 frames between heartbeats instead (findings 20 changed it); at 60 fps that is the same. A2's hardware numbers:

| Heartbeat | Work average | Worst work |
|---:|---:|---:|
| 600 | 0.68 ms | 70.08 ms |
| 1200 | 0.59 ms | 20.83 ms |
| 1800 | 0.56 ms | 20.39 ms |
| 2400 | 0.57 ms | 19.68 ms |
| 3000 | 0.56 ms | 19.86 ms |
| 3600 | 0.57 ms | 20.53 ms |
| 4200 | 0.57 ms | 20.47 ms |

These are `LOGS/log2.txt:46-66`. They prove the no-renderer app had a lot of spare CPU time. They do not say anything about real rendering cost.

Once a second (every 60 frames before findings 20), the same system has `log.txt` synced. The 20 ms worst frame every 10 seconds is most likely the previous heartbeat's file writes and fsync. A3 added logging attribution to the heartbeat line, so the next hardware run can confirm this (`lifecycle.rs:246-251`). Skate 3's runs did (sk8 findings 27), so a log writer thread now does those writes and syncs, and the system only asks it to (`request_log_sync`, `lifecycle.rs:793-797`; [logging-and-crash-reports.md](logging-and-crash-reports.md#flushing-and-the-cost-of-flushing)).

## Every way the program can end

The end path matters because services and thread stacks can leave heap pages locked for the loader.

| End | How it starts | Does `userAppExit` run? | Where you land | What to read |
|---|---|---:|---|---|
| Main returns after (+) | `AppExit::Success` makes `App::run()` return; `main` returns | Yes | HOME with no report if heap pages are still locked (any Bevy app), through `svcExitProcess`; sphaira menu if nothing is locked | End of `log.txt`: the `[Exit]` lines say which |
| `std::process::exit` | Rust calls newlib `exit` | Yes | Same as main return | `log.txt` |
| Rust panic | Panic hook logs; target is `panic-strategy = abort`; abort calls `_exit(1)` | Yes | Same as main return | `panic.txt`, `log.txt` |
| Closing from HOME without exit lock | HOME closes the title process | No | HOME menu, usually no report | Last synced `log.txt` |
| CPU exception | libnx exception path and `svcBreak` | No normal hook; creport terminates process | HOME menu with report | `crash.log`, Atmosphère report |
| libnx fatal error | `diagAbortWithResult` then `svcBreak` | No normal hook | HOME menu with report | `abort.log`, Atmosphère report |
| Loader crash after our exit | Our NRO already returned to loader; loader fails | Our hook already ran, if this is A3 | HOME menu with report | End of `log.txt`, report module list |

A2 is the last row without the A3 fix. The app logged Success, then sphaira's hbl crashed with 2001-0106 (`LOGS/log2.txt:70`, `crash_reports/01791176509_05446530aca7e000.log:2`, `:52-54`, `:121-125`). With the `svcExitProcess` fallback, that row should no longer happen: when anything is still locked, the process ends before the loader reads into the heap.

## libnx's exit chain

The normal exit chain is the path that reaches `userAppExit`. It is useful because it tells you which services are still alive when the hook runs.

```text
main returns
└─ exit                         newlib
   ├─ __call_exitprocs           atexit handlers
   ├─ stdio_exit_handler         closes FILE handles
   └─ _exit
      └─ __syscall_exit
         └─ __libnx_exit
            ├─ __appExit
            │  ├─ userAppExit          bevy_horizon hook
            │  ├─ __nx_win_exit        only if linked
            │  ├─ fsdevUnmountAll
            │  ├─ fsExit
            │  ├─ timeExit
            │  ├─ hidExit
            │  ├─ appletExit
            │  └─ smExit
            └─ __nx_exit               jump back into the loader
```

This chain was checked in the A3 ELF in the findings. The upstream citations there are libnx v4.12.0 `nx/source/runtime/init.c:154-166, 190-196`, `nx/source/runtime/newlib.c:48-51`, and devkitPro newlib `exit.c`, `_exit.c`, and `findfp.c`.

Two details matter:

- **When the heap is still locked, `userAppExit` never returns.** `svcExitProcess` ends the process inside it, so everything below `userAppExit` in this tree is skipped, and the system closes those sessions instead.
- **Sockets are not in libnx's default `__appExit`.** If your app called `socketInitialize`, your app must call `socketExit`.
- **`log.txt` can still be written.** newlib closes stdout and stderr before `_exit`, but `open_log_file` kept its own `LOG_FD` and duplicated it to stdout/stderr (`logging.rs:239-288`). devkitPro's handle manager reference-counts `dup2` handles, so closing stdout and stderr does not close `LOG_FD`. libnx shuts down the SD card only after `userAppExit` returns. The three fds share one handle, so writes through any of them still take turns ([logging-and-crash-reports.md](logging-and-crash-reports.md#why-writes-take-turns)). They go straight to the card by then: `release_horizon_services` stops the log writer right after its first line (`lifecycle.rs:233`).

## The `userAppExit` hook

`compat.rs` exposes libnx's weak `userAppExit` symbol and calls `release_horizon_services()` (`lifecycle.rs:219`).

### What it does

The code path is:

1. **Run once.** An `AtomicBool` prevents recursion if something aborts inside the hook.
2. **Log the start and put focus handling back.** It writes the fixed line below, then returns libnx's focus-handling mode to `SuspendHomeSleep` for the loader.

   ```text
   [Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
   ```

3. **Check the heap before release.** `check_heap_for_exit("before releasing services")` scans for pages the loader cannot use as an IPC read buffer (`memory.rs:203`).
4. **Stop threads.** It stops Bevy's global task pools and joins their threads (`stop_task_pools`), runs the hooks the app registered with `register_release_hook`, newest first, and closes detached threads that have exited.
5. **Release sockets.** If `socketInitialize` succeeded, it calls `socketExit` and writes:

   ```text
   [Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
   ```

6. **Unmount RomFS if mounted.**
7. **Close nvdrv, unless the GPU driver still uses it** (`release_nvdrv`, `lifecycle.rs:293`). On a clean exit the Vulkan driver has closed nvdrv already: Mesa's horizon runtime takes a reference for each device and calls `nvExit` when the last one goes (`nouveau_horizon_runtime_get`/`_put`, mesa-switch `src/nouveau/horizon/nouveau_horizon_runtime.c:240`, `:398-424`), and libnx counts the references. When the app ends without dropping the device, after a panic for instance, nvdrv is still open and Mesa still has GPU memory. On hardware, render_check's exit then stopped inside `nvExit` (run 5: the log ends at `socketExit: done`, and with no RomFS to unmount, `nvExit` came next). The likely spot is libnx's `_nvCleanup`: it closes the service, then waits with no time limit for nvdrv to hand back its 8 MiB transfer memory (libnx `nv.c:102-106`, `tmem.c:128-136`). mesa-switch's own teardown for a dying process, `nouveau_horizon_runtime_shutdown`, leaves `nvExit` out too, because a thread that ended inside the driver can hold its lock (`nouveau_horizon_runtime.c:389-392`). Ryujinx can't show the hang: there `nvExit` returns even in this state (run rc9p), because Ryujinx leaves borrowed transfer memory readable and writable. So the hook checks first:

   | nvdrv | Heap pages held for the GPU | What happens | Log line |
   |---|---|---|---|
   | Closed | Any | Nothing to do | `[Exit] nvdrv: already closed (the GPU driver released it), so there is nothing for nvExit to do` |
   | Open | None | Calls `nvExit` | `[Exit] nvdrv: still open, but no heap pages are held for the GPU; calling nvExit`, then `[Exit] nvExit: done` |
   | Open | Some, or the scan failed | Skips `nvExit` and exits through AM | `[Exit] nvdrv: still open, and N heap blocks (X MiB) are still held for the GPU, so the GPU driver never shut down; skipping nvExit, ...` |

   "Held for the GPU" means heap pages with the DeviceMapped or Uncached memory attribute. libnx's `nvMapCreate` marks the pages of every uncached nvmap object Uncached (`svcSetMemoryAttribute`, libnx `nvidia/map.c:59-60`) and `nvMapClose` clears it again (`:106`), and all NVK memory is a `memalign` from this heap (mesa-switch `nouveau_horizon_memory.c:591`). `nvGetServiceSession` (libnx `nv.c:109`) tells whether nvdrv is open.

   Calling `nouveau_horizon_runtime_shutdown` here would give the GPU buffers back, but not the transfer memory: libnx allocates that from the heap too (`tmem.c:19`), and only `nvExit` frees it. The loader would still fail on it, so the AM exit is needed either way, and the hook leaves Mesa alone.
8. **Wait for unlock.** If the first check found bad blocks, it waits up to one second, checking every millisecond, for services to let go of transfer memory (`memory.rs:266`).
9. **Check again and write the verdict.** It logs PASS, WARN, or heap-check failure, then fsyncs `log.txt` so the exit evidence reaches the SD card.
10. **Exit through AM if anything is still locked.** It sets libnx's `__nx_applet_exit_mode` to 1, so AM ends the whole process and the Switch goes back to HOME, as when any game exits. Returning would let sphaira's loader stop with 2001-0106 on the next NRO.

   ```text
   [Exit] WARN: heap pages are still locked, so the loader would crash reading the next NRO (2001-0106); exiting through AM to HOME instead
   ```

### Why it allocates nothing

Exit can happen because allocation failed, or because a panic is already unwinding to abort. Allocating inside the hook could fail again. There is also a transfer-memory trap: libnx's `tmemClose` can return memory to malloc before the service has unmapped it. If exit code allocated that just-freed block and wrote to it too soon, it could data-abort.

So the heap scanner stores up to eight bad blocks in a fixed stack array (`memory.rs:122-141`). `write_log_line` formats into `FixedBuf<256>` and writes directly to the log file (`logging.rs:333-390`). The A3 binary check found no `malloc`, `free`, or `realloc` on the release path.

### How to read heap-check lines

Expected A3 run 1 lines are:

```text
[Exit] heap check, before releasing services: 1 block, 2.20 MiB, would fail the loader's read:
[Exit]   0xSTART-0xEND, 2.20 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, as an open transfer memory or a thread stack leaves it
[Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
[Exit] the heap was unlocked ... ms later, once the services had let go of their transfer memory
[Exit] heap check, after releasing services: the loader can read into all 3132.7 MiB of the malloc heap; every page is read/write and unlocked
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

Field meanings:

| Field | Meaning |
|---|---|
| Address range | The heap block returned by `svcQueryMemory`. |
| MiB | Size of that bad block. Sockets should be 2.20 MiB with defaults. A Rust thread stack is usually just over 2 MiB. |
| `memory type 0x5` | Heap memory. Locking does not change the type. |
| `permission 0x0` | This process currently has no read/write access to the original heap pages. The loader needs read/write. |
| `attributes 0x1` | Locked/borrowed pages. Transfer memory and mapped thread stacks have this shape. |
| PASS | No bad blocks remain. sphaira's whole-heap read should be accepted. |
| FAIL | At least one bad block remains. sphaira's loader will likely stop with 2001-0106. |

The heap checker is a [`svc`](glossary.md#svc)-level approximation of the kernel's IPC buffer rule. It does not prove sphaira will do the read, but it checks the condition that made A2 fail.

## Quitting: the App drop

`App::run` doesn't keep the App. It swaps it for an empty one and hands the real one to the runner (`bevy_app-0.19.1/src/app.rs:192-194`). `ScheduleRunnerPlugin`'s loop returns as soon as it sees `AppExit` (`schedule_runner.rs:105-107`), and the App is dropped there, still inside `run()`. Everything it owns is dropped with it, including bevy_audio's `AudioOutput`. Bevy 0.19 keeps the audio stream in that resource (`bevy_audio-0.19.1/src/audio_output.rs:16-31`); Bevy 0.18 leaked it on purpose (`mem::forget`, `bevy_audio-0.18.1/src/audio_output.rs:33-35`), so a 0.18 game's stream outlives the drop (see [the audio close at exit](#the-audio-close-at-exit)).

On Horizon, the chain from there is:

```text
App drop
└─ AudioOutput drop                   bevy_audio
   └─ cpal Stream::drop               patches/cpal/src/host/horizon/mod.rs:755-767
      └─ Shared::close                sets `stop`, waits up to 1 s for the thread (CLOSE_WAIT,
         │                            :64), then joins it (:374-389)
         └─ the audio thread          sees `stop` within a renderer frame (it waits 10 ms at most,
            │                         :520)
            └─ Output drop            audrvClose, audrenExit, frees the buffers (:340-349)
```

How long the close takes depends on what the thread is doing when it's told to stop. In Ryujinx, Skate 3's close at exit took 331.6 ms, 15 ms of it Ryujinx stopping its renderer (run au6), and captest's App drop, which closes it along with the rest of the App under Bevy 0.19, took 542 ms (run au7). On the Switch, Skate 3's close at exit took 19.0 ms, and the exit passed the heap check (perf35, findings 54). If the thread doesn't stop within 1 s, say in a callback stuck in the game's code, the close gives up and logs `[Audio] the audio thread didn't stop within 1000 ms; audren stays open`; the exit then goes through AM (step 10 above).

With SDL, which the host used before, the close could wait forever ([why not SDL2](input-and-audio.md#why-not-sdl2)): in Ryujinx, `app.run()` never returned after `AppExit` (captest run 12). On hardware, captest's `bevy-app-drop` with SDL, the audio close included, took 110 to 146 ms (runs 1 to 4, findings 16, 17).

So:

- **Expect `app.run()` to take as long as the drop.** Save anything that matters (game saves, a last log line) before sending `AppExit`, not after `app.run()` returns.
- **To test or time the drop on its own,** do what captest does: run a copy of `ScheduleRunnerPlugin`'s loop that hands the App back (`examples/switch_captest/src/bevy_phase.rs:928`), then drop it last, under a watchdog (`:952`).

### The audio close at exit

Bevy 0.18 never drops its stream, and Bevy 0.19 drops it only with the App, which a game that ends with `std::process::exit` skips. An open stream keeps audren's transfer memory until the process ends, and locked heap pages make the exit hook leave through AM, to HOME (step 10 above), not back to the homebrew menu. So both cpal hosts close such streams themselves (0.19's lines; 0.18's are the same code):

```text
exit                                  newlib, after main returns
└─ atexit handlers                    run before bevy_horizon's exit hook
   └─ close_at_exit                   registered with the first stream (:706-711); closes each
      │                               stream a Stream::drop didn't (:718-735)
      └─ Shared::close                as above: sets `stop`, waits up to 1 s, joins the thread
```

It logs `[Audio] exit: closed the audio renderer in <ms> ms, which gives audren its memory back`. If the thread doesn't stop within 1 s, it logs `[Audio] exit: the audio thread didn't stop within 1000 ms; audren keeps its memory`, and the exit goes through AM.

Skate 3 in Ryujinx (run au6) logged the close, 331.6 ms, before `[Exit] userAppExit`, and the exit hook then found nothing in the heap locked (`[Exit] PASS`).

## Rules for services you add later

Every service that locks heap pages needs an exit rule before it is added to a game. The table below is the current checklist.

| Thing | What locks heap pages | What releases it | Does the release wait? | Rule for this layer |
|---|---|---|---|---|
| Sockets (`bsd`) | libnx `socketInitialize(NULL)` creates a 2.20 MiB transfer memory from malloc. | `socketExit` | No. `_bsdCleanup` closes handles, but bsd may unmap later. | A3 calls `socketExit`, waits up to 1 s, then checks the heap. |
| GPU (`nv`) | libnx's default `__nx_nv_transfermem_size` is 8 MiB for nv allocation bookkeeping, and every NVK allocation comes from the heap (from the top down, or by `memalign` when that region can't grow; [platform notes](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap)), marked Uncached while nvmap has it. | Mesa's horizon runtime calls `nvExit` when the last Vulkan device is destroyed. | Yes: libnx's `_nvCleanup` waits for the transfer memory with no time limit (`nv.c:102-106`). | Drop the Vulkan device before exit. If it wasn't dropped, `release_nvdrv` skips `nvExit` and exits through AM (step 7 above). |
| Audio (`audren`) | audren uses a transfer-memory work buffer. | `audrenExit` | No in the cited libnx path. | The cpal hosts call `audrenInitialize` and `audrenExit` themselves (`Output::open`, `Drop for Output`; [the audio host](input-and-audio.md#the-audio-host)). Closing a stream waits up to 1 s for its thread, which closes audren; an `atexit` handler closes streams still open at exit, before the exit hook ([the audio close at exit](#the-audio-close-at-exit)). If one stays open, the AM exit ends the process anyway. |
| SevenSixAxisSensor | `hidInitializeSevenSixAxisSensor` creates transfer memory. | `hidFinalizeSevenSixAxisSensor` | Not established here. | Ordinary HID pad input does not use this. Add a release rule before enabling this sensor. |
| Rust thread stacks | libnx maps stack memory out of the heap with locked original pages. | `pthread_join` / `threadClose` | Join waits for thread exit; close releases the thread object. | Never drop a `JoinHandle` unless you have another way to close the thread. |
| Bevy task pools | Pool worker stacks, if Bevy's `multi_threaded` feature is enabled. | A task-pool shutdown path, not implemented here. | Not implemented. | The `svcExitProcess` fallback covers them: the exit lands on HOME instead of sphaira. captest runs the multi-threaded pools this way. |

The upstream citations in the findings are libnx v4.12.0 `bsd.c:73-80, 91-101, 237-239, 268-274`, `tmem.c:11-33, 115-161`, `nv.c:11, 22-23, 102-106`, `audren.c:89, 104-107`, `hid.c:1457, 1476-1503`, `thread.c:103, 132, 234-248`, and Atmosphère `KTransferMemory` / `KPageTableBase` paths.

## Logs and call chains to recognize

Use these snippets to decide where a run is in the lifecycle.

### Good startup through `main`

```text
[Boot] TPIDR_EL0 = ...
[Boot] next: C constructors, Rust's runtime setup (std::rt::lang_start), then main
[Main] init_switch_logging("hello_switch"): Rust's runtime is up and main is running
[Init] Step 6: Calling app.run()...
```

No `[Boot]` line means either `userAppInit` did not run or the log could not be opened. A crash before `main` should now still leave either `log.txt`, `crash.log`, or `abort.log` if the hook got far enough.

### Normal A2 run before the loader crash

```text
[Check] thread: PASS ...
[Heartbeat] frame 4200, 70.1 s after the first frame, 60.0 fps on average
[Input] (A) pressed: cycling the cube's colour
[Input] (+) pressed: exiting
[Exit] app.run() returned Success
```

That is a clean return from Bevy, not proof that the loader returned to sphaira. In A2 it was followed by a new Atmosphère report in `hbl`.

### Loader crash signature

```text
Result: 0xD401 (2001-0106)
PC:     hbl + 0x3810
Stack Trace [00]: hbl + 0x54c
Module Info: hbl
```

This means the crash report belongs to sphaira's loader, not to the NRO that just exited. A2 and build A both had this signature (`01791176509_05446530aca7e000.log:2`, `:52-54`, `:121-125`; `01791151534_05446530aca7e000.log:2`, `:52-54`, `:125`).

### A3 success signature, measured on hardware

hello_switch in hardware run 5 (`LOGS/hello_switchlog.txt:84-95`, shortened):

```text
[Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
[Exit] heap check, before releasing services: 1 block, 2.20 MiB, would fail the loader's read:
[Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
[Exit] heap check, after releasing services: the loader can read into all 3113.9 MiB of the malloc heap; every page is read/write and unlocked
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

The console went back to sphaira with no new crash report, which confirms A3's exit path on hardware. The heap is 18.8 MiB smaller than A2's 3132.7 MiB with the same 3189.0 MiB process total (`LOGS/hello_switchlog.txt:13-14`, `LOGS/log2.txt:13-14`): the loader gives the heap what is left after loading the NRO, and this one links NVK.

### Bevy app exit signature, seen in Ryujinx

captest run 3 (`ryujinx-bh/out/captest-exit1/log.run3.txt:637-660`, shortened). The 2.00 MiB blocks are thread stacks: Rust's default stack size is 2 MiB, and Bevy's pool threads are never joined.

```text
[Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
[Exit] heap check, before releasing services: 11 blocks, 16.66 MiB in all, would fail the loader's read (the first 8 are listed):
[Exit]   0x200de2000-0x200fe3000, 2.00 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, as an open transfer memory or a thread stack leaves it
[Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
[Exit] romfsUnmount: done
[Exit] the heap was still locked 1000 ms later: a service kept its transfer memory, or something else still holds heap pages
[Exit] heap check, after releasing services: 10 blocks, 14.46 MiB in all, would fail the loader's read (the first 8 are listed):
[Exit] FAIL: heap pages are still locked; sphaira's loader would stop with 2001-0106 when it reads the next NRO
[Exit] ending the whole process (svcExitProcess) instead of returning to the loader: you land on HOME, not the homebrew menu
```

Then Ryujinx logged each remaining thread's wait ending with `TerminationRequested`. On a Switch, expect HOME with no new crash report.

## What is proven on hardware and what is not

| Topic | Hardware status |
|---|---|
| `userAppInit` sets main-thread `TPIDR_EL0` | Proven by runs that reached `main`, including A2 boot logs. |
| `--wrap=threadCreate` sets new-thread `TPIDR_EL0` | Proven by A2's joined thread-local check (`LOGS/log2.txt:42-44`). |
| Bevy App, DefaultPlugins without renderer, ECS schedules | Proven by A2 startup and checks. |
| 60 Hz `ScheduleRunnerPlugin` loop | Proven by heartbeats through frame 4200. |
| HID input for A and + | Proven by A2 log lines. |
| SD card file write, `stat`/`fstat`/`read_dir`, AssetServer load | Proven by A2 `[Check] fs` and `[Check] asset` lines (`LOGS/log2.txt:31-38`). |
| Memory figures | Proven for this launch path: 3,189.0 MiB total, 3,132.7 MiB malloc heap. |
| A2 exit with sockets still open | Proven indirectly by the post-exit loader crash and the decoded IPC buffer. |
| `userAppExit` service release | Proven on hardware: captest's run 4 log ends with `[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it`, and (+) exited without a crash report (findings 17). |
| Closing from HOME | Not yet measured for A3. Expected: without an exit lock, the process ends and `userAppExit` does not run. |
| HOME and back | Run 3, in `SuspendHomeSleepNotify`: no focus message, the app frozen for 5.13 s, sound fine afterwards. The `NoSuspend` path: Ryujinx run 21 logged the mode, `WillResume` then `Running` at startup, and the mode put back at exit. Ryujinx can't press HOME: it only ever reports `InFocus`. Hardware run 4, in `NoSuspend`: Bevy got unfocused, `WillSuspend`, `Suspended`, focused, `WillResume` and `Running`, and sound came back (findings 17). |
| Fallback when the heap stays locked | bevy_horizon no longer calls `svcExitProcess`, which skips AM (findings 16 §4, §6). It logs `[Exit] WARN` and sets `__nx_applet_exit_mode = 1`, so AM ends the process and the Switch returns to HOME. Not needed on hardware so far: run 4 had nothing locked. Ryujinx run rc9p2 took this path (below). |
| Exit while the GPU driver still runs | Hardware run 5: render_check hit a DeviceLost and a panic, and its exit hung in `nvExit` (findings 19). `release_nvdrv` now skips `nvExit` then. Ryujinx: render_check with its `panic` option logged the skip (1830 blocks, 182.31 MiB held for the GPU) and the AM exit (run rc9p2); a normal run logged `nvdrv: already closed` and PASS (rc9). Ryujinx can't show the hang itself: its transfer memory never waits. Not yet run on hardware. |
| App drop and audio close | With SDL: hung in Ryujinx (captest run 12), in SDL's wait ([why not SDL2](input-and-audio.md#why-not-sdl2)); on hardware captest's `bevy-app-drop` took 110 to 146 ms in runs 1 to 4 (findings 16, 17). With the audren host, in Ryujinx: Skate 3's close at exit took 331.6 ms, and the exit passed the heap check (run au6); captest's `bevy-app-drop`, under Bevy 0.19, took 542 ms (run au7). On hardware, under Bevy 0.18: Skate 3's close at exit took 19.0 ms, and the exit passed (perf35, findings 54). captest's `bevy-app-drop` with the audren host hasn't run on hardware. |
| Docked display resize | Code exists; not tested. |
| Rendering | Through wgpu's Vulkan backend and NVK. On hardware (run 5) hello_switch held 60 fps once started (its heartbeats came 600 frames and 10.0 to 10.1 s apart) and exited with PASS; render_check drew, then lost the device (findings 19). See [rendering.md](rendering.md). |
| Audio | Worked on hardware through SDL: captest's `cpal` and `bevy_audio` checks passed, and the chime was heard (run 3). In Ryujinx, SDL's sound stopped for good after 6 s (run au4); the audren host that replaced it plays on through gameplay, and closes at exit (runs au5, au6). Under Bevy 0.19, captest's `bevy-audio-despawn` and `bevy-app-drop`, which failed and hung with SDL, pass (run au7). On hardware, the Bevy 0.18 audren host played Skate 3's sound clean through play, never ran dry, came back after HOME and after sleep, and closed at exit in 19.0 ms (perf35, findings 54); the 0.19 host hasn't run there. |

## Source map

Use these repo locations first when changing the lifecycle:

| Area | Files |
|---|---|
| Plugin composition and 60 Hz runner | `crates/bevy_horizon/src/lib.rs:22-43` |
| Main-thread and new-thread TLS hooks | `crates/bevy_horizon/src/compat.rs:28-75`, `:91-156` |
| Service init, release hooks, the exit release, nvdrv and the AM exit | `crates/bevy_horizon/src/lifecycle.rs:122-330` (`register_release_hook`, `init_horizon_services` at `:149`, `release_horizon_services` at `:219`, `release_nvdrv` at `:293`) |
| Running Bevy's loop without dropping the App | `examples/switch_captest/src/bevy_phase.rs:928-968` (`run_app`, `drop_app`) |
| Applet messages, HOME and focus, heartbeat | `crates/bevy_horizon/src/lifecycle.rs:419` (`check_applet_loop`), `:490` (`process_applet_messages`), `:520` (`suspend_until_out_of_background`), `:703` (`flush_logs_system`) |
| Heap scanning | `crates/bevy_horizon/src/memory.rs:143-258` |
| Early logging and result-code formatting | `crates/bevy_horizon/src/logging.rs:120-155`, `:333-390`, `:351-358` |
| Input ordering | `crates/bevy_horizon/src/input.rs:30-36`, `:106-199` |
| Dock status | `crates/bevy_horizon/src/display.rs:13-52` |
| Example `main` and exit after `App::run` | `examples/hello_switch/src/main.rs:20-104` |
| Asset/thread checks | `examples/hello_switch/src/checks.rs:76-112`, `:184-259` |
