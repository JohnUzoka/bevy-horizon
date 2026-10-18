//! SD card logging, stdout/stderr redirection, and panic reporting for Horizon.
//!
//! `log.txt` is opened before `main`, by libnx's `userAppInit` hook (`compat.rs`), so a crash
//! in Rust's runtime setup still leaves a log. `<app>` is then the NRO file name without
//! `.nro`; `main`'s `init_switch_logging(app)` only opens the log itself if that didn't happen.
//!
//! Files go to `sdmc:/switch/<app>/`, or the SD card root if that directory can't be used:
//!
//! | file        | written by                                     | mode     |
//! |-------------|------------------------------------------------|----------|
//! | `log.txt`   | stdout + stderr: `println!`, Bevy/tracing logs | each run |
//! | `panic.txt` | the panic hook installed here                  | appended |
//! | `crash.log` | `__libnx_exception_handler` in `compat.rs`     | appended |
//! | `abort.log` | `diagAbortWithResult` in `compat.rs`           | appended |
//!
//! Panic, crash, and abort reports are copied into `log.txt` first. That file is already open,
//! so writing to it needs no allocation, which matters when the heap is what broke.
//!
//! Writes to `log.txt` from different threads take turns (`lock_log_writes`), so lines can't
//! overwrite each other.
//!
//! Once `init_switch_logging` has run, a log writer thread does the SD card writes: other threads
//! copy their text into a queue and go on, and it writes it out in order and syncs once a second
//! (`start_log_writer`). Panic, crash, abort and exit reports are still written on their own
//! thread, after what is queued, before they return (`write_log`).
//!
//! The same early hook also sets `BEVY_ASSET_ROOT` (see `set_asset_root`): it has to happen
//! before `DefaultPlugins` is added, and it needs the NRO path that the log name comes from.

use std::panic::PanicHookInfo;
use std::sync::Once;

#[cfg(target_os = "horizon")]
use std::ffi::{CStr, CString};
#[cfg(target_os = "horizon")]
use std::sync::OnceLock;
#[cfg(target_os = "horizon")]
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};

static LOGGING_INIT: Once = Once::new();
#[cfg(target_os = "horizon")]
static LOG_FD: AtomicI32 = AtomicI32::new(-1);
#[cfg(target_os = "horizon")]
static LOG_DIR: OnceLock<String> = OnceLock::new();

#[cfg(target_os = "horizon")]
pub(crate) mod ffi {
    use core::ffi::{c_char, c_int, c_void};

    // newlib <sys/_default_fcntl.h>
    pub const O_WRONLY: c_int = 0x1;
    pub const O_APPEND: c_int = 0x8;
    pub const O_CREAT: c_int = 0x200;
    pub const O_TRUNC: c_int = 0x400;

    /// libnx `MemoryInfo` (switch/kernel/svc.h).
    #[repr(C)]
    #[derive(Default)]
    pub struct MemoryInfo {
        pub addr: u64,
        pub size: u64,
        pub kind: u32,
        pub attr: u32,
        pub perm: u32,
        pub ipc_refcount: u32,
        pub device_refcount: u32,
        pub padding: u32,
    }

    unsafe extern "C" {
        /// libnx crt0 entry point. It is the first byte of the NRO, so its address is the load base.
        pub fn _start();
        /// End of the loaded image (data + bss), defined by the linker script.
        pub static __end__: u8;
        /// Start of the thread-local template (`.tdata`), defined by the linker script.
        pub static __tdata_lma: u8;
        /// Alignment of the thread-local block, stored by the linker script.
        pub static __tls_align: usize;
        /// libnx's parsed launch arguments; `argv[0]` is the NRO path.
        pub static __system_argc: c_int;
        pub static __system_argv: *const *const c_char;

        pub fn fsdevMountSdmc() -> u32;
        pub fn consoleDebugInit(device: c_int);
        pub fn svcOutputDebugString(str: *const c_char, size: usize) -> u32;
        pub fn svcQueryMemory(info: *mut MemoryInfo, page_info: *mut u32, addr: u64) -> u32;
        pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        pub fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
        pub fn close(fd: c_int) -> c_int;
        pub fn dup2(oldfd: c_int, newfd: c_int) -> c_int;
        pub fn fsync(fd: c_int) -> c_int;
        pub fn mkdir(path: *const c_char, mode: c_int) -> c_int;

        /// newlib `sys/iosupport.h`: the device table, indexed by `NewlibHandle::device`.
        pub static devoptab_list: [*const usize; STD_MAX];
        pub fn AddDevice(device: *const usize) -> c_int;
        pub fn __get_handle(fd: c_int) -> *mut NewlibHandle;

        /// libnx `Mutex` (switch/kernel/mutex.h): a `u32`, 0 when unlocked.
        pub fn mutexLock(mutex: *mut u32);
        pub fn mutexUnlock(mutex: *mut u32);
        pub fn mutexIsLockedByCurrentThread(mutex: *const u32) -> bool;
        /// libnx `CondVar` (switch/kernel/condvar.h): a `u32`. Releases `mutex` while it waits
        /// and holds it again on return; 0xEA01 means the time ran out.
        pub fn condvarWaitTimeout(condvar: *mut u32, mutex: *mut u32, timeout_ns: u64) -> u32;
        /// What libnx's inline `condvarWake` calls: wakes up to `count` waiters.
        pub fn svcSignalProcessWideKey(key: *mut u32, count: i32);
    }

