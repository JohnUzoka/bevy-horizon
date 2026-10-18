//! A sampling CPU profiler: where each thread of the game spends its time on the Switch.
//!
//! A thread at a higher priority than the game's (0x1E) wakes every `interval` and, for every
//! thread of the process, pauses it (`svcSetThreadActivity`), reads its registers
//! (`svcGetThreadContext3`) and walks its frame records for the call chain, then lets it run
//! again. A thread paused in a system call (its PC on or just after an `svc`) is waiting: on a
//! lock, a condition variable, the GPU or the SD card. The kernel's count of each thread's CPU
//! time (`svcGetInfo(ThreadTickCount)`) gives how busy it really was.
//!
//! The technique is UnleashedRecomp-NX's (github.com/ChanseyIsTheBest/UnleashedRecomp-NX, branch
//! switch-1.0.0, `cpu_profiler_switch.cpp`; GPL-3.0). This is a separate implementation and
//! copies none of its code.
//!
//! `BEVY_HORIZON_CPU_PROFILE=1` turns it on, sampling every 2 ms (or `=<microseconds>`, at least
//! 500); `HorizonPlugin` starts it. Every `BEVY_HORIZON_CPU_PROFILE_EVERY` seconds (30 unless
//! set, 0 for never) it rewrites `cpu-profile.txt` beside `log.txt`; `write_report_to` writes the
//! same report on demand, and `reset` starts the counts again. The report holds offsets into the
//! NRO; `scripts/symbolize_cpu_profile.py` names them from the game's ELF.
//!
//! Each pass pauses each thread for a few microseconds, and the sampler takes that time on one of
//! cores 0-2, so a profiled game runs slightly slower than it otherwise would.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasherDefault;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::logging::{ffi, nro_base, nro_end, write_log_line};

/// Threads the profiler can follow at once.
const MAX_THREADS: usize = 64;
/// A slot being filled in by `register_thread`.
const CLAIMING: u32 = u32::MAX;

const CUR_THREAD_HANDLE: u32 = 0xFFFF_8000;
/// `InfoType_ThreadTickCount`: a thread's CPU time, in system ticks. Its id is 25 from 13.0.0;
/// 1.0.0-12.1.0 have only `InfoType_ThreadTickCountDeprecated`, 0xF0000002, which later kernels
/// reject (libnx switch/kernel/svc.h).
const INFO_THREAD_TICK_COUNT: u32 = 25;
const INFO_THREAD_TICK_COUNT_DEPRECATED: u32 = 0xF000_0002;
const ACTIVITY_RUNNABLE: u32 = 0;
const ACTIVITY_PAUSED: u32 = 1;
/// Kernel results (module 1): InvalidHandle (114), InvalidState (125), TerminationRequested (59).
const RESULT_INVALID_HANDLE: u32 = 0xE401;
const RESULT_INVALID_STATE: u32 = 0xFA01;
const RESULT_TERMINATION_REQUESTED: u32 = 0x7601;
/// Above every thread of the game (pthreads 0x3B, the main thread 0x2C) and within the 0x1C-0x3B
/// the homebrew loader lets an NRO use.
const SAMPLER_PRIORITY: u32 = 0x1E;
/// The system tick (`CNTPCT_EL0`) and thread CPU times count at 19.2 MHz.
const TICKS_PER_MS: f64 = 19_200.0;

const DEFAULT_INTERVAL_US: u64 = 2_000;
const MIN_INTERVAL_US: u64 = 500;
const DEFAULT_REPORT_EVERY_S: u64 = 30;

/// Return addresses kept from the leaf end of a chain, and from the outer end (the thread's entry)
/// when a chain is longer than fits.
const TOP_FRAMES: usize = 22;
const BOTTOM_FRAMES: usize = 6;
/// `[flags, pc, lr, returns...]`: TOP_FRAMES, a GAP, BOTTOM_FRAMES.
const CHAIN_LEN: usize = 3 + TOP_FRAMES + 1 + BOTTOM_FRAMES;
/// Frame records followed at most.
const MAX_WALK: usize = 256;
/// Without a frame record, the stack above SP is scanned this far for return addresses.
const SCAN_BYTES: u64 = 2048;
const SCAN_FRAMES: usize = 8;
/// Distinct chains kept, over all threads; past it, a new chain is kept as its PC alone.
const MAX_CHAINS: usize = 40_000;

