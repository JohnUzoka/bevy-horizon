//! Horizon OS POSIX and runtime stubs for newlib / Rust std, plus libnx crash hooks.

use core::sync::atomic::{AtomicBool as CoreAtomicBool, AtomicU8, Ordering as AtomicOrdering};

/// Which CPU cores new threads may run on. Applied to every thread created with libnx's default
/// core (`-2`) after it is set: Rust's, Bevy's task pools' and C libraries' threads alike (they
/// all go through `threadCreate`). A thread created for a chosen core keeps it.
///
/// libnx's `threadCreate` with core `-2` starts a thread on the process's default core, and only
/// that core. std's threads go through newlib's `pthread_create` and libnx's
/// `__syscall_thread_create` (`nx/source/runtime/newlib.c`), which creates them that way and then,
/// before starting them, gives them no ideal core and the whole process core mask (cores 0-3
/// for homebrew on Atmosphère), so the kernel may move them to any idle core. They also get
/// priority 0x3B (59), lower than the main thread's 0x2C (44), and Horizon never time-slices a
/// lower-priority thread in while a higher one is runnable on its core: a main thread that spins
/// waiting for a worker on the same core starves it. See docs/platform-notes.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreadCorePolicy {
    /// libnx's behaviour: std's threads may run on any core, threads created directly by
    /// `threadCreate` with core `-2` only on the process's default core.
    Default = 0,
    /// Threads may run on any application core (cores 0-2), with no preferred core. Core 3,
    /// which the system also uses, is left out.
    AnyCore = 1,
    /// Each new thread starts on the next application core in turn (1, 2, 0, 1, ...), and the
    /// kernel may still move it to any of them.
    Spread = 2,
    /// Each new thread is fixed to the next application core in turn (1, 2, 0, 1, ...).
    Pinned = 3,
}

impl ThreadCorePolicy {
    pub const ALL: [ThreadCorePolicy; 4] = [Self::Default, Self::AnyCore, Self::Spread, Self::Pinned];

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::AnyCore => "AnyCore",
            Self::Spread => "Spread",
            Self::Pinned => "Pinned",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::AnyCore,
            2 => Self::Spread,
            3 => Self::Pinned,
            _ => Self::Default,
        }
    }
}

static THREAD_CORE_POLICY: AtomicU8 = AtomicU8::new(ThreadCorePolicy::Default as u8);

/// Sets the core policy for threads created from now on; threads that already exist keep theirs.
/// Set it before `App::new()` / `DefaultPlugins` for it to cover Bevy's task pools. Does nothing
/// off Horizon.
pub fn set_thread_core_policy(policy: ThreadCorePolicy) {
    THREAD_CORE_POLICY.store(policy as u8, AtomicOrdering::SeqCst);
}

pub fn thread_core_policy() -> ThreadCorePolicy {
    ThreadCorePolicy::from_u8(THREAD_CORE_POLICY.load(AtomicOrdering::SeqCst))
}

static THREAD_LOGGING: CoreAtomicBool = CoreAtomicBool::new(true);

/// Whether each new thread writes its two `[Thread]` lines to `log.txt` (on by default). Each
/// line is a separate write to the SD card.
pub fn set_thread_logging(enabled: bool) {
    THREAD_LOGGING.store(enabled, AtomicOrdering::Relaxed);
}

pub fn thread_logging() -> bool {
    THREAD_LOGGING.load(AtomicOrdering::Relaxed)
}

#[cfg(target_os = "horizon")]
pub mod horizon_stubs {
    use core::arch::asm;
    use core::ffi::{c_char, c_int, c_long, c_void};
    use core::fmt::{self, Write};
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use crate::logging::{
        FixedBuf, ResultCode, append_crash_file, ffi, nro_base, nro_end, start_boot_log,
        sync_log, write_log,
    };

    /// Points `TPIDR_EL0` at this thread's TLS block, so Rust's thread-locals work. Returns it.
    ///
    /// Rust (LLVM, local-exec TLS) finds `thread_local!` data at `TPIDR_EL0` + a link-time
    /// offset. libnx never writes `TPIDR_EL0`, so it is 0 and the first thread-local access
    /// faults; std makes one in `rt::init`, before `main`. libnx keeps the thread pointer in
    /// `ThreadVars.tls_tp`, at 0x1F8 in the kernel's per-thread TLS page (`TPIDRRO_EL0`), where
    /// devkitA64's C code reads it through `__aarch64_read_tp`. libnx fills it in `newlibSetup`
    /// for the main thread and in `_EntryWrap` for new threads, before our code runs. The kernel
    /// saves `TPIDR_EL0` per thread, so it only needs setting once per thread.
    ///
    /// Do not touch thread-locals in the function that calls this: LLVM treats the thread
    /// pointer as constant and may read it before the `msr`.
    #[inline(never)]
    pub fn set_thread_pointer() -> u64 {
        let tp: u64;
        unsafe {
            asm!(
                "mrs {tp}, tpidrro_el0",
                "ldr {tp}, [{tp}, #0x1f8]",
                "msr tpidr_el0, {tp}",
                tp = out(reg) tp,
                options(nostack, preserves_flags),
            );
        }
        tp
    }

    /// Returns (`TPIDR_EL0`, libnx `tls_tp`). They differ on a thread we failed to set up.
    pub fn thread_pointers() -> (u64, u64) {
        let (tpidr, tls_tp): (u64, u64);
        unsafe {
            asm!(
                "mrs {tpidr}, tpidr_el0",
                "mrs {tls_tp}, tpidrro_el0",
                "ldr {tls_tp}, [{tls_tp}, #0x1f8]",
                tpidr = out(reg) tpidr,
                tls_tp = out(reg) tls_tp,
                options(nostack, preserves_flags, readonly),
            );
        }
        (tpidr, tls_tp)
    }