    /// newlib `sys/iosupport.h`: `STD_MAX`, the size of `devoptab_list`.
    pub const STD_MAX: usize = 35;
    /// newlib `devoptab_t`: 32 pointer-sized fields; the name is the first, `write_r` the fifth.
    pub const DEVOPTAB_WORDS: usize = 32;
    pub const DEVOPTAB_NAME: usize = 0;
    pub const DEVOPTAB_WRITE_R: usize = 4;
    pub type WriteR = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char, usize) -> isize;

    /// newlib `__handle`: what an fd refers to. `dup2` makes two fds share one.
    #[repr(C)]
    pub struct NewlibHandle {
        pub device: u32,
        pub refcount: u32,
        pub file_struct: *mut c_void,
    }
}

/// Starts SD card logging and installs the panic hook. Call it first thing in `main`, before
/// `App::new()`.
///
/// On an NRO, `userAppInit` has normally opened `log.txt` and set `BEVY_ASSET_ROOT` before
/// `main` (see `start_boot_log`), so this just notes that `main` is running. Otherwise it opens
/// `sdmc:/switch/{app_name}/log.txt` (falling back to `sdmc:/log.txt`), points stdout and
/// stderr at it with `dup2`, writes a header recording how each step went, and sets
/// `BEVY_ASSET_ROOT`.
pub fn init_switch_logging(app_name: &str) {
    LOGGING_INIT.call_once(|| {
        #[cfg(target_os = "horizon")]
        {
            // Bevy's tracing layer honours NO_COLOR; ANSI colour codes are noise in a text file.
            unsafe { std::env::set_var("NO_COLOR", "1") };
            if LOG_FD.load(Ordering::SeqCst) < 0 {
                open_log_file(app_name, "init_switch_logging in main (no log was started before main)");
            }
            let line = format!(
                "[Main] init_switch_logging(\"{app_name}\"): Rust's runtime is up and main is running\n"
            );
            write_log(line.as_bytes());
            if std::env::var_os("BEVY_ASSET_ROOT").is_none() {
                set_asset_root(nro_path(), app_name);
            }
            sync_log();
            start_log_writer();
        }

        let app_name = app_name.to_string();
        std::panic::set_hook(Box::new(move |info| record_panic(&app_name, info)));
    });
}

/// Opens `log.txt` before `main`. Called from `userAppInit` right after `TPIDR_EL0` is set,
/// so that crashes in C constructors or Rust's runtime setup still leave a log.
///
/// The app name is the NRO file name: `sdmc:/switch/x/hello_switch.nro` -> `hello_switch`.
/// Without `argv[0]` (not launched as an NRO), `init_switch_logging` opens the log in `main`.
///
/// Never inlined, so nothing in here can be scheduled before `userAppInit` sets `TPIDR_EL0`.
#[cfg(target_os = "horizon")]
#[inline(never)]
pub(crate) fn start_boot_log(thread_pointer: u64) {
    let Some(nro_path) = nro_path() else {
        return;
    };
    let Some(app_name) = nro_stem(nro_path) else {
        return;
    };
    open_log_file(app_name, "userAppInit, before Rust's runtime setup and main");

    // Rust puts thread-locals at TPIDR_EL0 + an offset that assumes the template's start is
    // aligned like the block; libnx copies the template to a block aligned that way. Unless
    // the start is aligned, the two disagree.
    let tdata = &raw const ffi::__tdata_lma as u64;
    let align = unsafe { ffi::__tls_align } as u64;
    let tls_layout = if align.is_power_of_two() && tdata % align == 0 {
        "Rust and libnx agree on thread-local addresses"
    } else {
        "MISALIGNED: Rust and libnx put thread-locals at different addresses"
    };
    let line = format!(
        "[Boot] launched from {nro_path}\n\
         [Boot] TPIDR_EL0 = {thread_pointer:#x} (Rust's thread-local base, copied from libnx ThreadVars.tls_tp)\n\
         [Boot] TLS template at nro+{:#x}, align {align}: {tls_layout}\n",
        tdata - nro_base(),
    );
    write_log(line.as_bytes());
    write_log(crate::memory::boot_report().as_bytes());
    set_asset_root(Some(nro_path), app_name);
    write_log(b"[Boot] next: C constructors, Rust's runtime setup (std::rt::lang_start), then main\n");
    sync_log();
}

