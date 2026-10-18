# Input and audio

This page covers controllers and sound: how bevy_horizon turns the Switch's buttons and sticks into a Bevy gamepad, how a game's sound reaches the speakers, the settings that tune it, and what has been tested. Read it when you wire up input or audio in a port, or when sound misbehaves.

## In short

- **Input is one Bevy gamepad.** `HorizonPlugin` spawns it at startup and fills it from the Switch's [HID](glossary.md#hid) service every frame. Read it with Bevy's `Gamepad`, as on any platform.
- **Buttons map by position, not by letter.** Nintendo A is Bevy's `East` and B is `South`. A game that jumps on `South` gets the bottom button: B on the Switch, A on an Xbox pad.
- **Give the sticks a dead zone of your own, about 0.1.** `Gamepad::left_stick()` returns the raw value, and released sticks on the tested Switch Lite read about 0.08.
- **Touch, motion, rumble and keyboards aren't in bevy_horizon.** A game can call [libnx](glossary.md#libnx) for [touch](glossary.md#touch-screen) and [motion](glossary.md#motion-sensor), which work on the console. [Rumble](glossary.md#rumble) is untested, because a Switch Lite has no rumble motors.
- **Sound works through bevy_audio as usual.** bevy-horizon patches [cpal](glossary.md#cpal) with a host that feeds the Switch's audio renderer, [audren](glossary.md#audren). `scripts/build_game.sh` applies the patch that matches the Bevy version in your `Cargo.lock`.
- **Look for `[Audio] audren output` in `log.txt`.** If bevy_audio logs `No audio device found.` instead, the patch usually isn't applied.
- **Set the `BH_AUDIO_*` variables before you add `DefaultPlugins`.** The host reads them when bevy_audio opens its stream. If `BH_AUDIO_STATS=1` shows the sound running dry, raise `BH_AUDIO_FRAMES` to 2048 or 4096.
- **Tested on a Switch Lite, in [handheld mode](glossary.md#handheld-mode):** all 16 buttons and both sticks pass [captest](glossary.md#captest), and the Bevy 0.18 audio host plays cleanly in [a full 3D Bevy 0.18 game](https://github.com/JohnUzoka/skate-3-rust-engine/tree/horizon-port). The Bevy 0.19 audio host has passed only in the [emulator](glossary.md#emulator) so far.

## Controllers

`HorizonPlugin` adds `HorizonInputPlugin` (`crates/bevy_horizon/src/input.rs`). At startup, `setup_horizon_gamepad` spawns one gamepad entity, connected as "Nintendo Switch Controller" (vendor 0x057e, product 0x2009). Every frame, `poll_horizon_input` reads the [pad](glossary.md#pad) through libnx in `PreUpdate`, before Bevy's `InputSystems`, and writes Bevy's raw gamepad events, so `Gamepad` is up to date in the same frame:

```rust
fn player_input(gamepads: Query<&Gamepad>) {
    for gamepad in &gamepads {
        if gamepad.just_pressed(GamepadButton::South) {
            // B on the Switch
        }
        let stick = gamepad.left_stick(); // raw: apply your own dead zone
        if stick.length() > 0.1 {
            // move
        }
    }
}
```

### The button map

| Switch | Bevy `GamepadButton` |
|---|---|
| A, B, X, Y | `East`, `South`, `North`, `West` |
| L, R | `LeftTrigger`, `RightTrigger` |
| ZL, ZR | `LeftTrigger2`, `RightTrigger2` |
| +, − | `Start`, `Select` |
| D-pad | `DPadUp`, `DPadDown`, `DPadLeft`, `DPadRight` |
| Left and right stick clicks | `LeftThumb`, `RightThumb` |

- **Positions match an Xbox pad's.** A game written for one keeps its layout. If you want Nintendo's convention, A to confirm and B to go back, use `East` to confirm on the Switch.
- **Each button reads 1.0 or 0.0** and is sent when it changes. ZL and ZR are buttons on Switch controllers, so they never read in between.
- **SL and SR aren't mapped, and HOME goes to the system.** A game learns about HOME through focus and lifecycle messages ([HOME and focus](lifecycle-and-exit.md#home-and-focus)).
- **The FPS overlay uses − and +.** With the `fps_overlay` feature (Bevy 0.19), `HorizonFpsOverlayPlugin` shows or hides Bevy's FPS overlay when you hold − and press +. If + also pauses or quits your game, ignore it while `bevy_horizon::fps_chord_held` returns true ([the FPS overlay](rendering.md#the-fps-overlay)).

### Sticks

- **Range:** each axis is the HID value divided by 32767, clamped to −1.0..1.0, with +y up. On the tested Switch Lite both sticks reach 1.0 in every direction, and diagonals stay on a circle: the longest reading in any direction was 1.01.
- **Changes under 0.01 aren't sent.**
- **Use your own dead zone.** `Gamepad::left_stick()`, `right_stick()` and `get()` return the value as sent. Bevy's `GamepadSettings` dead zone (±0.05 by default) applies only to `GamepadAxisChangedEvent`. Released sticks on the tested console read 0.081, so a dead zone of about 0.1 keeps a character still.

### Which controller is read

- **One player.** On its first run, `poll_horizon_input` configures HID for one player and the standard controller styles (`hid_ffi::NPAD_STYLE_SET_STANDARD`: Pro Controller, handheld, dual Joy-Con, and a single left or right Joy-Con). It reads player 1 and the handheld controls together. libnx merges the two: a button held on either counts, and the first stick that is off center wins.
- **Always connected.** The gamepad exists from startup and is never disconnected. With no controller attached it reads zero.
- **Styles seen:** the Switch Lite reports handheld (0x2); [Ryujinx](glossary.md#ryujinx) reports a Pro Controller (0x1).
- **Other Bevy input stays empty.** Nothing fills `Touches`, keyboard or mouse input.

### What it logs

The plugin logs through Bevy's log, so with `LogPlugin` (part of `DefaultPlugins`) these lines reach `log.txt`:

```text
[Input] HID configured: 1 player, standard controllers; reading player 1 and handheld
[Input] controller styles now 0x2 (0 = none, 0x1 = Pro Controller, 0x2 = handheld, 0x4 = dual Joy-Con)
[Input] first button press read from HID (buttons 0x1)
```

The styles line repeats whenever the connected controllers change. If you press buttons and no `first button press` line appears, HID input isn't reaching the game.

### Reading the pad yourself

For a game with its own input layer, `bevy_horizon::raw_pad()` returns a `RawPad` with this frame's read:

| Field | Holds |
|---|---|
| `buttons` | libnx's `HidNpadButton` bits held down. `bevy_horizon::npad_button` names the 16 the plugin maps |
| `sticks` | Left, then right, as `[x, y]`, each −32767..=32767, with +y up |
| `style_set` | The controller style: 0 none, 0x1 Pro Controller, 0x2 handheld, 0x4 dual Joy-Con |

It is refreshed in `PreUpdate`, before `InputSystems`. Before the first read, off [Horizon](glossary.md#horizon), and with no controller, it is all zero.

## Sound

Use bevy_audio as on any platform: `AudioPlayer`, `PlaybackSettings` and `AudioSink`. It plays through [rodio](glossary.md#rodio) and cpal, and bevy-horizon's cpal patch adds a Horizon host that feeds audren ([the audio host](#the-audio-host)).

- **Build with the patch set for your Bevy version.** `scripts/build_game.sh` reads the version from your `Cargo.lock` (or takes `BEVY=0.18` or `BEVY=0.19`) and passes `.cargo/bevy-0.18.toml` or `.cargo/bevy-0.19.toml`, which patch cpal 0.15.3 or 0.17.3 ([porting, step 4](porting-a-bevy-game.md#4-match-the-patch-set-to-your-bevy-version-and-build)). The host links libnx itself, so your `build.rs` needs nothing for sound.
- **Check the log once.** The host logs `[Audio] audren output: ...` when bevy_audio opens its stream. If bevy_audio warns `No audio device found.` instead, cpal couldn't open a stream. Usually the patch isn't applied ([checking a patch](toolchain.md#how-to-check-that-a-patch-is-really-applied)). Otherwise audren failed to start: bevy_audio doesn't log why, but captest's `sound-cpal` check shows the host's error, such as `audrenInitialize failed, 0x...`.
- **Set the variables first.** bevy_audio opens its stream while its `AudioPlugin` is built, that is, when you add `DefaultPlugins`, and the host reads `BH_AUDIO_*` then ([settings](#settings)).
- **HOME, sleep and exit need nothing from you.** Sound comes back after HOME and after sleep. At exit, the host closes audren even if bevy_audio never dropped its stream ([the audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit)).
- **One stream, output only.** bevy_audio mixes every sound into one stream. A second cpal stream opened while that one is open fails with `the Switch's audio renderer takes one stream at a time`. There is no microphone input.

## What captest checks

captest asks you to press, move, touch and listen, and reads the result through the same paths a game uses ([testing](testing.md)).

| Area | What you do | Read through | On a Switch Lite |
|---|---|---|---|
| Buttons | Press each of the 16 buttons when asked, 20 s each | Bevy's `Gamepad`, so the mapping is checked too | All 16 pass |
| Sticks | Push the left stick up and the right stick right (15 s each), spin both for 5 s, let go for 2 s | Bevy's `Gamepad` | Pass: 1.0 in every direction. Released, 0.081: a WARN (`stick-center`) against Bevy's 0.05 dead zone |
| Touch | Drag one finger, then two, into the corners (8 s) | libnx | Pass, two fingers |
| Motion | Hold still for 2 s, then tilt and turn for 4 s | libnx | Pass: 1.00 g at rest |
| Rumble | Feel both Joy-Con rumble for 1 s | libnx | The calls pass; `rumble-felt` is skipped, because a Lite has no rumble motors |
| Sound through cpal | Say whether you heard a beep on the left, one on the right, then a higher one on both. captest then closes the stream and opens a second one | cpal, through the patch | Pass |
| Sound through Bevy | Listen for a chime that despawns itself, a looping chime that pauses and then stops, and a chime after HOME | bevy_audio | Pass |
| Closing audio | Nothing: `bevy-app-drop` drops the Bevy App, which closes the stream (20 s limit) | Bevy | Pass, 138 ms |

The console's sound results came from an earlier host that played through [SDL2](glossary.md#sdl2), since replaced by the audren hosts ([why not SDL2](#why-not-sdl2)). With the current Bevy 0.19 host, captest's Bevy audio checks and its App drop have passed in the emulator.

## The audio host

bevy_audio plays through rodio, and rodio through cpal, which has no Switch backend of its own. bevy-horizon adds one in a [crate patch](glossary.md#crate-patch), the same code on two cpal versions:

| Bevy | rodio | cpal | Host |
|---|---|---|---|
| 0.19 | 0.22 | 0.17.3 | `patches/cpal/src/host/horizon/mod.rs` |
| 0.18 | 0.20 | 0.15.3 | `patches/bevy-0.18/cpal/src/host/horizon/mod.rs` |

The host shows one output device, "Nintendo Switch", and opens audren as follows (`CONFIG`, `Output::open`):

- **Output:** 48 kHz stereo, through 24 voices, one sink and a two-channel final mix.
- **Stream:** f32 samples, one or two channels, converted to 16-bit PCM for voice 0 at the stream's rate. A mono stream plays on both speakers. The host offers 2 channels at 48 kHz, which is what bevy_audio opens.
- **Buffers:** three, of 256 to 8192 frames each, kept queued.

### Settings

The host reads three environment variables in `build_output_stream_raw`, each time a stream opens. Horizon has no shell, so set them in `main`, before you add `DefaultPlugins`:

```rust
fn main() {
    // Before DefaultPlugins: bevy_audio opens its stream then, and the host reads these.
    unsafe {
        std::env::set_var("BH_AUDIO_FRAMES", "2048");
        std::env::set_var("BH_AUDIO_STATS", "1");
    }
    // App::new().add_plugins(DefaultPlugins)...
}
```

| Variable | Default | What it sets |
|---|---|---|
| `BH_AUDIO_FRAMES` | What the stream asks for: 2048 frames (42.7 ms) under Bevy 0.19; 1024 (21.3 ms) under Bevy 0.18, whose rodio asks for cpal's default | The length of each of the three buffers, in frames, decimal or `0x` hex, clamped to 256–8192. Longer buffers ride out longer stalls and add latency |
| `BH_AUDIO_STATS` | Off | `1` adds a stats line every 10 s |
| `BH_AUDIO_PRIORITY` | `0x2B` | The audio thread's [priority](glossary.md#thread-priority), decimal or `0x` hex; lower numbers run first. `default` leaves the thread at the priority it starts with. A value that isn't a number logs a line and uses `0x2B` |

Choosing a buffer length: three buffers ride out a stall of about two buffers (43 ms at 1024 frames, 85 ms at 2048) and add up to three buffers of latency (about 64 ms or 128 ms). Start with the default. If the stats show `ran dry` above 0 outside loading, or you hear crackles, raise `BH_AUDIO_FRAMES` a step: 2048, then 4096.

### What it logs

The host writes to stderr, which bevy_horizon sends to `log.txt`. With `BH_AUDIO_FRAMES=2048` and `BH_AUDIO_STATS=1`, the log shows the lines below; the stats line is from a full 3D game on the console:

```text
[Audio] audren output: 48000 Hz, 2 channels, f32 to 16-bit, 3 buffers of 2048 frames (42.7 ms); audio thread priority 0x2b; stats every 10 s
[Audio] audio thread priority 0x3b -> 0x2b
[Audio] 10.0 s: 235 callbacks of 42666 us, 6954 us average, 9653 us slowest, 16.3% of a core; longest gap 45.5 ms, 0 over 1.5 buffers; ran dry 0 times
```

The stream line says `audio thread priority unchanged` when `BH_AUDIO_PRIORITY=default`, and drops `; stats every 10 s` without `BH_AUDIO_STATS=1`. The stats line reads:

| Part | Meaning |
|---|---|
| `235 callbacks of 42666 us` | Callbacks in the window, and the buffer length in microseconds. Real time is about 234 callbacks in 10 s at 2048 frames, 469 at 1024 |
| `6954 us average, 9653 us slowest` | How long the stream's callback took: bevy_audio's whole mix |
| `16.3% of a core` | Time in the callback over the window |
| `longest gap 45.5 ms` | The longest time between two callbacks |
| `0 over 1.5 buffers` | Gaps longer than one and a half buffers: the thread fell behind, and the queue covered it |
| `ran dry 0 times` | Times the thread found every buffer finished after the first fill: an audible gap |

Other lines to search for:

| Line | Meaning |
|---|---|
| `[Audio] exit: closed the audio renderer in 19.0 ms, which gives audren its memory back` | The close at exit worked |
| `[Audio] exit: the audio thread didn't stop within 1000 ms; audren keeps its memory` | The close at exit timed out. The [heap](glossary.md#heap) stays locked, so the game exits to HOME instead of the homebrew menu |
| `[Audio] the audio thread didn't stop within 1000 ms; audren stays open` | Dropping a stream timed out |
| `[Audio] audio thread stays at priority 0x3b: svcSetThreadPriority(...) failed, ...` | The priority couldn't be applied |
| `[Audio] libnx refused buffer N; it stays empty`, `[Audio] audrvUpdate failed, ...; the thread keeps trying` | The renderer rejected work; expect gaps |
| `[Audio] atexit failed: the renderer stays open at exit` | Close the stream yourself before exit |

### Measured on the console

In [a full 3D Bevy 0.18 game](https://github.com/JohnUzoka/skate-3-rust-engine/tree/horizon-port) on a Switch Lite, the sound never ran dry and sounded clean at both lengths:

| Buffer | Callback, average | Share of a core | Longest gap in play | Longest gap while loading |
|---|---|---|---|---|
| 1024 frames (21.3 ms) | 3.1–3.4 ms | 15–16% | 25–30 ms | 34.8 ms |
| 2048 frames (42.7 ms) | 6.4–9.7 ms | 15–23% | about 46 ms | 54.7 ms |

A callback is the game's whole mix, so these times are mostly the game's decoding and mixing, not the host.

### Inside the host

For contributors; nothing here needs action in a game.

- **One thread per stream** (`run`), named `bh-audio`. Every renderer frame (5 ms; it waits at most 10 ms for one) it refills each buffer the renderer has finished, oldest first, by calling the stream's callback. It converts the samples to 16-bit and sends them all in one `audrvUpdate`. A late thread leaves a gap in the sound and carries on; it never waits for one buffer's state.
- **Priority 0x2B** (`audio_priority`, `set_priority`) puts the thread ahead of every std thread, which starts at 0x3B (Bevy's [task pools](glossary.md#task-pool) included), and of the main thread at 0x2C. Threads at one priority on a busy core take turns of about 10 ms, against a 21 ms buffer.
- **Memory:** the sample buffers come from the heap, and audren holds [transfer memory](glossary.md#transfer-memory) taken from the heap until the stream closes.
- **Closing:** dropping a stream (`Drop for Stream`) stops its thread and waits at most 1 s. The thread drops its `Output`, which closes the driver and calls `audrenExit` (`Drop for Output`). `close_at_exit`, registered with `atexit` when the first stream opens, closes any stream still open at exit: Bevy 0.18's bevy_audio leaks its stream, and Bevy 0.19's goes only when the App is dropped. Without it, locked heap pages would send the game to HOME instead of back to the homebrew menu ([the audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit), [rules for services](lifecycle-and-exit.md#rules-for-services-you-add-later)).
- **Linking:** each patch's `build.rs` (`link_libnx`) links libnx.

## Why not SDL2

Both hosts used to open an SDL2 audio device, through [devkitPro](glossary.md#devkitpro)'s switch-sdl2 2.28.5, which the toolchain image still installs. Its Switch backend can go silent for good and then never close. `SWITCHAUDIO_PlayDevice` waits until the buffer it just queued reports `Playing`, but one libnx update marks every finished buffer `Done` and only the next queued one `Playing` (libnx v4.12.0 `nx/source/audio/voice.c:225-236`). If SDL's thread is held off for about one buffer (21 ms at 1024 frames), its buffer goes from queued straight to `Done` and the wait never ends: the game's callback never runs again, and `SDL_CloseAudioDevice` waits forever.

In Ryujinx, a full game's sound stopped after about 6 s. On the console it takes HOME, a load spike or a busy core at the wrong moment. The audren hosts never wait for one buffer's state, so a late thread costs a gap, not the sound.

## Known gaps

- **The Bevy 0.19 audio host has passed only in the emulator.** It is the Bevy 0.18 host's code on cpal 0.17.3. The 0.18 host is proven on the console in a full 3D game: no crackle, sound back after HOME and after sleep, and about 19 ms to close at exit.
- **One player.** The plugin reads player 1 and the handheld controls as one gamepad, and doesn't show the system's controller [applet](glossary.md#applet).
- **Not tested on hardware:** [docked mode](glossary.md#docked-mode), a Pro Controller and detached Joy-Con. Testing so far is on a Switch Lite, which is handheld only.
- **No touch screen, motion sensors, rumble, keyboards or software keyboard in bevy_horizon.** libnx's touch and motion calls work on the console, and its rumble calls succeed; captest makes them through FFI (`examples/switch_captest/src/interactive.rs`: `touch`, `motion`, `rumble`).
- **SL and SR aren't mapped.**
- **Output only, one stream at a time.**

Symptoms and fixes for sound and input problems are in [troubleshooting](troubleshooting.md).
