# Testing

This page covers [captest](glossary.md#captest) (`examples/switch_captest`), the capability-test [NRO](glossary.md#nro) that checks what a Bevy game needs from the console, along with render_check and testing in an [emulator](glossary.md#emulator). Read it before you trust a port on new ground, add a check, or rely on an emulator result.

## In short

- **captest checks what a Bevy game needs, one item at a time:** the system, threads, memory, the SD card, sound, rumble, touch and motion, [RomFS](glossary.md#romfs), networking, native crates, Bevy's [task pools](glossary.md#task-pool), a Bevy app (assets, audio, buttons, sticks, HOME, exit), and the GPU through [EGL](glossary.md#egl) and [OpenGL ES](glossary.md#opengl-es).
- **Build it with `JOBS=4 scripts/build_example.sh switch_captest`,** copy `switch_captest.nro` to `sdmc:/switch/switch_captest/`, and start it with a game's memory. Answer prompts with A, B or X; each one times out on its own.
- **Every check is one line in `captest_results.txt`, synced as it's written.** The statuses are PASS, FAIL, WARN, INFO and SKIP, plus CRASH, HANG and PANIC for a check that stopped the app.
- **A crash or hang doesn't cost the run.** Start captest again: it records the check that was running as CRASH, skips it from then on, and carries on.
- **On a Switch Lite: 179 pass, 0 fail, 2 warn.**
- **Stock [Ryujinx](glossary.md#ryujinx) runs captest, but not the [Vulkan](glossary.md#vulkan) builds.** Set its memory manager mode to Software. An emulator finds bugs in your own code fast, but the console is the answer.

## What captest checks

captest runs these phases in order. Each one logs `[Captest] phase: <name>` in `log.txt`, and HOME and sleep are handled between phases.

1. **Exit lock** (`exit-lock`): closing captest from HOME asks it to exit and gives it 15 s to save its results ([exit lock](glossary.md#exit-lock)).
2. **System:** the [Horizon](glossary.md#horizon) version, applet type, operation and [performance modes](glossary.md#performance-mode), [core masks](glossary.md#core-mask), random numbers, clocks, and the CPU.
3. **Threads:** thread-locals, stacks, locks and channels, many spawns, and cores.
4. **Memory:** the [heap](glossary.md#heap), the largest allocation, small allocations, alignment, and allocation failure.
5. **SD card:** speed, renames and copies, error codes, metadata, names (long, non-ASCII, invalid, letter case), the current directory, and free space.
6. **Interactive checks,** if you answer yes: sound through [cpal](glossary.md#cpal), rumble, touch and motion ([input and audio](input-and-audio.md#what-captest-checks)).
7. **Bevy setup:** builds the Bevy App.
8. **RomFS:** reads the NRO's RomFS, copies `assets/` to the SD card, and maps files with memmap2.
9. **Network:** loopback TCP and UDP, socket options and limits, and, when the console is online, DNS and HTTP.
10. **Native crates:** each crate in the `native` feature does real work, such as compressing, decoding or running Lua.
11. **Bevy pools:** Bevy's task pools.
12. **The Bevy app,** in seven stages: `bevy-executor` (parallel systems, fixed update, frame pacing, the startup messages), `bevy-assets`, `bevy-audio`, `bevy-buttons` (16 prompts, 20 s each), `bevy-sticks`, `bevy-suspend` ([the HOME stage](#the-home-stage)) and `bevy-exit` (+ sends `AppExit`, or wait 2 minutes).
13. **GPU:** EGL and OpenGL ES through [devkitPro](glossary.md#devkitpro)'s [Mesa](glossary.md#mesa): shaders, render targets, [BC textures](glossary.md#bc-texture), compute, readback and teardown. It closes the [text console](glossary.md#text-console) to draw, then asks whether you saw the drawing.
14. **App drop** (`bevy-app-drop`): drops the Bevy App, which closes its audio stream.

Then it writes the summary and shows "Press + to exit"; it exits by itself after 2 minutes.

## Building and running captest

```text
JOBS=4 scripts/build_example.sh switch_captest      # writes switch_captest.nro at the repo root
```

The first build also builds the toolchain image ([getting started](building.md)). `JOBS` caps cargo's parallel jobs (each rustc on a Bevy crate can take 1–2 GB of RAM), and `CONTAINER_ENGINE` picks Docker or Podman. The script packs `examples/switch_captest/romfs` into the NRO and checks the crate patches and the asset section. captest doesn't need [NVK](glossary.md#nvk).

The script passes no Cargo features, so captest's are set in `examples/switch_captest/Cargo.toml`:

| Feature | Default | What it adds |
|---|---|---|
| `native` | On | Crates games use, among them ones that build C code (zstd, ring, mlua): flate2, zip, zstd, ring, mlua (Lua 5.4), tokio, symphonia, bcdec and image. Each is also a feature of its own, so one that stops building can be turned off alone. All nine build and pass on the console |
| `gpu` | On | The GPU checks: EGL and OpenGL ES on devkitPro's Mesa, not Vulkan, so captest runs in stock Ryujinx |
| `quinn` | Off | quinn, rustls and tokio's networking. A build check only: no check uses them |

To run it:

1. Copy `switch_captest.nro` to `sdmc:/switch/switch_captest/`.
2. Start it with a game's memory: hold R while you start any game, or use a [sphaira](glossary.md#sphaira) [forwarder](glossary.md#forwarder) ([getting started](building.md)). Started from the Album, it runs in [applet mode](glossary.md#applet-mode), and `applet-type` warns that memory is limited.
3. Answer each question with A (yes), B (no) or X (not sure). Every prompt times out, so an unattended run finishes on its own.
4. At the end it shows the summary. Press + to exit, or wait 2 minutes.

captest writes its files next to the NRO, in `BEVY_ASSET_ROOT`, or in `sdmc:/switch/switch_captest/` when that isn't set:

| File | What it holds |
|---|---|
| `captest_results.txt` | One line per check, synced as it's written, then the summary |
| `captest_results.prev.txt` | The previous run's results |
| `log.txt` | bevy_horizon's log, with every result line too, prefixed `[Captest] `. It's synced about once a second ([logging](logging-and-crash-reports.md)) |
| `panic.txt` | Written by bevy_horizon's panic hook |
| `captest_running.txt` | The check in progress; left behind if the app dies |
| `captest_skip.txt` | Checks to skip, one id per line. Delete it to retry them |
| `assets/` | Copied from RomFS on the first run, and again when a file's size differs |

Each line of `captest_results.txt` gives the seconds since start, the status, the check's id and what it found:

```text
   87.6s WARN  stick-center                 released sticks read 0.081 (Bevy's default dead zone is 0.05)
```

The file ends with a summary line, then every FAIL, WARN and CRASH again, indented. The note in brackets is `finished`, or `closed from HOME` when HOME closed captest early:

```text
==== SUMMARY: 179 pass, 0 fail, 2 warn, 73 info, 1 skip, 0 crash (finished) ====
```

## How captest protects a run

A check that crashes or hangs costs one launch, not the run. The sources are in `examples/switch_captest/src/`.

| Guard | What it does | Source |
|---|---|---|
| [Crash marker](glossary.md#crash-marker) | Before each check, `captest_running.txt` names it. If the file is still there at the next start, that check is recorded as CRASH and added to `captest_skip.txt` | `report.rs` (`init`, `begin`) |
| [Skip list](glossary.md#skip-list) | Checks named in `captest_skip.txt` are recorded as SKIP. Delete the file to retry them | `report.rs` (`init`, `begin`) |
| [Watchdog](glossary.md#watchdog) | Each check has a time limit. Past it, the watchdog records HANG and ends the app, and the next run skips that check. Time in HOME doesn't count | `report.rs` (`start_watchdog`, `arm`, `fire`) |
| Frame watch | During the Bevy app, 15 s without a finished frame is a HANG | `report.rs` (`kick_frame`) |
| Panic hook | Stops the watchdog and records PANIC. The marker stays, so the next run skips that check. bevy_horizon's hook writes `panic.txt` | `report.rs` (`install_panic_hook`) |
| Single-threaded fallback | If `bevy-executor` crashed before, the next run records WARN `bevy-single-threaded` and runs every schedule on `SingleThreadedExecutor` | `main.rs` (`main`), `bevy_phase.rs` (`build_app`) |
| Own Bevy loop | `run_app` runs `ScheduleRunnerPlugin`'s 60 Hz loop itself and hands the App back, so it isn't dropped inside `App::run`. `drop_app` drops it last, as `bevy-app-drop`, with a 20 s limit | `bevy_phase.rs` (`run_app`, `drop_app`) |
| Exit lock | Closing captest from HOME asks it to exit. It skips the phases left, and has 15 s to write its summary and clear the marker | `main.rs` (`main`) |

The own loop exists because dropping a Bevy App closes its audio stream, and with an earlier audio host that played through [SDL2](glossary.md#sdl2), that close never returned in Ryujinx ([why not SDL2](input-and-audio.md#why-not-sdl2)). The current host, which feeds [audren](glossary.md#audren), waits at most 1 s to close; the own loop still times the drop as a check of its own ([quitting: the App drop](lifecycle-and-exit.md#quitting-the-app-drop)).

## The HOME stage

The Bevy app's sixth stage, `bevy-suspend`, checks what [HOME and focus](lifecycle-and-exit.md#home-and-focus) describes. When asked, press HOME, wait 3 seconds, and go back into captest. B skips the stage, and so does waiting 60 s without pressing HOME. The first stage, `bevy-executor`, adds one check of how the app starts.

| Check | Passes when | What a failure means |
|---|---|---|
| `lifecycle-startup` | The first two `AppLifecycle` messages were `WillResume`, then `Running`, as winit sends them | The lifecycle messages aren't sent, or not in winit's order |
| `suspend-hooks` | [libnx](glossary.md#libnx)'s focus hook ran at least once for losing focus and once for getting it back. The line also counts resumes, which stay at 0: they need `SetRestartMessageEnabled`, which nothing turns on | HOME isn't reported: the [focus-handling mode](glossary.md#focus-handling-mode) isn't `NoSuspend` |
| `suspend-messages` | Bevy got, in this order: unfocused, `WillSuspend`, `Suspended`, focused, `WillResume`, `Running` | A game wouldn't learn that it was left, or would learn it in the wrong order |
| `suspend-gap` (INFO) | Always: the longest time between two frames, which is the time away | |
| `suspend-virtual-clamp` | `Time<Virtual>` moved at most 0.26 s in one frame | Game time would jump by the whole time away |
| `suspend-audio` | You heard the chime played after coming back | Sound doesn't come back after HOME |

In an emulator the stage can only be skipped ([what the emulator gets wrong](#what-the-emulator-gets-wrong)).

## What passes on hardware

On a Switch Lite at [handheld](glossary.md#handheld-mode) clocks, with Bevy 0.19 and captest's default features, a full run ends with `179 pass, 0 fail, 2 warn, 73 info, 1 skip, 0 crash`, and `log.txt` ends with `[Exit] PASS`.

- **`fs-unicode-name` (WARN):** a file with a non-ASCII name can be created and read by name, but listing its folder doesn't show it. Keep ASCII names for files your game finds by listing a folder, such as saves.
- **`stick-center` (WARN):** released sticks read 0.081, against Bevy's default dead zone of 0.05. Give the sticks a dead zone of your own ([sticks](input-and-audio.md#sticks)).
- **The skip** is `rumble-felt`: a Switch Lite has no rumble motors.

That run predates the current audio host: its sound checks went through the SDL2-based host that the audren hosts replaced ([the audio host](input-and-audio.md#the-audio-host)). With the current Bevy 0.19 host, the `bevy-audio` stage and the App drop have passed only in the emulator. [Docked mode](glossary.md#docked-mode), a Pro Controller and detached Joy-Con are untested.

## render_check

`examples/render_check` checks Bevy's 3D path on the Vulkan stack: six cameras, each with one render setting changed, draw the same four cubes, and it checks every cube on a screenshot. Build it with `scripts/build_example.sh render_check` and copy it to `sdmc:/switch/render_check/render_check.nro`, where it also reads `options.txt` and saves its screenshots.

At frame 240 it logs one `[Check]` line per camera and a total such as `[Check] screenshot 1: 24/24 cubes drawn, mesh preprocessing on the GPU`, and saves `render_check_1.png`. (A) checks again; (+) exits. On a Switch Lite all 24 cubes draw; stock Ryujinx can't run it. Its options and results are in [rendering](rendering.md#render_check).

## Testing in an emulator

Stock Ryujinx ([Ryubing](glossary.md#ryubing) 1.3.3) can't run the Vulkan builds, which are hello_switch, breakout, render_check and games built with bevy_horizon's `vulkan` feature: NVK hits emulator bugs at startup and in the first frames ([what the emulator gets wrong](rendering.md#what-the-emulator-gets-wrong)). captest draws through EGL and OpenGL ES instead, so it runs in stock Ryujinx, with the differences [below](#what-the-emulator-gets-wrong). Other emulators are untested. The project's own emulator runs use a Ryujinx with local fixes for the NVK bugs, which aren't published.

An emulator finds bugs in your own code fast, but it isn't a Switch. Treat the console as the answer, and the emulator's speed as no guide to the console's.

### Setting up Ryujinx

- **Memory manager mode:** Settings → CPU → Memory Manager Mode → Software. The default, Host Unchecked (`HostMappedUnsafe`), crashed Ryujinx within 4 s of starting captest.
- **Sound:** Ryujinx plays sound on the PC through SDL2. On a PC without a sound device, start Ryujinx with `SDL_AUDIODRIVER=disk SDL_DISKAUDIOFILE=/dev/null`. Don't use `SDL_AUDIODRIVER=dummy`: nothing drains Ryujinx's audio buffer then, and it grew about 1.3 GB every 5 s until Ryujinx ran out of memory.
- **Memory:** captest takes about 4.7 GB of RAM in Ryujinx.
- **Keys:** NROs need none. Ryujinx logs `Keys not found` and runs homebrew anyway.
- **Controller:** map yours to Player 1 or to handheld; bevy_horizon reads both, and Ryujinx reports a Pro Controller.

### Reading the results

- **Find the files on the emulated SD card.** Ryujinx starts an NRO without a [loader](glossary.md#loader), so captest writes to `sdmc:/switch/switch_captest/`, which is `sdcard/switch/switch_captest/` in Ryujinx's data folder (File → Open Ryujinx Folder).
- **Judge an exit from the app's own `log.txt`,** not from Ryujinx's window, since there is no homebrew loader to return to. `[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it` means the exit path ran to the end.
- **Read `captest_results.txt` for the last moments before a crash.** It's synced after every line. `log.txt` is synced about once a second, so its last lines can be lost when Ryujinx dies.
- **Use `captest_skip.txt` to reach later stages.** Put a check's id on a line of its own to skip it, for example `fs-errors`, whose folder deletes crash Ryujinx. After any crash, starting captest again skips the check that was running.

## What the emulator gets wrong

| Area | Ryujinx | What it means |
|---|---|---|
| Rendering | Stock Ryujinx can't run the Vulkan builds: NVK hits emulator bugs at startup and in the first frames | Test Vulkan builds on the console. [Rendering](rendering.md#what-the-emulator-gets-wrong) lists the bugs |
| Speed | Depends on the PC | Measure frame times and load times on the console |
| Focus | Only ever reports `InFocus`, and has no HOME button | The HOME stage can only be skipped. Test HOME and sleep on the console |
| Audio | Plays the sound on the PC through SDL2, so `SDL_AUDIODRIVER` decides where it goes | With no sound device, or SDL's disk driver, there's nothing to hear: `BH_AUDIO_STATS=1` shows whether the audio renderer is fed ([the audio host](input-and-audio.md#the-audio-host)). SDL's dummy driver runs Ryujinx out of memory ([setting up](#setting-up-ryujinx)) |
| Networking | Non-blocking connect gives `EAGAIN`; UDP returns a short address and std asserts; DNS is off; IPv6 bind fails | `net-*` failures say nothing about the console |
| Files | Deleting a non-empty folder crashes Ryujinx; `statvfs` reports the PC's disk | Skip-list `fs-errors`. Free-space figures are the PC's |
| System | Without firmware, reports system version 3.0.0 | The version captest logs isn't a console's |
| Loader | Starts the NRO without a loader, and without `argv[0]` | No `[Boot]` lines. bevy_horizon opens `log.txt` in `main` instead, in `sdmc:/switch/<name>/`, where `<name>` is what the app passes to `init_switch_logging`, and points `BEVY_ASSET_ROOT` there |
| RomFS | A bevy_horizon NRO without a RomFS makes stock Ryujinx abort at startup: with no loader, libnx asks Ryujinx's file-system service for the RomFS, and Ryujinx throws | Pack a RomFS ([platform notes](platform-notes.md)). captest has one |

## Source map

| Area | Source |
|---|---|
| Phases and their order, the exit lock, the single-threaded fallback | `examples/switch_captest/src/main.rs` (`main`) |
| Results, crash marker, skip list, watchdog, frame watch, panic hook, summary | `examples/switch_captest/src/report.rs` (`init`, `record`, `begin`, `section`, `arm`, `kick_frame`, `start_watchdog`, `fire`, `install_panic_hook`, `finish`) |
| The Bevy app's stages, the button prompts, the own loop, the App drop | `examples/switch_captest/src/bevy_phase.rs` (`STAGES`, `BUTTONS`, `build_app`, `drive`, `run_app`, `after_run`, `drop_app`) |
| Interactive checks | `examples/switch_captest/src/interactive.rs` (`interactive`, `sound`, `rumble`, `touch`, `motion`) |
| SD card, RomFS, asset install, memory maps | `examples/switch_captest/src/files.rs` (`sd_card`, `romfs`, `install_assets`, `memmap`) |
| System, threads, memory | `examples/switch_captest/src/platform.rs` (`system_info`), `threads.rs` (`threads`, `core_policies`), `memory.rs` (`memory`) |
| Network | `examples/switch_captest/src/net.rs` (`network`) |
| Native crates, Bevy's task pools | `examples/switch_captest/src/native.rs` (`native`, `bevy_pools`) |
| GPU | `examples/switch_captest/src/gpu.rs` (`gpu`, `ask_seen`) |
| libnx bindings | `examples/switch_captest/src/nx.rs` |
| Features | `examples/switch_captest/Cargo.toml` |
| The build | `scripts/build_example.sh` |
| render_check | `examples/render_check/src/main.rs` |