/// Tells Bevy where `assets/` is: next to the NRO, as it is next to the executable on desktop.
///
/// bevy_asset's `FileAssetReader` takes its root from `BEVY_ASSET_ROOT`, then
/// `CARGO_MANIFEST_DIR`, then the directory of `std::env::current_exe()`, which it unwraps
/// (bevy_asset 0.19.1 `src/io/file/mod.rs`). Rust's std has no `current_exe` on Horizon; it
/// returns an `Unsupported` error, so `AssetPlugin` panicked as soon as `DefaultPlugins` was
/// added. That is why this runs before `main`. A value that is already set, such as
/// `romfs:/`, is kept.
#[cfg(target_os = "horizon")]
fn set_asset_root(nro_path: Option<&str>, app_name: &str) {
    if let Some(root) = std::env::var_os("BEVY_ASSET_ROOT") {
        let line =
            format!("[Assets] BEVY_ASSET_ROOT is already {}; keeping it\n", root.to_string_lossy());
        write_log(line.as_bytes());
        return;
    }
    // sdmc:/switch/hello_switch/hello_switch.nro -> sdmc:/switch/hello_switch
    let root = match nro_path.and_then(|path| path.rsplit_once('/')) {
        Some((dir, _)) => dir.to_string(),
        None => format!("sdmc:/switch/{app_name}"),
    };
    // Sound: this runs before main, or at the start of it, before any other thread exists.
    unsafe { std::env::set_var("BEVY_ASSET_ROOT", &root) };
    let line = format!(
        "[Assets] BEVY_ASSET_ROOT = {root} (Bevy's default asset folder is {root}/assets)\n"
    );
    write_log(line.as_bytes());
}

/// `argv[0]`, the path the NRO was launched from, as parsed by libnx's `argvSetup`.
#[cfg(target_os = "horizon")]
fn nro_path() -> Option<&'static str> {
    unsafe {
        if ffi::__system_argc < 1 || ffi::__system_argv.is_null() || (*ffi::__system_argv).is_null() {
            return None;
        }
        CStr::from_ptr(*ffi::__system_argv).to_str().ok()
    }
}

/// `sdmc:/switch/hello_switch/hello_switch.nro` -> `hello_switch`.
#[cfg(target_os = "horizon")]
fn nro_stem(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', ':']).next()?;
    let stem = name.strip_suffix(".nro").unwrap_or(name);
    (!stem.is_empty()).then_some(stem)
}

#[cfg(target_os = "horizon")]
fn open_log_file(app_name: &str, opened_by: &str) {
    // libnx's default __appInit has already mounted sdmc, so 0x559 ("already mounted") is normal.
    let mount_rc = unsafe { ffi::fsdevMountSdmc() };
    unsafe {
        // If the dup2 below fails, stderr still reaches svcOutputDebugString.
        ffi::consoleDebugInit(1);
        ffi::mkdir(c"sdmc:/switch".as_ptr(), 0o777);
    }

    let app_dir = format!("sdmc:/switch/{app_name}");
    let app_dir_c = CString::new(app_dir.as_str()).unwrap_or_default();
    unsafe { ffi::mkdir(app_dir_c.as_ptr(), 0o777) };

    let mut dir = app_dir;
    let mut fd = open_truncated(&dir);
    if fd < 0 {
        dir = "sdmc:".to_string();
        fd = open_truncated(&dir);
    }
    if fd < 0 {
        return;
    }
    let _ = LOG_DIR.set(dir.clone());
    LOG_FD.store(fd, Ordering::SeqCst);

    let locked = lock_log_writes(fd);
    LOG_WRITES_LOCKED.store(locked.is_ok(), Ordering::SeqCst);
    let stdout_ok = unsafe { ffi::dup2(fd, 1) } == 1;
    let stderr_ok = unsafe { ffi::dup2(fd, 2) } == 2;
    let status = |ok: bool| if ok { "ok" } else { "FAILED (println!/Bevy logs will be missing)" };
    let header = format!(
        "==== bevy_horizon log: {app_name} ====\n\
         opened by:      {opened_by}\n\
         log file:       {dir}/log.txt (fd {fd})\n\
         fsdevMountSdmc: {mount_rc:#x} (0x559 = already mounted by libnx, which is normal)\n\
         stdout -> log:  {}\n\
         stderr -> log:  {}\n\
         log writes:     {}\n\
         nro base:       {:#x} (crash address - base = address in the ELF)\n\
         ========================================\n",
        status(stdout_ok),
        status(stderr_ok),
        match locked {
            Ok(()) => "one at a time, so threads' lines can't overwrite each other",
            Err(why) => why,
        },
        nro_base(),
    );
    write_log(header.as_bytes());
    sync_log();
}