/// Chain flags (element 0).
const FLAG_WAITING: u32 = 1;
const FLAG_LR: u32 = 2;
const FLAG_SCANNED: u32 = 4;
const FLAG_CAPPED: u32 = 8;
/// Frames left out between the leaf end and the outer end of a chain.
const GAP: u32 = 0xFFFF_FFFE;
/// An address outside the NRO.
const OUTSIDE: u32 = 0xFFFF_FFFF;

unsafe extern "C" {
    fn svcSetThreadActivity(handle: u32, activity: u32) -> u32;
    fn svcGetThreadContext3(context: *mut ThreadContext, handle: u32) -> u32;
    fn svcSetThreadPriority(handle: u32, priority: u32) -> u32;
    fn svcGetInfo(out: *mut u64, id0: u32, handle: u32, id1: u64) -> u32;
    fn svcSleepThread(nano: i64);
    fn svcWaitSynchronization(index: *mut i32, handles: *const u32, count: i32, timeout: u64) -> u32;
    fn threadGetCurHandle() -> u32;
    fn envGetMainThreadHandle() -> u32;
}

/// libnx `ThreadContext` (switch/kernel/svc.h), 0x320 bytes. For a thread in a system call the
/// kernel fills in only x19-x28 of the general registers, but always `fp`, `lr`, `sp` and `pc`.
#[repr(C, align(16))]
#[allow(dead_code)]
struct ThreadContext {
    gprs: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    psr: u32,
    _padding: u32,
    fpu: [u64; 64],
    fpcr: u32,
    fpsr: u32,
    tpidr: u64,
}
const _: () = assert!(core::mem::size_of::<ThreadContext>() == 0x320);

// ---- The threads to sample. `compat.rs` adds every thread `threadCreate` makes, whether or not
// the profiler runs; `start` adds the main thread. Allocation- and lock-free: it runs inside
// `pthread_create`.

struct Slot {
    /// 0 for a free slot.
    handle: AtomicU32,
    /// `[Thread] #N` in `log.txt`; 0 for the main thread.
    number: AtomicU32,
}

static SLOTS: [Slot; MAX_THREADS] =
    [const { Slot { handle: AtomicU32::new(0), number: AtomicU32::new(0) } }; MAX_THREADS];
static UNTRACKED_THREADS: AtomicU32 = AtomicU32::new(0);

/// Adds a thread to sample. Kernel handles aren't reused soon (each carries a counter), so a
/// handle names one thread.
pub(crate) fn register_thread(handle: u32, number: u32) {
    if handle == 0 || handle == CLAIMING {
        return;
    }
    if SLOTS.iter().any(|slot| slot.handle.load(Ordering::Relaxed) == handle) {
        return;
    }
    for slot in &SLOTS {
        if slot.handle.compare_exchange(0, CLAIMING, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            slot.number.store(number, Ordering::Relaxed);
            slot.handle.store(handle, Ordering::Release);
            return;
        }
    }
    UNTRACKED_THREADS.fetch_add(1, Ordering::Relaxed);
}

