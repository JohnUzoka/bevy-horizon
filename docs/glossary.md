# Glossary

This page explains the Switch, homebrew, build, Bevy and graphics terms used across these docs. It's for Bevy developers new to Horizon and for contributors; other pages link a term here the first time they use it.

## In short

- **Terms are grouped by topic:** the Switch, loading and running a homebrew program, building, Bevy, graphics, and this project. Each entry says what a term is, why it matters to `bevy_horizon`, and which page covers it.
- **New to the Switch?** Start with [Horizon](#horizon), [Atmosphère](#atmosphère), [NRO](#nro), [sdmc](#sdmc) and [Heap](#heap).
- **Reading a crash?** See [Crash report](#crash-report), [hbloader](#hbloader), [Result code](#result-code) and [Heap check](#heap-check).
- **Working on rendering?** See [wgpu](#wgpu), [Vulkan](#vulkan), [NVK](#nvk), [NAK](#nak) and [mesa-switch](#mesa-switch). Don't confuse mesa-switch with [switch-mesa](#switch-mesa), an older Mesa that only captest's OpenGL ES checks use.

## The Switch

### Horizon

Horizon is the Nintendo Switch's operating system. Rust code built for it sees `target_os = "horizon"`, and homebrew reaches the system through [libnx](#libnx). Its rules for memory, threads and exit differ from a desktop OS's, and [platform-notes.md](platform-notes.md) lists the ones a port runs into.

### Atmosphère

Atmosphère is the custom firmware that lets a Switch run homebrew, and `bevy_horizon` is tested under it. When a program or the loader hits a fatal error, Atmosphère's [creport](#creport) writes a [crash report](#crash-report), the first thing to read when an app dies ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### Homebrew

Homebrew is software made to run on the Switch outside Nintendo's publishing path. `bevy_horizon` builds Bevy games as homebrew [NROs](#nro), which a homebrew menu such as [sphaira](#sphaira) starts ([building.md](building.md)).

### NRO

An NRO is a homebrew executable, which a [loader](#loader) reads from the SD card and runs. The build links an [ELF](#elf), then devkitPro's `elf2nro` packs it into an NRO with a [NACP](#nacp), an icon and, optionally, a [RomFS](#romfs). Each example and game builds to one NRO, which you copy to its own folder on the SD card, such as `sdmc:/switch/hello_switch/` ([building.md](building.md)).

### NACP

A NACP is a Switch program's title metadata, such as its name, author and version. devkitPro's `nacptool` makes one, and `elf2nro --nacp` puts it in the NRO's asset section next to the icon. A sphaira [forwarder](#forwarder) aborts before your code runs if the NRO has no asset section, so the build scripts always add a NACP and an icon ([building.md](building.md)).

### RomFS

RomFS is a read-only filesystem packed inside an NRO with `elf2nro --romfsdir`; `build_game.sh` adds one when you set `ROMFS`. `bevy_horizon` mounts it at startup as `romfs:/` and unmounts it at exit, and an NRO without one logs `this NRO has no RomFS, which is normal unless elf2nro was given --romfsdir`. Bevy loads assets from an `assets` folder beside the NRO by default, but your game can read files from `romfs:/` too ([porting-a-bevy-game.md](porting-a-bevy-game.md)).

### sdmc

`sdmc:` is libnx's name for the SD card's filesystem, so `sdmc:/switch/hello_switch/` is a folder on the card. `bevy_horizon` writes `log.txt`, `panic.txt` and its other logs to the app's folder there, and Bevy loads assets from beside the NRO ([logging-and-crash-reports.md](logging-and-crash-reports.md)). Rust's `Path` doesn't count `sdmc:/…` as absolute, so prefer relative asset paths ([platform-notes.md](platform-notes.md)).

### FAT32

FAT32 is the filesystem on most Switch SD cards. One file can be at most 4 GiB minus 1 byte, and names are case-insensitive. Keep each file under that limit, and check asset names on a case-sensitive system too, because a wrong-case name that loads on the Switch fails on Linux ([platform-notes.md](platform-notes.md)).

### HOME menu

The HOME menu is the Switch's system menu, opened with the HOME button. Pressing HOME while your game runs sends it to the background, and `bevy_horizon` lets Bevy react before the system suspends it ([HOME and focus](lifecycle-and-exit.md#home-and-focus)). Landing on HOME unexpectedly means the app was closed, crashed, or exited through [AM](#am), so look for a [crash report](#crash-report) and the end of `log.txt` ([troubleshooting.md](troubleshooting.md)).

### Applet

An applet is any program that Horizon's applet manager ([AM](#am)) runs. A game runs as the *Application*; the software keyboard, the album and other *library applets* run beside it with much less memory. `bevy_horizon`'s boot log says which kind your NRO got, for example `[Memory] applet type 0 = Application: running as a game (a forwarder icon or title override), so it gets a game's memory` ([platform-notes.md](platform-notes.md)).

### Applet mode

Applet mode is homebrew running as a library [applet](#applet), for example when started from the album. It gets far less memory than a game. `bevy_horizon` hasn't been measured in applet mode, so launch through a [forwarder](#forwarder) or [title override](#title-override) to get a game's memory ([platform-notes.md](platform-notes.md)).

### AM

AM is Horizon's applet manager, the service that starts, focuses, suspends and closes [applets](#applet); libnx's `applet*` functions talk to it. `bevy_horizon` handles AM's HOME, focus and exit messages each frame, and when heap pages are still locked at exit it sets libnx's `__nx_applet_exit_mode` so that AM ends the process and the Switch returns to HOME ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### Handheld mode

Handheld mode is the Switch playing on its own 1280×720 screen, at lower clocks than docked. All hardware testing so far is on a Switch Lite, which is handheld-only, so every number measured on hardware in these docs is a handheld number ([platform-notes.md](platform-notes.md)).

### Docked mode

Docked mode is the Switch in its dock, drawing to a TV at up to 1920×1080 and at higher clocks. When AM reports docked, `bevy_horizon` sets the primary window to 1920×1080, and back to 1280×720 when undocked (`monitor_dock_status` in `crates/bevy_horizon/src/display.rs`). This path is untested, because the test console is a Switch Lite, which can't dock ([platform-notes.md](platform-notes.md)).

### Focus

Focus says whether the app is the one the player is using. libnx reports it with `appletGetFocusState()`: `InFocus` (1), `OutOfFocus` (2, a library applet such as the keyboard is over the app) or `Background` (3, HOME or another program is in front). It changes only when the system sends a `FocusStateChanged` message, and whether HOME sends one depends on the [focus-handling mode](#focus-handling-mode). `bevy_horizon` passes focus changes to Bevy as `WindowFocused` and `AppLifecycle` messages, as bevy_winit does on other platforms ([HOME and focus](lifecycle-and-exit.md#home-and-focus)).

### Focus-handling mode

The focus-handling mode tells Horizon what to do with an application that loses focus. libnx sets `SuspendHomeSleep` at startup: HOME suspends the app, and the app is never told. `NoSuspend` tells the app (`Background`) and leaves it running, while `SuspendHomeSleepNotify`, despite its name, also suspends the app without a message on hardware. `bevy_horizon` sets `NoSuspend` so that it hears about HOME; once the app has run its last frame in the background, it switches to `SuspendHomeSleepNotify` so that the system suspends it, and back to `NoSuspend` when the app returns ([HOME and focus](lifecycle-and-exit.md#home-and-focus)).

### Performance mode

Performance mode is Horizon's clock profile: normal (handheld clocks) or boost (docked clocks), as `appletGetPerformanceMode` reports it. captest logs it at startup, and every number measured on the Switch Lite is at normal clocks ([testing.md](testing.md)).

### Boost mode

Boost mode is a temporary CPU clock boost that an application can request with libnx's `appletSetCpuBoostMode`; its `FastLoad` mode is meant for loading. captest's `cpu-boost` check turns it on, times the same work, and turns it off again, while `bevy_horizon` never requests it ([testing.md](testing.md)).

### Tegra X1

The Tegra X1 is the Switch's NVIDIA chip: four Arm Cortex-A57 CPU cores and a Maxwell GPU, the GM20B (shader model 5.3). wgpu reports the GPU as `NVIDIA Tegra X1 (GM20B) (NVK GM20B)`, and large 3D games are limited by it at handheld clocks ([rendering.md](rendering.md)).

### CPU core

The Switch's CPU has four cores, numbered 0 to 3. Homebrew on Atmosphère may use all four, but the system also runs on core 3, so `bevy_horizon` treats cores 0–2 as the application cores. Where your threads run is set by the [core mask](#core-mask) policy ([platform-notes.md](platform-notes.md)).

### Core mask

A core mask is the set of CPU cores a thread may run on. libnx lets std threads run on any core, but a thread created directly with `threadCreate` on the default core stays on that one core. `bevy_horizon::set_thread_core_policy` changes this for threads created afterwards (`ThreadCorePolicy::Default`, `AnyCore`, `Spread` or `Pinned`); call it before `App::new()` so that it covers Bevy's task pools ([platform-notes.md](platform-notes.md)).

### Thread priority

Thread priority decides which runnable thread Horizon runs first; a lower number means a higher priority. std threads get 0x3B and the main thread 0x2C, and Horizon never time-slices a lower-priority thread in while a higher one is runnable on its core. So a main thread that spins while waiting for a worker on the same core starves that worker ([platform-notes.md](platform-notes.md)).

### SVC

An SVC is a supervisor call, a direct request to the Horizon kernel, such as `svcQueryMemory` or `svcSetThreadCoreMask`. `bevy_horizon` uses them through libnx to check the heap at exit and to place threads on cores, and a fatal error ends in `svcBreak`, which makes Atmosphère write a [crash report](#crash-report) ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### Service

A service is an interface that a Horizon system process offers, which programs call through [IPC](#ipc): the file system, [`bsd`](#bsd) for sockets, [`audren`](#audren) for audio, [`nv`](#nv) for the GPU and [`nifm`](#nifm) for network status, among others. Some take [transfer memory](#transfer-memory) from your heap while open, so each one your game opens must be closed before exit ([rules for services you add later](lifecycle-and-exit.md#rules-for-services-you-add-later)).

### IPC

IPC is inter-process communication: how a program calls a Horizon [service](#service). The kernel checks every buffer a call passes before the service sees it, so a single locked page fails the whole call with `2001-0106`. That's how locked heap pages break the loader's read of the next NRO ([platform-notes.md](platform-notes.md)).

### Result code

A result code is Horizon's packed error value, made of a module and a description. The docs write it as module-description with 2000 added to the module, such as `2001-0106`: the kernel's `InvalidCurrentMemory`, which is how the loader fails when heap pages are still locked. Decode a crash report's `Result` before guessing at a cause ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### HID

HID is Horizon's input service for controllers, touch and motion. `bevy_horizon` reads it through libnx's [pad](#pad) functions and turns the controller state into Bevy gamepad input ([input-and-audio.md](input-and-audio.md)).

### Pad

A pad is libnx's view of a controller: the buttons and sticks of a Pro Controller, a pair of Joy-Con, a single Joy-Con or the handheld controls. `bevy_horizon` reads player 1 and the handheld controls as one pad and gives it to Bevy as a single gamepad ([input-and-audio.md](input-and-audio.md)).

### Rumble

Rumble is controller vibration. `bevy_horizon` doesn't support it yet; captest's interactive rumble test calls libnx's vibration functions directly ([input-and-audio.md](input-and-audio.md)).

### Motion sensor

A motion sensor is a controller's accelerometer and gyroscope, the six-axis sensor. `bevy_horizon` doesn't pass motion data to Bevy yet; captest's interactive motion test reads it through libnx directly ([input-and-audio.md](input-and-audio.md)).

### Touch screen

The touch screen is the Switch's built-in touch input. `bevy_horizon` doesn't pass touches to Bevy yet; captest's interactive touch test reads them through libnx directly ([input-and-audio.md](input-and-audio.md)).

### audren

`audren` is Horizon's audio renderer service, which mixes and plays sound. bevy-horizon's patched [cpal](#cpal), the audio-renderer host, feeds it through libnx's audio driver from a thread of its own ([the audio host](input-and-audio.md#the-audio-host)). It closes `audren` with `audrenExit` when the stream closes or, failing that, at exit, because until then its [transfer memory](#transfer-memory) keeps heap pages locked ([the audio close at exit](lifecycle-and-exit.md#the-audio-close-at-exit)).

### nv

`nv` is Horizon's GPU driver service, which libnx opens as nvdrv. [NVK](#nvk) sends the GPU's work through it and maps heap memory for the GPU through its `nvmap` device. On a clean exit the driver has already closed nvdrv; if it hasn't and heap pages are still held for the GPU, `nvExit` could wait forever, so `bevy_horizon` skips it and exits through [AM](#am) ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### nifm

`nifm` is Horizon's network interface service, which says whether the console is online and gives its IP address. `bevy_horizon` doesn't open it; captest's network checks use it to report the connection ([testing.md](testing.md)).

### BSD

BSD is Horizon's socket service, which libnx wraps as BSD sockets so that Rust's `std::net` works; with libnx's default buffers, about 42 UDP sockets fit before `ENOBUFS`. Opening it takes about 2.2 MiB of [transfer memory](#transfer-memory) from the heap, so `bevy_horizon` calls `socketExit` before the NRO returns to the loader ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### Transfer memory

Transfer memory is memory a program lends to a [service](#service); libnx takes it from the heap for sockets, audio and the GPU. The pages stay locked until the service is closed, and the loader can't read the next NRO into a heap with locked pages. That's why `bevy_horizon` closes its services and runs the [heap check](#heap-check) at exit ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### Heap

The heap is the large memory area that Rust and C allocations come from; launched with a game's memory, it is about 3.1 GiB of malloc heap. NVK takes GPU memory from the top of the same heap, and the loader reuses the heap for the next NRO, so pages still locked at exit make the loader crash. Plan your game's memory around this one heap ([GPU memory comes from the same heap](platform-notes.md#gpu-memory-comes-from-the-same-heap)).

### creport

`creport` is the Atmosphère program that writes [crash reports](#crash-report). After a fatal error or exception it records the result code, the registers, the loaded modules and each thread's state, which is how an NRO crash leaves evidence after the app is gone ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### Crash report

A crash report is a file that [creport](#creport) writes under `sdmc:/atmosphere/crash_reports/` when a program dies of a fatal error; a Rust [panic](#panic) doesn't make one. Read the module list first: if it names only `hbl`, the crash is in the [loader](#loader), not your app. Then decode the `Result` and turn addresses into source lines with the [ELF](#elf) ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### Gameplay recording

Gameplay recording is the Switch's capture of video clips during play. A title whose [NACP](#nacp) asks for it, as the tested sphaira [forwarder](#forwarder)'s does, gets 96 MiB less process memory. The measured 3,189.0 MiB process total already has that 96 MiB taken out ([platform-notes.md](platform-notes.md)).

### Emulator

An emulator is a PC program that imitates the Switch. It's handy for quick checks, but it runs on the PC's hardware, so its speed says nothing about the console; only [Ryujinx](#ryujinx) has been tried, and other emulators are untested ([testing.md](testing.md)).

### Ryujinx

Ryujinx is an open-source Switch emulator, now maintained as [Ryubing](#ryubing). Stock Ryujinx can't run `bevy_horizon`'s Vulkan builds, because NVK hits emulator bugs at startup and on the first frames; the project tests them in a Ryujinx with unpublished fixes, and the non-Vulkan captest runs in the stock one. In these runs it draws through [lavapipe](#lavapipe) on the CPU, so treat hardware as the answer ([what the emulator gets wrong](rendering.md#what-the-emulator-gets-wrong)).

### Ryubing

Ryubing is the community-maintained fork of [Ryujinx](#ryujinx) that carries on its development. "Stock Ryujinx" in these docs means Ryubing 1.3.3, which runs the non-Vulkan captest but not the Vulkan builds ([testing.md](testing.md)).

## Loading and running a homebrew program

### hbloader

hbloader is the homebrew loader: it reads an NRO into its process's heap, jumps to it, and when the NRO exits, loads the next one, usually the homebrew menu. nx-hbloader is the standard one and sphaira brings its own; both show up as `hbl` in a crash report's module list. A report that names only `hbl` means the loader crashed, typically because the last NRO left heap pages locked (`2001-0106`) or the NRO has no asset section ([troubleshooting.md](troubleshooting.md)).

### Loader

The loader reads an NRO into memory, jumps to its startup code, and gets control back when the NRO exits normally, ready to read the next one into the same heap. sphaira's loader reads the next NRO with a single file read into the whole heap, which the kernel rejects with `2001-0106` if any page of it is still locked. That's why `bevy_horizon` releases every service and checks the heap before returning ([the userAppExit hook](lifecycle-and-exit.md#the-userappexit-hook)).

### Forwarder

A forwarder is a HOME-menu icon that starts one NRO through a homebrew loader, so the NRO runs as a game with a game's memory. sphaira can install forwarders, and launching from one is a tested path. A sphaira forwarder aborts before your code runs if the NRO has no asset section (icon and [NACP](#nacp)), so the build scripts always add one ([building.md](building.md)).

### Title override

Title override is Atmosphère's way to start the homebrew menu in place of a game: hold R while you start any installed game from HOME. The homebrew menu and the NRO you pick then run in that game's process with a game's memory, which `bevy_horizon` logs as `[Memory] applet type 0 = Application` ([platform-notes.md](platform-notes.md)).

### sphaira

sphaira is a homebrew menu for the Switch with its own loader, and it can install [forwarders](#forwarder). `bevy_horizon` is tested with apps started from sphaira's menu or a sphaira forwarder. Its loader reads the next NRO into the whole heap in one go, so it crashes with `2001-0106` if your app leaves heap pages locked ([troubleshooting.md](troubleshooting.md)).

### crt0

crt0 is libnx's startup code at the very beginning of an NRO: it sets up the process, then runs libnx's initialisation, the [userAppInit](#userappinit) hook, C constructors and `main`. The [target specification](#target-specification)'s linker script puts it first in `.text`, because the loader starts execution at the beginning of the image ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### userAppInit

`userAppInit` is a weak libnx hook that runs once libnx has opened its default services and the SD card, before C constructors, Rust's runtime and `main`. `bevy_horizon` defines it to set [TPIDR_EL0](#tpidr_el0) before any Rust thread-local is touched, open `log.txt` and set Bevy's asset root, so even a crash during startup leaves a log ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### userAppExit

`userAppExit` is a weak libnx hook that runs when the NRO ends itself: `main` returns, `std::process::exit` is called, or the app aborts. Closing the app from HOME doesn't run it, because the system ends the whole process. `bevy_horizon` uses it to stop Bevy's task pools, release its services, wait for locked heap pages to clear and log the [heap check](#heap-check) ([the userAppExit hook](lifecycle-and-exit.md#the-userappexit-hook)).

### Exit lock

The exit lock is libnx's `appletLockExit`. With it, closing the app from HOME sends an `ExitRequest` message and gives the app about 15 seconds to finish, instead of ending the process at once. `bevy_horizon` doesn't take the lock (captest does), but if your game takes it, `bevy_horizon` turns `ExitRequest` into `AppExit::Success` ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### Heap check

The heap check walks the malloc heap with `svcQueryMemory` at exit, looking for pages that would make the loader's read of the next NRO fail: [transfer memory](#transfer-memory) still lent to a service, or a thread stack still mapped. It logs `[Exit] PASS: nothing in the heap is locked, so the loader can read the next NRO into it`; otherwise it logs `[Exit] WARN: heap pages are still locked, …`, and `bevy_horizon` exits through [AM](#am) to HOME instead of returning to the loader ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### TPIDR_EL0

`TPIDR_EL0` is the AArch64 register where Rust's code finds the current thread's [thread-local storage](#thread-local-storage). libnx never sets it, so the first thread-local access would crash. `bevy_horizon` points it at libnx's TLS block in `userAppInit` and on every new thread ([platform-notes.md](platform-notes.md)).

### Thread-local storage

Thread-local storage (TLS) is data with a separate copy for each thread, such as Rust's `thread_local!` values. Rust's std uses it from startup on, so `bevy_horizon` sets [TPIDR_EL0](#tpidr_el0) in `userAppInit` and on every thread created through its `threadCreate` wrapper, and the [target specification](#target-specification) makes rust-lld and libnx agree on the TLS layout ([toolchain.md](toolchain.md)).

### pthread

pthread is the POSIX thread API, which devkitA64's newlib provides on top of libnx threads; Rust's `std::thread` uses it. libnx v4.12 has no back end for `pthread_detach`, so a finished detached thread would keep its 2 MiB stack and leave the heap pages under it locked; `bevy_horizon` supplies the back end and closes such threads once they've exited ([platform-notes.md](platform-notes.md)).

### Condition variable

A condition variable lets a thread sleep until another thread changes shared state and wakes it, as Rust's `std::sync::Condvar` does. captest's `thread-condvar` check passes a value back and forth between two threads to show that it works on the Switch ([testing.md](testing.md)).

### newlib

newlib is the C library in devkitA64, with libnx providing its system calls. Rust's std calls into it for files, threads, time and exit, so the Rust `libc` crate's types and constants must match devkitA64's headers; bevy-horizon patches `libc` where they didn't ([the crate patches](toolchain.md#the-crate-patches)).

### libnx

libnx is the C library that Switch homebrew is built on: startup code ([crt0](#crt0)), wrappers for Horizon's services, threads, sockets, RomFS and the text console. `bevy_horizon` calls it for everything below Bevy, and the toolchain image installs devkitPro's build of it ([toolchain.md](toolchain.md)).

### ABI

ABI means the binary agreement between pieces of compiled code: struct layouts, type sizes, constants, symbol names and calling rules. Rust's `libc` crate disagreed with devkitA64's headers on parts of the Horizon ABI, which is why bevy-horizon patches it ([platform-notes.md](platform-notes.md)).

### ELF

An ELF is the linked executable that the build makes before `elf2nro` packs it into an [NRO](#nro). Keep the unstripped ELF of every NRO you run: it turns a crash report's addresses back into functions and source lines ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### Text console

The text console is libnx's simple on-screen text output. captest shows its prompts and results on it, closes it so that [EGL](#egl) can take the default [NWindow](#nwindow) for the GPU checks, and opens it again afterwards ([testing.md](testing.md)).

### NWindow

NWindow is libnx's native window, the surface the display shows; `nwindowGetDefault()` returns the app's one. `bevy_horizon` hands it to wgpu as a `RawWindowHandle::Horizon` handle through a patched raw-window-handle, and [NVK](#nvk) presents each frame into it; captest's OpenGL ES checks draw into it through [EGL](#egl) ([rendering.md](rendering.md)).

## Building

### devkitPro

devkitPro is the organisation and package set behind the homebrew toolchains, including [devkitA64](#devkita64), [libnx](#libnx), `switch-tools` (`elf2nro`, `nacptool`) and portlibs such as [switch-mesa](#switch-mesa). bevy-horizon's toolchain image (`Containerfile`) starts from devkitPro's `devkita64` image and installs these, so you don't need devkitPro on your own machine ([building.md](building.md)).

### devkitA64

devkitA64 is devkitPro's AArch64 toolchain for the Switch: GCC, binutils and [newlib](#newlib). bevy-horizon builds against its C library and headers while rustc links with `rust-lld`, and NVK is built with the same devkitA64 and libnx so that the C side matches ([toolchain.md](toolchain.md)).

### build-std

`build-std` is Cargo's nightly feature for building Rust's `core`, `alloc`, `std` and `panic_abort` from source. bevy-horizon needs it because `aarch64-nintendo-horizon` is a custom target with no prebuilt standard library, which is why the toolchain image pins a nightly Rust with `rust-src` ([toolchain.md](toolchain.md)).

### Target specification

The target specification is `aarch64-nintendo-horizon.json` at the repository root, a custom Rust target. It tells rustc the OS (`horizon`), the C library (newlib), `panic=abort`, the thread-local model, the linker (`rust-lld`) and its script, and the `--wrap` options that route `threadCreate` and some file calls to `bevy_horizon` ([toolchain.md](toolchain.md)).

### Crate patch

A crate patch replaces a crates.io crate with a modified copy, through Cargo's `[patch.crates-io]`. bevy-horizon keeps its copies in `patches/`, for Bevy, wgpu, cpal and system crates, and your game must use the set that matches its Bevy version ([the crate patches](toolchain.md#the-crate-patches), [how to check that a patch is really applied](toolchain.md#how-to-check-that-a-patch-is-really-applied)).

### ring

`ring` is a Rust cryptography crate that TLS libraries such as rustls can use. Its build script built no assembly for AArch64 Horizon, so linking failed; bevy-horizon's patch builds its `linux64` assembly instead, which works because Switch binaries are ELF ([the crate patches](toolchain.md#the-crate-patches)).

### perlasm

perlasm is the Perl-based assembly generator that OpenSSL, BoringSSL and ring use; each flavour targets one platform's assembler. The [ring](#ring) patch picks the `linux64` flavour for AArch64 Horizon ([the crate patches](toolchain.md#the-crate-patches)).

### getrandom

`getrandom` is the Rust crate that fills buffers with random bytes from the OS; `rand` and crypto crates depend on it. bevy-horizon patches both versions in use, 0.2 (which ring uses) and 0.3 or later, and both end in libnx's `randomGet` through C functions that `bevy_horizon` defines ([the crate patches](toolchain.md#the-crate-patches)).

### csrng

`csrng` is Horizon's random-number service. Nothing in bevy-horizon opens it, because random bytes come from libnx's `randomGet` instead, so there's no extra service to close at exit ([the crate patches](toolchain.md#the-crate-patches)).

### zstd

zstd is a fast compression format. Bevy's `zstd_rust` feature uses it to unpack KTX2 textures, including the tonemapping lookup table that hello_switch's lit cube needs ([rendering.md](rendering.md)), and captest's `crate-zstd` check runs the C library through the `zstd` crate ([testing.md](testing.md)).

### Lua

Lua is a small scripting language that games embed. captest's `crate-mlua` checks run Lua 5.4 through the `mlua` crate, built from source with bevy-horizon's `lua-src` patch ([testing.md](testing.md)).

### bcdec

bcdec is a small decoder for block-compressed ([BC](#bc-texture)) texture formats; captest uses its Rust port, `bcdec_rs`. captest's `crate-bcdec` checks decode BC1 on the CPU and time it ([testing.md](testing.md)).

### tokio

Tokio is a widely used async runtime for Rust, with tasks, timers, channels and a blocking-thread pool. captest's `crate-tokio` checks run its multi-threaded runtime on the Switch, because networking crates often pull it in ([testing.md](testing.md)).

### mio

mio is the low-level I/O event library under tokio's networking. Horizon has no epoll or kqueue, so check that any crate needing mio's networking works on the Switch before you rely on it ([porting-a-bevy-game.md](porting-a-bevy-game.md)).

### Loopback

Loopback is network traffic that a device sends to itself, usually through `127.0.0.1`. captest's `net-tcp-loopback` and `net-udp-loopback` checks use it to test Rust's sockets without a network ([testing.md](testing.md)).

### DNS

DNS turns host names into IP addresses. captest's `net-dns` checks resolve `localhost` and, when the console is online, `example.com` ([testing.md](testing.md)).

## Bevy

### Bevy

Bevy is a data-driven game engine written in Rust. bevy-horizon supports Bevy 0.19 and 0.18: the `bevy_horizon` crate adds the Switch plugins, and crate patches let Bevy and its dependencies build and run on Horizon ([match the patch set to your Bevy version](porting-a-bevy-game.md#4-match-the-patch-set-to-your-bevy-version-and-build)).

### ECS

ECS means Entity Component System, Bevy's way of storing game data as components on entities and running systems over them. `bevy_horizon` works through it too: its plugins add systems that read input, follow HOME and focus, and log the [heartbeat](#heartbeat) ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### Plugin

A plugin is a Bevy unit that adds systems, resources or other plugins to an app. `HorizonPlugin` is the one your game adds: it brings the lifecycle, input, display and window plugins, upload pacing with the `render` feature, and a 60 Hz [ScheduleRunnerPlugin](#schedulerunnerplugin) if the app has no runner ([porting-a-bevy-game.md](porting-a-bevy-game.md)).

### DefaultPlugins

`DefaultPlugins` is Bevy's standard group of engine plugins, chosen by Cargo features. On the Switch you leave out `bevy_winit` and `bevy_gilrs`, because [winit](#winit) has no Switch backend and `HorizonPlugin` supplies the window, run loop and controller input instead ([porting-a-bevy-game.md](porting-a-bevy-game.md)).

### winit

winit is the Rust windowing and event-loop library that Bevy's `bevy_winit` uses on desktop and mobile. It has no Switch backend, so `HorizonPlugin` stands in for it: the window, the run loop and the lifecycle messages winit would send ([porting-a-bevy-game.md](porting-a-bevy-game.md)).

### Task pool

A task pool is one of Bevy's worker-thread pools (compute, async compute and I/O). Bevy never joins their threads, whose stacks lock heap pages, so at exit `bevy_horizon` stops the pools and joins their threads, logging `[Exit] Bevy's task pools: stopped and joined …`; where those threads run is set by the [core mask](#core-mask) policy ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### ScheduleRunnerPlugin

`ScheduleRunnerPlugin` is Bevy's runner for apps without a window event loop: it runs the schedules once, or in a loop at a set rate. `DefaultPlugins` adds it only when `bevy_window` is off, and without a runner Bevy runs one frame and the NRO exits, so `HorizonPlugin` adds it at 60 Hz when your app has none ([lifecycle-and-exit.md](lifecycle-and-exit.md)).

### Panic

A panic is Rust's way of stopping on a bug or an unrecoverable error. The target uses `panic=abort`, so nothing unwinds: `bevy_horizon`'s panic hook writes the message, location, thread and memory figures to `log.txt` and `panic.txt`, and the app exits without a [crash report](#crash-report) ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### cpal

cpal is Rust's cross-platform audio output library, which Bevy reaches through [rodio](#rodio). bevy-horizon patches two versions with the same Horizon host, which feeds [audren](#audren) directly: cpal 0.17.3 for Bevy 0.19 (`patches/cpal`) and cpal 0.15.3 for Bevy 0.18 (`patches/bevy-0.18/cpal`). The 0.18 host is proven on hardware in a full 3D game; the 0.19 host has passed only in the emulator so far ([the audio host](input-and-audio.md#the-audio-host)).

### rodio

rodio is the audio playback library that `bevy_audio` uses to decode and mix sounds; it plays them through a [cpal](#cpal) output stream. It works on the Switch because the patched cpal gives it a working output device ([input-and-audio.md](input-and-audio.md)).

### symphonia

symphonia is a pure-Rust audio decoding library (MP3, WAV, ADPCM and more) that rodio can use. captest's `crate-symphonia` checks decode audio with it on the Switch ([testing.md](testing.md)).

### SDL2

SDL2 is a C library for cross-platform audio, input and windows, and devkitPro ships a Switch port. The cpal hosts first played through it, until its Switch audio driver turned out to wait forever for a buffer that libnx had already finished. They now feed [audren](#audren) directly, and `bevy_horizon` no longer needs SDL2 ([why not SDL2](input-and-audio.md#why-not-sdl2)).

## Graphics

### wgpu

wgpu is the Rust graphics library Bevy renders through; it maps one API onto Vulkan, Metal, DirectX 12 or OpenGL. On Horizon its only backend is [Vulkan](#vulkan), driven by [NVK](#nvk), which the `vulkan` feature links into the NRO. bevy-horizon patches wgpu, wgpu-core and wgpu-hal so that they build and run on Horizon ([rendering.md](rendering.md)).

### naga

naga is wgpu's shader translator: it turns Bevy's WGSL shaders into SPIR-V for Vulkan, which [NAK](#nak) then compiles for the GPU. Its index clamping is how the default [buffer robustness](#buffer-robustness) mode, `clamp`, keeps shaders' buffer accesses in range ([buffer robustness](rendering.md#buffer-robustness)).

### Vulkan

Vulkan is a low-level, cross-platform graphics API from Khronos. Bevy on the Switch draws through it: wgpu's Vulkan backend calls [NVK](#nvk), which the `vulkan` feature links into the NRO, so there's no driver to install ([linking NVK](toolchain.md#linking-nvk-the-vulkan-feature)).

### NVK

NVK is [Mesa](#mesa)'s open-source Vulkan driver for NVIDIA GPUs. `scripts/build_nvk.sh` builds it for the Switch's GM20B from [mesa-switch](#mesa-switch), as a static `nvk/lib/libvulkan.a` that the `vulkan` feature links into the NRO. Every frame your game draws goes through it, and it takes GPU memory from the top of the [heap](#heap) ([linking NVK](toolchain.md#linking-nvk-the-vulkan-feature), [NVK takes GPU memory from the top of the heap](platform-notes.md#nvk-takes-gpu-memory-from-the-top-of-the-heap)).

### NAK

NAK is NVK's shader compiler, written in Rust. It compiles the SPIR-V that [naga](#naga) produces into GM20B machine code the first time a shader is needed, which is why the first launch is slower ([shader cache](#shader-cache)). Switch builds turn some `NAK_DEBUG` optimisations on by default, and `NAK_DEBUG=stats` logs each shader's size and register use ([logging-and-crash-reports.md](logging-and-crash-reports.md)).

### Shader cache

The shader cache is where NVK keeps compiled shaders between launches, under `sdmc:/.mesa/mesa_shader_cache_sf/`, in a folder per driver build that every app with that build shares. The first launch compiles each shader it meets and later launches load them, so judge start-up time and hitches from a second launch; `MESA_SHADER_CACHE_DISABLE=true` turns the cache off ([rendering.md](rendering.md)).

### OpenGL ES

OpenGL ES is the embedded-systems version of OpenGL. Bevy on the Switch doesn't use it; it draws through [Vulkan](#vulkan). Only captest's GPU checks use OpenGL ES, through [switch-mesa](#switch-mesa) and [EGL](#egl), as a test of the GPU that doesn't involve wgpu or NVK ([testing.md](testing.md)).

### EGL

EGL is the API that creates an [OpenGL ES](#opengl-es) context and connects it to a native window. captest's GPU checks use switch-mesa's EGL on the default [NWindow](#nwindow); the Vulkan path doesn't use EGL ([testing.md](testing.md)).

### GLSL

GLSL is OpenGL's shading language. captest's GPU checks compile small GLSL ES shaders through switch-mesa; Bevy's own shaders are WGSL, which [naga](#naga) translates for Vulkan ([testing.md](testing.md)).

### Mesa

Mesa is the open-source project behind many Linux graphics drivers. It appears here three ways: [NVK](#nvk), its Vulkan driver for NVIDIA GPUs, built from [mesa-switch](#mesa-switch) and linked into every `vulkan` build; [switch-mesa](#switch-mesa)'s OpenGL ES driver, used only by captest's GPU checks; and [lavapipe](#lavapipe), which draws the emulator's output on the PC's CPU ([rendering.md](rendering.md)).

### switch-mesa

switch-mesa is devkitPro's package of an older Mesa (20.1, with the nouveau OpenGL driver) for the Switch, which the toolchain image installs. It provides the EGL and OpenGL ES that captest's GPU checks use. Don't confuse it with [mesa-switch](#mesa-switch), the newer port whose NVK `bevy_horizon` links ([toolchain.md](toolchain.md)).

### mesa-switch

mesa-switch is danfromtico and NaGaa95's port of Mesa 26.2 to the Switch, with NVK and NAK for the GM20B. `scripts/build_nvk.sh` builds NVK from it after applying bevy-horizon's patches in `patches/mesa-switch/` (top-down GPU memory, ZCULL and other Switch tuning, NAK optimisations), and records the commit and patches in `nvk/BUILD_INFO` ([toolchain.md](toolchain.md)).

### NVN

NVN is Nintendo's own graphics API, which official Switch games use. [Ryujinx](#ryujinx) assumes NVN's layouts in places, such as where a shader finds a storage buffer's address and size, which is one reason NVK hits emulator bugs ([what the emulator gets wrong](rendering.md#what-the-emulator-gets-wrong)).

### deko3d

deko3d is a low-level homebrew graphics API made for the Switch's GPU. wgpu has no deko3d backend, so `bevy_horizon` draws through [Vulkan](#vulkan) and [NVK](#nvk) instead ([rendering.md](rendering.md)).

### khronos-egl

khronos-egl is a Rust crate that binds the EGL API, used by wgpu's OpenGL backend. Bevy on the Switch draws through Vulkan, so neither the crate nor that backend is used ([rendering.md](rendering.md)).

### glow

glow is the Rust OpenGL bindings crate that wgpu's OpenGL backend uses. Bevy on the Switch draws through Vulkan, so glow isn't used ([rendering.md](rendering.md)).

### FBO

An FBO, or framebuffer object, is an OpenGL render target. captest's `gpu-target-*` checks render through switch-mesa into FBOs in the color and depth formats Bevy uses, such as RGBA16F and Depth32F ([testing.md](testing.md)).

### SSBO

An SSBO, or shader storage buffer object, is a buffer that shaders can read and write; Vulkan and WGSL call it a storage buffer. Bevy's renderer relies on storage buffers, for example for GPU mesh preprocessing, and captest's `gpu-compute` check writes one through OpenGL ES ([testing.md](testing.md)).

### MSAA

MSAA means multisample anti-aliasing: the GPU keeps several samples per pixel and averages them to smooth edges. Bevy's cameras use MSAA ×4 by default, which costs GPU time, and [render_check](#render_check) has tiles with and without it to tell a multisampling problem from others ([rendering.md](rendering.md)).

### BC texture

A BC texture uses a block-compressed GPU format such as BC1 (DXT1) or BC3 (DXT5), which saves memory and bandwidth. captest's `gpu-bc*` checks sample BC1 and BC3 textures through OpenGL ES, and its `crate-bcdec` checks decode BC1 on the CPU ([testing.md](testing.md)).

### Compute shader

A compute shader is a GPU program for general data work instead of drawing triangles. Bevy's renderer uses them by default to preprocess and cull meshes on the GPU, and wgpu runs one to check [indirect draws](#indirect-draw) in debug builds ([rendering.md](rendering.md)). captest's `gpu-compute` check runs one through OpenGL ES ([testing.md](testing.md)).

### Indirect draw

An indirect draw takes its parameters, such as vertex and instance counts, from a GPU buffer instead of from the CPU, so a compute shader can decide what's drawn. Bevy draws its batches indirectly by default. wgpu checks the arguments with an extra compute pass, which bevy-horizon's wgpu-core patch turns off on Horizon in release builds ([indirect-call validation](rendering.md#indirect-call-validation)).

### Buffer robustness

Buffer robustness stops a shader's out-of-range buffer accesses from reaching other memory. By default (`BH_ROBUST_BUFFER_ACCESS` unset or `clamp`) the driver's robustness is off and [naga](#naga) clamps indices in every shader instead, because on the GM20B each driver check costs a branch; `driver` (or `1`) uses the driver's checks, and `off` (or `0`) uses neither ([buffer robustness](rendering.md#buffer-robustness)).

### Render target compression

Render target compression lets the Switch's Maxwell GPU keep a render target compressed, which cuts the memory traffic of clears, depth tests and blending. NVK compresses an image only when its memory was allocated for it alone, which wgpu doesn't do. `BH_RT_COMPRESSION=1` gives large render targets memory of their own so that NVK can compress them; it's off by default ([render-target compression](rendering.md#render-target-compression)).

### ZCULL

ZCULL is the NVIDIA GPU's coarse depth test, which rejects hidden pixels in blocks before they're shaded. It works best set up for the direction the depth test compares in, so `bevy_horizon` sets it to `GREATER` for Bevy's reversed depth, unless `BEVY_HORIZON_ZCULL_DIR=less` asks for NVK's own `LESS` ([rendering.md](rendering.md)).

### Device lost

Device lost is wgpu's error when the GPU can no longer be used, after a hang or a fault; from then on every GPU call fails. In the log a GPU hang shows as `channel N lost: notification={type=8 (fifo idle timeout) …}`, followed by a wgpu panic, for example in `Surface::present`. One Bevy 0.18 scene is known to cause this, for a reason not yet found ([rendering.md](rendering.md)).

### lavapipe

lavapipe is Mesa's Vulkan driver that runs on the CPU. [Ryujinx](#ryujinx) draws through it in the project's emulator runs, so emulator speed says nothing about the console ([testing.md](testing.md)).

## This project

### hello_switch

hello_switch (`examples/hello_switch`) is the first example to build: a lit, rotating cube, plus `[Check]` lines in `log.txt` for assets, files, threads and RomFS. It holds 60 fps on the console, and its `Cargo.toml` shows the Bevy features a 3D game needs on the Switch ([building.md](building.md)).

### Breakout

Breakout (`examples/breakout`) is Bevy 0.19.1's Breakout example adapted to the Switch: the D-pad or stick moves the paddle, the game logs `[Game]` lines, and there's no collision sound. It's a small, complete 2D game that plays to the end at about 59 fps on the console ([building.md](building.md)).

### render_check

render_check (`examples/render_check`) is a rendering test: six cameras draw the same four cubes, each camera changing one thing about Bevy's 3D path (indirect draws, MSAA, HDR, projection). It checks every cube on a screenshot, `render_check_N.png` beside `log.txt`, and draws all 24 at about 20 fps, since it's a stress test. Its results and `options.txt` switches help narrow down which part of Bevy's renderer a drawing problem is in ([rendering.md](rendering.md)).

### captest

captest (`examples/switch_captest`) is the capability-test NRO: it checks what the Switch gives Rust and Bevy, from threads, memory, files and sockets to native crates, Bevy's app and audio, controller input and OpenGL ES. It writes one synced line per test to `captest_results.txt`; on the console it passes 179 tests, fails none and warns on 2 ([testing.md](testing.md)).

### Watchdog

The watchdog is a captest thread that ends the app if a guarded test or a Bevy frame hangs too long: it writes a `HANG` line to `captest_results.txt` and ends the process with `svcExitProcess`, so you land on HOME. It runs on core 2 at priority 0x2B and doesn't count time the app spends out of [focus](#focus) ([testing.md](testing.md)).

### Crash marker

The crash marker is `captest_running.txt`, which names the test in progress. If the app dies, the next launch finds the name there and adds the test to the [skip list](#skip-list) ([testing.md](testing.md)).

### Skip list

The skip list is `captest_skip.txt`: tests that crashed or hung on an earlier launch, which captest skips so that one crash can't block the rest. Delete the file to run them again ([testing.md](testing.md)).

### Heartbeat

The heartbeat is the status `bevy_horizon` writes to `log.txt` every 10 seconds: `[Heartbeat] frame N, … fps on average` and how long frames' work took, plus a `[Memory]` line once a minute. `BEVY_HORIZON_MEMORY_EVERY` sets the memory line's interval in seconds (`0` turns it off, because reading malloc's figures stalls that frame), and the heartbeat is the quickest way to see a game's frame rate on the console ([heartbeat lines](logging-and-crash-reports.md#heartbeat-lines)).