    /// libnx calls this weak hook at the end of `__appInit`, once services are up and sdmc is
    /// mounted, and before C constructors, Rust's runtime setup (`std::rt::lang_start`) and
    /// `main`. It is the earliest point where both the main thread pointer and logging can work.
    #[unsafe(no_mangle)]
    pub extern "C" fn userAppInit() {
        let tp = set_thread_pointer();
        start_boot_log(tp);
    }

    /// libnx calls this weak hook first in `__appExit`, whenever the NRO ends itself: `main`
    /// returning or `std::process::exit` (newlib's `exit`), or an abort (`_exit`); all reach
    /// `__libnx_exit`. Closing the app from HOME doesn't: without `appletLockExit` the system
    /// ends the process, and the kernel frees everything. `main` and Rust's runtime are done
    /// by now, but libnx hasn't yet closed fs, hid, applet or sm, so `log.txt` can still be
    /// written.
    #[unsafe(no_mangle)]
    pub extern "C" fn userAppExit() {
        crate::lifecycle::release_horizon_services();
    }

    unsafe extern "C" {
        /// newlib's own `fchmod`; `--wrap=fchmod` gives it this name.
        fn __real_fchmod(fd: c_int, mode: u32) -> c_int;
        fn __errno() -> *mut c_int;
    }

    /// newlib's `ENOSYS`.
    const ENOSYS: c_int = 88;