/// Called by a thread as it ends. A thread that exits some other way stays until its handle is
/// closed, when the sampler drops it.
pub(crate) fn unregister_current_thread() {
    let handle = unsafe { threadGetCurHandle() };
    for slot in &SLOTS {
        let _ = slot.handle.compare_exchange(handle, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}

// ---- The counts.

type Deterministic = BuildHasherDefault<DefaultHasher>;

struct ThreadProfile {
    number: u32,
    /// System ticks of the first and latest pass that sampled it.
    first_seen: u64,
    last_seen: u64,
    /// Its CPU time then.
    first_cpu: u64,
    last_cpu: u64,
    samples: u64,
    waiting: u64,
    chains: HashMap<Box<[u32]>, u32, Deterministic>,
}

struct Profile {
    started: u64,
    passes: u64,
    /// Time the sampler spent in its passes.
    sampler_ticks: u64,
    pause_failures: u64,
    resume_failures: u64,
    /// Threads that exited while paused for a sample, so had nothing to resume.
    exited_while_paused: u64,
    chains: usize,
    capped: u64,
    /// By thread handle.
    threads: BTreeMap<u32, ThreadProfile>,
}

impl Profile {
    fn new() -> Self {
        Self {
            started: system_tick(),
            passes: 0,
            sampler_ticks: 0,
            pause_failures: 0,
            resume_failures: 0,
            exited_while_paused: 0,
            chains: 0,
            capped: 0,
            threads: BTreeMap::new(),
        }
    }

    fn add(&mut self, sample: &Sample, now: u64) {
        let thread = self.threads.entry(sample.handle).or_insert_with(|| ThreadProfile {
            number: sample.number,
            first_seen: now,
            last_seen: now,
            first_cpu: sample.cpu,
            last_cpu: sample.cpu,
            samples: 0,
            waiting: 0,
            chains: HashMap::default(),
        });
        thread.last_seen = now;
        thread.last_cpu = sample.cpu;
        thread.samples += 1;
        let chain = &sample.chain[..sample.len];
        if chain[0] & FLAG_WAITING != 0 {
            thread.waiting += 1;
        }
        if let Some(count) = thread.chains.get_mut(chain) {
            *count += 1;
        } else if self.chains < MAX_CHAINS {
            thread.chains.insert(chain.into(), 1);
            self.chains += 1;
        } else {
            self.capped += 1;
            let short = [chain[0] & FLAG_WAITING | FLAG_CAPPED, chain[1]];
            *thread.chains.entry(short[..].into()).or_insert(0) += 1;
        }
    }
}

static PROFILE: Mutex<Option<Profile>> = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static INTERVAL_US: AtomicU64 = AtomicU64::new(0);

fn system_tick() -> u64 {
    let tick: u64;
    unsafe { asm!("mrs {}, cntpct_el0", out(reg) tick, options(nomem, nostack, preserves_flags)) };
    tick
}

/// Starts the profiler if `BEVY_HORIZON_CPU_PROFILE` asks for it. `HorizonPlugin` calls this.
pub fn start_from_env() {
    let Ok(value) = std::env::var("BEVY_HORIZON_CPU_PROFILE") else {
        return;
    };
    let interval_us = match value.trim() {
        "" | "0" => return,
        "1" => DEFAULT_INTERVAL_US,
        other => other.parse().unwrap_or(DEFAULT_INTERVAL_US).max(MIN_INTERVAL_US),
    };
    let every = std::env::var("BEVY_HORIZON_CPU_PROFILE_EVERY")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_REPORT_EVERY_S);
    start(Duration::from_micros(interval_us), (every > 0).then(|| Duration::from_secs(every)));
}

/// Starts sampling every thread every `interval`, rewriting `cpu-profile.txt` beside `log.txt`
/// every `report_every`. Returns false if it was already running or could not start.
pub fn start(interval: Duration, report_every: Option<Duration>) -> bool {
    if STARTED.swap(true, Ordering::SeqCst) {
        return false;
    }
    register_thread(unsafe { envGetMainThreadHandle() }, 0);
    let interval = interval.max(Duration::from_micros(MIN_INTERVAL_US));
    INTERVAL_US.store(interval.as_micros() as u64, Ordering::Relaxed);
    *PROFILE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Profile::new());
    RUNNING.store(true, Ordering::SeqCst);
    let spawned = std::thread::Builder::new()
        .name("bevy_horizon cpu profiler".into())
        .stack_size(256 * 1024)
        .spawn(move || sample_threads(interval, report_every));
    if let Err(error) = spawned {
        RUNNING.store(false, Ordering::SeqCst);
        write_log_line(format_args!("[Profiler] could not start its thread: {error}"));
        return false;
    }
    crate::lifecycle::register_release_hook(stop_for_exit);
    write_log_line(format_args!(
        "[Profiler] sampling every thread every {} us{}",
        interval.as_micros(),
        match report_every {
            Some(every) => format!("; cpu-profile.txt every {} s", every.as_secs()),
            None => String::new(),
        }
    ));
    true
}

/// Whether the profiler is sampling.
pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Forgets every sample so far, so the next report covers only what follows.
pub fn reset() {
    if let Some(profile) = PROFILE.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        *profile = Profile::new();
    }
}

/// Writes the report to `path`. Fails if the profiler was never started.
pub fn write_report_to(path: impl AsRef<Path>) -> io::Result<()> {
    let guard = PROFILE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(profile) = guard.as_ref() else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the CPU profiler is not running"));
    };
    let mut out = BufWriter::with_capacity(64 * 1024, std::fs::File::create(path)?);
    write_profile(profile, &mut out)?;
    out.flush()
}