#[cfg(target_os = "horizon")]
fn open_truncated(dir: &str) -> i32 {
    let Ok(path) = CString::new(format!("{dir}/log.txt")) else {
        return -1;
    };
    unsafe { ffi::open(path.as_ptr(), ffi::O_WRONLY | ffi::O_CREAT | ffi::O_TRUNC, 0o666) }
}

/// The libnx `Mutex` that `locked_write` holds around each write to `log.txt`.
#[cfg(target_os = "horizon")]
static LOG_LOCK: AtomicU32 = AtomicU32::new(0);
/// Whether `lock_log_writes` pointed the log at `locked_write`, which the log writer needs.
#[cfg(target_os = "horizon")]
static LOG_WRITES_LOCKED: AtomicBool = AtomicBool::new(false);
/// The SD card device's own `write_r`, which `locked_write` calls.
#[cfg(target_os = "horizon")]
static SDMC_WRITE: AtomicUsize = AtomicUsize::new(0);
/// A copy of the SD card's `devoptab_t`, with `write_r` replaced by `locked_write`.
#[cfg(target_os = "horizon")]
static mut LOG_DEVICE: [usize; ffi::DEVOPTAB_WORDS] = [0; ffi::DEVOPTAB_WORDS];

/// Makes writes to `log.txt` take turns.
///
/// stdout and stderr share the log's newlib handle through `dup2`, and with it one libnx file
/// offset. libnx's `fsdev_write` (`fs_dev.c`) writes at that offset and then advances it, with no
/// lock, so two threads writing at once can write over each other's text, or push the offset past
/// what was written. The next write then leaves a gap, which reads back as whatever the SD card
/// held there before: Skate 3's run-6 log has 171 bytes of unrelated text in one.
///
/// So the log's handle is pointed at a copy of the SD card's device whose `write_r` holds
/// `LOG_LOCK` around the SD card's own. Every write to the log, by fd 1, 2 or `LOG_FD`, goes
/// through it; other files don't. Runs before the `dup2`s, while nothing else can write the log.
#[cfg(target_os = "horizon")]
fn lock_log_writes(fd: i32) -> Result<(), &'static str> {
    unsafe {
        let handle = ffi::__get_handle(fd);
        if handle.is_null() {
            return Err("NOT locked: newlib has no handle for the log's fd");
        }
        let index = (*handle).device as usize;
        if index >= ffi::STD_MAX {
            return Err("NOT locked: the log's device is outside newlib's device table");
        }
        let sdmc = (&raw const ffi::devoptab_list).cast::<*const usize>().add(index).read();
        if sdmc.is_null() {
            return Err("NOT locked: the log's device isn't in newlib's device table");
        }
        let device = (&raw mut LOG_DEVICE).cast::<usize>();
        core::ptr::copy_nonoverlapping(sdmc, device, ffi::DEVOPTAB_WORDS);
        let sdmc_write = device.add(ffi::DEVOPTAB_WRITE_R).read();
        if sdmc_write == 0 {
            return Err("NOT locked: the SD card device has no write_r");
        }
        SDMC_WRITE.store(sdmc_write, Ordering::SeqCst);
        device.add(ffi::DEVOPTAB_NAME).write(c"bh_log".as_ptr() as usize);
        device.add(ffi::DEVOPTAB_WRITE_R).write(locked_write as ffi::WriteR as usize);
        let added = ffi::AddDevice(device);
        if added < 0 {
            return Err("NOT locked: newlib's device table is full");
        }
        (*handle).device = added as u32;
    }
    Ok(())
}

