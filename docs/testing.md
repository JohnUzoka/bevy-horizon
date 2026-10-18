# Testing on the emulator and the console

This page covers `switch_captest`, the capability-test NRO, and the Ryujinx harness used to run it and the other examples before they go to a real Switch. Read it before adding a check, or before trusting an emulator result. The full run-by-run record is in findings `15-CAPTEST-IN-THE-EMULATOR-AND-YOUR-STEPS.md`, `16-HARDWARE-RUNS-TRIANGLE-HOME-AND-EXIT.md` and `17-HARDWARE-RUNS-3-4-WARNINGS-AND-NVK-TRIANGLE.md`. Vulkan runs (hello_switch, render_check) are in [rendering.md](rendering.md#what-has-been-seen).

## In short

- **captest checks what a Bevy game needs, one item at a time.** About 180 checks across the system, threads, memory, the SD card, hands-on input and sound, RomFS, networking, native crates, Bevy's task pools, a Bevy app in 7 stages, and the GPU (EGL and OpenGL ES) (`examples/switch_captest/src/main.rs:86-160`).
- **Every check is one line in `captest_results.txt`, synced as it's written.** Status words: PASS, FAIL, WARN, INFO, SKIP, and CRASH, HANG or PANIC for a check that stopped the program.
- **A crash or hang doesn't cost the run.** The next launch marks the check that was running as CRASH, adds it to `captest_skip.txt`, and carries on past it (`report.rs:86-135`).
- **Ryujinx finds bugs in our code fast, but it isn't a Switch.** Its audio, networking, file system and focus differ from the console, and it has no HOME button. Treat hardware as the answer. Runs now finish: since the harness gave SDL its disk audio driver, Ryujinx levels off at about 4.7 GB (runs 20 and 21).

## Building and running captest

```text
JOBS=4 scripts/build_example.sh switch_captest      # writes switch_captest.nro at the repo root
```

Copy it to `sdmc:/switch/switch_captest/` and start it from the homebrew menu. It writes its files next to the NRO:

| File | What it holds |
|---|---|
| `captest_results.txt` | One line per check; synced after each line |
| `captest_results.prev.txt` | The previous run's results |
| `log.txt` | bevy_horizon's log. Its log writer thread writes lines as they come and fsyncs once a second; reports and exit lines are written and synced on their own thread ([logging](logging-and-crash-reports.md)) |
| `panic.txt` | Written by the panic hook |
| `captest_running.txt` | The check in progress; left behind if the app dies |
| `captest_skip.txt` | Checks to skip; delete it to retry them |
| `assets/` | Unpacked from RomFS on first run; reinstalled when a size differs (`files.rs:525`) |

## How captest protects a run

| Guard | What it does | Source |
|---|---|---|
| Resume marker | `captest_running.txt` names the running check. If it's there at start, that check is marked CRASH and skipped from then on | `report.rs:86-135` |
| Per-check watchdog | Each check has a time limit. Past it, the watchdog writes HANG and ends the process. It only counts while the app has focus, so HOME doesn't trip it | `report.rs:329-380` |
| Frame watch | During the Bevy stages, no finished frame for about 15 s is a HANG | `report.rs:329-336` |
| Panic hook | Stops the watchdog, writes `panic.txt`, marks the check PANIC | `report.rs:411-414`, `:460-461` |
| Own Bevy loop | `run_app` copies `ScheduleRunnerPlugin`'s 60 Hz loop but hands the App back, so it isn't dropped inside `App::run`. `drop_app` drops it last, as the `bevy-app-drop` check with a 20 s limit | `bevy_phase.rs:928`, `:952` |

The own loop exists because dropping a Bevy App closes the audio stream, and with the SDL host before it, in Ryujinx, that never returned ([why not SDL2](input-and-audio.md#why-not-sdl2)). The audren host's close waits at most 1 s; the own loop still times the drop on its own. See [Quitting: the App drop](lifecycle-and-exit.md#quitting-the-app-drop).

## The HOME stage

The Bevy app's `bevy-suspend` stage, the sixth of seven, asks you to press HOME, wait 3 seconds and go back into the app; B skips it, and so does waiting 60 s. It checks what [HOME and focus](lifecycle-and-exit.md#home-and-focus) describes. The first stage, `bevy-executor`, adds one check for the app's start.

| Check | Passes when | What a failure means |
|---|---|---|
| `lifecycle-startup` | The first two `AppLifecycle` messages were `WillResume`, then `Running`, as winit sends them | The lifecycle messages aren't sent, or not in winit's order |
| `suspend-hooks` | libnx's focus hook ran at least once for losing focus and once for getting it back; the line also counts `Resume` messages, which nothing asks for | HOME isn't reported: the focus-handling mode isn't `NoSuspend` |
| `suspend-messages` | Bevy got, in this order: unfocused, `WillSuspend`, `Suspended`, focused, `WillResume`, `Running` | The game wouldn't learn it was left, or would learn it in the wrong order |
| `suspend-gap` (INFO) | Always: the longest time between two frames, which is the time away | |
| `suspend-virtual-clamp` | `Time<Virtual>` moved at most 0.26 s in one frame | Game time would jump by the whole time away |
| `suspend-audio` | You heard the chime played after coming back | Sound doesn't come back after HOME |

In Ryujinx the stage can only be skipped: Ryubing gives the app `InFocus` at boot and after a library applet closes, and nothing else.

## The Ryujinx harness

The harness lives outside this repo, in `~/ryujinx-bh/`. `run.sh` runs inside a `nixos/nix` container, under Docker or Podman: Ryujinx from Nix, a virtual X screen, `xdotool` for key presses at set seconds, a screenshot after each press, and the container's memory once a second.

```text
docker run --rm --name ryuN --memory 7800m --memory-swap 12000m \
  -e RYU_BIN=/work/ryupatch/Ryujinx.sh \
  -v ryunix:/nix -v ~/ryujinx-bh:/work:z docker.io/nixos/nix:latest \
  sh /work/run.sh <tag> /work/data/sdcard/switch/switch_captest/switch_captest.nro <seconds> [<at> <key>]...
```

`RYU_BIN` runs the patched copy of Ryubing in `ryupatch/ryubing/`; Vulkan builds don't get a device without it ([What the emulator gets wrong](rendering.md#what-the-emulator-gets-wrong)). Keys are xdotool names: A = `z`, B = `x`, X = `c`, Y = `v`, + = `p`, − = `minus`, and `minus+p` presses both. A key the emulator doesn't map, such as `n`, presses nothing and only takes a screenshot. Output goes to `out/<tag>/`: `ryujinx.out` (the emulator's log), `mem.txt` (seconds, bytes, wall clock, swap bytes), `result.txt` (exit status and peak memory), and the screenshots. The guest's SD card is `data/sdcard/`.

Practical rules:

- **Don't build while it runs.** The host has about 16 GB of RAM and 4 GB of swap. Ryujinx takes about 4.7 GB for captest and up to 8.2 GB for a Vulkan build (render_check, run rc2).
- **Long Skate 3 runs outgrow the cap.** Once the test world draws frames, Ryujinx's memory grows by about 38 MB/s past `--memory` into swap. Run twdrv2 (`--memory 7000m`) filled the host's 4 GB of swap, and the kernel killed Ryujinx at 299 s (`result.txt`: `exited by itself at 299s, status 137`). Ryujinx caps its texture cache at half the GPU memory lavapipe reports, which is the host's RAM (`ryusrc/src/Ryujinx.Graphics.Gpu/Image/AutoDeleteCache.cs`). For longer runs, raise `--memory`, or the swap.
- **The run always lasts the full time.** With `--no-gui`, Ryujinx stays open after the app exits, so `result.txt` says "still running". Judge the exit from the app's own `log.txt` (for example render_check's `[Exit] PASS`).
- **Read `captest_results.txt`, not `log.txt`, for a run's last moments.** When the container is killed, Ryujinx loses whatever `log.txt` writes it hadn't flushed to the host.
- **Use `captest_skip.txt` to reach later stages.** One run can't get through everything.
- **NROs don't need keys.** Ryujinx prints `Keys not found`; homebrew runs anyway.

## What the emulator gets wrong

| Area | Ryujinx | Effect |
|---|---|---|
| Memory | With SDL's dummy audio driver, nothing drained Ryujinx's audio ring buffer, which grew about 1.3 GB every 5 s until Ryujinx ran out of memory. `run.sh` now gives SDL its disk driver writing to `/dev/null` (`RYU_SDL_AUDIO`, `SDL_DISKAUDIOFILE`) | Runs finish; the container levels off at about 4.7 GB |
| Focus | Only ever reports `InFocus`, and has no HOME button | The HOME stage can only be skipped |
| Audio | Under `run.sh`, Ryujinx's own SDL plays the sound into `/dev/null` with its disk driver | Nothing to hear; `BH_AUDIO_STATS=1` shows whether the renderer was fed. The "sound starts but never finishes" of earlier runs, which hung the `bevy-audio` checks and the App drop, was the SDL in our cpal host, not Ryujinx ([why not SDL2](input-and-audio.md#why-not-sdl2)); with the audren host both pass (run au7) |
| Networking | Non-blocking connect gives `EAGAIN`; UDP returns a short address and std asserts; DNS is off; IPv6 bind fails | `net-*` failures that say nothing about the Switch |
| Files | Deleting a non-empty folder crashes Ryujinx; rename over a file gives `EEXIST`; `statvfs` reports the host disk | `fs-errors` is on the emulator's skip list |
| System | Version 3.0.0, no firmware; `available_parallelism` fails | captest asks Bevy for at least 4 threads |
| Loader | Starts the NRO without a loader | No `[Boot]` lines |
| Rendering | Needs seven local fixes for NVK: five before it runs at all, one for the address-space allocator, one so indirect draws work with wgpu's indirect-call validation on; the host GPU is lavapipe, a software driver, so frames take 90 to 200 ms; texture readback is wrong past the first MiB; one run died with a segmentation fault that didn't come back | See [rendering.md](rendering.md#what-the-emulator-gets-wrong). Judge tiles from the harness's own screenshots, not render_check's |

Its faster memory mode, `HostMappedUnsafe`, crashes Ryujinx within 4 s.

## Source map

| Area | Files |
|---|---|
| Phases and order | `examples/switch_captest/src/main.rs:86-160` |
| Reporting, resume, watchdog, panic hook | `examples/switch_captest/src/report.rs` |
| Bevy stages, `run_app`, `drop_app` | `examples/switch_captest/src/bevy_phase.rs:25-33`, `:928-968` |
| Hands-on prompts | `examples/switch_captest/src/interactive.rs` |
| GPU checks | `examples/switch_captest/src/gpu.rs` |
| Harness | `~/ryujinx-bh/run.sh` (outside this repo) |