/// Release hook: stops the sampler before the loader takes the heap back, so its thread can be
/// closed. Allocates nothing.
fn stop_for_exit() {
    STOP.store(true, Ordering::SeqCst);
    for _ in 0..250 {
        if !RUNNING.load(Ordering::SeqCst) {
            return;
        }
        unsafe { svcSleepThread(2_000_000) };
    }
}

// ---- Sampling.

#[derive(Clone, Copy)]
struct Sample {
    handle: u32,
    number: u32,
    cpu: u64,
    len: usize,
    chain: [u32; CHAIN_LEN],
}

/// The NRO's code, to tell return addresses from other values on a stack.
struct Code {
    base: u64,
    start: u64,
    end: u64,
}

impl Code {
    fn find() -> Self {
        let base = nro_base();
        let mut info = ffi::MemoryInfo::default();
        let mut page = 0u32;
        // The text segment is the first mapping of the NRO, read and execute only.
        let rc = unsafe { ffi::svcQueryMemory(&mut info, &mut page, base) };
        let end = if rc == 0 && info.perm == 5 && info.addr <= base { info.addr + info.size } else { nro_end() };
        Self { base, start: base, end }
    }

    fn instruction(&self, address: u64) -> Option<u32> {
        (address & 3 == 0 && address >= self.start && address < self.end)
            .then(|| unsafe { core::ptr::read_volatile(address as *const u32) })
    }

    /// Paused in a system call: the kernel reports a thread blocked in one with its PC on the
    /// `svc`, and one on its way out just after it.
    fn in_system_call(&self, pc: u64) -> bool {
        let svc = |insn: u32| insn & 0xFFE0_001F == 0xD400_0001;
        self.instruction(pc).is_some_and(svc) || self.instruction(pc.wrapping_sub(4)).is_some_and(svc)
    }

    /// In the code, just after a BL or BLR.
    fn is_return_address(&self, address: u64) -> bool {
        self.instruction(address.wrapping_sub(4))
            .is_some_and(|insn| insn & 0xFC00_0000 == 0x9400_0000 || insn & 0xFFFF_FC1F == 0xD63F_0000)
    }

    fn offset(&self, address: u64) -> u32 {
        match address.checked_sub(self.base) {
            Some(offset) if offset < u64::from(GAP) => offset as u32,
            _ => OUTSIDE,
        }
    }
}

/// Reads one word of a paused thread's stack, within the mapping that holds it.
fn stack_word(stack: (u64, u64), address: u64) -> Option<u64> {
    (address & 7 == 0 && address >= stack.0 && address.checked_add(8).is_some_and(|end| end <= stack.1))
        .then(|| unsafe { core::ptr::read_volatile(address as *const u64) })
}

/// Fills `chain` from a paused thread's registers and stack; returns its length. Only reads
/// memory and makes no calls that could wait on the paused thread.
fn walk(
    code: &Code,
    context: &ThreadContext,
    stack: Option<(u64, u64)>,
    returns: &mut [u64; MAX_WALK],
    chain: &mut [u32; CHAIN_LEN],
) -> usize {
    let mut flags = 0;
    if code.in_system_call(context.pc) {
        flags |= FLAG_WAITING;
    }
    chain[1] = code.offset(context.pc);
    let mut len = 2;
    // Right in a leaf, or before a function's prologue has saved it, LR is the caller.
    if code.is_return_address(context.lr) {
        chain[2] = code.offset(context.lr);
        len = 3;
        flags |= FLAG_LR;
    }
    let mut found = 0;
    if let Some(stack) = stack {
        // Frame records, `[fp]` the caller's record and `[fp + 8]` the return address: Rust code
        // keeps them (the target spec's `"frame-pointer": "always"`). Each must lie above the
        // last and hold a real return address.
        let mut fp = context.fp;
        let mut floor = context.sp;
        let mut records = 0;
        while found < MAX_WALK && fp >= floor {
            let (Some(next), Some(ret)) = (stack_word(stack, fp), stack_word(stack, fp.wrapping_add(8)))
            else {
                break;
            };
            if !code.is_return_address(ret) {
                break;
            }
            records += 1;
            floor = fp + 16;
            fp = next;
            // After the prologue, the function's own record holds the LR already taken.
            if records == 1 && len == 3 && ret == context.lr {
                continue;
            }
            returns[found] = ret;
            found += 1;
        }
        // No frame record: C built without frame pointers. The return addresses still on the
        // stack, nearest first; one may be left over from an earlier call.
        if records == 0 {
            let mut address = context.sp.max(stack.0).next_multiple_of(8);
            let end = stack.1.min(address.saturating_add(SCAN_BYTES));
            while address < end && found < SCAN_FRAMES {
                if let Some(word) = stack_word(stack, address).filter(|&word| code.is_return_address(word)) {
                    returns[found] = word;
                    found += 1;
                }
                address += 8;
            }
            if found > 0 {
                flags |= FLAG_SCANNED;
            }
        }
    }
    let room = CHAIN_LEN - len;
    if found <= room {
        for &ret in &returns[..found] {
            chain[len] = code.offset(ret);
            len += 1;
        }
    } else {
        let top = room - 1 - BOTTOM_FRAMES;
        for &ret in &returns[..top] {
            chain[len] = code.offset(ret);
            len += 1;
        }
        chain[len] = GAP;
        len += 1;
        for &ret in &returns[found - BOTTOM_FRAMES..found] {
            chain[len] = code.offset(ret);
            len += 1;
        }
    }
    chain[0] = flags;
    len
}

