# Input and audio

What bevy_horizon provides for controllers and sound, what has been proven on the console, and what captest checks. Short on purpose: most of it is waiting for captest's first full hardware run.

## In short

- **Input is one Bevy gamepad.** `HorizonInputPlugin` creates it at startup (`crates/bevy_horizon/src/input.rs:40`). Every frame it reads libnx's pad state in `PreUpdate`, before Bevy's input systems (`input.rs:36`, `:107`), and sends Bevy's raw gamepad button and axis events.
- **Buttons map by position, not by letter.** Nintendo A is Bevy's East, B is South, X is North, Y is West (`input.rs:177-180`). A game that jumps on `South` gets the bottom button: B on the Switch, A on an Xbox pad.
- **Proven on hardware: A and +, in handheld.** hello_switch's log on the Switch shows `[Input] controller styles now 0x2` (handheld), then `(A) pressed` and `(+) pressed: exiting`. The other buttons and the sticks wait for captest's prompts.
- **Touch, motion and rumble aren't in bevy_horizon.** captest tests them by calling libnx directly, before Bevy starts (`examples/switch_captest/src/interactive.rs`). That tells us what works before we add them.
- **Audio goes bevy_audio → rodio → cpal → the Switch's audio renderer (`audren`).** Our cpal patches add a Horizon host that feeds `audren`, through libnx's audio driver, from a thread of its own: `patches/cpal` (cpal 0.17.3) for Bevy 0.19 and `patches/bevy-0.18/cpal` (cpal 0.15.3) for Bevy 0.18, the same code on each API ([the audio host](#the-audio-host)). Both used to go through SDL2, whose Switch backend can stop the sound for good ([why not SDL2](#why-not-sdl2)).
- **Audio worked on hardware with Bevy 0.19 through SDL2.** captest's beeps and bevy_audio's
  chime were heard, and sound came back after HOME ([platform notes](platform-notes.md)).
  **The Bevy 0.18 audren host has since played on the Switch, in Skate 3:** clean sound in
  the hill launches and about 8½ minutes of logged play, with 1024- and 2048-frame buffers.
  It never ran dry; you reported sound recovering after HOME and sleep, and at exit it closed
  audren in 19.0 ms and the heap check passed (perf35, findings 54). The 0.19 host and captest
  haven't run on hardware with it yet. In Ryujinx, Skate 3 (Bevy 0.18) plays through loading
  and gameplay (runs au5, au6), where SDL stopped after 6 s (run au4), and at exit the host
  closes audren and the heap check passes (au6). With Bevy 0.19, captest's one-shot chime ends
  on time and `bevy-app-drop` passes in 542 ms (run au7).

## What captest checks

| Area | Checks | How |
|---|---|---|
| Buttons | 16 prompts, "Press {button}", 20 s each; two unanswered in a row skip the rest (`bevy_phase.rs:626`, `:658`) | Through Bevy's `Gamepad`, so the mapping is tested too |
| Sticks | Left stick up, then right stick right (15 s each); spin both for 5 s; let go | Through Bevy's `Gamepad` |
| Touch | One finger, then two, dragging into the corners (8 s) | libnx directly |
| Motion | Hold still for 2 s, then tilt and turn for 4 s | libnx directly |
| Rumble | Both Joy-Con rumble for 1 s, then stop; you answer | libnx directly |
| Sound, through cpal | A beep on the left, then the right, then a higher beep on both; you answer. Then `audio-reopen` closes the stream and plays a second one (`interactive.rs:281-291`) | cpal directly, through our patch; 120 s limit |
| Sound, through Bevy | A chime that despawns itself; a repeating chime that pauses once, then stops when despawned; a chime after HOME and back | bevy_audio, through our cpal patch |
| Closing audio | `bevy-app-drop`: drops the Bevy App, which closes the audio device; 20 s limit | Runs last, after everything else is saved |

**Closing a stream is the first place a hang could show.** Closing a cpal stream stops the host's thread, which closes the renderer, and waits at most 1 s for it ([Quitting: the App drop](lifecycle-and-exit.md#quitting-the-app-drop)). The hands-on sound test does that before Bevy ever starts. If something hangs anyway, captest's watchdog writes `HANG sound-cpal` after 120 s and closes the app; you land on HOME. Run captest again: it skips that test and carries on. No Ryujinx run has reached this test. On hardware nothing hung with SDL: captest's App drop, audio close included, took 110 to 146 ms in runs 1 to 4 (findings 16, 17). The audren host hasn't run captest yet.

Prompts and answers: `iw4l-switch-findings/15-CAPTEST-IN-THE-EMULATOR-AND-YOUR-STEPS.md` §9.

## The audio host

`patches/cpal/src/host/horizon/mod.rs` (Bevy 0.19) and `patches/bevy-0.18/cpal/src/host/horizon/mod.rs` (Bevy 0.18) are the same host on cpal 0.17.3's and 0.15.3's APIs. Line numbers below are the 0.19 file's.

- **One stream at a time: f32, one or two channels.** It opens `audren` as SDL did: 48 kHz output, 24 voices, one sink and a two-channel final mix (`CONFIG`, `:83`). The stream is voice 0, 16-bit PCM at the stream's rate (`Output::open` and `start`, `:195-290`).
- **A thread of the stream's own feeds it** (`run`, `:454-525`). It keeps three buffers queued. Every renderer frame (5 ms; it waits at most 10 ms for one, `:520`), it refills whichever buffers the renderer has finished, oldest first, through the game's callback, converts them to 16-bit and sends them in one update. A thread that runs late leaves a gap in the sound, then carries on.
- **The thread runs at priority 0x2B** (`set_priority`, `:738-753`), ahead of the 0x3B that every thread a Bevy game starts gets. On a busy core those take turns of about 10 ms, against a 21 ms buffer.
- **It links libnx itself** (`patches/cpal/build.rs:17-20`, `patches/bevy-0.18/cpal/build.rs:22-25`), so a game needs nothing in its own `build.rs` for sound.
- **It closes, at exit, a stream the game never dropped.** Bevy 0.18's bevy_audio leaks its stream, and Bevy 0.19's goes only with the App; see [the audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit).

It reads three environment variables when the stream is built (`:16-23`):

| Variable | Default | What it sets |
|---|---|---|
| `BH_AUDIO_PRIORITY` | `0x2B` | The audio thread's priority, decimal or `0x` hex. `default` keeps the one it starts with. |
| `BH_AUDIO_FRAMES` | what the stream asks for | Each of the three buffers, 256 to 8192 frames, whatever the stream asks for. rodio 0.22 (Bevy 0.19) asks for 2048 (42.7 ms at 48 kHz); rodio 0.20 (Bevy 0.18) asks for the default, which is 1024 (21.3 ms). |
| `BH_AUDIO_STATS` | off | `1`: an `[Audio]` line every 10 s: how long the callbacks took, the longest time between two, how many gaps were over 1.5 buffers, and how often the renderer ran dry, each a gap in the sound. |

It logs the stream once, then the priority change, then with `BH_AUDIO_STATS=1` a line every 10 s (Skate 3 in Ryujinx, run au5):

```text
[Audio] audren output: 48000 Hz, 2 channels, f32 to 16-bit, 3 buffers of 1024 frames (21.3 ms); audio thread priority 0x2b; stats every 10 s
[Audio] audio thread priority 0x3b -> 0x2b
[Audio] 10.0 s: 469 callbacks of 21333 us, 482 us average, 1298 us slowest, 2.3% of a core; longest gap 38.4 ms, 1 over 1.5 buffers; ran dry 0 times
```

469 callbacks in 10 s is real time (468.75). While Skate 3 loaded, Ryujinx held the thread off for up to 436 ms, and it ran dry up to 9 times in 10 s; the sound came back each time (runs au5, au6). In gameplay, Ryujinx slows the whole guest about 40 times (au6's perf steps: 750 ms a frame on the main thread), and the callbacks with it, to about 20 ms each and 75 to 94% of a core. That is the emulator's host running out of CPU, not a hardware number.

On the Switch, Skate 3's lines read like this in play (perf35, findings 54), here with `BH_AUDIO_FRAMES=2048`:

```text
[Audio] 10.0 s: 235 callbacks of 42666 us, 6954 us average, 9653 us slowest, 16.3% of a core; longest gap 45.5 ms, 0 over 1.5 buffers; ran dry 0 times
```

At the default 1024 frames, steady hill callbacks took 3.1 to 3.4 ms on average,
14.6 to 16.0% of a core, with longest gaps of 25.4 to 29.6 ms over about two minutes of
sound. The final partial interval averaged 2.9 ms. At 2048, about 8½ minutes of steady
play took 6.4 to 9.7 ms per callback, 15.1 to 22.8% of a core, with longest gaps of
45.4 to 46.2 ms. Loading's longest gaps were 34.8 ms at 1024 and 54.7 ms at 2048.
Neither log recorded an underrun, and both sounded clean. A callback is the game's whole
mix, so these are Skate 3's costs, not pure host overhead: its mixer accounts for most of
them (findings 54).

### Why not SDL2

Both hosts used to open an SDL2 audio device: devkitPro's switch-sdl2 2.28.5-4, still in the toolchain image. Its Switch backend can stop the sound for good, and then never close:

- **`SWITCHAUDIO_PlayDevice` waits for the buffer it just queued to start playing.** It loops `audrvUpdate`, `audrenWaitFrame` until libnx reports that buffer `Playing` (disassembled from `/opt/devkitpro/portlibs/switch/lib/libSDL2.a`).
- **libnx can skip `Playing`.** One update marks every buffer the renderer has finished `Done`, and only the next one still queued `Playing` (libnx v4.12.0 `nx/source/audio/voice.c:225-236`). SDL plays from two buffers, 1024 frames each for Skate 3. If its thread is held off for about a buffer, 21 ms there, the renderer can start and finish the new one between two updates. It goes from queued straight to `Done`, and the loop never ends.
- **Then the game is silent, and closing the device waits forever.** The thread never calls the game's callback again, and the loop never looks at the shutdown flag that `SDL_CloseAudioDevice` sets before it joins the thread.

Ryujinx showed it in run au4, with its audio renderer logging what it was sent: the renderer consumed 288 buffers in the first 6.1 s, then none, while SDL's thread kept sending about 182 updates a second. The game's callback never ran again, so it logged no `[Audio]` stats line. That is also why closing a device never returned in Ryujinx (captest run 12), not that Ryujinx's renderer stopped sending frames, as these docs said before. On the Switch it needs the audio thread held off for a buffer at the wrong moment: HOME, a load spike or a busy core. captest's hardware runs didn't hit it.

## Known gaps

- **One player.** `padConfigureInput(1, standard styles)` (`input.rs:151`) reads player 1 or handheld. The Switch reported handheld (style 0x2); Ryujinx reports a Pro Controller (0x1).
- **Docked mode isn't a target for now.** Testing is handheld only; docked is expected to work and run faster.
- **An open audio stream keeps `audren`'s work buffer**, taken from the heap, until it's closed. If it's still open at exit, the exit hook leaves through AM, to HOME, not back to the homebrew menu ([lifecycle](lifecycle-and-exit.md#rules-for-services-you-add-later)). Both hosts close streams still open at exit for this reason.
- **The Bevy 0.19 audren host hasn't run on hardware, and captest hasn't run with either.** The 0.18 host has, in Skate 3 (perf35, findings 54): no crackle, never ran dry, sound after HOME and after sleep, and a clean exit. For 0.19, captest's prompts check the same, and its `bevy-app-drop` times the close.
