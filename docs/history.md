# History: from a rejected NRO to a clean exit

This page tells the current bring-up history for `bevy_horizon`: how a [homebrew](glossary.md#homebrew) [NRO](glossary.md#nro) went from being rejected by sphaira's [loader](glossary.md#loader) to a build that passes the basic checks and is ready to prove a clean exit. Read it if you are porting your own [Bevy](glossary.md#bevy) game to [Horizon](glossary.md#horizon) and need to know which failure you are seeing.

All hardware evidence here is from a Switch in [handheld mode](glossary.md#handheld-mode), under [Atmosphère](glossary.md#atmosphère), launched from [sphaira](glossary.md#sphaira) 1.0.0. Docked mode is not tested.

## In short

- **The first three crashes were not in `bevy_horizon`.** sphaira's `hbl` rejected an NRO with no asset section. The [crash report](glossary.md#crash-report) listed only `hbl`, not the app.
- **Crash 4 was Rust thread-local storage.** Rust uses [TPIDR_EL0](glossary.md#tpidr_el0) for [thread-local storage](glossary.md#thread-local-storage). [libnx](glossary.md#libnx) did not set it, so Rust startup faulted before `main`.
- **Run 5 was a panic, not a CPU crash.** A stale `.disable::<PipelinedRenderingPlugin>()` line panicked at `plugin_group.rs:505:14`; `panic.txt` contains it twice because the file is appended.
- **Build A ran the frame loop.** It held about 60 fps for 3,600 frames, with 16.6727 ms between steady-state heartbeats, and accepted (A) and (+).
- **A2 passed every basic check.** It proved input, files on the [SD card](glossary.md#sdmc), Bevy `AssetServer`, a Rust thread with thread-locals, and memory reporting.
- **The exit crash was the loader after our code exited.** A2 logged `[Exit] app.run() returned Success`, then sphaira crashed with `2001-0106` while reading the next NRO.
- **A3 is the current fix, not yet hardware-run.** It releases sockets in [userAppExit](glossary.md#userappexit), waits for the [heap](glossary.md#heap) to unlock, checks all heap pages, and logs PASS or FAIL.

| Step | What you saw on the console | What it really was | Fix |
|---|---|---|---|
| Crashes 1-3 | HOME menu with new reports | sphaira `hbl` rejected an NRO with no ASET section | Build with [NACP](glossary.md#nacp) and icon, and check `ASET` |
| Crash 4 | HOME menu with a report | Rust read a thread-local with `TPIDR_EL0 = 0` before `main` | Set `TPIDR_EL0` in [userAppInit](glossary.md#userappinit) and wrap `threadCreate` |
| Run 5 | Back to sphaira | Rust [panic](glossary.md#panic), then `panic=abort` and `_exit` | Remove the `.disable` line |
| Build A / run 6 | Ran for about 67 s, then (+) | Our code exited; later evidence shows the loader crashed after it | A2/A3 added checks and exit cleanup |
| C layouts | File checks would have lied | Rust `libc` had 3DS layouts for Switch [newlib](glossary.md#newlib) structs | Patch `libc`; fail if a [crate patch](glossary.md#crate-patch) is unused |
| A2 | Every check passed; then HOME with report | Basics passed; loader crashed after our exit | Release socket transfer memory before exit |
| Exit crash | HOME menu with `2001-0106` | sphaira read into the whole heap; 2.2 MiB socket [transfer memory](glossary.md#transfer-memory) was locked | A3: `socketExit`, wait, [heap check](glossary.md#heap-check) |
| A3 | Built only | Current step awaiting console run | NRO 53,745,861 bytes; SHA-256 `36941504c59d3456c6b4b5d3665d70d3130ab36ab4c161fe4aed296821458ec6` |

## Crashes 1-3: the loader rejected the NRO

This matters because the most important field in a crash report is often the module list. If your NRO is not listed, your code was not running.

The three reports showed only sphaira's `hbl` module. The report result was `0x0 (2000-0000)`, the PC was `hbl + 0x3810`, and stack frame `[00]` was `hbl + 0x54c`. The loader had successfully read the file, then failed its size check and called `diagAbortWithResult(0)`.

The reason was packaging. `elf2nro` accepts `--nacp` and `--icon`; title strings belong in `nacptool`, not in `elf2nro`. The old build passed `--name`, `--author` and `--version`, which `elf2nro` ignored. The file ended at the image size, with no asset section after it. sphaira requires the NRO to have at least the image plus the 0x80 bytes that make room for the asset section.

The current build script says this directly:

```sh
# scripts/build_example.sh:60-74
# elf2nro has no --name/--author/--version flags ...
nacptool --create "Hello Switch" "bevy-horizon" "0.1.0" "$OUT/hello_switch.nacp"
elf2nro "$OUT/hello_switch" hello_switch.nro \
    --nacp="$OUT/hello_switch.nacp" \
    --icon=/opt/devkitpro/libnx/default_icon.jpg
```

It also checks the result:

```text
scripts/build_example.sh:91-94
if head[0x10:0x14] != b"NRO0" or asset_magic != b"ASET" or file_size < image_size + 0x80:
    ... fail ...
print(f"ok: image 0x{image_size:x} + ASET section, file 0x{file_size:x} bytes")
```

**Lesson:** read the module list before symbolizing stack frames. The early wrong analyses quoted addresses that were not in any report. Do not do that. Quote only addresses that appear in the report.

## Crash 4: Rust needed `TPIDR_EL0`

This matters because a Rust program can fail before your first line of `main`. Logging that starts in `main` is too late for this class of failure.

The crash happened after sphaira accepted the packaged NRO. The app's module was listed, but `main` had not printed anything. Rust startup uses thread-local variables. On this target, Rust's local-exec TLS model reads them at `TPIDR_EL0 + offset`. The thread dump showed `TPIDR_EL0 = 0`, so the first access faulted near address `0x700`.

The fix is in `compat.rs`. `userAppInit` runs after libnx has mounted services and the SD card, but before C constructors, Rust startup and `main`. It copies libnx's `ThreadVars.tls_tp` into `TPIDR_EL0`, then starts the boot log:

```rust
// crates/bevy_horizon/src/compat.rs:58-65
pub extern "C" fn userAppInit() {
    let tp = set_thread_pointer();
    start_boot_log(tp);
}
```

New Rust threads go through libnx `threadCreate`, so the target links with `--wrap=threadCreate` (`aarch64-nintendo-horizon.json:28`). The wrapper replaces the entry point with `thread_start`, sets `TPIDR_EL0`, and then calls the real thread function (`compat.rs:108-157`). A2 later proved the second-thread half on hardware.

Logging also moved before `main`. `start_boot_log` opens `log.txt`, prints the NRO base and TLS layout, and writes the memory report (`logging.rs:120-158`). The first hardware logs show it working:

```text
LOGS/log2.txt:9-17
[Boot] launched from sdmc:/switch/hello_switch/hello_switch.nro
[Boot] TPIDR_EL0 = 0x2a6e7a60 ...
[Boot] TLS template at nro+0x331fef0, align 8: Rust and libnx agree ...
[Main] init_switch_logging("hello_switch"): Rust's runtime is up and main is running
```

**Lesson:** on this target, set the thread pointer before any Rust code runs. Then open the log before `main`, because Rust startup is part of the port.

## Run 5: back to sphaira was a panic

This matters because not every return to sphaira is a clean exit. A Rust panic does not create an Atmosphère report here.

Run 5 reached `main`, printed `[Init] Step 1`, then panicked in Bevy's plugin group code:

```text
LOGS/panic.txt:2-8
==================== RUST PANIC ====================
app:      hello_switch
thread:   main
location: .../bevy_app-0.19.1/src/plugin_group.rs:505:14
message:  Cannot disable a plugin that does not exist.
On Switch (panic=abort) the app now exits to the menu without a crash report.
```

The panic was caused by a stale line that tried to disable `PipelinedRenderingPlugin`. With the example's features, that plugin was not in `DefaultPlugins`, and `disable` panics if the plugin is missing. Current `main.rs` keeps the warning in code and does not call `disable` (`examples/hello_switch/src/main.rs:30-33`).

`panic.txt` has the same report twice (`LOGS/panic.txt:2-16`) because `log.txt` is opened with `O_TRUNC`, while `panic.txt` is opened with `O_APPEND`. The current code makes that explicit: `open_truncated` uses `O_TRUNC` (`logging.rs:255-260`), while `append` uses `O_APPEND` (`logging.rs:333-342`). Delete `panic.txt` before a run, or read only the last report.

The target uses `panic-strategy = "abort"` (`aarch64-nintendo-horizon.json:24`). After the panic hook writes `log.txt` and `panic.txt` (`logging.rs:408-432`), abort goes through `_exit` and back to the loader. No Atmosphère report is expected.

**Lesson:** when you land in sphaira, read `log.txt` and `panic.txt` first. A new crash report is not required for a real failure.

## Build A / run 6: the frame loop and input worked

This matters because it was the first hardware run where Bevy built an app, ran schedules, read input, and exited through the app's own code.

The log showed the no-renderer path and the Switch frame loop:

```text
LOGS/log1.txt:19-27
ERROR bevy_render::extract_resource: Render app did not exist ...
[Init] Step 2: DefaultPlugins added (no renderer yet)
WARN bevy_horizon::lifecycle: romfsMountSelf returned error code: 5465
INFO bevy_horizon::lifecycle: BSD socket layer initialized successfully
...
[Init] Step 6: Calling app.run()...
```

`romfsMountSelf` returned `5465`, later printed as `0x1559 = 2345-0010`. That is libnx `IoError` for an NRO with no [RomFS](glossary.md#romfs), which is harmless when assets come from `sdmc:/switch/hello_switch/assets/`. Current `lifecycle.rs` decodes it and says this (`lifecycle.rs:31-58`).

The frame loop was steady:

| Frames | Log evidence | Meaning |
|---|---|---|
| 0 | `frame 0` at `22:04:25.806563` (`LOGS/log1.txt:34`) | First PostUpdate heartbeat |
| 600 | `frame 600` at `22:04:35.813718` (`LOGS/log1.txt:36`) | About 10 s later |
| 1200-3600 | Heartbeats every 10.0036 s (`LOGS/log1.txt:37-41`) | 16.6727 ms per frame steady-state |

This is only a frame-loop result. `HorizonPlugin` adds `ScheduleRunnerPlugin::run_loop` at 60 Hz (`lib.rs:20-44`), and the runner sleeps the rest of the frame. A steady 60 fps without rendering does not prove rendering speed.

Input also worked. The log showed handheld style `0x2`, Bevy accepted the gamepad as `Gamepad 177v0`, and (A) and (+) were read (`LOGS/log1.txt:30-45`). The surprise was the Bevy line; it is good news, because Bevy's own gamepad connection system accepted the event emitted by this layer.

One correction matters: run 6 did not exit cleanly overall. It logged `[Exit] app.run() returned Success` (`LOGS/log1.txt:45`), but a later-uploaded report shows the loader crashed after that with `2001-0106` (`LOGS/crash_reports/01791151534_05446530aca7e000.log:1-2`).

**Lesson:** `[Exit] app.run() returned Success` proves the app ended. It does not prove the loader survived after taking the heap back.

## C struct layouts and the silent patch miss

This matters because ABI bugs in file structs do not always crash. They can make every file look wrong.

The `libc` crate definitions for Horizon/newlib were effectively the 3DS layouts. On the Switch, devkitA64's newlib has different sizes and offsets for fields such as `dev_t`, `ino_t`, `struct stat` and `struct dirent`. Before the fix, Rust would read `st_mode`, `st_size` and directory entry names from the wrong offsets. The likely symptoms were:

| Call | What would be wrong |
|---|---|
| `metadata(path).is_file()` | false even for a file |
| `metadata(path).len()` | a timestamp-sized value, not 45 bytes |
| `create_dir_all(existing_folder)` | failure because the existing folder did not look like a folder |
| `read_dir` | names losing their first two letters |

A2's `fs_check` tests the fixed path with `stat`, `fstat` and `read_dir` (`examples/hello_switch/src/checks.rs:110-180`). The hardware result passed:

```text
LOGS/log2.txt:31-32
[Check] asset: wrote sdmc:/switch/hello_switch/assets/a2_check.txt (45 bytes) ...
[Check] fs: PASS, stat and fstat say a2_check.txt is a 45-byte file, ... read_dir lists it as a file
```

The same investigation found that the `bevy_asset` patch had been remade for the wrong Bevy version earlier. Cargo skipped it silently because Bevy used 0.19.1 and the patch was 0.19.0. The build script now stops if a patch goes unused.

**Lesson:** compare Rust FFI layouts with the C headers for this exact target, and make the build fail when a patch is not applied.

## A2: every basic check passed

This matters because A2 is the first run that proves the non-rendering basics together on hardware.

A2 booted as an application, not applet mode, and got full game memory:

```text
LOGS/log2.txt:12-14
[Memory] applet type 0 = Application: running as a game ...
[Memory] process: 3189.0 MiB in total, 3185.0 MiB used ...
[Memory] malloc heap: 3132.7 MiB ...
```

The `3189.0 MiB` total corrects earlier notes that expected about 3,285 MiB. The missing 96 MiB is held back for [gameplay recording](glossary.md#gameplay-recording), because the forwarder's NACP asks for automatic recording. The malloc heap is still 3,132.7 MiB. Do not repeat the old "about 450 MB unless title override" claim as the status for this forwarder; the HOME-menu [forwarder](glossary.md#forwarder) runs as an application.

A2 also proved:

| Check | Evidence |
|---|---|
| Assets | `AssetServer` loaded `a2_check.txt` in frame 0, 31.9 ms after the request (`LOGS/log2.txt:38`) |
| C file structs | `fs` check passed (`LOGS/log2.txt:32`) |
| Thread-local storage on a second thread | wrapper logged `threadCreate`, `TPIDR_EL0 = libnx tls_tp`, and the check passed (`LOGS/log2.txt:42-44`) |
| Input | handheld style `0x2`, gamepad connected, (A) and (+) pressed (`LOGS/log2.txt:35-37`, `67-69`) |
| Memory stability | malloc stayed at 9.6 MiB in use and 10.0 MiB taken from frames 600-4200 (`LOGS/log2.txt:48-66`) |
| App code exit | `[Exit] app.run() returned Success` (`LOGS/log2.txt:70`) |

The work-time lines explain the frame loop better than build A did. A frame normally used about 0.56-0.68 ms of work, not 16.67 ms. One frame about every 10 seconds took around 20 ms, likely because the heartbeat writes multiple log lines and fsyncs. The first window was worse, 70.08 ms, because frame 0 did startup work and logged the first heartbeat (`LOGS/log2.txt:46-50`).

**Lesson:** measure frame work, not only frame period. A sleeping runner can hide almost all of the budget.

## The exit crash: the loader, not the app

This matters because the crash happened after the app had returned, but the root cause was still a resource the app left behind.

A2's report says:

```text
LOGS/crash_reports/01791176509_05446530aca7e000.log:1-2
Atmosphère Crash Report (v1.7):
Result:                          0xD401 (2001-0106)
```

The module list contains only `hbl`, not the NRO (`LOGS/crash_reports/01791176509_05446530aca7e000.log:121-126`). PC and LR were in `hbl` (`lines 50-54`). The stack then loops through old app exit frames (`lines 55-85`). That means our NRO had exited and sphaira's loader was running again.

The thread's [IPC](glossary.md#ipc) message buffer in [TLS](glossary.md#thread-local-storage) showed the failed call:

```text
LOGS/crash_reports/01791176509_05446530aca7e000.log:103-110
TLS Address: 00000000327ed200
0000327ed200 04 00 00 01 12 00 00 00 00 00 00 c7 00 00 c0 5a
...
0000327ed230 53 46 43 49 ...
...
0000327ed250 00 00 00 c7 00 00 00 00 00 40 23 00 00 00 00 00
```

Read little-endian, that is an `IFile::Read` request (`SFCI`, command 0) with a B buffer of `0xC7000000` bytes: 3,184 MiB. That is sphaira's whole heap read for the next NRO. The leftover `0x234000` at `+0x58` is 2.20 MiB, matching libnx's default socket transfer memory.

The root cause is in the current startup path. `HorizonLifecyclePlugin` calls `socketInitialize(NULL)` (`lifecycle.rs:64-72`). libnx creates a 2.2 MiB transfer memory for the `bsd` [service](glossary.md#service). It stays locked until `socketExit` and until the service lets go. Before A3, nothing called `socketExit`, so sphaira's whole-heap read included a locked page and the kernel returned `InvalidCurrentMemory`, `2001-0106`.

Run 6 crashed the same way. Its later report also says `0xD401 (2001-0106)`, PC `hbl + 0x3810`, and frame `[00]` `hbl + 0x54c` (`LOGS/crash_reports/01791151534_05446530aca7e000.log:1-2`, `50-55`).

**Lesson:** clean app exit is not enough. Anything that locks heap pages must be released before handing the heap back to a loader that will reuse it.

## A3: release, wait, check, then return

This matters because it is the current step. It is built, but not yet proven on hardware.

A3 adds `release_horizon_services`, called from libnx's `userAppExit` hook. That hook runs when `main` returns, `std::process::exit` runs, or panic abort goes through `_exit` (`compat.rs:67-75`). The cleanup:

1. Logs that `userAppExit` is running.
2. Checks the heap before releasing services.
3. Calls `socketExit` if sockets were started.
4. Unmounts RomFS only if it had mounted.
5. Waits up to 1 second for locked pages to become readable and writable again.
6. Checks the heap again and logs PASS or FAIL.
7. fsyncs `log.txt`.

The code is in `lifecycle.rs:77-128`; the heap scan and wait are in `memory.rs:143-255`. The scan walks the malloc heap with `svcQueryMemory` and flags blocks that are not read/write or have attributes the kernel's IPC buffer check will reject (`memory.rs:113-188`).

The expected A3 exit lines look like this:

```text
[Exit] userAppExit: main has returned; releasing services before the loader takes the heap back
[Exit] heap check, before releasing services: 1 block, 2.20 MiB, would fail the loader's read:
[Exit]   0xSTART-0xEND, 2.20 MiB: memory type 0x5, permission 0x0, attributes 0x1: locked with no access, as an open transfer memory or a thread stack leaves it
[Exit] socketExit: done; the socket sessions and our handle to their transfer memory are closed
[Exit] the heap was unlocked ... ms later, once the services had let go of their transfer memory
[Exit] heap check, after releasing services: the loader can read into all 3132.7 MiB of the malloc heap; every page is read/write and unlocked
[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it
```

A3 also extends the heartbeat to attribute the worst frame's time to `log.txt` writes and fsyncs (`lifecycle.rs:141-260`). That will confirm whether the 20 ms frame every 10 seconds is just logging.

**Status:** A3 is awaiting a console run. The NRO is 53,745,861 bytes, SHA-256 `36941504c59d3456c6b4b5d3665d70d3130ab36ab4c161fe4aed296821458ec6`. Do not treat the PASS lines as proven until they appear in a hardware log.

## Lessons

- **Read the module list first.** If the NRO is not listed, do not symbolize addresses as app code.
- **Start logging before `main`.** Rust startup and C constructors are part of the port.
- **A panic is not an Atmosphère crash report.** On this target it appends `panic.txt` and exits back to sphaira.
- **A steady 60 fps can prove very little.** The runner sleeps the spare time; measure work per frame.
- **Check ABI layouts with code, not by eye.** Rust `libc` definitions must match devkitA64 newlib exactly.
- **Release every locked heap page before returning to the loader.** Sockets, GPU, audio, and unjoined thread stacks can all matter.
- **Say what is not proven.** Rendering is not tested yet. A3's exit fix is built, not hardware-confirmed.