/// `write_r` of `LOG_DEVICE`: queues the text for the log writer, or else writes it with the SD
/// card's own `write_r`, one thread at a time.
#[cfg(target_os = "horizon")]
unsafe extern "C" fn locked_write(
    r: *mut core::ffi::c_void,
    file: *mut core::ffi::c_void,
    ptr: *const core::ffi::c_char,
    len: usize,
) -> isize {
    // Set before the log's handle pointed here.
    let sdmc_write: ffi::WriteR =
        unsafe { core::mem::transmute::<usize, ffi::WriteR>(SDMC_WRITE.load(Ordering::Relaxed)) };
    let lock = LOG_LOCK.as_ptr();
    // A thread that crashes inside a write still holds the lock, and its crash report comes
    // through here too, so it mustn't wait for itself. Writes made while holding it on purpose
    // (`write_out_queue`'s, `write_log`'s) come straight through as well.
    if unsafe { ffi::mutexIsLockedByCurrentThread(lock) } {
        return unsafe { sdmc_write(r, file, ptr, len) };
    }
    if len > 0 && !ptr.is_null() && enqueue(unsafe { core::slice::from_raw_parts(ptr.cast(), len) }) {
        return len as isize;
    }
    // No log writer, or no room in the queue: write it here, after what is queued.
    unsafe { ffi::mutexLock(lock) };
    write_out_queue(LOG_FD.load(Ordering::Relaxed));
    let written = unsafe { sdmc_write(r, file, ptr, len) };
    unsafe { ffi::mutexUnlock(lock) };
    written
}

// ---- The log writer.
//
// A write to log.txt is a round trip through newlib, libnx's fsdev and the FS service to the SD
// card, and the card sometimes takes a few hundred ms to answer one. With every thread writing
// its own lines, Skate 3's heartbeat frame was the slowest of its 10 s window about half the
// time, having spent about 20 ms writing its lines and syncing, and once 1.2 s (sk8 findings 27).
// So once `start_log_writer` has run, `locked_write` copies the text into `QUEUE` and returns,
// and the writer thread writes it out, oldest first, and syncs log.txt when asked
// (`request_log_sync`).
//
// `write_log` and `sync_log` still write and sync on their own thread, after writing out what is
// queued, so the panic, crash and abort reports and the exit lines are in log.txt, in order, when
// they return, even if the process ends next. With the queue full, `locked_write` does the same.
// Text still queued when the system ends the process (closing it from HOME) is lost; the writer
// keeps up within a write or two, so that is the last lines at most.

/// How much text can wait for the writer. Skate 3 writes about 200 KB in a 20-minute run.
#[cfg(target_os = "horizon")]
const QUEUE_BYTES: usize = 512 * 1024;
/// The writer's stack. It formats nothing: newlib, fsdev and the IPC need little.
#[cfg(target_os = "horizon")]
const WRITER_STACK: usize = 64 * 1024;
/// Text waiting for the writer: the bytes counted from `QUEUE_HEAD` up to `QUEUE_TAIL`, each at its
/// count modulo `QUEUE_BYTES`. A static, so queuing never allocates.
#[cfg(target_os = "horizon")]
static mut QUEUE: [u8; QUEUE_BYTES] = [0; QUEUE_BYTES];
/// libnx `Mutex` for the counts and flags below. Never held across a write to the SD card.
#[cfg(target_os = "horizon")]
static QUEUE_LOCK: AtomicU32 = AtomicU32::new(0);
/// libnx `CondVar` the writer sleeps on while it has nothing to do.
#[cfg(target_os = "horizon")]
static QUEUE_WAKE: AtomicU32 = AtomicU32::new(0);
/// Bytes ever queued.
#[cfg(target_os = "horizon")]
static QUEUE_TAIL: AtomicUsize = AtomicUsize::new(0);
/// Bytes ever written out. Only a thread holding `LOG_LOCK` moves it, and only once the write has
/// returned, so queuing never overwrites text still being written.
#[cfg(target_os = "horizon")]
static QUEUE_HEAD: AtomicUsize = AtomicUsize::new(0);
/// Whether `locked_write` may queue: from the writer's start until `stop_log_writer`.
#[cfg(target_os = "horizon")]
static QUEUE_OPEN: AtomicBool = AtomicBool::new(false);
/// Asks the writer to sync log.txt once it has written out the queue (`request_log_sync`).
#[cfg(target_os = "horizon")]
static SYNC_WANTED: AtomicBool = AtomicBool::new(false);
/// Asks the writer to write out the queue, sync, and return (`stop_log_writer`).
#[cfg(target_os = "horizon")]
static WRITER_STOP: AtomicBool = AtomicBool::new(false);

/// Runs `f` holding `QUEUE_LOCK`. Returns `None` if this thread holds it already, which only a
/// thread that crashed while holding it can: its report then goes straight to the SD card.
#[cfg(target_os = "horizon")]
fn with_queue<T>(f: impl FnOnce() -> T) -> Option<T> {
    let lock = QUEUE_LOCK.as_ptr();
    if unsafe { ffi::mutexIsLockedByCurrentThread(lock) } {
        return None;
    }
    unsafe { ffi::mutexLock(lock) };
    let value = f();
    unsafe { ffi::mutexUnlock(lock) };
    Some(value)
}

