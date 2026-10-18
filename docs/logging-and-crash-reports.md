# Logs and crash reports: finding out what happened

This page is for you when a port misbehaves on the Switch: it drops back to the homebrew menu, lands on the [HOME menu](glossary.md#home-menu), runs slowly, or leaves a new file in `sdmc:/atmosphere/crash_reports/`. It covers the logs `bevy_horizon` writes, Atmosphère's crash reports, the CPU profiler, and the shader statistics of [NVK](glossary.md#nvk), the Vulkan driver. [troubleshooting.md](troubleshooting.md) goes from symptoms to fixes; this page is the reference behind it.

Read the log first, then the crash report. The log says what your app did. The report says which process crashed, and that can be another program: your [NRO](glossary.md#nro) runs inside the [loader](glossary.md#loader)'s process, so a crash after your app has exited is reported against the loader.

## In short

- **Call `bevy_horizon::init_switch_logging("<app>")` first in `main`.** The log itself opens earlier, in [`userAppInit`](glossary.md#userappinit); the call adds the panic hook and the log writer thread.
- **`log.txt` is rewritten at every launch**, in `sdmc:/switch/<app>/`, so copy it off the card before you launch again. `panic.txt`, `crash.log` and `abort.log` are appended to, so an old report can look new.
- **Everything goes to `log.txt`:** `bevy_horizon`'s own lines, `println!`, Bevy's `tracing` output, and Mesa's and the audio host's messages.
- **A Rust [panic](glossary.md#panic) makes no Atmosphère crash report.** Its report is in `log.txt` and `panic.txt`.
- **The `[Exit]` lines say how the app ended.** `[Exit] PASS` means the loader can take the heap back; `[Exit] WARN` means the app went to HOME instead, on purpose.
- **A `[Heartbeat]` comes every 10 s.** Its once-a-minute `[Memory]` line costs a hitch of tens of milliseconds; `BEVY_HORIZON_MEMORY_EVERY=0` turns it off.
- **In a crash report, read the module list first, then decode the `Result`.** A list with only `hbl` means the loader crashed, not your app.
- **Symbolize with the exact ELF the NRO was made from.** A crash address minus the log's `nro base` is the address in the ELF.
- **`BEVY_HORIZON_CPU_PROFILE=1` samples every thread** ([Part 3](#part-3-where-the-time-goes-the-cpu-profiler)), and **`NAK_DEBUG=stats` logs what each shader costs** ([Part 4](#part-4-what-a-shader-costs-nak_debugstats)).
- **Asking for help?** Send what [What to collect when asking for help](#what-to-collect-when-asking-for-help) lists.

## Setting the variables on this page

[Horizon](glossary.md#horizon) has no shell and no launch arguments, so your game sets these environment variables itself, with `std::env::set_var` at the start of `main`, before `init_switch_logging` starts the log writer thread. (`set_var` is `unsafe` in Rust's 2024 edition, because another thread reading the environment at the same time is undefined behaviour.)

```rust
fn main() {
    #[cfg(target_os = "horizon")]
    {
        // SAFETY: no other thread is running yet.
        unsafe {
            std::env::set_var("BEVY_HORIZON_CPU_PROFILE", "1");
            std::env::set_var("NAK_DEBUG", "stats");
            std::env::set_var("MESA_SHADER_CACHE_DISABLE", "true");
        }
        bevy_horizon::init_switch_logging("my_game");
    }
    // Build the App and add HorizonPlugin as usual.
}
```

`bevy_horizon` reads `BEVY_HORIZON_MEMORY_EVERY` ([Heartbeat lines](#heartbeat-lines)), `BEVY_HORIZON_CPU_PROFILE` and `BEVY_HORIZON_CPU_PROFILE_EVERY` ([Part 3](#part-3-where-the-time-goes-the-cpu-profiler)) when `HorizonPlugin` is added. Mesa reads `NAK_DEBUG` and `MESA_SHADER_CACHE_DISABLE` ([Part 4](#part-4-what-a-shader-costs-nak_debugstats)) when the renderer starts. The audio host reads `BH_AUDIO_STATS` when it opens a stream: `1` logs an `[Audio]` timing line every 10 s ([The audio host](input-and-audio.md#the-audio-host)).

To change them without a rebuild, read them from a file: Skate 3 Rust Engine, a full 3D Bevy 0.18 game ported with bevy-horizon (https://github.com/JohnUzoka/skate-3-rust-engine, branch `horizon-port`, Switch setup in `crates/skate-game/src/horizon.rs`), reads `env NAME=VALUE` lines from an `options.txt` beside its NRO. [porting-a-bevy-game.md](porting-a-bevy-game.md) lists every variable bevy-horizon reads.

## Part 1: the logs `bevy_horizon` writes

The log is usually more precise than anything on screen: it shows what the app did before it stopped, and it catches panics that never make a crash report.

### Where the files go

| File | Path | Written | By |
|---|---|---|---|
| `log.txt` | `sdmc:/switch/<app>/log.txt` | rewritten at each launch | `bevy_horizon`, stdout and stderr |
| `panic.txt` | `sdmc:/switch/<app>/panic.txt` | appended | the panic hook |
| `crash.log` | `sdmc:/switch/<app>/crash.log` | appended | the CPU exception handler |
| `abort.log` | `sdmc:/switch/<app>/abort.log` | appended | the libnx abort hook |
| `cpu-profile.txt` | beside `log.txt` | rewritten every 30 s | the CPU profiler, when it is on |

`<app>` is the NRO's file name without `.nro`: `sdmc:/switch/games/hello_switch.nro` logs to `sdmc:/switch/hello_switch/`, which `bevy_horizon` creates. If that folder can't be used, the files go to the root of the [SD card](glossary.md#sdmc): `sdmc:/log.txt`, and so on. The module doc of `crates/bevy_horizon/src/logging.rs` has the same table.

Before `main`, `bevy_horizon` also sets `BEVY_ASSET_ROOT` to the NRO's folder, so Bevy loads assets from `assets/` beside the NRO; to use another folder, set it yourself at the start of `main`. Put `<app>.nro` in `sdmc:/switch/<app>/`, and its logs and assets share one folder.

### When the log opens

[libnx](glossary.md#libnx) calls `userAppInit` once its services are up and the SD card is mounted, before C constructors, Rust's runtime setup and `main`. `bevy_horizon`'s `userAppInit` (`crates/bevy_horizon/src/compat.rs`, `horizon_stubs`) sets [`TPIDR_EL0`](glossary.md#tpidr_el0) for Rust's thread-locals and opens the log (`start_boot_log`), so even a failure in Rust's startup is logged.

In `main`, `init_switch_logging(name)` (`crates/bevy_horizon/src/logging.rs`) sets `NO_COLOR=1`, so Bevy's log has no colour codes; writes `[Main] init_switch_logging("<name>"): Rust's runtime is up and main is running`; starts the log writer; and installs the panic hook. If the log isn't open yet, it opens it, in a folder named after `name` when there is no NRO path.

Every log starts with a header. From one hello_switch launch:

```text
==== bevy_horizon log: hello_switch ====
opened by:      userAppInit, before Rust's runtime setup and main
log file:       sdmc:/switch/hello_switch/log.txt (fd 3)
fsdevMountSdmc: 0x559 (0x559 = already mounted by libnx, which is normal)
stdout -> log:  ok
stderr -> log:  ok
log writes:     one at a time, so threads' lines can't overwrite each other
nro base:       0x273ae000 (crash address - base = address in the ELF)
========================================
```

- **`opened by:`** reads `init_switch_logging in main (no log was started before main)` if `userAppInit` didn't open the log.
- **`stdout -> log:` and `stderr -> log:`** read `FAILED (println!/Bevy logs will be missing)` if the redirect failed.
- **`log writes:`** reads `NOT locked: <why>` if the write lock couldn't be put in place. The log still works, but lines from two threads can collide ([Why writes take turns](#why-writes-take-turns)).
- **`nro base:`** changes at every launch. A crash address minus it is the address in the [ELF](glossary.md#elf) ([Symbolizing addresses](#symbolizing-addresses)).

### How lines get into `log.txt`

`bevy_horizon` points stdout (fd 1) and stderr (fd 2) at `log.txt` with `dup2`, so everything ends up in one file, in the order it was written:

- **`bevy_horizon`'s startup and exit lines** (`[Boot]`, the boot-time `[Memory]` and `[Assets]` lines, `[Main]`, `[Log]`, `[Thread]`, `[Profiler]`, `[Exit]`) and the panic, crash and abort reports go straight to the file (`write_log`).
- **Bevy's `tracing` output**, with `bevy_horizon`'s other lines, goes to stderr, each line led by the time, level and module: `<time>  INFO bevy_horizon::lifecycle: [Heartbeat] ...`.
- **Everything else written to stdout or stderr:** the examples' `[Init]` lines (`println!`), the audio host's `[Audio]` lines, and Mesa's lines: `NAK_DEBUG in effect`, `NAK_STATS`, NVK's own `[NVK]` reports, and the normal `WARNING: NVK is not a conformant Vulkan implementation, testing use only.`

If you set `LogPlugin`'s filter or level yourself, keep `bevy_horizon` at `info`, for example `filter: "warn,bevy_horizon=info".into()`, or the heartbeat disappears.

| Prefix | What it reports | More |
|---|---|---|
| `[Boot]`, `[Memory]`, `[Assets]` | startup, before `main` | [Boot lines](#boot-lines) |
| `[Main]`, `[Log]` | `init_switch_logging` and the log writer | [Flushing and the cost of flushing](#flushing-and-the-cost-of-flushing) |
| `[Thread]` | each thread created, and whether its thread-locals work | below |
| `[Heartbeat]`, `[Memory]`, `[Threads]` | every 10 s while frames run | [Heartbeat lines](#heartbeat-lines) |
| `[Applet]` | focus, HOME, suspend and resume | [HOME and focus](lifecycle-and-exit.md#home-and-focus) |
| `[Input]` | controller setup, and the controller styles seen | [input-and-audio.md](input-and-audio.md) |
| `[Upload]` | the bytes of new meshes sent to the GPU each frame, and meshes held for later frames | [Pacing uploads](platform-notes.md#pacing-uploads-the-render-feature) |
| `[NVK]` | the [ZCULL](glossary.md#zcull) direction (`BEVY_HORIZON_ZCULL_DIR`), from `bevy_horizon`; NVK's own reports on ZCULL, image compression and register caps | [rendering.md](rendering.md) |
| `[FPS]` | the FPS overlay, turned on and off by pressing (−) and (+) together | [The FPS overlay](rendering.md#the-fps-overlay) |
| `[Audio]` | the audio host's output format, `BH_AUDIO_STATS` timings, and its close at exit | [The audio host](input-and-audio.md#the-audio-host) |
| `[Profiler]` | the CPU profiler | [Part 3](#part-3-where-the-time-goes-the-cpu-profiler) |
| `[Exit]` | releasing services, and the heap check | [Exit lines and the heap check](#exit-lines-and-the-heap-check) |

Each thread made by libnx's `threadCreate` (all of std's threads, so Bevy's and Mesa's too) gets two `[Thread]` lines, at creation and at start:

```text
[Thread] #<n>: threadCreate with a <k> KiB stack, priority <0x..>, core <c>: created; core policy <policy>: ideal core <c>, may run on cores <0b....>
[Thread] #<n>: running on core <c>, ideal core <c>, may run on cores <0b....>; TPIDR_EL0 = libnx tls_tp = <0x..>, so thread_local! works
```

`#<n>` is also the CPU profiler's number for the thread. `FAILED with <code>` in place of `created` means no thread was made, and a second line ending `MISMATCH, thread_local! will fault` means that thread's thread-locals point at the wrong memory. `bevy_horizon::set_thread_logging(false)` turns the lines off, except when something failed.

### Boot lines

From the same launch (addresses and sizes differ every time):

```text
[Boot] launched from sdmc:/switch/hello_switch/hello_switch.nro
[Boot] TPIDR_EL0 = 0x2a6e7a60 (Rust's thread-local base, copied from libnx ThreadVars.tls_tp)
[Boot] TLS template at nro+0x331fef0, align 8: Rust and libnx agree on thread-local addresses
[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
[Memory] process: 3189.0 MiB in total, 3185.0 MiB used (svcGetInfo; "used" already counts the whole malloc heap below)
[Memory] malloc heap: 3132.7 MiB, set up by the homebrew loader; malloc has 0.0 MiB in use, ...
[Assets] BEVY_ASSET_ROOT = sdmc:/switch/hello_switch (Bevy's default asset folder is sdmc:/switch/hello_switch/assets)
[Boot] next: C constructors, Rust's runtime setup (std::rt::lang_start), then main
```

- **`TLS template ...`** should end `Rust and libnx agree on thread-local addresses`. `MISALIGNED: Rust and libnx put thread-locals at different addresses` means a broken build: every `thread_local!` reads the wrong memory.
- **`applet type 0 = Application`** gets a game's memory. `applet type 2 = LibraryApplet: applet mode, ...` is [applet mode](glossary.md#applet-mode), with far less: launch from a [forwarder](glossary.md#forwarder) or a title override instead.
- **`process: 3189.0 MiB in total`** is the tested Switch Lite's: a game's memory, less the 96 MiB held back for [gameplay recording](glossary.md#gameplay-recording) when the title asks for it ([platform-notes.md](platform-notes.md)).
- **`malloc heap:`** is the [heap](glossary.md#heap) that every Rust, C and GPU allocation comes from ([GPU memory comes from the same heap](platform-notes.md#gpu-memory-comes-from-the-same-heap)). A heap libnx made itself reads `set up by libnx`.
- **`[Assets]`** says where Bevy loads assets from, or `[Assets] BEVY_ASSET_ROOT is already <value>; keeping it`.

When `HorizonPlugin` is added, it mounts [RomFS](glossary.md#romfs), starts the socket layer and sets up HOME handling, and logs each through `tracing`. A WARN, `romfsMountSelf: 0x1559 = 2345-0010 (libnx IoError): this NRO has no RomFS, which is normal unless elf2nro was given --romfsdir`, is expected when assets are on the SD card. Then come `BSD socket layer initialized successfully` (or `socketInitialize failed: <code>`) and `[Applet] focus handling: NoSuspend, so HOME sends FocusStateChanged; the app is suspended after its last frame`.

### Heartbeat lines

`flush_logs_system` (`crates/bevy_horizon/src/lifecycle.rs`) runs in `PostUpdate` and logs a heartbeat every 10 s from the first frame:

```text
[Heartbeat] frame <n>, <s> s after the first frame, <fps> fps on average
[Heartbeat] work per frame over the last <n> frames: <a> ms on average, <w> ms at worst (60 fps allows 16.67 ms); the worst was frame <k>, which spent <x> ms of it in flush_logs_system (heartbeat lines and [Memory]; the log writer thread writes and syncs log.txt)
[Memory] malloc has <u> MiB in use, <t> MiB taken from the <h> MiB heap; <f> MiB free at the top, <g> MiB free in holes below it
[Threads] <n> created since launch; detached: <c> closed after exiting, <r> still running
```

- **`fps on average`** is over the whole run, not the last 10 s. For the rate now, compare two heartbeats' frame numbers and times. Time in the background is left out: `... after the first frame not counting <t> s suspended, ...`.
- **The work line** times the main schedule, `First` to `Last`, from the second heartbeat on. It leaves out the runner's sleep, and Bevy's render step, which runs after the main schedule in the same `app.update()` (bevy_app 0.19.1, `sub_app.rs`). If the app has no runner, `HorizonPlugin` adds a 60 Hz [ScheduleRunnerPlugin](glossary.md#schedulerunnerplugin). So low fps with little work means the time goes to rendering, or to waiting for the GPU.
- **`spent <x> ms of it in flush_logs_system`** is the heartbeat's own share of the worst frame ([Flushing and the cost of flushing](#flushing-and-the-cost-of-flushing)).
- **`[Memory]`** comes with the first heartbeat, then once a minute; compare it across heartbeats to spot a leak. While NVK has GPU memory above the heap's top ([NVK takes GPU memory from the top of the heap](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap)), the line adds `; the top-down region above the top: <r> MiB, <g> MiB of it in GPU blocks, <p> MiB at the most`. With the `top_down_alloc` feature it also reports Rust's large blocks and the small-block cache.
- **`[Threads]`** counts threads; a `still running` count that keeps growing means threads that never finish.

`BEVY_HORIZON_MEMORY_EVERY=<seconds>` sets how often `[Memory]` comes, and `0` turns it off. The log confirms the setting (`[Heartbeat] a [Memory] line every <N> s ...` or `[Heartbeat] no [Memory] line ...`), or says the value is `not a whole number of seconds`. For the figures at a moment you choose, such as after a level load, log `bevy_horizon::memory_report()`: the same figures, plus the heap mapped for the GPU. It walks the heap, so don't call it every frame.

### Flushing and the cost of flushing

Every write to the card is a round trip through [newlib](glossary.md#newlib), libnx, [IPC](glossary.md#ipc), the FS [service](glossary.md#service) and the card, and `fsync` waits for the card too. In a full 3D game on a Switch Lite, writing and syncing the log on the frame cost each heartbeat frame about 20 ms, or 74 ms with the `[Memory]` line; one such frame took 1.2 s, and one sync alone 274 ms. So a log writer thread does the card's work:

- **Start.** `init_switch_logging` starts it and logs `[Log] log writer started: threads queue their log.txt text (up to 512 KiB) and go on, ...`. If it can't, the log says `[Log] couldn't start the log writer (<error>): each thread writes its own lines`, or `[Log] no log writer, since log.txt's writes don't go through locked_write: ...` when the header said `NOT locked`.
- **Queue.** Writes to stdout and stderr are copied into a 512 KiB static queue, so logging never allocates and the thread goes straight on. A write bigger than the free space waits for the queue to empty, then goes to the card directly.
- **Sync.** The writer syncs `log.txt` once a second, and `flush_logs_system` asks for a sync at each heartbeat (`request_log_sync`). The frame waits for neither the writes nor the sync.
- **Reports.** `write_log` writes out the queue, then its own text, on the calling thread, so the header, the reports and the `[Exit]` lines are in `log.txt`, in order, when it returns.
- **Exit.** `release_horizon_services` stops the writer after its first `[Exit]` line; from then on each line is written before the next step runs.

Call `bevy_horizon::flush_switch_logs()` before something that might hang or kill the app: it flushes stdout, writes out the queue and syncs `log.txt` before it returns. A sync can take hundreds of milliseconds, so don't call it every frame.

The writer is one detached thread, `bevy_horizon log writer`, with a 64 KiB stack at the default priority; `[Threads]` counts it as still running. If the system ends the process, as closing the game from HOME does, the last queued lines are lost. A crash or a panic writes out the queue before its report, so it loses nothing.

What stays on the heartbeat frame is formatting its lines and, once a minute, the `[Memory]` line's `mallinfo` call, which walks the heap holding malloc's lock: about 41–50 ms in a full 3D game on a Switch Lite, a hitch once a minute. Another thread wouldn't help, because the next allocation would wait for the lock. For play, set `BEVY_HORIZON_MEMORY_EVERY=0`.

### Panic reports

The panic hook (`record_panic` in `crates/bevy_horizon/src/logging.rs`) writes its report to the debug output, then `log.txt`, flushes the logs, and appends the report to `panic.txt`. This one is from an early hello_switch that disabled `PipelinedRenderingPlugin`, which `DefaultPlugins` has only with Bevy's `multi_threaded` feature:

```text
==================== RUST PANIC ====================
app:      hello_switch
thread:   main
location: <path>/bevy_app-0.19.1/src/plugin_group.rs:<line>:<column>
message:  Cannot disable a plugin that does not exist.
memory:   malloc has <u> MiB in use, <t> MiB taken from the <h> MiB heap; ...
On Switch (panic=abort) the app now exits without a crash report: to the homebrew menu,
or to HOME if other threads are still running (log.txt's [Exit] lines say which).
====================================================
```

- **`thread:`** is the thread's name, or `<unnamed>`.
- **`memory:`** gives the `[Memory]` figures at the panic. After a wgpu "Out of Memory" panic, it says whether the heap really was full.
- **Nothing unwinds.** The target spec, `aarch64-nintendo-horizon.json`, sets `"panic-strategy": "abort"`, so Atmosphère makes no crash report, but the `[Exit]` lines still run. In a game that draws or has Bevy's audio, a panic ends at HOME: neither the GPU device nor the audio is closed, so their heap pages stay locked ([Exit lines and the heap check](#exit-lines-and-the-heap-check)).
- **`panic.txt` is appended to.** Two identical reports mean two launches, not two panics.

To try the panic path, put the word `panic` in `sdmc:/switch/render_check/options.txt`, and render_check panics at frame 120.

### CPU exceptions and libnx aborts

A CPU exception, such as a bad pointer or a Rust trap, runs `bevy_horizon`'s `__libnx_exception_handler` (`crates/bevy_horizon/src/compat.rs`) on a 64 KiB stack, unless a debugger is attached. It writes its report to the debug output and `log.txt`, syncs the log, and appends the report to `crash.log` (`record_fatal`). Then libnx calls `svcBreak`, so Atmosphère writes a crash report too.

```text
==================== CPU EXCEPTION ====================
kind:     0x101 DataAbort (libnx calls this Other)
esr:      0x92000006 (EC 0x24: data abort: bad memory access, e.g. null or dangling pointer)
far:      0x<address> (address being accessed, for aborts)
nro:      0x<base>-0x<end> (nro+X = address X in the ELF)
pc:       0x<address> (nro+0x<offset>)
lr:       0x<address> (nro+0x<offset>)
sp:       0x<address>
fp:       0x<address>
tpidr_el0: 0x<address> (Rust thread-local base; libnx tls_tp 0x<address>, ok)
x0  0x<value>  x1  0x<value>  x2  0x<value>  x3  0x<value>
...

backtrace (newest first; return addresses point just past each call):
  #0  0x<address> (nro+0x<offset>)
  #1  0x<address> (nro+0x<offset>)
=======================================================
```

- **`kind:` and `esr:`** name the exception and its class in words. For an abort, **`far:`** is the address accessed; a small one means a null pointer.
- **`pc:`, `lr:` and the backtrace** add `(nro+0x...)`, the address in the ELF, to addresses in the NRO ([Symbolizing addresses](#symbolizing-addresses)). The backtrace follows frame records, 32 entries at most.
- **`tpidr_el0:`** ends `MISMATCH: thread_local! access faults` if the thread's thread-local base is wrong.

When libnx can't go on, for example because a service failed to start, it calls `diagAbortWithResult`. `bevy_horizon`'s replacement records this the same way, in `abort.log`, then calls `svcBreak`, so Atmosphère's report has the same result. [Decode the result](#result-decode-it-before-guessing) as for a crash report.

```text
==================== LIBNX ABORT ====================
result:   0x<code> = error <2xxx>-<yyyy> (module <m>, description <d>)
nro base: 0x<address>
diagAbortWithResult -> svcBreak; Atmosphère writes a crash report with this result.
=====================================================
```

### Exit lines and the heap check

The loader reads the next NRO into the heap your app used. If a page there is still locked, by a service's [transfer memory](glossary.md#transfer-memory) or a thread's stack, the read fails and the loader crashes with `2001-0106`. So when `main` returns, or the app calls `exit` or aborts (as a panic does), libnx calls [`userAppExit`](glossary.md#userappexit), and `bevy_horizon`'s `release_horizon_services` (`crates/bevy_horizon/src/lifecycle.rs`) releases what it can and runs the [heap check](glossary.md#heap-check) ([The userAppExit hook](lifecycle-and-exit.md#the-userappexit-hook)). None of this runs when the system ends the process, as closing from HOME does, or after a CPU exception or a libnx abort.

A clean exit (numbers vary; lines for services the app didn't use are left out):

```text
[Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
[Exit] focus handling back to SuspendHomeSleep: done
[Exit] heap check, before releasing services: 1 block, 2.20 MiB, would fail the loader's read:
[Exit]   0x<start>-0x<end>, 2.20 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, as an open transfer memory or a thread stack leaves it
[Exit] Bevy's task pools: stopped and joined their <n> thread(s) in <x> ms
[Exit] detached threads: closed <n> now and <m> while the app ran (<k> threads were created in all); 0 still running after <x> ms
[Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
[Exit] nvdrv: already closed (the GPU driver released it), so there is nothing for nvExit to do
[Exit] the heap was unlocked <x> ms later, once the services had let go of their transfer memory
[Exit] heap check, after releasing services: the loader can read into all <h> MiB of the malloc heap; every page is read/write and unlocked
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

The examples print `[Exit] app.run() returned Success` just before this. The first check's blocks are normal: here, the socket layer's 2.20 MiB transfer memory, which `socketExit` gives back. Only the check after releasing services counts. Lines that mean trouble:

- **`[Exit] WARN: heap pages are still locked, so the loader would crash reading the next NRO (2001-0106); exiting through AM to HOME instead`.** The `heap check` lines above it list the blocks, the first 8 of them. This exit is tested only in the emulator so far.
- **`[Exit] Bevy's task pools: <l> of <n> thread(s) still running after <x> ms, ...`**, or detached threads `still running`. A running thread keeps its stack locked. (Without Bevy's `multi_threaded` feature, the line says the pools have `no threads of their own`.)
- **`[Exit] nvdrv: still open, and <n> heap blocks (<x> MiB) are still held for the GPU, ...`.** The GPU device was never dropped, as after a panic or `std::process::exit` in a game that draws, so the hook skips `nvExit`, which could hang, and the app goes to HOME. With no pages held, the line ends `calling nvExit` instead, and `[Exit] nvExit: done` follows.
- **`[Exit] the heap was still locked 1000 ms later: ...`.** A service kept its transfer memory.
- **`[Exit] heap check failed: svcQueryMemory returned an error; ...`.** The check couldn't run, and the app returns to the loader anyway.

`[Exit] release hook <n>: done` is normal: one line for each hook registered with `bevy_horizon::register_release_hook` ([Rules for services you add later](lifecycle-and-exit.md#rules-for-services-you-add-later)), such as the CPU profiler's. [lifecycle-and-exit.md](lifecycle-and-exit.md) lists every way the app can end and where each leaves the console, and [troubleshooting.md](troubleshooting.md) starts from where you land.

### Why writes take turns

This section is internals: what `lock_log_writes` (`crates/bevy_horizon/src/logging.rs`) fixes, and what it doesn't.

`dup2` doesn't give fds 1 and 2 their own copies of the log's handle. devkitPro's newlib (libsysbase `handle_manager.c`) points them at the same `__handle`, so they share one libnx file, with one offset. Nothing on the way down takes a lock: not newlib's `_write_r` (libsysbase `write.c`), and not libnx's `fsdev_write` (`nx/source/runtime/devices/fs_dev.c`), which reads the offset, writes there, then adds the length. Rust's stdout and stderr each have their own lock, and `write_log` takes neither. So two writes at once can land at the same offset, one over the other, and both add their length, which leaves a gap. The file system doesn't seem to clear the space it gives a file, so a gap reads back as whatever was on the card before: in one full game's log, 171 bytes of unrelated text.

`lock_log_writes` fixes this before the `dup2`s, while nothing else can write to the log yet. It:

1. copies the SD card's `devoptab_t` (newlib `sys/iosupport.h`);
2. swaps the copy's `write_r` for `locked_write`, and its name for `bh_log`;
3. adds the copy to newlib's device table (`AddDevice`);
4. points the log's handle at the copy.

Every write through fd 1, fd 2 or the log's own fd then goes through `locked_write`. Without the log writer, it holds a libnx mutex, `LOG_LOCK`, around the SD card's own `write_r`; with the writer, it queues the text. The rest of the copy is the SD card's, so `fsync`, `close` and `fstat` work as before, and other files don't take the lock. A thread that crashes in the middle of a write still holds the lock when its report is written, so `locked_write` doesn't wait for a lock its own thread holds.

What it doesn't do: a line written in several pieces can still have another thread's line inside it. Rust's stderr is unbuffered, and `eprintln!` with arguments writes each piece separately, as Mesa's `NAK_STATS` lines are written. Each piece is whole, and no text is lost. Bevy's `tracing` lines are formatted first and written at once, and `println!` lines go out whole when stdout's line buffer flushes.

## Part 2: reading an Atmosphère crash report

A crash report is easy to over-read: it often holds dead stack frames, zeroed registers and old IPC bytes. Read it field by field.

**The example in this part** is a real report from an early build of hello_switch, which exited with the socket layer's 2.20 MiB transfer memory still locked; the loader then failed to read the next NRO and crashed with `2001-0106`. Current builds release that memory and check the heap at exit, so you shouldn't meet this crash, but it shows every part of a report.

### Where reports go, and what creates them

[creport](glossary.md#creport), Atmosphère's crash reporter, writes a [crash report](glossary.md#crash-report) when a process stops on a CPU exception or calls `svcBreak`. `bevy_horizon`'s exception handler and its libnx abort hook both end in `svcBreak`, so each leaves a report as well as `crash.log` or `abort.log`. A Rust panic doesn't ([Panic reports](#panic-reports)).

```text
sdmc:/atmosphere/crash_reports/<seconds>_<program id>.log
```

`<seconds>` is the console clock's Unix time in 11 digits (or the system tick, if the clock isn't available), and `<program id>` is the crashed process's: for example, `01791176509_05446530aca7e000.log`. creport also saves `dumps/<seconds>_<program id>_thread_info.bin` in that folder and, for an application on firmware 9.0.0 or later, a screenshot, `<seconds>_<program id>.jpg`. Read the newest report, and check that its time matches your launch.

The report's sections are `Process Info`, `Exception Info` (its `Type:` is `User Break` for an `svcBreak`, or a CPU exception such as `Data Abort`), `Crashed Thread Info`, `Dying Message Info`, `Module Info`, and `Thread Report`, which lists every thread.

### `Result`: decode it before guessing

The report starts:

```text
Atmosphère Crash Report (v1.7):
Result:                          0xD401 (2001-0106)
```

A [result code](glossary.md#result-code) packs a module and a description, and creport shows both the raw and the decoded form. To decode one yourself:

```text
module      = code & 0x1ff
description = (code >> 9) & 0x1fff
shown as      (2000 + module)-description
```

`bevy_horizon` prints result codes in `log.txt` the same way, as `0x1559 = 2345-0010` (`ResultCode` in `crates/bevy_horizon/src/logging.rs`). The same abort site can report different results, so decode before you guess:

| Raw | Shown | Meaning |
|---|---|---|
| `0x1559` | `2345-0010` | libnx `IoError`; from `romfsMountSelf`, the NRO has no RomFS, which is normal for assets on the SD card |
| `0xD401` | `2001-0106` | kernel `InvalidCurrentMemory`: an IPC buffer covers pages in the wrong state, such as heap pages a transfer memory still locks |
| `0x0` | `2000-0000` | no error: the loader called `diagAbortWithResult(0)` after a check failed. sphaira's loader does this, before your code runs, for an NRO without an asset section ([troubleshooting.md](troubleshooting.md)) |

### Module list first

The module list matters more than the stack trace. The example's:

```text
Module Info:
    Number of Modules:           01
    Module 00:
        Address:                 000000002dc00000-000000002dc06000
        Name:                    hbl
```

creport lists each module that a thread's PC, LR or stack trace points into, if that address is still mapped as code. Each entry gives the module's code range, its name and its `Module Id` (build ID).

- **Your NRO's entry** starts at the `nro base` in the same launch's `log.txt`. Find it by that address rather than by its name.
- **If only `hbl` is listed,** the loader ([hbloader](glossary.md#hbloader)) crashed, and your NRO had already exited or never started. In the example, the stack trace below holds addresses in the app's old range, yet no app module is listed: the NRO had been unmapped. Don't force its symbols onto the trace.

### PC, LR, and stack trace

The example's crashed thread (an excerpt, with the spacing trimmed):

```text
LR: 000000002dc0373c (hbl + 0x373c)
PC: 000000002dc03810 (hbl + 0x3810)
Stack Trace:
    ReturnAddress[00]: 000000002dc0054c (hbl + 0x54c)
    ReturnAddress[01]: 00000000274c202c
    ReturnAddress[02]: 00000000274cbaa8
    ReturnAddress[03]: 00000000274d2d38
    ReturnAddress[04]: 000000002745cbf4
    ReturnAddress[05]: 000000002745cbc0
    ... repeats ...
```

- **`PC`** is where the thread stopped. A thread stopped inside an SVC is reported at its `svc` instruction: here, `svcBreak` in the loader.
- **`LR`** is the return address of the last call: here, just past the loader's call to its `svcBreak` wrapper.
- **The stack trace** is creport's walk up the frame records from `FP`, 32 entries at most. An address inside a module gets `(module + offset)`, and `(symbol + offset)` too when the module has a symbol table. `[00]` is the loader's return from `diagAbortWithResult`.
- **The rest are leftovers:** addresses from the app's exit path, left on a stack the loader reused, in no module any more. A trace that repeats up to the 32-entry limit is a walk through dead frames, not recursion.

### Symbolizing addresses

An address means something only relative to the right image. For an address inside your NRO's module:

1. Subtract the module's start, or the `nro base` from the same launch's `log.txt`. For example, with `nro base: 0x273ae000`, the return address `0x274c202c` is `0x11402c` in the ELF.
2. For a return address, subtract 4 more to reach the call itself: `0x114028`. `PC` needs no adjustment.
3. Look it up in the ELF the NRO was made from, with [devkitA64](glossary.md#devkita64)'s tools in the toolchain image ([building.md](building.md)), from your project's folder:

```sh
docker run --rm -v "$PWD:/workspace:z" -w /workspace bevy-horizon-toolchain:local \
  aarch64-none-elf-addr2line -f -C -e target/aarch64-nintendo-horizon/release/hello_switch 0x114028
docker run --rm -v "$PWD:/workspace:z" -w /workspace bevy-horizon-toolchain:local \
  aarch64-none-elf-objdump -d -C --start-address=0x114020 --stop-address=0x114030 \
  target/aarch64-nintendo-horizon/release/hello_switch
```

- **Use the exact ELF.** A rebuild moves every function, so keep the ELF of each NRO you copy to the card. For a game it is `target/aarch64-nintendo-horizon/release/<bin>`, beside `<bin>.nro`; for this repo's examples, `target/aarch64-nintendo-horizon/release/<example>`.
- **Expect a function name, perhaps without a line.** `addr2line` can name a line only if the build kept debug line tables, and Cargo's default release profile keeps none; then it prints `??:?`. `objdump` shows the instructions around the address.
- **Use podman in place of docker** if that is your container engine. The image has no `llvm-symbolizer`.
- **Addresses in `hbl` are the loader's.** Your ELF can't name them.

### X8-X28 can be zero because the thread is inside an SVC

In the example, X8 to X28 are all zero. For a thread stopped inside an [SVC](glossary.md#svc), here `svcBreak`, Atmosphère's kernel gives creport only X0–X7, and the other general registers read 0 (`libraries/libmesosphere/source/arch/arm64/kern_k_debug.cpp`, `KDebug::GetThreadContextImpl`). FP, LR and SP are still real. X0–X2 hold `svcBreak`'s arguments: the reason, and the address and size of the data passed with it. Zeros in X8–X28 are not corruption.

### The TLS dump: the IPC message buffer

The first 0x100 bytes of a thread's [thread-local storage](glossary.md#thread-local-storage) are its IPC message buffer, so the TLS dump shows the last request the thread sent to a service, or the reply. The example's (the row at `+0x40` left out):

```text
TLS Address: 00000000327ed200
0000327ed200 04 00 00 01 12 00 00 00 00 00 00 c7 00 00 c0 5a
0000327ed210 01 00 00 c0 00 00 00 00 00 00 00 00 00 00 00 00
0000327ed220 01 00 28 00 88 00 00 00 00 00 00 00 00 00 00 00
0000327ed230 53 46 43 49 00 00 00 00 00 00 00 00 00 00 00 00
0000327ed250 00 00 00 c7 00 00 00 00 00 40 23 00 00 00 00 00
```

Read it little-endian:

| Offset | Bytes | Meaning |
|---|---|---|
| `+0x00` | `04 00 00 01` | a request with one B buffer, which the service writes into |
| `+0x04` | `12 00 00 00` | 0x12 data words follow |
| `+0x08` | `00 00 00 c7` | the B buffer's size, `0xC7000000`: 3,184 MiB |
| `+0x20` | `01 00 28 00 88 00 00 00` | a domain header: a message for object `0x88`, with 0x28 bytes of data |
| `+0x30` | `53 46 43 49` | `SFCI`, the command header that starts those bytes |
| `+0x50` | `00 00 00 c7 00 00 00 00` | the size to read, `0xC7000000` |
| `+0x58` | `00 40 23 00 00 00 00 00` | past the data's end, left from an older message: `0x234000` |

Together: `IFile::Read` from offset 0 into the loader's whole heap (the read of the next NRO), refused with `2001-0106` because part of the buffer was still locked. The leftover `0x234000`, 2.20 MiB, is libnx's default size for the socket layer's transfer memory: the block that the heap check now reports and `socketExit` releases.

### Thread dump and stack dump

The crashed thread appears twice: under `Crashed Thread Info`, and again in the `Thread Report` with every other thread. Each entry has `Thread ID`, `Thread Name`, `Stack Region`, the registers, `Stack Trace`, `Stack Dump`, `TLS Address` and `TLS Dump`.

- **Check the thread ID** when there are many threads, so you read the crashed one.
- **The stack dump** is the 0x100 bytes just below `SP`, space that calls which have returned used. It can hold their leftovers, such as a result code or the size a read returned, and the old frame records the trace walked through.

## Part 3: where the time goes (the CPU profiler)

A slow frame on the Switch has many possible owners: the main schedule, Bevy's render step, its [task pools](glossary.md#task-pool), NVK's shader compiler, or a thread waiting on the GPU or the SD card. The CPU profiler (`crates/bevy_horizon/src/profiler.rs`) samples every thread and says which.

**Turning it on.** Set `BEVY_HORIZON_CPU_PROFILE=1` to sample every 2000 µs, or set it to a number of microseconds, 500 at least; `0` or unset is off ([how](#setting-the-variables-on-this-page)). Every `BEVY_HORIZON_CPU_PROFILE_EVERY` seconds (30 by default, `0` for never) it rewrites `cpu-profile.txt` beside `log.txt`. The log confirms it: `[Profiler] sampling every thread every 2000 us; cpu-profile.txt every 30 s`. Failures are logged as `[Profiler] could not start its thread: <error>` and `[Profiler] could not write cpu-profile.txt: <error>`.

**From code.** `bevy_horizon::profiler::start(interval, report_every)` starts it, and returns `false` if it is already running or can't start. `running()` says whether it is on, `reset()` forgets the samples so far, and `write_report_to(path)` writes a report now; it fails if the profiler never started. To profile one stretch, such as a level load, call `reset()` at its start and `write_report_to` at its end.

**Reading it.** Copy the report off the card, and name its addresses with the ELF the NRO was made from:

```sh
scripts/symbolize_cpu_profile.py cpu-profile.txt <game>/target/aarch64-nintendo-horizon/release/<bin>
```

The script needs Python 3 and GNU binutils' `nm`, plus `addr2line` for `--lines`, which read an AArch64 ELF on any host; the toolchain image has all three. It prints each thread's busy % (its CPU time over the time watched) and the function it started in; how many of the three cores were busy, and which functions all the CPU time went to; and, for the busiest threads, the functions they run in (self), run under (inclusive) and wait at. Threads are named `main` or `#<n>`, as in the `[Thread]` lines. `--top N` sets the rows per table (15), `--threads N` how many threads to break down (6), `--chains N` adds each thread's N commonest call chains, `--lines` adds the hottest addresses with file and line (a minute or two on a large game), and `--width N` cuts names to N characters.

**How it works (internals).** Each interval, a sampler thread at priority 0x1E, above the game's threads, pauses each thread in turn (`svcSetThreadActivity`), reads its registers (`svcGetThreadContext3`), follows its frame records up the stack, and lets it run again. Rust code keeps frame records because the target spec sets `"frame-pointer": "always"`; for C code built without them, the sampler scans the stack for return addresses instead. A thread whose PC is on or just after an `svc` instruction counts as waiting. The kernel's CPU time for each thread (`svcGetInfo`: id 25 from firmware 13.0.0, `0xF0000002` before) gives its busy %. The profiler follows the main thread and every thread `threadCreate` makes, and a release hook stops it before exit. The technique is UnleashedRecomp-NX's; the code is separate ([CREDITS.md](../CREDITS.md)).

**The cost.** Each pass pauses each thread for a few microseconds, so a profiled game runs a little slower. The report's `sampler_ms` is the sampler's own time.

**Limits.**

- **A running sample is a thread that wasn't in a system call.** With more threads ready than the three cores an NRO gets, a thread waiting for a core still counts as running. Its busy % is the measure of its CPU time; the samples say where that time went.
- **Inlined functions count as their callers,** because names come from symbols, not debug info.
- **It follows 64 threads at most.** `untracked_threads` in the report counts the rest.
- **A resume can fail.** A thread that had exited is counted in `exited_while_paused`. Any other stays paused until a later pass resumes it: the report counts it in `resume_failures`, and the log says `[Profiler] ERROR: <n> thread(s) could not be resumed after a sample (#<n>: <code>); the next pass tries again`, then `[Profiler] thread #<n>, left paused, resumed on a later pass` (or `has exited since`). Tested on a Switch Lite: two profiles of over 14,000 passes each had no failed resumes.
- **Without the kernel's CPU times,** after `[Profiler] svcGetInfo(0xf0000002) for a thread's CPU time failed: <code>; the report's CPU times will read 0`, the report says `cpu_times none` and every `cpu_ms` is 0. The script then estimates busy % from each thread's share of running samples, an upper bound, and says so.

**The report format.** `cpu-profile.txt` is text: `key value` lines (`interval_us`, `passes`, `sampler_ms`, `resume_failures`, `cpu_times` and the others), then a `thread main` or `thread #<n>` line for each thread with its `samples=`, `cpu_ms=` and `busy_pct=`, each followed by its call chains, most samples first, as hex offsets from the NRO's start. A chain keeps its 22 innermost and 6 outermost frames, and after 40,000 distinct chains a new one is kept as its PC alone (`capped`). The comment above `write_profile` in `crates/bevy_horizon/src/profiler.rs` gives the chain line's format and flags.

## Part 4: what a shader costs (`NAK_DEBUG=stats`)

A GPU-bound frame is usually a few shaders' fault, and the numbers that say why (instructions, registers, how many warps fit) never reach the log otherwise. [NAK](glossary.md#nak), NVK's shader compiler, prints them only under `NAK_DEBUG=print`, which also prints the whole IR after every pass and makes a large shader take minutes to compile. bevy-horizon's `patches/mesa-switch/0003-nak-debug-stats.patch` adds `NAK_DEBUG=stats`: one line on stderr, so in `log.txt`, for each shader NAK compiles.

**Turning it on.** Set `NAK_DEBUG=stats` and `MESA_SHADER_CACHE_DISABLE=true` ([how](#setting-the-variables-on-this-page)). The second matters: a shader NVK loads from its [shader cache](glossary.md#shader-cache) in `sdmc:/.mesa/` isn't compiled, so it prints nothing. With the cache off, every launch compiles every shader again, so leave both out of normal play. In a Switch build, `NAK_DEBUG`'s flags add to the ones NAK turns on by default (patches `0019` and `0021`; [Linking NVK](toolchain.md#linking-nvk-the-vulkan-feature)), and `-name` turns a default off, as in `NAK_DEBUG=stats,-ofold`. NAK reads `NAK_DEBUG` once, when NVK creates its compiler as the renderer starts, and a Switch build logs the flags in effect: `NAK_DEBUG in effect: <flags> (Switch defaults xbar,sink,remat,xbrk,lsink,plat,ofold; "-name" turns one off)`.

**A line.** Each shader NAK compiles logs one:

```text
NAK_STATS stage=<stage> instrs=<n> static_cycles=<n> max_warps=<n> gprs=<n> slm=<bytes> spills_mem=<n> fills_mem=<n> spills_reg=<n> fills_reg=<n> tex=<n> txd=<n> tld=<n> ld=<n> ipa=<n> bra=<n> waits=<n> bra_waits=<n> source=<hash> name=<name> cap=<n>
```

For example, a fragment shader compiled to 56 registers (`gprs=56`) has room for 36 warps (`max_warps=36`); capped at 48 registers, the same shader has room for 40.

- `instrs`: instructions in the final shader. `static_cycles`: NAK's own estimate of their cost, without branches or memory waits.
- `gprs` and `max_warps`: registers per thread, and how many warps (32 threads each) fit on an SM at once with that many. A Maxwell SM hides a texture fetch's or memory load's latency by running other warps meanwhile, so a shader with few warps waits instead.
- `slm`: local memory per thread, in bytes. `spills_mem` and `fills_mem`: registers spilled to local memory and read back. `spills_reg` and `fills_reg`: values moved into GPRs from a smaller register file, such as predicates, for want of room there.
- `tex`, `txd`, `tld`: texture instructions: sampled with an implicit or given level, with explicit gradients, and fetched or gathered.
- `ld`: loads from global, local, shared and constant-buffer memory. `ipa`: fragment inputs interpolated. `bra`: branches. `waits`: instructions that wait on a scoreboard, that is, for an earlier load or texture fetch to finish; `bra_waits`: the branches among them.
- `source`: the first 4 bytes of the BLAKE3 hash of the SPIR-V the shader came from (Mesa `spirv_to_nir.c`), so lines with the same `source` are one module compiled for different pipelines. `name`: the source file the SPIR-V names, if any; in a release build, wgpu's shaders name none, so it is empty. Mesa's own shaders are built in NIR, so they print `source=00000000` and a name, such as `vk-meta-fill-buffer` and `vk-meta-copy-buffer` for the compute shaders behind NVK's buffer fills and copies (mesa-switch `src/vulkan/runtime/vk_meta_copy_fill_update.c`).
- `cap`: the register cap the shader was compiled under, 0 for none. `NAK_GPR_LIMIT=<n>`, a multiple of 8 from 24 to 248, caps every shader, and an entry point named like `fragment_gpr48` caps one; with both, the lower applies (patch `0014`; [Linking NVK](toolchain.md#linking-nvk-the-vulkan-feature)). A lower cap fits more warps, and spills what doesn't fit.

**On the console.** `NAK_STATS` lines have so far been collected in the emulator, which runs the NRO's own NVK and NAK; no console run with `stats` on has been recorded. For a register cap, NVK also writes a line of its own for each of the first 32 shaders with `_gpr` in their entry point's name, and that line has appeared in a console's `log.txt`:

```text
[NVK] register cap: fragment_gpr48 compiled to 2302 instructions in 48 registers, 40 warps per SM, 44 spills and 31 fills to 32 bytes of local memory
```

**Finding the costly ones.** Sort by `instrs`: `grep -a NAK_STATS log.txt | sort -t= -k3 -n`. On a GPU older than Turing, such as the Switch's Maxwell, the driver's buffer checks put a branch around every storage-buffer access. So a shader that branches little of its own, such as a vertex shader, with about as many `bra` as `ld` was built with them (`BH_ROBUST_BUFFER_ACCESS=1`). naga's clamp, the default, adds a few instructions but no branches or loads ([Buffer robustness](rendering.md#buffer-robustness)).

## What to collect when asking for help

The next launch rewrites `log.txt`, while the other files keep old reports, so collect right after the problem:

1. **`log.txt`**, copied off the card before you launch the app again.
2. **`panic.txt`, `crash.log` and `abort.log`**, if they exist, and whether you deleted them before this launch.
3. **The newest crash report** in `sdmc:/atmosphere/crash_reports/`, if its time matches this launch, with the `.jpg` beside it if there is one.
4. **Where you landed** (the homebrew menu or HOME), and how the app ended: your game's own quit, closing it from HOME, sleep, or a freeze.
5. **The ELF the NRO was built from**, or the addresses already symbolized with it ([Symbolizing addresses](#symbolizing-addresses)).
6. **Versions:** the bevy-horizon commit, Bevy, the console model, its firmware, Atmosphère, the launcher ([sphaira](glossary.md#sphaira)'s menu or a forwarder), and the `[Memory] applet type` line.
7. **The variables you set** ([Setting the variables on this page](#setting-the-variables-on-this-page)).
8. **For slowness,** `cpu-profile.txt` with the script's output ([Part 3](#part-3-where-the-time-goes-the-cpu-profiler)); for a GPU-bound frame, the `NAK_STATS` lines ([Part 4](#part-4-what-a-shader-costs-nak_debugstats)).