/// The stack mapping around `sp`, from the cache if it is still the same thread's.
fn stack_bounds(cache: &mut (u32, u64, u64), handle: u32, sp: u64) -> Option<(u64, u64)> {
    if cache.0 != handle || sp < cache.1 || sp >= cache.2 {
        let mut info = ffi::MemoryInfo::default();
        let mut page = 0u32;
        if unsafe { ffi::svcQueryMemory(&mut info, &mut page, sp) } != 0 || info.perm & 1 == 0 {
            return None;
        }
        *cache = (handle, info.addr, info.addr + info.size);
    }
    Some((cache.1, cache.2))
}

/// The `svcGetInfo` id the last CPU time was read with, 0 if none has been: the report says
/// whether its CPU times are real.
static CPU_TIME_INFO: AtomicU32 = AtomicU32::new(0);

/// A thread's CPU time in ticks, or `None` once its handle is closed. `info` is the id to ask
/// with: the pre-13.0.0 one when the kernel rejects the current one, and 0, every time reading
/// as 0, when it rejects both. Called with no thread paused, so it may log.
fn thread_cpu_ticks(info: &mut u32, handle: u32) -> Option<u64> {
    while *info != 0 {
        let mut ticks = 0;
        match unsafe { svcGetInfo(&mut ticks, *info, handle, u64::MAX) } {
            0 => {
                CPU_TIME_INFO.store(*info, Ordering::Relaxed);
                return Some(ticks);
            }
            RESULT_INVALID_HANDLE => return None,
            rc => {
                let next = if *info == INFO_THREAD_TICK_COUNT { INFO_THREAD_TICK_COUNT_DEPRECATED } else { 0 };
                write_log_line(format_args!(
                    "[Profiler] svcGetInfo({:#x}) for a thread's CPU time failed: {rc:#x}; {}",
                    *info,
                    if next == 0 { "the report's CPU times will read 0".to_string() } else { format!("trying {next:#x}") },
                ));
                *info = next;
            }
        }
    }
    Some(0)
}

/// Whether a thread has exited. Its handle is signalled then (libnx's `threadWaitForExit` waits
/// for that), so a wait that doesn't wait succeeds at once.
fn has_exited(handle: u32) -> bool {
    let mut index = 0;
    unsafe { svcWaitSynchronization(&mut index, &handle, 1, 0) == 0 }
}

