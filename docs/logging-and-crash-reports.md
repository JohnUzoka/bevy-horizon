# Logs and crash reports: finding out what happened

This page explains the logs that `bevy_horizon` writes and how to read an Atmosphère crash report when a Switch run fails. It is for you when your [homebrew](glossary.md#homebrew) app returns to sphaira, lands on the [HOME menu](glossary.md#home-menu), or leaves a new report in `sdmc:/atmosphere/crash_reports/`.

Use the log first, then the report. The log tells what the app did. The report tells what process crashed. Those can be different, because this layer runs as an [NRO](glossary.md#nro) inside sphaira's [loader](glossary.md#loader) process.

## In short

- **`log.txt` is recreated every launch.** It lives beside the NRO, normally `sdmc:/switch/hello_switch/log.txt`.
- **`panic.txt` is appended.** Delete it before a test, or read only its last report. An old panic can look new.
- **Logging starts in `userAppInit`, before `main`.** That is early enough for Rust startup failures.
- **stdout and stderr both go to `log.txt`.** `dup2(fd, 1)` and `dup2(fd, 2)` make `println!`, Bevy `tracing`, and warnings land in one file.
- **Writes to `log.txt` take turns.** Without that, two threads writing at once could overwrite each other's lines, or leave a gap that reads back as old SD card data. Builds since findings 20 route every write through a lock ([Why writes take turns](#why-writes-take-turns)).
- **A log writer thread does the SD card writes.** Other threads queue their lines and go on. The writer writes them in order and syncs `log.txt` once a second. The header, panic, crash and abort reports, the exit lines and explicit `flush_switch_logs()` calls still write and sync on their own thread, after what is queued ([Flushing and the cost of flushing](#flushing-and-the-cost-of-flushing)).
- **A Rust panic does not make an Atmosphère report.** With `panic=abort`, it writes `panic.txt` and exits back to sphaira.
- **An Atmosphère report filename starts with Unix seconds.** For example, `01791176509_05446530aca7e000.log` is timestamp `1791176509`.
- **Read the module list first.** If only `hbl` is listed, your NRO was not running, or it had already exited.
- **`BEVY_HORIZON_CPU_PROFILE=1` profiles every thread.** It finds where the CPU time goes and where threads wait; `scripts/symbolize_cpu_profile.py` names the result ([Part 3](#part-3-where-the-time-goes-the-cpu-profiler)).
- **`NAK_DEBUG=stats` prints each compiled shader's size and register use.** With `MESA_SHADER_CACHE_DISABLE=true`, every shader is compiled and printed ([Part 4](#part-4-what-a-shader-costs-nak_debugstats)).

## Part 1: the logs `bevy_horizon` writes

This matters because the text log is usually more precise than the crash screen. It includes app state before the crash, and it catches panics that never create a crash report.

### Where the files go

The code documents the files at the top of `logging.rs`:

| File | Normal path | Open mode | What writes it |
|---|---|---|---|
| `log.txt` | `sdmc:/switch/<app>/log.txt` | truncate each launch | stdout, stderr, Bevy/`tracing`, boot and exit lines |
| `panic.txt` | `sdmc:/switch/<app>/panic.txt` | append | Rust panic hook |
| `crash.log` | `sdmc:/switch/<app>/crash.log` | append | CPU exception handler |
| `abort.log` | `sdmc:/switch/<app>/abort.log` | append | libnx fatal abort hook |

The code says the same in `logging.rs:1-28`. The fallback is the SD root: if `sdmc:/switch/<app>/` cannot be used, `log.txt` opens as `sdmc:/log.txt` (`logging.rs:264-268`), and appended reports fall back to `sdmc:/panic.txt`, `sdmc:/crash.log` or `sdmc:/abort.log` (`logging.rs:711-743`). Crash 4 used that root fallback because the crash happened before the log folder was known.

The app name comes from the NRO path. `sdmc:/switch/hello_switch/hello_switch.nro` becomes `hello_switch` (`logging.rs:230-248`).

### When the log opens

This matters because the earliest failures happened before `main`.

`compat.rs` defines libnx's weak `userAppInit` hook. It sets `TPIDR_EL0`, then calls `start_boot_log`:

```rust
// crates/bevy_horizon/src/compat.rs:58-65
pub extern "C" fn userAppInit() {
    let tp = set_thread_pointer();
    start_boot_log(tp);
}
```

`start_boot_log` opens the log and writes the boot lines before C constructors, Rust runtime setup and `main` (`logging.rs:160-199`). `init_switch_logging("hello_switch")` still runs first in `main`, but normally it only writes the `[Main]` line, starts the log writer and installs the panic hook (`logging.rs:127-158`; `examples/hello_switch/src/main.rs:19-22`).

A real A2 header:

```text
LOGS/log2.txt:1-17
==== bevy_horizon log: hello_switch ====
opened by:      userAppInit, before Rust's runtime setup and main
log file:       sdmc:/switch/hello_switch/log.txt (fd 3)
fsdevMountSdmc: 0x559 (0x559 = already mounted by libnx, which is normal)
stdout -> log:  ok
stderr -> log:  ok
nro base:       0x273ae000 (crash address - base = address in the ELF)
...
[Main] init_switch_logging("hello_switch"): Rust's runtime is up and main is running
```

The `nro base` line matters for symbolizing crash addresses. The [ELF](glossary.md#elf) is linked at 0, so runtime address minus NRO base gives the address to look up in the ELF (`logging.rs:385-390`).

Builds since findings 20 add a `log writes:` line after `stderr -> log`. It reads `one at a time, so threads' lines can't overwrite each other` when the lock below is in place, or `NOT locked: <why>` when it isn't; the log still works then, as it did before.

### How lines get into `log.txt`

This matters because different lines use different Rust and C paths, but they end in the same file.

`open_log_file` mounts `sdmc`, creates `sdmc:/switch/<app>`, opens `log.txt`, then duplicates the file descriptor onto stdout and stderr:

```rust
// crates/bevy_horizon/src/logging.rs:264-278
let mut fd = open_truncated(&dir);
...
let locked = lock_log_writes(fd);
let stdout_ok = unsafe { ffi::dup2(fd, 1) } == 1;
let stderr_ok = unsafe { ffi::dup2(fd, 2) } == 2;
```

After that:

| Lines | Route |
|---|---|
| Header, `[Boot]`, `[Assets]`, `[Main]`, `[Exit]` heap lines | `write_log`, a direct `write(fd, bytes)` (`logging.rs:678-686`) |
| `println!` lines such as `[Init]`, `[Scene]`, `(A)` and `(+)` | Rust stdout, fd 1, which now points at `log.txt` |
| Bevy `INFO`, `WARN`, `ERROR`, and `tracing::info!` lines | Rust stderr, fd 2, which now points at `log.txt` |
| Panic report | panic hook writes `log.txt`, flushes, then appends `panic.txt` (`logging.rs:806-843`) |
| CPU exception and libnx abort report | `record_fatal` writes debug output, `log.txt`, then `crash.log` or `abort.log` (`compat.rs:280-313`, `315-370`) |

Once the log writer has started, writes by fds 1 and 2 wait in its queue instead of going to the card at once, and `write_log` writes what is queued before its own bytes ([Flushing and the cost of flushing](#flushing-and-the-cost-of-flushing)). The order of lines in the file is the order of the writes either way.

The code sets `NO_COLOR=1` before Bevy logging starts (`logging.rs:139`), so ANSI color codes do not pollute the file.

### Why writes take turns

This matters because a log that two threads wrote over each other can say things that never happened, and can carry text from other files.

Skate 3's first hardware log (findings 20 §1.4) has four broken places. Lines run into each other with no newline, the start of one line is covered by another, and two places hold text that no part of the game writes, 171 bytes in one of them. They fit the way `log.txt`'s writes reached the card:

- `dup2` doesn't give fds 1 and 2 copies of the log's handle. devkitPro's newlib (libsysbase `handle_manager.c`) points them at the same `__handle` and counts the references. So fds 1, 2 and `LOG_FD` share one libnx file, with one offset.
- `write` takes no lock on the way down (libsysbase `write.c`, `_write_r`), and nor does libnx's `fsdev_write` (`nx/source/runtime/devices/fs_dev.c`). That reads the file's offset, writes there, and then adds the length.
- Rust's stdout and stderr each have their own lock, and `write_log` takes neither. So a `println!` line, a Bevy `tracing` line on stderr, and a `[Profiler]` line can be written at once from three threads.

Two writes at once can land at the same offset, so the later one covers the earlier one. Both then add their length, which moves the offset past what was written. The next write leaves a gap there. In Skate 3's log the gap read back as someone else's text, not zeros: the file system doesn't seem to clear the space it gives a file, so the gap held whatever was on the card before.

`lock_log_writes` (`logging.rs:335-365`) fixes it without changing the `dup2`s. Before them, while nothing else can write to the log yet, it:

1. copies the SD card's `devoptab_t` (newlib `sys/iosupport.h`, 32 pointers);
2. swaps the copy's `write_r` for `locked_write` and its name for `bh_log`;
3. adds the copy to newlib's device table (`AddDevice`);
4. points the log's handle at it.

The `dup2`s then share that handle, so a write by fd 1, 2 or `LOG_FD` goes through `locked_write` (`logging.rs:370-395`). Without a log writer, it holds a libnx `Mutex`, `LOG_LOCK`, around the SD card's own `write_r`. With one, it copies the text into the writer's queue and returns, and the writer holds `LOG_LOCK` around its writes. Every other function in the copy is the SD card's, so `fsync`, `close` and `fstat` work as before, and other files on the card don't take the lock. A thread that crashes in the middle of a write still holds the lock when its crash report is written, so `locked_write` doesn't wait when the lock is already its own.

What it doesn't do:

- **A line made of several writes can still have another line inside it.** Rust's stderr is unbuffered, and `eprintln!` with arguments writes each piece separately (`Write::write_fmt`, which `StderrLock` doesn't override). Each piece is whole, and no text is lost, but another thread's line can come between two pieces. `NAK_STATS` lines are printed that way. Bevy `tracing` lines are formatted first and written at once, and `println!` lines go out whole when stdout's line buffer flushes them.
- **Logs from older builds can still have the damage.** The `log writes:` header line says whether a log came from a build with the lock.

### Flushing and the cost of flushing

This matters because the log itself changes timing.

`flush_logs_system` runs in PostUpdate. It has `log.txt` synced once a second and writes heartbeat lines every 10 seconds, with the `[Memory]` line once a minute (`lifecycle.rs:747-825`). `BEVY_HORIZON_MEMORY_EVERY=<seconds>` in the game's environment, read when `HorizonLifecyclePlugin` is built, changes how often the `[Memory]` line comes; `0` leaves it out (`lifecycle.rs:715-745`). Its `mallinfo` walk stalls the frame (below), which in play is a hitch once a minute: Skate 3's `no_memory_log` option sets it to `0` (sk8 findings 33). Builds before findings 20 counted frames instead, 60 and 600, which at 1-2 fps left minutes between heartbeats. `flush_switch_logs()` flushes Rust stdout and then calls `sync_log`, which writes out what the log writer has queued and calls `fsync` on the log descriptor, all before it returns (`logging.rs:626-631`, `:702-711`).

Each line is a separate write to the SD card path. On Horizon, that is a round trip through newlib, libnx fsdev, [IPC](glossary.md#ipc), the FS [service](glossary.md#service), and the card. A2's heartbeat shows the cost:

| Window | Average work | Worst work | Evidence |
|---|---:|---:|---|
| frames 0-599 | 0.68 ms | 70.08 ms | `LOGS/log2.txt:46-48` |
| frames 600-1199 | 0.59 ms | 20.83 ms | `LOGS/log2.txt:49-51` |
| frames 1200-1799 | 0.56 ms | 20.39 ms | `LOGS/log2.txt:52-54` |
| frames 1800-2399 | 0.57 ms | 19.68 ms | `LOGS/log2.txt:55-57` |
| frames 2400-4199 | 0.56-0.57 ms | 19.86-20.53 ms | `LOGS/log2.txt:58-66` |

A3 adds one more number to the heartbeat: how much of the worst frame was spent on `log.txt` writes and fsyncs (`lifecycle.rs:141-260`). Until a hardware log proves it, the 20 ms spike is best read as "probably logging".

Skate 3 proved it on a Switch Lite (sk8 findings 27). In its perf9 and perf10 runs the heartbeat frame was the slowest of its 10 s window in 134 of 273 windows. Those frames spent a median of about 20 ms writing and syncing the log without the minute's `[Memory]` line, and about 74 ms with it, of which `mallinfo` takes about 41-50 ms. The card made the rest uneven. One heartbeat frame took 1.2 s, with each of its lines taking 115-300 ms to write. Once, a sync alone took 274 ms.

So a log writer thread now does the card's work (`logging.rs:397-651`):

- **Start.** `init_switch_logging` starts it (`start_log_writer`) once `log.txt` is open and its writes go through `locked_write`. The log says `[Log] log writer started`. If the writer can't start, the log says so, and each thread writes its own lines as before.
- **Queue.** `locked_write` copies each write into a 512 KiB queue, a static, so queuing never allocates. The writer writes the queue out, oldest first, holding `LOG_LOCK`. A write too big for the free space waits for what is queued and goes straight to the card.
- **Sync.** Once a second and with each heartbeat, `flush_logs_system` calls `request_log_sync`. It flushes Rust's stdout into the queue and asks the writer to sync `log.txt`. The frame waits for neither.
- **Reports.** `write_log` and `sync_log`, and so `flush_switch_logs()`, still write and sync on the calling thread, after writing out what is queued. Panic, crash and abort reports, the header and the exit lines are in `log.txt`, in order, when they return.
- **Exit.** `release_horizon_services` stops the writer (`stop_log_writer`) right after its first line. The writer writes out the queue, syncs and returns, and `close_detached_threads` closes it with the other detached threads. From then on every line is written before the next step runs.

What it costs:

- **One more thread.** The writer is a detached std thread with a 64 KiB stack at the default priority. `[Threads]` counts it among the detached threads still running, and thread numbers in logs and CPU profiles are one higher than in builds before it.
- **The last lines on a kill.** Text still queued when the system ends the process, as closing the game from HOME does, is lost. The writer keeps up within a write or two, so that is the last lines at most. A crash or panic writes out the queue before its report, so those lose nothing.

The heartbeat's work line now ends `which spent X ms of it in flush_logs_system (...)`, and the brackets say whether the writer is running. With it, X is formatting the heartbeat lines and, once a minute, `[Memory]`'s `mallinfo` walk. That walk holds malloc's lock, so moving it to another thread would only move the wait to the next thread that allocates.

### Boot lines

This matters because the boot lines tell you whether Rust startup, memory setup and asset root setup are sane.

A good A2 boot looks like this:

```text
[Boot] launched from sdmc:/switch/hello_switch/hello_switch.nro
[Boot] TPIDR_EL0 = 0x2a6e7a60 (Rust's thread-local base, copied from libnx ThreadVars.tls_tp)
[Boot] TLS template at nro+0x331fef0, align 8: Rust and libnx agree on thread-local addresses
[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory
[Memory] process: 3189.0 MiB in total, 3185.0 MiB used ...
[Memory] malloc heap: 3132.7 MiB, set up by the homebrew loader; malloc has 0.0 MiB in use ...
[Assets] BEVY_ASSET_ROOT = sdmc:/switch/hello_switch ...
```

The memory lines come from `memory::boot_report()` (`memory.rs:72-99`). They use `appletGetAppletType`, `svcGetInfo` total and used memory, libnx heap bounds, and `mallinfo`. The important measured values are:

| Value | Meaning |
|---|---|
| applet type `0` | Application, so the forwarder is running with game memory, not [applet mode](glossary.md#applet-mode) |
| `3189.0 MiB in total` | Measured total after the 96 MiB gameplay-recording holdback |
| `3132.7 MiB` malloc heap | The heap Rust and C allocations use |
| `0.0 MiB in use` at boot | Nothing large allocated yet |

The asset line comes from `set_asset_root`, which sets `BEVY_ASSET_ROOT` to the NRO's folder unless it is already set (`logging.rs:201-228`). Bevy's default asset folder is then `<root>/assets`.

### Heartbeat lines

This matters because the heartbeat separates frame period from real work.

`HorizonPlugin` adds a 60 Hz runner when no runner exists (`lib.rs:31`, `:52-54`). That runner sleeps out the rest of each frame, so `60.0 fps` does not mean the app used all 16.67 ms. The work line measures the main schedule only: from just before `First` to just after `Last`, excluding runner sleep (`lifecycle.rs:554-646`). Bevy's render step is not in it either. After the main schedule, the same `app.update()` extracts to and runs the render sub-app (bevy_app 0.19.1, `sub_app.rs:574-606`), and the timer has already stopped. In a build that draws, the frame rate and the work line together give the render step and the wait for the GPU only as a remainder. Breakout in the harness (bo1) ran 19 fps from frame 600 on, about 52 ms a frame, with 8 to 10 ms of work: the other 43 ms or so is the render step and the emulator, in unknown shares.

A2's steady-state lines:

```text
LOGS/log2.txt:49-51
[Heartbeat] frame 1200, 20.0 s after the first frame, 59.9 fps on average
[Heartbeat] work per frame over the last 600 frames: 0.59 ms on average, 20.83 ms at worst (60 fps allows 16.67 ms)
[Memory] malloc has 9.6 MiB in use, 10.0 MiB taken from the 3132.7 MiB heap
```

Read them this way:

- **`59.9 fps` or `60.0 fps`** says the loop period is near 60 Hz.
- **`0.59 ms on average`** says almost the whole 16.67 ms budget is spare.
- **`20.83 ms at worst`** says one frame missed the budget, probably due to logging.
- **`malloc has ...`** lets you spot leaks between heartbeat windows.

A steady 60 fps without rendering proves little for a game. A2 drew nothing: hello_switch then set `RenderPlugin` to `backends: None` on Horizon. Its Vulkan build draws, and the work line leaves the render step out (above).

### Panic reports

This matters because a panic looks like an exit from the console side.

The panic hook formats one report, writes it to debug output, `log.txt`, and `panic.txt`, then the target aborts (`logging.rs:806-843`). The target's JSON sets `panic-strategy` to `abort` (`aarch64-nintendo-horizon.json:24`).

A real panic report:

```text
LOGS/panic.txt:2-8
==================== RUST PANIC ====================
app:      hello_switch
thread:   main
location: .../bevy_app-0.19.1/src/plugin_group.rs:505:14
message:  Cannot disable a plugin that does not exist.
On Switch (panic=abort) the app now exits without a crash report: to the homebrew menu,
or to HOME if other threads are still running (log.txt's [Exit] lines say which).
```

Because `panic.txt` appends, `LOGS/panic.txt` had the same report twice (`lines 2-16`). That meant the bad NRO had been launched twice. It did not mean the later build panicked.

### Exit lines and the heap check

This matters because A2's app exited cleanly, but the loader crashed after it.

A3 adds exit cleanup in `userAppExit`. The hook calls `release_horizon_services` (`compat.rs:67-75`). That code checks the heap, calls `socketExit`, waits for unlock, checks again, prints PASS or FAIL, and fsyncs (`lifecycle.rs:77-128`). The heap check walks the malloc heap and looks for pages the loader's next `IFile::Read` cannot use (`memory.rs:143-233`).

Expected A3 lines:

```text
[Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
[Exit] heap check, before releasing services: 1 block, 2.20 MiB, would fail the loader's read:
[Exit]   0xSTART-0xEND, 2.20 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, as an open transfer memory or a thread stack leaves it
[Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
[Exit] the heap was unlocked ... ms later, once the services had let go of their transfer memory
[Exit] heap check, after releasing services: the loader can read into all 3132.7 MiB of the malloc heap; every page is read/write and unlocked
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

These lines are not proven on hardware yet. A3 is built, awaiting a run.

## Part 2: reading an Atmosphère crash report

This matters because a crash report is easy to over-read. It often contains dead stack frames, zeroed registers, and old IPC bytes. Read it field by field.

### Where reports go, and what creates them

Atmosphère writes reports under:

```text
sdmc:/atmosphere/crash_reports/<unix seconds>_<program id>.log
```

Example: `01791176509_05446530aca7e000.log` has Unix timestamp `1791176509`, and program ID `05446530aca7e000`. The findings decode it to the same minute as A2's last log write.

Reports are made by [creport](glossary.md#creport), after a CPU exception or a libnx fatal error that reaches `svcBreak`. A Rust panic does not make one here.

Where you land is a clue:

| Where you land | Likely meaning | First file to read |
|---|---|---|
| sphaira menu | clean app exit or Rust panic | `log.txt`, then `panic.txt` |
| HOME menu with a new report | CPU exception, libnx fatal error, or loader crash after app exit | newest crash report |
| HOME menu, `log.txt` ends with `[Exit] ending the whole process (svcExitProcess)` | app exit or Rust panic with threads still running: the normal end of a Bevy app ([lifecycle](lifecycle-and-exit.md#the-userappexit-hook)) | `log.txt`, then `panic.txt` |
| HOME menu, no app exit lines | system closed the whole process, or a crash before sync | crash reports and last synced `log.txt` |

### `Result`: decode it before guessing

This matters because the same abort site can report different causes.

A Horizon [result code](glossary.md#result-code) packs module and description:

```text
module      = code & 0x1ff
description = (code >> 9) & 0x1fff
shown as      (2000 + module)-description
```

`bevy_horizon` prints this format in logs through `ResultCode` (`logging.rs:745-758`). Examples:

| Raw | Shown | Meaning in these runs |
|---|---|---|
| `0x1559` | `2345-0010` | libnx `IoError`: `romfsMountSelf` found no RomFS; harmless for SD-card assets (`LOGS/log2.txt:24`) |
| `0xD401` | `2001-0106` | kernel `InvalidCurrentMemory`: an IPC buffer contains pages in the wrong state (`LOGS/crash_reports/01791176509_05446530aca7e000.log:2`) |
| `0x0` | `2000-0000` | loader aborted after a successful read; earlier rejected-NRO reports used this |

### Module list first

This matters more than the stack trace.

In A2's exit crash, the module list was only:

```text
LOGS/crash_reports/01791176509_05446530aca7e000.log:121-126
Module Info:
    Number of Modules:           01
    Module 00:
        Address:                 000000002dc00000-000000002dc06000
        Name:                    hbl
```

That means the app's NRO was no longer mapped. The crash was in sphaira's loader after our code exited. If your NRO is listed, subtract its module base or the `nro base` header line from addresses in that module. If it is not listed, do not force its symbols onto the trace.

### PC, LR, and stack trace

This matters because PC and LR tell where the report stopped, while the stack may walk through old frames.

A2's report:

```text
LOGS/crash_reports/01791176509_05446530aca7e000.log:50-55
LR: 000000002dc0373c (hbl + 0x373c)
PC: 000000002dc03810 (hbl + 0x3810)
Stack Trace:
    ReturnAddress[00]: 000000002dc0054c (hbl + 0x54c)
```

Those are sphaira loader addresses. `PC` is the `svcBreak` instruction, `LR` is just after the call to `svcBreak`, and frame `[00]` is the return after `diagAbortWithResult` in loader code.

The rest of the trace loops through old app exit frames:

```text
LOGS/crash_reports/01791176509_05446530aca7e000.log:55-85
ReturnAddress[01]: 00000000274c202c
ReturnAddress[02]: 00000000274cbaa8
ReturnAddress[03]: 00000000274d2d38
ReturnAddress[04]: 000000002745cbf4
ReturnAddress[05]: 000000002745cbc0
... repeats ...
```

Those addresses were left on the loader's main stack from the app's exit path. They are useful only after you prove the module and base. They are not a live call stack inside the app.

### Symbolizing addresses

This matters because an address is only meaningful relative to the right image.

For an app address while the NRO is listed:

1. Read `nro base` from `log.txt`.
2. Subtract it from the crash address.
3. Look up the result in the ELF.

Example command shape:

```sh
/opt/devkitpro/devkitA64/bin/aarch64-none-elf-addr2line \
  -e target/aarch64-nintendo-horizon/release/hello_switch 0x11402c

/opt/devkitpro/devkitA64/bin/aarch64-none-elf-objdump \
  -d target/aarch64-nintendo-horizon/release/hello_switch | less
```

For a loader address, use the loader's module base and the extracted loader ELF, not the app ELF. If the module list names only `hbl`, app symbols are the wrong tool.

### X8-X28 can be zero because the thread is inside an SVC

This matters because zero registers can look like data corruption.

A2's report has X8 through X28 all zero (`LOGS/crash_reports/01791176509_05446530aca7e000.log:28-48`). The thread stopped inside `svcBreak`, an [SVC](glossary.md#svc) call. For this kind of stop, the debugger/report path does not preserve every general register in the text report. Treat X0-X2 as the `svcBreak` arguments here; do not build a theory from X8-X28 being zero.

### The TLS dump: the IPC message buffer

This matters because the first 0x100 bytes of a thread's TLS are the IPC message buffer. It can identify the last service call.

A2's TLS dump:

```text
LOGS/crash_reports/01791176509_05446530aca7e000.log:103-110
TLS Address: 00000000327ed200
0000327ed200 04 00 00 01 12 00 00 00 00 00 00 c7 00 00 c0 5a
0000327ed210 01 00 00 c0 00 00 00 00 00 00 00 00 00 00 00 00
0000327ed220 01 00 28 00 88 00 00 00 00 00 00 00 00 00 00 00
0000327ed230 53 46 43 49 00 00 00 00 00 00 00 00 00 00 00 00
0000327ed250 00 00 00 c7 00 00 00 00 00 40 23 00 00 00 00 00
```

Read little-endian:

| Offset | Bytes | Meaning |
|---|---|---|
| `+0x00` | `04 00 00 01` | request type with one B buffer, which the service writes into |
| `+0x04` | `12 00 00 00` | 0x12 data words follow |
| `+0x08` | `00 00 00 c7` | B buffer size `0xC7000000`, 3,184 MiB |
| `+0x30` | `53 46 43 49` | `SFCI`, an IPC command header |
| `+0x50` | `00 00 00 c7 00 00 00 00` | read size `0xC7000000` |
| `+0x58` | `00 40 23 00 00 00 00 00` | leftover `0x234000`, not part of this shorter request |

The command is `IFile::Read` from offset 0 into the loader's whole heap. The leftover `0x234000` matches libnx's default socket transfer memory size, 2.20 MiB. It is a fingerprint of the resource left locked by `socketInitialize`.

### Thread dump and stack dump

This matters because the report includes the crashed thread twice: once under `Crashed Thread Info`, then again under `Thread Report`.

Check:

- **Thread ID and stack region.** If there are many threads, make sure you are reading the crashed one.
- **Stack dump.** It may contain local variables near the abort. In loader rejections, it held the number of bytes read. In A2's exit crash it shows the result and old frame records.
- **Stack trace repetition.** A loop does not mean recursive code. It can mean the loader reused a stack with old frame pointers and creport walked through dead frames until its 32-entry limit.

### What to collect after a run

This matters because `log.txt` is overwritten by the next launch, while appended files keep old data.

1. **Rename `log.txt` per run** before launching again, for example `log-a3-run1.txt`.
2. **Copy `panic.txt`** if it exists, but note whether you deleted it before the run.
3. **Copy any new `crash.log` or `abort.log`** next to the NRO.
4. **Copy the newest Atmosphère crash report** from `sdmc:/atmosphere/crash_reports/`.
5. **Record how you exited:** (+), HOME close, HOME then return, or power/sleep.
6. **For A3 specifically, include whether the exit lines end in PASS or FAIL.** If there is a new `2001-0106` report after PASS, the loader failed for a different reason and the report is essential.

## Part 3: where the time goes (the CPU profiler)

This matters because a slow frame on the Switch has many possible owners: the main schedule, Bevy's render thread, its task pools, NVK's shader compiler, or a thread waiting on the GPU or the SD card. The profiler says which (`crates/bevy_horizon/src/profiler.rs`).

**Turning it on.** Set `BEVY_HORIZON_CPU_PROFILE=1` before `HorizonPlugin` is built. A game on the Switch sets environment variables from a file of its own; Skate 3's `options.txt` takes `env NAME=value` lines. `=<microseconds>` instead of `1` sets the interval: 2000 unless set, at least 500. Every `BEVY_HORIZON_CPU_PROFILE_EVERY` seconds (30 unless set, `0` for never) the profiler rewrites `cpu-profile.txt` beside `log.txt`. A game can instead pick its own window with `bevy_horizon::profiler::reset()` and `write_report_to(path)`, as Skate 3's `SKATE_PERF_REPORT` harness does (`<report>.cpu.txt`).

**How it works.** Every interval, a thread at priority 0x1E, above the game's threads, takes each thread in turn:
1. It pauses the thread (`svcSetThreadActivity`) and reads its registers (`svcGetThreadContext3`).
2. It follows the thread's frame records up its stack. Rust code keeps them, because the target spec has `"frame-pointer": "always"`. For C code built without them it scans the stack for return addresses instead, and marks the chain (flag 4).
3. It lets the thread run again.

A thread whose PC is on an `svc` instruction, or just after one, is waiting in a system call. The kernel's count of each thread's CPU time (`svcGetInfo`, `InfoType_ThreadTickCount`: id 25 from firmware 13.0.0, 0xF0000002 before it) gives how busy the thread really was. The report's `cpu_times` line says which id answered, or `none`. The profiler follows every thread `threadCreate` makes, which includes std's and Mesa's threads (`compat.rs` adds each one), plus the main thread. It drops a thread once its handle is closed. The technique is UnleashedRecomp-NX's; the code is separate ([CREDITS.md](../CREDITS.md)).

**The cost.** Each pass pauses each thread for a few microseconds. The sampler's own time is in the report as `sampler_ms`, and a profiled game runs a little slower than an unprofiled one. Before the loader takes the heap back, a [release hook](lifecycle-and-exit.md) stops the sampler.

**Reading it.** Copy the report off the card and run:

```sh
scripts/symbolize_cpu_profile.py cpu-profile.txt <game>/target/aarch64-nintendo-horizon/release/<bin>
```

It names the offsets with `nm`, so the ELF must be the one the NRO was made from: a rebuild moves every function. It prints:
- each thread's busy % (CPU time over the time watched), with the function it was started to run (`main`, or `#N` as in log.txt's `[Thread] #N` lines);
- where the CPU time goes over all threads: each thread's CPU time, shared out over the functions its running samples were in;
- for the busiest threads, the functions they run in (self), the functions they run under (inclusive), and where they wait.

`--chains N` adds each thread's N commonest whole call chains. `--lines` adds file:line for the hottest addresses with `addr2line`, which takes a minute or two on a large game.

**Limits.**
- **A running sample is a thread that wasn't in a system call.** With more threads ready than the three cores an NRO gets, a thread waiting for a core still counts as running. Its busy % is the measure of its CPU time; the samples only say where that time went.
- **Inlined functions count as their callers,** because names come from symbols, not debug info.
- **It follows 64 threads at most.** `untracked_threads` in the report counts the rest.
- **A resume can fail.** The profiler then checks whether the thread has exited: a thread's handle is signalled when it does, which is what libnx's `threadWaitForExit` waits for. If it has, nothing was left paused, and the report counts it in `exited_while_paused`. If not, the thread is still paused. The report counts it in `resume_failures`, and the log gets a `[Profiler] ERROR` line with the thread's number and the kernel's result code. Each later pass tries the resume again until it works or the thread has exited, and the log says which. On the Switch, perf1 had no failed resumes in 14,340 and 14,642 passes. The emulator had one (findings 20), before the profiler told the two cases apart.
- **Reports made before the firmware 13.0.0 fix have no CPU times.** The first profiler asked only with id 0xF0000002, which later kernels reject. Every `cpu_ms` in those reports reads 0, and a thread that had exited counted in `pause_failures` on every pass. For such a report (`cpu_times` missing, every `cpu_ms` 0), the script estimates each thread's busy % from its share of running samples, and says so.


## Part 4: what a shader costs (`NAK_DEBUG=stats`)

This matters because a GPU-bound frame is usually a few shaders' fault, and the numbers that say why (instructions, registers, how many warps fit) never reach the log otherwise. NAK, NVK's shader compiler, prints them only under `NAK_DEBUG=print`, which also prints the whole IR after every pass and makes a large shader take minutes to compile. bevy-horizon's `patches/mesa-switch/0003-nak-debug-stats.patch` adds `NAK_DEBUG=stats`: one line per shader NAK compiles, on stderr, so in `log.txt`.

**Turning it on.** Set `NAK_DEBUG=stats` before the Vulkan device is made; in Skate 3's `options.txt`, an `env NAK_DEBUG=stats` line. In a Switch build it adds to the flags NAK turns on by default ([toolchain](toolchain.md), patch `0019`), so the statistics are for the shaders the game runs. Add `env MESA_SHADER_CACHE_DISABLE=true` too: a shader that comes from NVK's disk cache ([rendering.md](rendering.md#on-the-switch)) isn't compiled, so it prints nothing. With the cache off every launch compiles every shader again, so leave both out of normal play.

**A line.** Skate 3's world fragment shader, in the emulator:

```text
NAK_STATS stage=fragment instrs=9321 static_cycles=56334 max_warps=8 gprs=192 slm=0 spills_mem=0 fills_mem=0 spills_reg=178 fills_reg=188 tex=127 txd=120 tld=8 ld=154 ipa=19 bra=384 source=3711ce8d name=
```

- `instrs`: instructions in the final shader. `static_cycles`: NAK's own estimate of their cost, without branches or memory waits.
- `gprs` and `max_warps`: registers per thread, and how many warps (32 threads each) fit on an SM at once with that many. A Maxwell SM hides a texture fetch's or memory load's latency by running other warps meanwhile, so a shader with few warps waits instead.
- `slm`: local memory per thread. `spills_mem` and `fills_mem`: registers spilled to local memory and read back. `spills_reg` and `fills_reg`: values moved into GPRs from a smaller register file, such as predicates, for want of room there.
- `tex`, `txd`, `tld`: texture instructions: sampled with an implicit or given level, with explicit gradients, and fetched or gathered.
- `ld`: loads from global, local, shared and constant-buffer memory. `ipa`: fragment inputs interpolated. `bra`: branches.
- `source`: the first 4 bytes of the BLAKE3 of the SPIR-V the shader came from (Mesa `spirv_to_nir.c`), so lines with the same `source` are the same module, compiled for different pipelines. `name`: NIR's name for the shader, which is empty for shaders from SPIR-V, as every wgpu shader is. Mesa's own shaders are built in NIR, so they have names and no SPIR-V: the compute shaders behind NVK's buffer fills and copies print `source=00000000` with names such as `vk-meta-fill-buffer`, `vk-meta-copy-buffer` and `vk-meta-copy-buffer-to-image-compute` (mesa-switch `src/vulkan/runtime/vk_meta_copy_fill_update.c:1102`, `:2334`, `:2529`).

To find one game shader, look for the largest `instrs` per stage: `grep -a NAK_STATS log.txt | sort -t= -k3 -n`. On a GPU older than Turing, the driver's buffer checks put a branch around every storage-buffer access, so a shader that branches little of its own, such as a vertex shader, with about as many `bra` as `ld` was built with them (`BH_ROBUST_BUFFER_ACCESS=1`). naga's clamp, the default, adds a few instructions but no branches or loads ([rendering.md](rendering.md#buffer-robustness)).
