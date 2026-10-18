# Glossary

Every technical word used in these documents, explained in plain words. The terms are grouped by topic: the Switch, loading and running a homebrew program, building, Bevy, graphics, and this project.

## The Switch

### Horizon

Horizon is the operating system on the Nintendo Switch. In this project, Rust sees it as `target_os = "horizon"` and runs homebrew code through libnx instead of a normal desktop OS.

### Atmosphère

Atmosphère is the custom firmware used for the hardware tests. It also writes crash reports through [creport](#creport) when a program or loader hits a fatal error.

### Homebrew

Homebrew is software made to run on the Switch outside Nintendo's normal game publishing path. This project builds Bevy examples as homebrew programs.

### NRO

An NRO is the homebrew executable file that the Switch loader starts. The build keeps the linked [ELF](#elf) for symbols, then packages it as an NRO for the SD card.

### NACP

A NACP is title metadata for a Switch program. In this project, `nacptool` creates it and `elf2nro --nacp` puts it into the NRO so loaders such as [sphaira](#sphaira) accept the file.

### RomFS

RomFS is a read-only filesystem packed into an NRO. The current examples mostly use files beside the NRO on [sdmc](#sdmc), so a missing RomFS is logged as normal unless the NRO was built with `--romfsdir`.

### sdmc

`sdmc:` is libnx's name for the Switch SD card filesystem. Logs, panic files, assets, and captest result files are written under paths such as `sdmc:/switch/hello_switch/`.

### FAT32

FAT32 is a common filesystem for Switch SD cards. It has a 4 GiB minus 1 byte limit for one file, and its case-insensitive names can hide filename case bugs that show up on a desktop.

### HOME menu

The HOME menu is the Switch system menu. Returning there can mean the app was closed, a crash report was made, or the loader failed after the NRO had already exited.

### Applet

An applet is a Horizon application type with more limited resources than a full application. The current tested forwarder path runs as an Application, not applet mode.

### Applet mode

Applet mode is a lower-memory launch mode for homebrew. The current measured path is not applet mode; it gets full application memory through a forwarder or title override.

### Handheld mode

Handheld mode is the Switch running on its built-in screen at 1280x720. It is the mode used for the current hardware tests; see [platform-notes.md](platform-notes.md).

### Docked mode

Docked mode is the Switch running as a TV application, expected at 1920x1080. The code can resize the Bevy window resource for it, but it has not been proven on hardware yet.

### Focus

Focus says whether the app is the one the player is using. libnx keeps it as `appletGetFocusState()`: `InFocus` (1), `OutOfFocus` (2, a library applet such as the keyboard is over the app) or `Background` (3, HOME or another program is in front). It changes only when the system sends a `FocusStateChanged` message, and whether HOME sends one depends on the [focus-handling mode](#focus-handling-mode). captest's [watchdog](#watchdog) does not count time spent out of focus against a test.

### Focus-handling mode

The focus-handling mode tells Horizon what to do with an application that loses focus. libnx sets `SuspendHomeSleep` at startup: HOME suspends the app, and the app is never told. `NoSuspend` tells the app (`Background`) and leaves it running. `SuspendHomeSleepNotify` suspends as `SuspendHomeSleep` does; despite its name, HOME sent no message with it on hardware (captest run 3). `bevy_horizon` uses `NoSuspend`, and `SuspendHomeSleepNotify` only while the app is in the background; see [HOME and focus](lifecycle-and-exit.md#home-and-focus).

### Performance mode

Performance mode is Horizon's current clock profile. The captest reports whether the system is in normal handheld clocks or boosted docked clocks.

### Boost mode

Boost mode is a higher-clock mode requested through applet APIs. captest checks the related path, but game performance still has to be measured on hardware.

### CPU core

A CPU core is one processor execution unit. Switch homebrew games generally have three application cores available; core 3 is treated as system-owned in this project.

### Core mask

A core mask is the set of CPU cores a thread is allowed to run on. bevy_horizon can change the mask for new threads so worker threads are not all stuck on core 0.

### Thread priority

Thread priority decides which runnable thread Horizon schedules first. Lower numeric priority is higher priority, so a busy main thread can starve lower-priority workers on the same core.

### SVC

An SVC is a supervisor call into the Horizon kernel. This project uses SVCs through libnx to query memory, set thread masks, set priority, and exit a hung captest process.

### Service

A service is a Horizon system service reached through [IPC](#ipc). Examples here include file services, `bsd` for sockets, `audren` for audio, `nv` for graphics, and `nifm` for network status.

### IPC

IPC is inter-process communication. Horizon services receive requests through IPC buffers, and a bad buffer page can make a call fail before the service sees it.

### Result code

A result code is Horizon's packed error value. The docs show it as `module-description`, such as `2001-0106` for the locked-memory loader failure.

### HID

HID is Horizon's input service. bevy_horizon reads it through libnx's pad API and turns Switch controller state into Bevy gamepad events.

### Pad

A pad is libnx's controller state object. It can represent a Pro Controller, Joy-Con pair, single Joy-Con, or handheld controls.

### Rumble

Rumble is controller vibration. captest has a hands-on rumble test using libnx vibration APIs.

### Motion sensor

A motion sensor is the controller's accelerometer and gyroscope data. captest has a hands-on motion test for the Switch six-axis sensor path.

### Touch screen

The touch screen is the Switch's built-in touch input. captest reads touch states directly through libnx during its hands-on tests.

### audren

`audren` is Horizon's audio renderer service. Both [cpal](#cpal) hosts feed it through libnx's audio driver from a thread of their own, and close it with `audrenExit` when the stream closes or, failing that, at exit: until then its transfer memory keeps heap pages locked ([the audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit)).

### nv

`nv` is the Horizon graphics driver service family. Future rendering through Mesa or OpenGL will need it, and it may hold transfer memory that must be released before exit.

### nifm

`nifm` is Horizon's network interface service. captest uses it to report network status and the current IP address.

### BSD

BSD is the socket service layer exposed by libnx. `socketInitialize` opens it and allocates transfer memory, so bevy_horizon calls `socketExit` before returning to the loader.

### Transfer memory

Transfer memory is heap memory lent to another Horizon process. While a service owns it, the original pages can be locked, which is why exit cleanup and the heap check matter.

### Heap

The heap is the large memory area used for Rust and C allocations. sphaira's loader reuses it for the next NRO, so locked heap pages can make the loader crash after an app exits.

### creport

`creport` is the Atmosphère component that writes crash reports. It records registers, module lists, thread state, and the result code after a fatal break or exception.

### Crash report

A crash report is a file under `sdmc:/atmosphere/crash_reports/`. Read the module list first: if it names only `hbl`, the crash belongs to the loader, not the app.

### Gameplay recording

Gameplay recording is the Switch video capture feature. The tested forwarder's NACP asks for automatic recording, which holds back 96 MiB from the process memory limit.

### Emulator

An emulator is a PC program that imitates Switch hardware. Emulator results are useful for quick tests, but they do not replace real hardware results.

### Ryujinx

Ryujinx is a Switch emulator used by this project for headless checks. Here it is run with Vulkan through [lavapipe](#lavapipe), so its graphics results are not hardware proof.

## Loading and running a homebrew program

### hbloader

hbloader is the homebrew loader used by many Switch setups. sphaira has its own loader behavior, but both are part of the path that starts an NRO.

### Loader

The loader maps an NRO into memory, jumps to its startup code, and later receives control again when the NRO exits normally. sphaira's loader is the one that exposed the locked-heap exit bug.

### Forwarder

A forwarder is a HOME-menu title that starts a homebrew loader or NRO. In this project, the forwarder path gives application memory and is the current tested launch path.

### sphaira

sphaira is the homebrew menu used for the hardware tests. Its loader rejected early NROs without an asset section and later crashed when the app exited with socket transfer memory still locked.

### userAppInit

`userAppInit` is a weak libnx hook that runs before C constructors, Rust startup, and `main`. bevy_horizon uses it to set [TPIDR_EL0](#tpidr_el0), open `log.txt`, and set the asset root.

### userAppExit

`userAppExit` is a weak libnx hook that runs when the NRO ends itself. bevy_horizon uses it to release services, wait for locked heap pages to clear, and write a final heap-check result.

### Exit lock

The exit lock is libnx `appletLockExit`. It makes HOME close ask the app to exit with an `ExitRequest` message, which `bevy_horizon`'s `check_applet_loop` turns into `AppExit`, and gives the app about 15 seconds to finish, instead of killing it immediately.

### Heap check

The heap check walks the malloc heap with `svcQueryMemory` and reports pages that the loader cannot use for its next read. It is designed to catch locked transfer memory and thread stacks before the NRO returns.

### TPIDR_EL0

`TPIDR_EL0` is the AArch64 register Rust uses as its thread-local base. libnx does not set it, so bevy_horizon copies libnx's thread pointer into it before Rust touches thread-local storage.

### Thread-local storage

Thread-local storage is data with a separate value for each thread. Rust uses it during startup and on spawned threads, so this project sets the thread pointer in `userAppInit` and every wrapped `threadCreate`.

### pthread

pthread is the POSIX thread API exposed by devkitA64 newlib and libnx. Rust threads go through it, and the Switch behavior around join and detach matters for stack cleanup.

### Condition variable

A condition variable is a thread synchronization primitive used to wait until another thread changes shared state. captest checks it with a ping-pong test because Bevy and Rust code may depend on it.

### newlib

newlib is the C library used by devkitA64. Rust `std` calls into it for files, threads, time, and process exit, so the Rust libc layouts must match devkitA64's headers.

### libnx

libnx is the Switch homebrew support library. It provides startup hooks, service access, HID, sockets, RomFS, console output, NRO support, and many Horizon wrappers.

### ABI

ABI means the binary agreement between code pieces: struct layouts, register use, symbol names, calling rules, and constants. The libc patch exists because Rust and devkitA64 disagreed on parts of the Horizon ABI.

### ELF

An ELF is the linked executable file produced before packaging. Keep it because crash addresses from an NRO run can be translated back to source with the unstripped ELF.

### Text console

The text console is libnx's simple on-screen console. captest uses it for prompts and results, and closes it before EGL takes the default window.

### NWindow

NWindow is the native Switch window object used by libnx and EGL. captest gets the default NWindow when it creates an EGL surface.

## Building

### devkitPro

devkitPro is the toolchain distribution used for Switch homebrew. The container installs its Switch packages instead of relying on a host SDK.

### devkitA64

devkitA64 is devkitPro's AArch64 compiler and C runtime toolchain for the Switch. It supplies GCC, newlib, startup support, and static libraries.

### build-std

`build-std` is Cargo's nightly feature for building Rust `core`, `alloc`, `std`, and `panic_abort` from source. This project needs it because `aarch64-nintendo-horizon` is a custom target.

### Target specification

The target specification is `aarch64-nintendo-horizon.json`. It tells Rust about AArch64, Horizon, newlib, panic aborts, TLS, the linker script, and the `threadCreate` wrapper.

### Crate patch

A crate patch replaces a crates.io crate with a local copy. The workspace patches Bevy and system crates so they can compile and behave correctly on Horizon.

### ring

`ring` is a Rust cryptography crate used by dependencies that need hashing, random keys, or TLS-related primitives. This project vendors it because upstream did not build AArch64 Horizon assembly correctly.

### perlasm

perlasm is the Perl-based assembly generator used by OpenSSL, BoringSSL, and ring. The ring patch adds an AArch64 Horizon entry using the `linux64` perlasm flavor.

### getrandom

`getrandom` is the Rust crate used to fill random bytes. In this project it is patched to use libnx `randomGet`, which uses the kernel random source, instead of opening the `csrng` service directly.

### csrng

`csrng` is Horizon's system service for random numbers. The current getrandom patch no longer opens it directly; it goes through libnx random support.

### zstd

zstd is a compression library. captest includes a native-crate check for zstd because game asset pipelines often depend on it.

### Lua

Lua is an embedded scripting language. captest can test Lua-related native crates because some target games use Lua or Lua bindings.

### bcdec

bcdec is a decoder for block-compressed texture formats. captest includes it because games may need to decode BC texture data on the Switch path.

### tokio

Tokio is an async Rust runtime. captest checks it because networking or tooling dependencies may pull in timers, threads, and async tasks.

### mio

mio is a low-level event and I/O polling crate used by async runtimes. It matters on Horizon because desktop polling backends are not automatically available.

### Loopback

Loopback is network traffic sent to the same device, usually through `127.0.0.1`. captest uses TCP and UDP loopback to test Rust socket behavior without needing the internet.

### DNS

DNS translates names such as hostnames into IP addresses. captest includes internet and address-resolution checks after basic local socket checks.

## Bevy

### Bevy

Bevy is the Rust game engine this project is bringing to Switch homebrew. bevy_horizon supplies the Switch plugin layer and crate patches needed to run Bevy apps on Horizon.

### ECS

ECS means Entity Component System, Bevy's way of storing game data and running systems over it. A Bevy ECS app can run headless before rendering is working.

### Plugin

A plugin is a Bevy unit that adds systems, resources, events, or other plugins to an app. `HorizonPlugin` adds lifecycle, input, display, and a runner when needed.

### DefaultPlugins

`DefaultPlugins` is Bevy's standard group of engine plugins. On Horizon, this project uses a reduced or configured path so Bevy can run without a desktop window backend.

### Task pool

A task pool is Bevy's worker-thread pool. On the Switch, task pools need careful core placement and a shutdown plan so worker stacks do not stay locked at exit.

### ScheduleRunnerPlugin

`ScheduleRunnerPlugin` runs Bevy schedules in a loop without a window event loop. bevy_horizon uses it on Horizon, and captest sets it to 60 Hz so frame pacing is explicit.

### Panic

A panic is Rust's runtime error path. This target uses `panic=abort`, so the panic hook writes `panic.txt` and the program exits instead of unwinding.

### cpal

cpal is Rust's cross-platform audio library and the audio layer Bevy reaches through rodio. The project patches two versions with the same Horizon host on [audren](#audren): cpal 0.17.3 for Bevy 0.19 (`patches/cpal`) and cpal 0.15.3 for Bevy 0.18 (`patches/bevy-0.18/cpal`).

### rodio

rodio is the audio playback library Bevy uses for higher-level audio. It sits above cpal and depends on cpal having a working output device.

### symphonia

symphonia is an audio decoding library. captest includes it because Bevy games may need to decode audio files before playback.

### SDL2

SDL2 is a C library that provides cross-platform services such as audio. The cpal hosts played through devkitPro's SDL2 until its Switch driver turned out to wait forever for a buffer libnx had already finished; they now feed [audren](#audren) themselves ([why not SDL2](input-and-audio.md#why-not-sdl2)). Ryujinx plays its own sound on the PC through SDL2, which is what `run.sh`'s `SDL_AUDIODRIVER` sets ([testing](testing.md#what-the-emulator-gets-wrong)).

## Graphics

### wgpu

wgpu is Bevy's graphics abstraction layer. On Horizon its only backend is [Vulkan](#vulkan): bevy_horizon's patches let it draw through NVK, which the `vulkan` feature links into the NRO ([rendering.md](rendering.md)).

### Vulkan

Vulkan is a modern graphics API. bevy_horizon draws through it: wgpu's Vulkan backend calls NVK, [Mesa](#mesa)'s Vulkan driver for NVIDIA GPUs, built from mesa-switch and linked into the NRO by the `vulkan` feature. Ryujinx headless runs also use Vulkan, through lavapipe, to draw the emulator's own output.

### OpenGL ES

OpenGL ES is the embedded version of OpenGL. Before NVK, the plan was to draw through wgpu's GL backend with EGL and OpenGL ES from [switch-mesa](#switch-mesa); Vulkan replaced that plan. captest still checks OpenGL ES directly.

### EGL

EGL is the API that creates an OpenGL ES context and connects it to a native window. captest uses it with the default [NWindow](#nwindow) for its OpenGL ES triangle check.

### GLSL

GLSL is the OpenGL shading language. captest compiles small GLSL shaders to check whether the OpenGL ES path can support needed features.

### Mesa

Mesa is an open-source graphics driver project. In this project it appears three ways: NVK, its Vulkan driver for NVIDIA GPUs, built from mesa-switch for `vulkan` builds; [switch-mesa](#switch-mesa)'s OpenGL ES driver, in captest's triangle check; and lavapipe, which draws the emulator's output on the PC's CPU.

### switch-mesa

switch-mesa is devkitPro's Mesa build for the Switch (Mesa 20.1, nouveau). It provides EGL and OpenGL ES, which captest's triangle check uses. It is not mesa-switch, the newer port (Mesa 26.2) whose NVK bevy_horizon links.

### deko3d

deko3d is a low-level homebrew graphics API for the Switch. It is useful context, but bevy_horizon draws through wgpu's Vulkan backend and NVK, not a deko3d backend.

### khronos-egl

khronos-egl is the Rust crate that binds the EGL API. The old GL plan would have used it to create an EGL context; Bevy's Vulkan path doesn't need it.

### glow

glow is a Rust OpenGL function loader used by wgpu's GL backend. The old GL plan would have loaded functions through `eglGetProcAddress`; Bevy's Vulkan path doesn't need it.

### FBO

An FBO, or framebuffer object, is an OpenGL render target. captest creates FBOs to check color and depth targets that Bevy rendering may need.

### SSBO

An SSBO, or shader storage buffer object, lets shaders read and write buffer data. captest probes it because modern renderers may need storage buffers.

### MSAA

MSAA means multisample anti-aliasing. It renders multiple samples per pixel to smooth edges, and captest checks whether the GL path can create multisampled targets.

### BC texture

A BC texture is a block-compressed texture format such as DXT1 or DXT5. captest checks BC texture support because game assets often use compressed GPU formats.

### Compute shader

A compute shader is a GPU program used for general data work instead of drawing triangles. captest probes it through OpenGL ES, and it ran correctly on hardware (findings 16). Bevy's renderer uses compute shaders too: by default it preprocesses meshes on the GPU ([rendering.md](rendering.md)).

### lavapipe

lavapipe is Mesa's CPU Vulkan driver. Ryujinx uses it in this project for headless emulator runs, but it is not the Switch GPU.

## This project

### captest

captest is `examples/switch_captest`, a capability-test NRO for this project. It writes `captest_results.txt` with one synced line per test for platform, threads, memory, files, network, native crates, Bevy behavior, input, audio, and GPU probes.

### Watchdog

The watchdog is a captest thread that ends the run if a guarded test or Bevy frame hangs too long. It is pinned to core 2 at priority `0x2B` and pauses its timing while the app is out of focus.

### Crash marker

The crash marker is `captest_running.txt`. captest writes the current risky test there before it starts and clears it afterward, so the next launch can tell which test crashed or hung.

### Skip list

The skip list is `captest_skip.txt`. If the previous run died during a test, captest adds that test to the list and skips it on the next run; deleting the file retries those tests.