fn sample_threads(interval: Duration, report_every: Option<Duration>) {
    let rc = unsafe { svcSetThreadPriority(CUR_THREAD_HANDLE, SAMPLER_PRIORITY) };
    if rc != 0 {
        write_log_line(format_args!("[Profiler] svcSetThreadPriority({SAMPLER_PRIORITY:#x}) failed: {rc:#x}"));
    }
    let me = unsafe { threadGetCurHandle() };
    let code = Code::find();
    let mut context: Box<ThreadContext> = Box::new(unsafe { core::mem::zeroed() });
    let mut returns = Box::new([0u64; MAX_WALK]);
    let mut stacks = [(0u32, 0u64, 0u64); MAX_THREADS];
    let mut batch: Vec<Sample> = Vec::with_capacity(MAX_THREADS);
    let mut next_report = report_every.map(|every| Instant::now() + every);
    let mut tick_info = INFO_THREAD_TICK_COUNT;
    let mut logged_pause_failure = false;
    // By slot: a thread left paused because its resume failed. Each pass tries it again.
    let mut stuck = [0u32; MAX_THREADS];

    while !STOP.load(Ordering::Relaxed) {
        std::thread::sleep(interval);
        let pass_start = system_tick();
        let (mut pause_failures, mut resume_failures, mut exited) = (0, 0, 0);
        let (mut failed_resume, mut unstuck) = (None, None);
        batch.clear();
        for (index, slot) in SLOTS.iter().enumerate() {
            let handle = slot.handle.load(Ordering::Acquire);
            if handle == 0 || handle == CLAIMING || handle == me {
                continue;
            }
            let number = slot.number.load(Ordering::Relaxed);
            let Some(cpu) = thread_cpu_ticks(&mut tick_info, handle) else {
                // Closed: joined, or detached and closed after exiting.
                let _ = slot.handle.compare_exchange(handle, 0, Ordering::AcqRel, Ordering::Relaxed);
                stuck[index] = 0;
                continue;
            };
            let rc = unsafe { svcSetThreadActivity(handle, ACTIVITY_PAUSED) };
            if rc != 0 {
                match rc {
                    // Closed since the CPU time was read.
                    RESULT_INVALID_HANDLE => {
                        let _ = slot.handle.compare_exchange(handle, 0, Ordering::AcqRel, Ordering::Relaxed);
                        stuck[index] = 0;
                    }
                    // An exited thread whose handle is still open can't be paused, and nor can
                    // one still paused because its resume failed: that one is resumed now.
                    RESULT_INVALID_STATE | RESULT_TERMINATION_REQUESTED => {
                        if stuck[index] == handle {
                            if unsafe { svcSetThreadActivity(handle, ACTIVITY_RUNNABLE) } == 0 {
                                stuck[index] = 0;
                                unstuck = Some((number, "resumed on a later pass"));
                            } else if has_exited(handle) {
                                stuck[index] = 0;
                                unstuck = Some((number, "has exited since"));
                            }
                        }
                    }
                    _ => {
                        pause_failures += 1;
                        if !std::mem::replace(&mut logged_pause_failure, true) {
                            // No thread is paused here, so logging can't wait on one.
                            write_log_line(format_args!(
                                "[Profiler] svcSetThreadActivity(#{number}) failed: {rc:#x}; \
                                 counted in pause_failures"
                            ));
                        }
                    }
                }
                continue;
            }
            // Paused: nothing here may allocate, lock or log, since the thread may hold the lock.
            let mut sample = Sample { handle, number, cpu, len: 0, chain: [0; CHAIN_LEN] };
            if unsafe { svcGetThreadContext3(&mut *context, handle) } == 0 {
                let stack = stack_bounds(&mut stacks[index], handle, context.sp);
                sample.len = walk(&code, &context, stack, &mut returns, &mut sample.chain);
            }
            let mut resume_rc = 0;
            for _ in 0..3 {
                resume_rc = unsafe { svcSetThreadActivity(handle, ACTIVITY_RUNNABLE) };
                if resume_rc == 0 {
                    break;
                }
            }
            if resume_rc == 0 {
                stuck[index] = 0;
            } else if has_exited(handle) {
                // It was already ending when it was paused, so there's nothing to resume.
                exited += 1;
            } else {
                resume_failures += 1;
                stuck[index] = handle;
                failed_resume = Some((number, resume_rc));
            }
            if sample.len > 0 {
                batch.push(sample);
            }
        }
        let pass_end = system_tick();
        {
            let mut guard = PROFILE.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(profile) = guard.as_mut() {
                profile.passes += 1;
                profile.sampler_ticks += pass_end - pass_start;
                profile.pause_failures += pause_failures;
                profile.resume_failures += resume_failures;
                profile.exited_while_paused += exited;
                for sample in &batch {
                    profile.add(sample, pass_start);
                }
            }
        }
        if let Some((number, rc)) = failed_resume {
            write_log_line(format_args!(
                "[Profiler] ERROR: {resume_failures} thread(s) could not be resumed after a \
                 sample (#{number}: {rc:#x}); the next pass tries again"
            ));
        }
        if let Some((number, what)) = unstuck {
            write_log_line(format_args!("[Profiler] thread #{number}, left paused, {what}"));
        }
        if let (Some(every), Some(due)) = (report_every, next_report)
            && Instant::now() >= due
        {
            next_report = Some(Instant::now() + every);
            if let Some(dir) = crate::logging::log_dir()
                && let Err(error) = write_report_to(format!("{dir}/cpu-profile.txt"))
            {
                write_log_line(format_args!("[Profiler] could not write cpu-profile.txt: {error}"));
            }
        }
    }
    RUNNING.store(false, Ordering::SeqCst);
}