#[cfg(target_os = "horizon")]
fn wake_writer() {
    unsafe { ffi::svcSignalProcessWideKey(QUEUE_WAKE.as_ptr(), 1) };
}

/// Copies `data` into the queue. Returns false, queuing none of it, if the writer isn't running
/// or there isn't room for all of it.
#[cfg(target_os = "horizon")]
fn enqueue(data: &[u8]) -> bool {
    let queued = with_queue(|| {
        let head = QUEUE_HEAD.load(Ordering::Relaxed);
        let tail = QUEUE_TAIL.load(Ordering::Relaxed);
        if !QUEUE_OPEN.load(Ordering::Relaxed) || data.len() > QUEUE_BYTES - (tail - head) {
            return None;
        }
        let start = tail % QUEUE_BYTES;
        let first = data.len().min(QUEUE_BYTES - start);
        unsafe {
            let queue = (&raw mut QUEUE).cast::<u8>();
            core::ptr::copy_nonoverlapping(data.as_ptr(), queue.add(start), first);
            core::ptr::copy_nonoverlapping(data.as_ptr().add(first), queue, data.len() - first);
        }
        QUEUE_TAIL.store(tail + data.len(), Ordering::Relaxed);
        Some(head == tail)
    });
    match queued.flatten() {
        Some(was_empty) => {
            // The writer only sleeps once the queue is empty, and checks it under the lock first.
            if was_empty {
                wake_writer();
            }
            true
        }
        None => false,
    }
}

/// Writes out what is queued so far, oldest first. The caller holds `LOG_LOCK`, which makes it
/// the only thread taking text out, and sends these writes straight to the SD card
/// (`locked_write`). Text queued meanwhile is left for the writer, or the next caller.
#[cfg(target_os = "horizon")]
fn write_out_queue(fd: i32) {
    let Some((mut head, end)) =
        with_queue(|| (QUEUE_HEAD.load(Ordering::Relaxed), QUEUE_TAIL.load(Ordering::Relaxed)))
    else {
        return;
    };
    while head != end && fd >= 0 {
        let start = head % QUEUE_BYTES;
        let len = (end - head).min(QUEUE_BYTES - start);
        // Queuing only adds past `end`, so this part stays put until `QUEUE_HEAD` moves. A write
        // that fails is dropped rather than tried again: the next would most likely fail too.
        unsafe { ffi::write(fd, (&raw const QUEUE).cast::<u8>().add(start).cast(), len) };
        head += len;
        with_queue(|| QUEUE_HEAD.store(head, Ordering::Relaxed));
    }
}

/// Holds `LOG_LOCK` until dropped, unless this thread held it already (it crashed in a write).
#[cfg(target_os = "horizon")]
struct LogLockGuard(bool);

#[cfg(target_os = "horizon")]
impl LogLockGuard {
    fn take() -> Self {
        let lock = LOG_LOCK.as_ptr();
        let already = unsafe { ffi::mutexIsLockedByCurrentThread(lock) };
        if !already {
            unsafe { ffi::mutexLock(lock) };
        }
        Self(!already)
    }
}

#[cfg(target_os = "horizon")]
impl Drop for LogLockGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { ffi::mutexUnlock(LOG_LOCK.as_ptr()) };
        }
    }
}