    /// libnx's SD card driver (`fsdev`) has no `fchmod` and fails it with `ENOSYS`: Horizon's file
    /// systems have no permission bits. std's `fs::copy` opens the destination with
    /// create + truncate and then calls `fchmod` on it, so every copy left an empty file and
    /// returned "Function not implemented". The target spec links with `--wrap=fchmod`, which
    /// sends calls here; `ENOSYS` becomes success because there is nothing to set.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_fchmod(fd: c_int, mode: u32) -> c_int {
        let rc = unsafe { __real_fchmod(fd, mode) };
        if rc == -1 && unsafe { *__errno() } == ENOSYS {
            return 0;
        }
        rc
    }

    unsafe extern "C" {
        /// newlib's per-thread `struct _reent`, which holds `errno`.
        fn __getreent() -> *mut c_void;
        /// libsysbase's rename, which calls the device's (libnx `fsdev_rename`).
        fn _rename_r(reent: *mut c_void, old: *const c_char, new: *const c_char) -> c_int;
    }

    /// newlib's `EEXIST`.
    const EEXIST: c_int = 17;
    /// Added to the destination's name while it is moved aside.
    const REPLACED_SUFFIX: &[u8] = b".bh-replaced";

    /// Horizon's file systems won't rename onto an existing file (libnx turns `PathAlreadyExists`
    /// into `EEXIST`), where POSIX, and std's `fs::rename` docs on Unix, replace it. Saving
    /// through a temporary file and a rename, as `tempfile`'s `persist` and much save code do,
    /// failed for every save after the first. The target spec links with `--wrap=rename`, which
    /// sends calls here instead of newlib's `rename` (`stdio/rename.c`, which only calls
    /// `_rename_r`). Defining `rename` itself doesn't work: a game's build script puts `-lc`
    /// ahead of every rlib, so newlib's copy is pulled in first and the link fails with a
    /// duplicate symbol.
    ///
    /// When both paths are files, it moves the old destination aside to `<new>.bh-replaced`,
    /// renames, then deletes that, or moves it back if the rename failed. Unlike POSIX it isn't
    /// atomic: if the app dies in between, `<new>` is missing and its old contents are in
    /// `<new>.bh-replaced`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_rename(old: *const c_char, new: *const c_char) -> c_int {
        let reent = unsafe { __getreent() };
        let rc = unsafe { _rename_r(reent, old, new) };
        if rc == 0 || unsafe { *__errno() } != EEXIST {
            return rc;
        }
        match unsafe { replace_file(reent, old, new) } {
            Ok(()) => 0,
            Err(errno) => {
                unsafe { *__errno() = errno };
                -1
            }
        }
    }

    /// `rename` onto an existing file. Errors are errno values.
    unsafe fn replace_file(
        reent: *mut c_void,
        old: *const c_char,
        new: *const c_char,
    ) -> Result<(), c_int> {
        use std::ffi::{CStr, CString, OsStr};
        use std::os::unix::ffi::OsStrExt;

        let path = |c: &CStr| OsStr::from_bytes(c.to_bytes()).to_owned();
        let is_file = |c: &CStr| std::fs::symlink_metadata(path(c)).is_ok_and(|m| m.is_file());
        let (old_c, new_c) = unsafe { (CStr::from_ptr(old), CStr::from_ptr(new)) };
        if !is_file(old_c) || !is_file(new_c) {
            return Err(EEXIST);
        }
        let mut aside = new_c.to_bytes().to_vec();
        aside.extend_from_slice(REPLACED_SUFFIX);
        let aside = CString::new(aside).map_err(|_| EEXIST)?;
        if std::fs::symlink_metadata(path(&aside)).is_ok() {
            return Err(EEXIST);
        }
        if unsafe { _rename_r(reent, new, aside.as_ptr()) } != 0 {
            return Err(EEXIST);
        }
        if unsafe { _rename_r(reent, old, new) } == 0 {
            let _ = std::fs::remove_file(path(&aside));
            return Ok(());
        }
        // Also the way out when `old` and `new` name the same file: it is now `aside`.
        let errno = unsafe { *__errno() };
        unsafe { _rename_r(reent, aside.as_ptr(), new) };
        Err(errno)
    }

    unsafe extern "C" {
        /// libsysbase's `rmdir`, which calls the device's (libnx `fsdev_rmdir`); `--wrap=rmdir`
        /// gives it this name.
        fn __real_rmdir(path: *const c_char) -> c_int;
        /// The Horizon `Result` that libnx's fsdev last turned into an errno on this thread.
        fn fsdevGetLastResult() -> u32;
    }

    /// newlib's `EIO` and `ENOTEMPTY`.
    const EIO: c_int = 5;
    const ENOTEMPTY: c_int = 90;
    /// Horizon's `fs::ResultDirectoryNotEmpty`, module 2 description 8 (Atmosphère's
    /// `libvapours/include/vapours/results/fs_results.hpp`), packed as libnx's `MAKERESULT` does.
    const FS_RESULT_DIRECTORY_NOT_EMPTY: u32 = 2 | (8 << 9);

    /// libnx's fsdev knows four FS results (`error_table` in `fs_dev.c`) and turns the rest into
    /// `EIO`, so removing a folder that isn't empty failed with "I/O error", where std's
    /// `fs::remove_dir` gives `ErrorKind::DirectoryNotEmpty`. The target spec links with
    /// `--wrap=rmdir`, which sends calls here; when the FS said `DirectoryNotEmpty`, the errno
    /// becomes `ENOTEMPTY`. `fs::remove_dir_all` removes its last folder through this too.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_rmdir(path: *const c_char) -> c_int {
        let rc = unsafe { __real_rmdir(path) };
        if rc == -1
            && unsafe { *__errno() } == EIO
            && unsafe { fsdevGetLastResult() } == FS_RESULT_DIRECTORY_NOT_EMPTY
        {
            unsafe { *__errno() = ENOTEMPTY };
        }
        rc
    }

    /// libnx `ThreadFunc`.
    type ThreadFunc = unsafe extern "C" fn(arg: *mut c_void);

    struct ThreadStart {
        entry: ThreadFunc,
        arg: *mut c_void,
        /// Numbers the thread in `log.txt`.
        number: u32,
        /// The core policy's (ideal core, affinity mask), set again by the thread itself.
        placement: Option<(i32, u64)>,
    }

    static THREADS_CREATED: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" {
        /// libnx's own `threadCreate`; `--wrap=threadCreate` gives it this name.
        fn __real_threadCreate(
            t: *mut c_void,
            entry: ThreadFunc,
            arg: *mut c_void,
            stack_mem: *mut c_void,
            stack_sz: usize,
            prio: c_int,
            cpuid: c_int,
        ) -> u32;
        fn malloc(size: usize) -> *mut c_void;
        fn free(ptr: *mut c_void);
    }

    /// `MAKERESULT(Module_Libnx, LibnxError_OutOfMemory)`, which libnx itself returns.
    const LIBNX_OUT_OF_MEMORY: u32 = 345 | (2 << 9);

    /// Every thread is created by libnx's `threadCreate`, Rust's included: `std::thread::spawn`
    /// -> `pthread_create` (newlib) -> `__syscall_thread_create` (libnx) -> `threadCreate`.
    /// The target spec links with `--wrap=threadCreate`, which sends those calls here, so each
    /// thread starts in `thread_start` and gets `TPIDR_EL0` before its real entry point runs.
    /// Both steps write a `[Thread]` line to `log.txt`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_threadCreate(
        t: *mut c_void,
        entry: ThreadFunc,
        arg: *mut c_void,
        stack_mem: *mut c_void,
        stack_sz: usize,
        prio: c_int,
        cpuid: c_int,
    ) -> u32 {
        // Frees the stacks of detached threads that have exited before taking another one.
        close_detached_threads(Duration::ZERO);
        // malloc, not Box: this must not depend on whichever #[global_allocator] the game uses.
        let start = unsafe { malloc(size_of::<ThreadStart>()) }.cast::<ThreadStart>();
        let number = THREADS_CREATED.fetch_add(1, Ordering::Relaxed) + 1;
        if start.is_null() {
            log_thread_created(number, stack_sz, prio, cpuid, LIBNX_OUT_OF_MEMORY, None);
            return LIBNX_OUT_OF_MEMORY;
        }
        let policy = super::thread_core_policy();
        let (placement, policy_rc) = if cpuid == PROCESS_DEFAULT_CORE {
            choose_placement(policy)
        } else {
            (None, 0)
        };
        // The kernel moves a thread only when its current core leaves its mask, so Spread and
        // Pinned threads are created on their core rather than moved there.
        let create_core = match placement {
            Some((ideal, _)) if ideal >= 0 => ideal,
            _ => cpuid,
        };
        unsafe {
            start.write(ThreadStart { entry, arg, number, placement });
            let rc = __real_threadCreate(
                t,
                thread_start,
                start.cast(),
                stack_mem,
                stack_sz,
                prio,
                create_core,
            );
            // libnx creates the thread suspended; it runs once its creator calls threadStart. So
            // the mask is set here, from the creating thread, for callers of threadCreate that
            // start the thread as created. pthread_create changes it before starting the thread,
            // so `thread_start` sets it again.
            // `Thread.handle` is the first field of libnx's `Thread` (switch/kernel/thread.h).
            let placed = if rc == 0 {
                let handle = t.cast::<u32>().read();
                crate::profiler::register_thread(handle, number);
                let rc = match placement {
                    Some((ideal, mask)) if policy_rc == 0 => {
                        svcSetThreadCoreMask(handle, ideal, mask)
                    }
                    _ => policy_rc,
                };
                Some(core_placement(policy, rc, handle))
            } else {
                None
            };
            log_thread_created(number, stack_sz, prio, create_core, rc, placed.as_ref());
            if rc != 0 {
                free(start.cast());
            }
            rc
        }
    }

    /// First code on every new thread. libnx's `_EntryWrap` calls it after setting `tls_tp`.
    unsafe extern "C" fn thread_start(start: *mut c_void) {
        set_thread_pointer();
        unsafe {
            let ThreadStart {
                entry,
                arg,
                number,
                placement,
            } = start.cast::<ThreadStart>().read();
            free(start);
            let reapplied = placement.and_then(|(ideal, mask)| reapply_placement(ideal, mask));
            log_thread_started(number, reapplied);
            entry(arg);
            crate::profiler::unregister_current_thread();
        }
    }

    const CUR_PROCESS_HANDLE: u32 = 0xFFFF_8001;
    const CUR_THREAD_HANDLE: u32 = 0xFFFF_8000;
    const INFO_TYPE_CORE_MASK: u32 = 0;
    /// `threadCreate`'s "the process's default core".
    const PROCESS_DEFAULT_CORE: c_int = -2;
    /// `svc::IdealCoreDontCare`: no preferred core. (Not `IdealCoreNoUpdate`, -3: with the
    /// don't-care ideal pthread_create leaves, the kernel rejects it, `kern_k_thread.cpp`.)
    const IDEAL_CORE_DONT_CARE: i32 = -1;
    /// Cores 0-2. Core 3 belongs to the system; it is never chosen even if the process may use it.
    const APPLICATION_CORES: u64 = 0b0111;

    unsafe extern "C" {
        // libnx's wrappers are bare `svc` instructions; u64 masks keep the registers' upper
        // halves zero, which the kernel reads.
        fn svcGetInfo(out: *mut u64, id0: u32, handle: u32, id1: u64) -> u32;
        fn svcSetThreadCoreMask(handle: u32, core_id: i32, affinity_mask: u64) -> u32;
        fn svcGetThreadCoreMask(core_id: *mut i32, affinity_mask: *mut u64, handle: u32) -> u32;
        fn svcGetCurrentProcessorNumber() -> u32;
    }

    /// Where a new thread ended up, for its `[Thread]` line.
    #[derive(Clone, Copy)]
    struct CorePlacement {
        policy: super::ThreadCorePolicy,
        /// Result of svcGetInfo or svcSetThreadCoreMask; 0 when the policy was applied.
        rc: u32,
        ideal_core: i32,
        mask: u64,
    }

    /// Round-robin counter for `Spread` and `Pinned`.
    static NEXT_CORE: AtomicU32 = AtomicU32::new(0);

    /// The `n`th core of `mask`, counting from core 1 so the first worker doesn't share core 0
    /// with the main thread: with cores 0-2, n = 0, 1, 2, 3 gives cores 1, 2, 0, 1.
    fn nth_core(mask: u64, n: u32) -> i32 {
        let cores = mask.count_ones().max(1);
        let order = (0u32..4).map(|i| (i + 1) % 4).filter(|&core| mask & (1u64 << core) != 0);
        order.cycle().nth((n % cores) as usize).unwrap_or(0) as i32
    }

    /// The policy's (ideal core, affinity mask) for the next thread, `None` for `Default`, and
    /// svcGetInfo's result.
    #[inline(never)]
    fn choose_placement(policy: super::ThreadCorePolicy) -> (Option<(i32, u64)>, u32) {
        use super::ThreadCorePolicy;
        if policy == ThreadCorePolicy::Default {
            return (None, 0);
        }
        let mut process_mask = 0u64;
        let rc = unsafe { svcGetInfo(&mut process_mask, INFO_TYPE_CORE_MASK, CUR_PROCESS_HANDLE, 0) };
        let mask = process_mask & APPLICATION_CORES;
        if rc != 0 || mask == 0 {
            return (None, rc);
        }
        let placement = match policy {
            ThreadCorePolicy::AnyCore => (IDEAL_CORE_DONT_CARE, mask),
            ThreadCorePolicy::Spread => {
                (nth_core(mask, NEXT_CORE.fetch_add(1, Ordering::Relaxed)), mask)
            }
            _ => {
                let core = nth_core(mask, NEXT_CORE.fetch_add(1, Ordering::Relaxed));
                (core, 1u64 << core)
            }
        };
        (Some(placement), 0)
    }

    /// Reads back where the new, not yet started, thread `handle` may run.
    fn core_placement(policy: super::ThreadCorePolicy, rc: u32, handle: u32) -> CorePlacement {
        let (mut ideal_core, mut mask) = (-1i32, 0u64);
        unsafe { svcGetThreadCoreMask(&mut ideal_core, &mut mask, handle) };
        CorePlacement { policy, rc, ideal_core, mask }
    }

    /// Runs on the new thread. pthread_create widens a thread's mask to the whole process between
    /// `threadCreate` and `threadStart` (libnx `newlib.c`, `__syscall_thread_create`), undoing
    /// the policy; this sets it again. Returns svcSetThreadCoreMask's result if it had to.
    #[inline(never)]
    fn reapply_placement(ideal: i32, mask: u64) -> Option<u32> {
        let (mut now_ideal, mut now_mask) = (0i32, 0u64);
        let rc = unsafe { svcGetThreadCoreMask(&mut now_ideal, &mut now_mask, CUR_THREAD_HANDLE) };
        if rc == 0 && now_ideal == ideal && now_mask == mask {
            return None;
        }
        Some(unsafe { svcSetThreadCoreMask(CUR_THREAD_HANDLE, ideal, mask) })
    }

    /// Stack-only formatting: this runs inside `pthread_create`.
    #[inline(never)]
    fn log_thread_created(
        number: u32,
        stack_sz: usize,
        prio: c_int,
        cpuid: c_int,
        rc: u32,
        placement: Option<&CorePlacement>,
    ) {
        let failed = rc != 0 || placement.is_some_and(|p| p.rc != 0);
        if !failed && !super::thread_logging() {
            return;
        }
        let mut out = FixedBuf::<384>::new();
        let _ = write!(
            out,
            "[Thread] #{number}: threadCreate with a {} KiB stack, priority {prio:#x}, core {cpuid}{}: ",
            stack_sz / 1024,
            if cpuid == PROCESS_DEFAULT_CORE { " (the process's default core)" } else { "" },
        );
        let _ = if rc == 0 {
            write!(out, "created")
        } else {
            write!(out, "FAILED with {}", ResultCode(rc))
        };
        if let Some(&CorePlacement { policy, rc, ideal_core, mask }) = placement {
            let _ = write!(
                out,
                "; core policy {}: ideal core {ideal_core}, may run on cores {mask:#06b}",
                policy.name(),
            );
            if rc != 0 {
                let _ = write!(out, " (applying the policy FAILED with {})", ResultCode(rc));
            }
        }
        let _ = writeln!(out);
        write_log(out.as_bytes());
    }

    /// Runs on the new thread, after `set_thread_pointer`. Separate and not inlined so that
    /// nothing here can read `TPIDR_EL0` before it is set. `reapplied` is the result of setting
    /// the core policy again, if pthread_create had changed it.
    #[inline(never)]
    fn log_thread_started(number: u32, reapplied: Option<u32>) {
        let (tpidr, tls_tp) = thread_pointers();
        let failed = tpidr != tls_tp || reapplied.is_some_and(|rc| rc != 0);
        if !failed && !super::thread_logging() {
            return;
        }
        let core = unsafe { svcGetCurrentProcessorNumber() };
        let (mut ideal_core, mut mask) = (0i32, 0u64);
        unsafe { svcGetThreadCoreMask(&mut ideal_core, &mut mask, CUR_THREAD_HANDLE) };
        let mut out = FixedBuf::<320>::new();
        let _ = write!(
            out,
            "[Thread] #{number}: running on core {core}, ideal core {ideal_core}, may run on cores {mask:#06b}"
        );
        let _ = match reapplied {
            None => Ok(()),
            Some(0) => write!(out, " (core policy set again after pthread_create)"),
            Some(rc) => write!(out, " (setting the core policy again FAILED with {})", ResultCode(rc)),
        };
        let _ = if tpidr == tls_tp {
            writeln!(
                out,
                "; TPIDR_EL0 = libnx tls_tp = {tpidr:#x}, so thread_local! works"
            )
        } else {
            writeln!(
                out,
                "; TPIDR_EL0 {tpidr:#x} != libnx tls_tp {tls_tp:#x}: MISMATCH, thread_local! will fault"
            )
        };
        write_log(out.as_bytes());
    }

    // ---- Detached threads.
    //
    // std detaches a thread whenever its `JoinHandle` is dropped unjoined (`blocking`'s pool,
    // fire-and-forget `thread::spawn`): `pthread_detach`. devkitPro
    // newlib's `pthread_detach` calls `__syscall_thread_detach` if anything defines that weak
    // symbol and otherwise fails with ENOSYS (libsysbase.a, pthread.o), and libnx v4.12 doesn't
    // define it (newlib.c: "Unsupported"). So a detached thread that finished kept its kernel
    // thread and handle and its stack, mapped at a second address with the heap pages under it
    // locked, for as long as the process ran: 2 MiB per std thread, and a loader that can't read
    // the next NRO into the heap at exit (2001-0106). A thread can't unmap the stack it is
    // running on, so detaching records the thread here and it is closed, as `pthread_join`
    // would, once its handle signals that it has exited: when a thread is created or detached,
    // once a second (`lifecycle.rs`), and at exit.

    /// newlib.c's `THRD_MAIN_HANDLE`, what `pthread_self` returns on the main thread.
    const MAIN_THREAD: usize = !0;
    const DETACHED_SLOTS: usize = 256;
    /// libnx `struct __pthread_t` pointers (newlib.c: `{ Thread thr; void *rc; }`, from
    /// `__libnx_alloc`); 0 is a free slot. Only the closer, holding `CLOSING`, empties slots.
    static DETACHED: [AtomicUsize; DETACHED_SLOTS] = [const { AtomicUsize::new(0) }; DETACHED_SLOTS];
    static DETACHED_COUNT: AtomicUsize = AtomicUsize::new(0);
    static CLOSING: AtomicBool = AtomicBool::new(false);
    static DETACHED_CLOSED: AtomicU32 = AtomicU32::new(0);
    static DETACHED_CLOSE_FAILED: AtomicU32 = AtomicU32::new(0);
    /// Detached while all slots were full: these are never closed.
    static DETACHED_UNTRACKED: AtomicU32 = AtomicU32::new(0);
    /// `svcWaitSynchronization` takes at most 0x40 handles.
    const MAX_WAIT_HANDLES: usize = 0x40;

    unsafe extern "C" {
        fn threadClose(t: *mut c_void) -> u32;
        fn __libnx_free(p: *mut c_void);
        fn svcWaitSynchronization(index: *mut i32, handles: *const u32, count: i32, timeout_ns: u64) -> u32;
        fn svcSleepThread(nano: i64);
    }

    /// libnx's missing `pthread_detach` back end. It never fails: std `debug_assert`s that.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __syscall_thread_detach(thread: *mut c_void) -> c_int {
        let thread = thread as usize;
        if thread == 0 || thread == MAIN_THREAD {
            return 0;
        }
        let tracked = DETACHED
            .iter()
            .any(|slot| slot.compare_exchange(0, thread, Ordering::AcqRel, Ordering::Relaxed).is_ok());
        if tracked {
            DETACHED_COUNT.fetch_add(1, Ordering::AcqRel);
        } else {
            DETACHED_UNTRACKED.fetch_add(1, Ordering::Relaxed);
        }
        close_detached_threads(Duration::ZERO);
        0
    }

    /// What `close_detached_threads` did, with totals since launch.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct DetachedThreads {
        /// Closed by this call.
        pub closed: u32,
        /// Detached and still running when this call returned.
        pub running: u32,
        /// Threads created since launch, by anything.
        pub created_total: u32,
        /// Detached threads closed since launch, this call's included.
        pub closed_total: u32,
        /// Exited detached threads that `threadClose` failed to close, since launch.
        pub close_failed_total: u32,
        /// Detached while every slot was taken, so never closed.
        pub untracked_total: u32,
    }

    /// Closes every detached thread that has exited: unmaps its stack (unlocking those heap
    /// pages), closes its handle and frees it, as `pthread_join` does. With a non-zero `wait`,
    /// keeps waiting that long in all for the running ones to exit. Returns `None` if another
    /// thread was closing them and kept doing so for all of `wait`.
    pub fn close_detached_threads(wait: Duration) -> Option<DetachedThreads> {
        let deadline = (!wait.is_zero()).then(|| Instant::now() + wait);
        let mut result = DetachedThreads::default();
        if DETACHED_COUNT.load(Ordering::Acquire) != 0 {
            while CLOSING.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                match deadline {
                    Some(deadline) if Instant::now() < deadline => unsafe { svcSleepThread(1_000_000) },
                    _ => return None,
                }
            }
            loop {
                let mut handles = [0u32; MAX_WAIT_HANDLES];
                let mut count = 0;
                result.running = 0;
                for slot in &DETACHED {
                    let thread = slot.load(Ordering::Acquire);
                    if thread == 0 {
                        continue;
                    }
                    // `Thread.handle` is the first field of libnx's `Thread`, and `thr` of the
                    // pthread. A thread's handle is signalled once the thread has exited.
                    let handle = unsafe { (thread as *const u32).read() };
                    let mut index = 0;
                    if unsafe { svcWaitSynchronization(&mut index, &handle, 1, 0) } != 0 {
                        result.running += 1;
                        if count < MAX_WAIT_HANDLES {
                            handles[count] = handle;
                            count += 1;
                        }
                        continue;
                    }
                    slot.store(0, Ordering::Release);
                    DETACHED_COUNT.fetch_sub(1, Ordering::AcqRel);
                    // libnx's `threadExit` has cleared `tls_array`, so this succeeds unless the
                    // stack can't be unmapped; then the thread stays leaked, as before.
                    if unsafe { threadClose(thread as *mut c_void) } == 0 {
                        unsafe { __libnx_free(thread as *mut c_void) };
                        result.closed += 1;
                    } else {
                        DETACHED_CLOSE_FAILED.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let left = deadline.map_or(Duration::ZERO, |d| d.saturating_duration_since(Instant::now()));
                if count == 0 || left.is_zero() {
                    break;
                }
                // Sleeps until one of them exits (0), the time is up, or a handle is bad.
                let mut index = 0;
                let nanos = u64::try_from(left.as_nanos()).unwrap_or(u64::MAX);
                if unsafe { svcWaitSynchronization(&mut index, handles.as_ptr(), count as i32, nanos) } != 0 {
                    break;
                }
            }
            DETACHED_CLOSED.fetch_add(result.closed, Ordering::Relaxed);
            CLOSING.store(false, Ordering::Release);
        }
        result.created_total = THREADS_CREATED.load(Ordering::Relaxed);
        result.closed_total = DETACHED_CLOSED.load(Ordering::Relaxed);
        result.close_failed_total = DETACHED_CLOSE_FAILED.load(Ordering::Relaxed);
        result.untracked_total = DETACHED_UNTRACKED.load(Ordering::Relaxed);
        Some(result)
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn _init() {}

    #[unsafe(no_mangle)]
    pub extern "C" fn _fini() {}

    // newlib <sys/unistd.h>
    const _SC_PAGESIZE: c_int = 8;
    const _SC_NPROCESSORS_CONF: c_int = 9;
    const _SC_NPROCESSORS_ONLN: c_int = 10;

    #[unsafe(no_mangle)]
    pub extern "C" fn sysconf(name: c_int) -> c_long {
        match name {
            _SC_PAGESIZE => 0x1000,
            // Applications may run on cores 0-2; core 3 is reserved for the system.
            _SC_NPROCESSORS_CONF | _SC_NPROCESSORS_ONLN => 3,
            _ => -1,
        }
    }

    // newlib <sys/errno.h>
    const EFAULT: c_int = 14;

    fn set_errno(value: c_int) {
        unsafe extern "C" {
            fn __errno() -> *mut c_int;
        }
        unsafe { *__errno() = value };
    }

    unsafe extern "C" {
        // libnx kernel/random.c: ChaCha seeded from svcGetInfo(InfoType_RandomEntropy). No service
        // session, so it works before any service is initialized and after userAppExit closes them.
        fn randomGet(buf: *mut c_void, len: usize);
    }

    // std's hashmap_random_keys calls this. It used to call csrngGetRandomBytes without
    // csrngInitialize, so the IPC went to handle 0 and failed, the result was ignored and every
    // HashMap got the keys (0, 0). Found in Ryujinx's trace log; see docs/platform-notes.md.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn getrandom(buf: *mut u8, buflen: usize, _flags: u32) -> isize {
        if buflen == 0 {
            return 0;
        }
        if buf.is_null() {
            set_errno(EFAULT);
            return -1;
        }
        unsafe { randomGet(buf.cast(), buflen) };
        buflen as isize
    }

    // newlib's getentropy calls this and arc4random calls getentropy. Nothing in devkitA64's newlib
    // or libnx defines it, so anything using arc4random_buf (the patched getrandom crate) failed to
    // link with an undefined reference to _getentropy_r.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn _getentropy_r(reent: *mut c_int, buf: *mut c_void, buflen: usize) -> c_int {
        if buflen == 0 {
            return 0;
        }
        if buf.is_null() {
            // struct _reent starts with int _errno.
            if !reent.is_null() {
                unsafe { *reent = EFAULT };
            }
            return -1;
        }
        unsafe { randomGet(buf, buflen) };
        0
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub union CpuRegister {
        pub x: u64,
        pub w: u32,
        pub r: u32,
    }

    /// libnx `ThreadExceptionDump` (switch/arm/thread_context.h).
    #[repr(C)]
    pub struct ThreadExceptionDump {
        pub error_desc: u32,
        pub pad: [u32; 3],
        pub cpu_gprs: [CpuRegister; 29],
        pub fp: CpuRegister,
        pub lr: CpuRegister,
        pub sp: CpuRegister,
        pub pc: CpuRegister,
        pub padding: u64,
        pub fpu_gprs: [[u64; 2]; 32],
        pub pstate: u32,
        pub afsr0: u32,
        pub afsr1: u32,
        pub esr: u32,
        pub far: CpuRegister,
    }

    const EXCEPTION_STACK_SIZE: usize = 0x10000;

    #[repr(C, align(16))]
    pub struct ExceptionStack([u8; EXCEPTION_STACK_SIZE]);

    // libnx runs __libnx_exception_handler on this stack. libnx's weak default is only 0x400
    // bytes, which the report buffer and file I/O below would overflow into the register dump.
    #[allow(non_upper_case_globals)]
    #[unsafe(no_mangle)]
    pub static mut __nx_exception_stack: ExceptionStack =
        ExceptionStack([0; EXCEPTION_STACK_SIZE]);

    #[allow(non_upper_case_globals)]
    #[unsafe(no_mangle)]
    pub static __nx_exception_stack_size: u64 = EXCEPTION_STACK_SIZE as u64;

    unsafe extern "C" {
        fn svcBreak(break_reason: u32, address: usize, size: usize) -> u32;
    }

    /// Writes a fatal report to the debug output, then `log.txt`, then its own file.
    /// `log.txt` goes first because it is already open: no malloc, which may be what broke.
    fn record_fatal(file: &str, report: &[u8]) {
        unsafe { ffi::svcOutputDebugString(report.as_ptr().cast(), report.len()) };
        if write_log(report) {
            sync_log();
        }
        append_crash_file(file, report);
    }

    /// Replaces libnx's `diagAbortWithResult`, which libnx calls on fatal errors such as a
    /// service failing to start in `__appInit`. Records the result, then breaks the same way
    /// libnx does, so Atmosphère still writes a crash report.
    #[unsafe(no_mangle)]
    pub extern "C" fn diagAbortWithResult(res: u32) -> ! {
        let module = res & 0x1ff;
        let description = (res >> 9) & 0x1fff;
        let mut out = FixedBuf::<512>::new();
        let _ = write!(
            out,
            "\n==================== LIBNX ABORT ====================\n\
             result:   {res:#x} = error {:04}-{description:04} (module {module}, description {description})\n\
             nro base: {:#x}\n\
             diagAbortWithResult -> svcBreak; Atmosphère writes a crash report with this result.\n\
             =====================================================\n",
            2000 + module,
            nro_base(),
        );
        record_fatal("abort.log", out.as_bytes());
        unsafe {
            svcBreak(0, core::ptr::addr_of!(res) as usize, 4);
            core::hint::unreachable_unchecked()
        }
    }

    /// Overrides libnx's weak `__libnx_exception_handler`, which is called for CPU exceptions
    /// (bad memory access, bad jump, ...) when no debugger is attached. When it returns, libnx
    /// calls `svcBreak`, so Atmosphère still writes its crash report afterwards.
    #[unsafe(no_mangle)]
    pub extern "C" fn __libnx_exception_handler(ctx: *mut ThreadExceptionDump) {
        let Some(dump) = (unsafe { ctx.as_ref() }) else {
            return;
        };
        let image = Image { base: nro_base(), end: nro_end() };
        let (pc, lr, sp, fp, far) =
            unsafe { (dump.pc.x, dump.lr.x, dump.sp.x, dump.fp.x, dump.far.x) };
        let ec = dump.esr >> 26;
        // The handler runs on the faulting thread, so these are that thread's registers.
        let (tpidr, tls_tp) = thread_pointers();
        let mut out = FixedBuf::<6144>::new();

        let _ = writeln!(out, "\n==================== CPU EXCEPTION ====================");
        let _ = writeln!(out, "kind:     {:#x} {}", dump.error_desc, error_desc_name(dump.error_desc));
        let _ = writeln!(out, "esr:      {:#010x} (EC {ec:#04x}: {})", dump.esr, ec_name(ec));
        let _ = writeln!(out, "far:      {far:#018x} (address being accessed, for aborts)");
        let _ = writeln!(out, "nro:      {:#018x}-{:#018x} (nro+X = address X in the ELF)", image.base, image.end);
        let _ = writeln!(out, "pc:       {}", image.addr(pc));
        let _ = writeln!(out, "lr:       {}", image.addr(lr));
        let _ = writeln!(out, "sp:       {sp:#018x}");
        let _ = writeln!(out, "fp:       {fp:#018x}");
        let _ = writeln!(
            out,
            "tpidr_el0: {tpidr:#018x} (Rust thread-local base; libnx tls_tp {tls_tp:#018x}{})",
            if tpidr == tls_tp { ", ok" } else { ", MISMATCH: thread_local! access faults" },
        );
        for (i, reg) in dump.cpu_gprs.iter().enumerate() {
            let sep = if i % 4 == 3 { "\n" } else { "  " };
            let _ = write!(out, "x{i:<2} {:#018x}{sep}", unsafe { reg.x });
        }

        let _ = writeln!(out, "\nbacktrace (newest first; return addresses point just past each call):");
        let _ = writeln!(out, "  #0  {}", image.addr(pc));
        let mut frame = fp;
        for depth in 1..32 {
            if frame == 0 || frame % 8 != 0 || !is_readable(frame, 16) {
                break;
            }
            let record = frame as *const u64;
            let (next, ret) = unsafe { (record.read(), record.add(1).read()) };
            if ret == 0 {
                break;
            }
            let _ = writeln!(out, "  #{depth:<2} {}", image.addr(ret));
            if next <= frame {
                break;
            }
            frame = next;
        }
        let _ = writeln!(out, "=======================================================");

        record_fatal("crash.log", out.as_bytes());
    }

    struct Image {
        base: u64,
        end: u64,
    }

    impl Image {
        fn addr(&self, addr: u64) -> Addr<'_> {
            Addr { image: self, addr }
        }
    }

    /// Prints an address, plus its ELF address when it falls inside the NRO.
    struct Addr<'a> {
        image: &'a Image,
        addr: u64,
    }

    impl fmt::Display for Addr<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:#018x}", self.addr)?;
            if (self.image.base..self.image.end).contains(&self.addr) {
                write!(f, " (nro+{:#x})", self.addr - self.image.base)?;
            }
            Ok(())
        }
    }

    /// Asks the kernel whether `len` bytes at `addr` are mapped readable, so following a
    /// corrupted frame chain can't fault inside the handler.
    fn is_readable(addr: u64, len: u64) -> bool {
        let mut info = ffi::MemoryInfo::default();
        let mut page_info = 0u32;
        let rc = unsafe { ffi::svcQueryMemory(&mut info, &mut page_info, addr) };
        rc == 0
            && info.perm & 1 != 0
            && addr.saturating_add(len) <= info.addr.saturating_add(info.size)
    }

    /// Kernel exception type: libnx `ThreadExceptionDesc` (switch/arm/thread_context.h), with
    /// the grouping Atmosphère's kernel actually uses (kern_exception_handlers.cpp).
    fn error_desc_name(desc: u32) -> &'static str {
        match desc {
            0x100 => "InstructionAbort (on Atmosphère also BRK traps and undefined instructions)",
            0x101 => "DataAbort (libnx calls this Other)",
            0x102 => "MisalignedPC",
            0x103 => "MisalignedSP or unaligned data access",
            0x104 => "Trap",
            0x106 => "SError (memory system error)",
            0x301 => "BadSVC",
            _ => "unknown",
        }
    }

    /// ARMv8 `ESR_ELx.EC`, the exception class.
    fn ec_name(ec: u32) -> &'static str {
        match ec {
            0x00 => "unknown reason / undefined instruction",
            0x0e => "illegal execution state",
            0x15 => "SVC instruction",
            0x20 | 0x21 => "instruction abort: jumped to a bad address",
            0x22 => "PC alignment fault",
            0x24 | 0x25 => "data abort: bad memory access, e.g. null or dangling pointer",
            0x26 => "SP alignment fault",
            0x3c => "BRK instruction (Rust trap)",
            _ => "see the ARM ESR_ELx.EC table",
        }
    }
}

/// Stands in for a Vulkan driver in apps built without bevy_horizon's `vulkan` feature, which
/// links NVK. Horizon has no dynamic loader, so wgpu's Vulkan backend there (`patches/wgpu-hal`)
/// calls `vkGetInstanceProcAddr` directly, and every app with bevy_render references it.
/// Returning null for every name makes wgpu report that no Vulkan driver is linked.
#[cfg(all(target_os = "horizon", not(feature = "vulkan")))]
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub extern "C" fn vkGetInstanceProcAddr(
    _instance: *mut core::ffi::c_void,
    _name: *const core::ffi::c_char,
) -> *const core::ffi::c_void {
    core::ptr::null()
}