// ---- The report: text, one `thread` line per thread followed by its chains, most samples
// first. A chain line is `c <samples> <flags> <pc> [<lr>] <return addresses...>`, in hex offsets
// from the NRO's start (= addresses in the ELF, which is linked at 0); `-` marks frames left out
// in the middle, `?` an address outside the NRO. Flags: 1 waiting in a system call, 2 the second
// address is LR, 4 return addresses found by scanning the stack, 8 kept as its PC alone.

fn write_profile(profile: &Profile, out: &mut impl Write) -> io::Result<()> {
    let ms = |ticks: u64| ticks as f64 / TICKS_PER_MS;
    let code = Code::find();
    writeln!(out, "# bevy_horizon CPU profile (bevy_horizon profiler.rs); name the offsets with")?;
    writeln!(out, "# bevy-horizon/scripts/symbolize_cpu_profile.py <report> <the game's ELF>")?;
    writeln!(out, "version 1")?;
    writeln!(out, "module_base {:#x}", code.base)?;
    writeln!(out, "text_size {:#x}", code.end - code.start)?;
    writeln!(out, "interval_us {}", INTERVAL_US.load(Ordering::Relaxed))?;
    writeln!(out, "window_ms {:.1}", ms(system_tick().saturating_sub(profile.started)))?;
    writeln!(out, "passes {}", profile.passes)?;
    writeln!(out, "sampler_ms {:.1}", ms(profile.sampler_ticks))?;
    writeln!(out, "pause_failures {}", profile.pause_failures)?;
    writeln!(out, "resume_failures {}", profile.resume_failures)?;
    writeln!(out, "exited_while_paused {}", profile.exited_while_paused)?;
    writeln!(out, "untracked_threads {}", UNTRACKED_THREADS.load(Ordering::Relaxed))?;
    // `none`: the kernel gave no CPU times, and every cpu_ms and busy_pct below reads 0.
    let cpu_times = CPU_TIME_INFO.load(Ordering::Relaxed);
    if cpu_times == 0 {
        writeln!(out, "cpu_times none")?;
    } else {
        writeln!(out, "cpu_times {cpu_times:#x}")?;
    }
    writeln!(out, "chains {}", profile.chains)?;
    writeln!(out, "capped {}", profile.capped)?;
    let mut threads: Vec<(&u32, &ThreadProfile)> = profile.threads.iter().collect();
    threads.sort_by_key(|(_, thread)| thread.number);
    for (handle, thread) in threads {
        let seen = thread.last_seen - thread.first_seen;
        let cpu = thread.last_cpu.saturating_sub(thread.first_cpu);
        let busy = if seen > 0 { 100.0 * cpu as f64 / seen as f64 } else { 0.0 };
        if thread.number == 0 {
            write!(out, "thread main")?;
        } else {
            write!(out, "thread #{}", thread.number)?;
        }
        writeln!(
            out,
            " handle={handle:#x} samples={} waiting={} cpu_ms={:.1} seen_ms={:.1} busy_pct={busy:.1}",
            thread.samples,
            thread.waiting,
            ms(cpu),
            ms(seen),
        )?;
        let mut chains: Vec<(&Box<[u32]>, &u32)> = thread.chains.iter().collect();
        chains.sort_unstable_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (chain, count) in chains {
            write!(out, "c {count} {:x}", chain[0])?;
            for &offset in &chain[1..] {
                match offset {
                    GAP => write!(out, " -")?,
                    OUTSIDE => write!(out, " ?")?,
                    offset => write!(out, " {offset:x}")?,
                }
            }
            writeln!(out)?;
        }
    }
    writeln!(out, "end")
}