/// Starts the log writer, once `log.txt` is open and its writes come through `locked_write`.
/// The thread is detached, so `close_detached_threads` closes it once `stop_log_writer` has
/// made it return.
#[cfg(target_os = "horizon")]
fn start_log_writer() {
    let fd = LOG_FD.load(Ordering::SeqCst);
    if fd < 0 || !LOG_WRITES_LOCKED.load(Ordering::SeqCst) {
        write_log(b"[Log] no log writer, since log.txt's writes don't go through locked_write: each thread writes its own lines\n");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("bevy_horizon log writer".into())
        .stack_size(WRITER_STACK)
        .spawn(move || run_log_writer(fd));
    let line = match spawned {
        Ok(_detached) => {
            with_queue(|| QUEUE_OPEN.store(true, Ordering::Relaxed));
            format!(
                "[Log] log writer started: threads queue their log.txt text (up to {} KiB) and go on, and it writes it to the SD card and syncs once a second. Panic, crash, abort and exit reports are still written on their own thread\n",
                QUEUE_BYTES / 1024
            )
        }
        Err(error) => {
            format!("[Log] couldn't start the log writer ({error}): each thread writes its own lines\n")
        }
    };
    write_log(line.as_bytes());
}

/// The log writer: writes out the queue as text arrives, and syncs log.txt when asked, until
/// `stop_log_writer`.
#[cfg(target_os = "horizon")]
fn run_log_writer(fd: i32) {
    loop {
        let (sync, stop) = with_queue(|| loop {
            let idle = QUEUE_HEAD.load(Ordering::Relaxed) == QUEUE_TAIL.load(Ordering::Relaxed);
            let sync = SYNC_WANTED.swap(false, Ordering::Relaxed);
            let stop = WRITER_STOP.load(Ordering::Relaxed);
            if !idle || sync || stop {
                break (sync, stop);
            }
            // Releases the lock while it sleeps. Everything that wakes it sets what it checks
            // under the lock, so the timeout is only a guard.
            unsafe {
                ffi::condvarWaitTimeout(QUEUE_WAKE.as_ptr(), QUEUE_LOCK.as_ptr(), 1_000_000_000)
            };
        })
        .unwrap_or((true, true));
        {
            let _log = LogLockGuard::take();
            write_out_queue(fd);
            if sync || stop {
                unsafe { ffi::fsync(fd) };
            }
        }
        if stop {
            return;
        }
    }
}

/// Has the log writer write out what is queued, sync and return, and makes every later write to
/// log.txt happen on its own thread. For `release_horizon_services`: the writer's stack keeps heap
/// pages locked until `close_detached_threads` closes it. Allocates nothing, and doesn't wait.
#[cfg(target_os = "horizon")]
pub(crate) fn stop_log_writer() {
    let was_open = with_queue(|| {
        WRITER_STOP.store(true, Ordering::Relaxed);
        QUEUE_OPEN.swap(false, Ordering::Relaxed)
    });
    if was_open == Some(true) {
        wake_writer();
    }
}

/// Whether the log writer thread is taking `log.txt`'s writes (`start_log_writer`).
#[cfg(target_os = "horizon")]
pub(crate) fn log_writer_running() -> bool {
    QUEUE_OPEN.load(Ordering::Relaxed)
}

/// Pushes buffered stdout into `log.txt` and asks the FS service to flush it to the SD card,
/// waiting for both, with whatever the log writer still had queued written first.
pub fn flush_switch_logs() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    #[cfg(target_os = "horizon")]
    sync_log();
}

/// `flush_switch_logs` without the wait: pushes buffered stdout into the queue and asks the log
/// writer to write it out and sync. Without a log writer it syncs here.
#[cfg(target_os = "horizon")]
pub(crate) fn request_log_sync() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let asked = with_queue(|| {
        let open = QUEUE_OPEN.load(Ordering::Relaxed);
        if open {
            SYNC_WANTED.store(true, Ordering::Relaxed);
        }
        open
    });
    if asked == Some(true) {
        wake_writer();
    } else {
        sync_log();
    }
}

/// Runtime address the NRO was loaded at. The ELF is linked at 0, so a crash address minus
/// this is the address to look up in the ELF.
#[cfg(target_os = "horizon")]
pub(crate) fn nro_base() -> u64 {
    ffi::_start as *const () as u64
}

/// Runtime address just past the loaded image.
#[cfg(target_os = "horizon")]
#[allow(unused_unsafe)]
pub(crate) fn nro_end() -> u64 {
    unsafe { &raw const ffi::__end__ as u64 }
}

/// The folder `log.txt` is in, once logging has started.
#[cfg(target_os = "horizon")]
pub(crate) fn log_dir() -> Option<&'static str> {
    LOG_DIR.get().map(String::as_str)
}

/// Writes straight to the log file, bypassing Rust's stdout buffer and lock and the log writer's
/// queue: after what is queued, and before it returns, since what calls it may end the process
/// next (the write still takes its turn with other threads': see `lock_log_writes`).
/// Returns false if there is no log file.
#[cfg(target_os = "horizon")]
pub(crate) fn write_log(data: &[u8]) -> bool {
    let fd = LOG_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return false;
    }
    let _log = LogLockGuard::take();
    write_out_queue(fd);
    unsafe { ffi::write(fd, data.as_ptr().cast(), data.len()) >= 0 }
}

/// Writes one formatted line to `log.txt` without allocating, for code that runs at exit or
/// after a failure. Text past 255 bytes is cut off.
#[cfg(target_os = "horizon")]
pub(crate) fn write_log_line(args: core::fmt::Arguments) {
    use core::fmt::Write;

    let mut line = FixedBuf::<256>::new();
    let _ = line.write_fmt(args);
    line.end_line();
    write_log(line.as_bytes());
}

/// Writes out what the log writer has queued, then syncs log.txt, on this thread.
#[cfg(target_os = "horizon")]
pub(crate) fn sync_log() {
    let fd = LOG_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let _log = LogLockGuard::take();
        write_out_queue(fd);
        unsafe { ffi::fsync(fd) };
    }
}

/// Appends `data` to `<log dir>/<name>`, falling back to `sdmc:/<name>`.
///
/// Paths are built on the stack so the abort and exception handlers can call this, but
/// newlib's `open` still mallocs a small handle, so those handlers write `log.txt` first.
#[cfg(target_os = "horizon")]
pub(crate) fn append_crash_file(name: &str, data: &[u8]) {
    use core::fmt::Write;

    if let Some(dir) = LOG_DIR.get() {
        let mut path = FixedBuf::<256>::new();
        if write!(path, "{dir}/{name}").is_ok() && append(&mut path, data) {
            return;
        }
    }
    let mut path = FixedBuf::<256>::new();
    if write!(path, "sdmc:/{name}").is_ok() {
        append(&mut path, data);
    }
}

#[cfg(target_os = "horizon")]
fn append<const N: usize>(path: &mut FixedBuf<N>, data: &[u8]) -> bool {
    unsafe {
        let fd = ffi::open(path.as_c_ptr(), ffi::O_WRONLY | ffi::O_CREAT | ffi::O_APPEND, 0o666);
        if fd < 0 {
            return false;
        }
        ffi::write(fd, data.as_ptr().cast(), data.len());
        ffi::fsync(fd);
        ffi::close(fd);
        true
    }
}

/// Prints a Horizon `Result` as hex and in the `2345-0010` form Atmosphère's crash reports and
/// switchbrew use: 2000 + module, then the description. libnx `result.h` packs the module into
/// the low 9 bits and the description into the next 13.
#[cfg(target_os = "horizon")]
pub(crate) struct ResultCode(pub u32);

#[cfg(target_os = "horizon")]
impl core::fmt::Display for ResultCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let module = self.0 & 0x1ff;
        let description = (self.0 >> 9) & 0x1fff;
        write!(f, "{:#x} = {:04}-{description:04}", self.0, 2000 + module)
    }
}

/// Fixed-capacity text buffer for building messages where allocating is not safe.
/// Text that doesn't fit is truncated; the last byte is kept for a NUL terminator.
#[cfg(target_os = "horizon")]
pub(crate) struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

#[cfg(target_os = "horizon")]
impl<const N: usize> FixedBuf<N> {
    pub(crate) const fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub(crate) fn as_c_ptr(&mut self) -> *const core::ffi::c_char {
        self.buf[self.len] = 0;
        self.buf.as_ptr().cast()
    }

    /// Ends the text with a newline, giving up its last byte if the buffer is full.
    pub(crate) fn end_line(&mut self) {
        if self.len > 0 && self.buf[self.len - 1] == b'\n' {
            return;
        }
        if self.len == N - 1 {
            self.len -= 1;
        }
        self.buf[self.len] = b'\n';
        self.len += 1;
    }
}

#[cfg(target_os = "horizon")]
impl<const N: usize> core::fmt::Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(N - 1 - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        if n == s.len() { Ok(()) } else { Err(core::fmt::Error) }
    }
}

fn record_panic(app_name: &str, info: &PanicHookInfo) {
    let thread = std::thread::current();
    let message = info.payload_as_str().unwrap_or("<non-string panic payload>");
    let location = info
        .location()
        .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
        .unwrap_or_else(|| "unknown".to_string());
    // A panic after a failed GPU allocation (wgpu's "Out of Memory") is often down to the heap.
    #[cfg(target_os = "horizon")]
    let memory = format!("memory:   {}\n", crate::memory::MallocFigures::now());
    #[cfg(not(target_os = "horizon"))]
    let memory = String::new();
    let report = format!(
        "\n==================== RUST PANIC ====================\n\
         app:      {app_name}\n\
         thread:   {}\n\
         location: {location}\n\
         message:  {message}\n\
         {memory}\
         On Switch (panic=abort) the app now exits without a crash report: to the homebrew menu,\n\
         or to HOME if other threads are still running (log.txt's [Exit] lines say which).\n\
         ====================================================\n",
        thread.name().unwrap_or("<unnamed>"),
    );

    #[cfg(target_os = "horizon")]
    {
        unsafe { ffi::svcOutputDebugString(report.as_ptr().cast(), report.len()) };
        if !write_log(report.as_bytes()) {
            unsafe { ffi::write(2, report.as_ptr().cast(), report.len()) };
        }
        flush_switch_logs();
        append_crash_file("panic.txt", report.as_bytes());
    }

    #[cfg(not(target_os = "horizon"))]
    eprintln!("{report}");
}
