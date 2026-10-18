//! std threads on Horizon, and which core policy lets Bevy's task pools actually run in parallel.

use std::cell::Cell;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use bevy_horizon::ThreadCorePolicy;

use crate::nx;
use crate::platform::{calibrate, spin};
use crate::report::{self, check, check_warn, fail, info, warn};

static DROPPED: AtomicU32 = AtomicU32::new(0);

struct DropCounter;

impl Drop for DropCounter {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}

thread_local! {
    static TLS_VALUE: Cell<u32> = const { Cell::new(0) };
    static TLS_DROP: DropCounter = const { DropCounter };
    // Non-zero initial values go in .tdata, the TLS image libnx copies for every thread.
    static TLS_INIT_A: Cell<u64> = const { Cell::new(TLS_MAGIC_A) };
    static TLS_INIT_B: Cell<u32> = const { Cell::new(TLS_MAGIC_B) };
}

const TLS_MAGIC_A: u64 = 0x7A11_5EED_0000_00A1;
const TLS_MAGIC_B: u32 = 0x7A11_00B2;

unsafe extern "C" {
    /// Linker-script symbols: the TLS alignment (a QUAD in .tls.align) and the TLS image start.
    static __tls_align: u64;
    static __tdata_lma: u8;
}

/// Runs before anything registers a thread-local destructor. If the TLS segment doesn't start on
/// its own alignment, rust-lld and libnx disagree about where thread-locals live: every
/// initialized one reads its neighbour's bytes and the first destructor registration crashes.
pub fn tls_layout() {
    report::section("tls-layout", 10, || {
        let a = TLS_INIT_A.with(|v| v.get());
        let b = TLS_INIT_B.with(|v| v.get());
        let align = unsafe { std::ptr::read_volatile(&raw const __tls_align) }.max(1);
        let offset = &raw const __tdata_lma as u64 % align;
        let (tpidr, tls_tp) = bevy_horizon::compat::horizon_stubs::thread_pointers();
        check(
            "tls-layout",
            a == TLS_MAGIC_A && b == TLS_MAGIC_B && offset == 0 && tpidr == tls_tp,
            format!(
                "thread-local initial values {} (TLS image at +{offset:#x} past a {align}-byte boundary, want +0; TPIDR_EL0 {} libnx tls_tp)",
                if a == TLS_MAGIC_A && b == TLS_MAGIC_B {
                    "read back correctly"
                } else {
                    "read back WRONG"
                },
                if tpidr == tls_tp { "matches" } else { "DIFFERS FROM" },
            ),
        );
    });
}

#[inline(never)]
fn recurse(n: u32) -> u64 {
    let buf = black_box([n as u8; 4096]);
    if n == 0 {
        return buf[7] as u64;
    }
    recurse(n - 1) + buf[4095] as u64
}

pub fn threads() {
    report::heading("Threads");
    report::section("thread-parallelism", 10, || {
        let bevy = bevy::tasks::available_parallelism();
        match thread::available_parallelism() {
            Ok(n) => info(
                "thread-parallelism",
                format!("available_parallelism() = {n}, Bevy counts {bevy}"),
            ),
            Err(e) => {
                check(
                    "thread-parallelism",
                    bevy > 1,
                    format!(
                        "std's available_parallelism() fails ({e}); bevy_tasks counts {bevy} cores from the process core mask instead. Crates that ask std, like rayon, still get 1 thread unless configured"
                    ),
                );
            }
        }
    });
    report::section("thread-tls", 20, || {
        TLS_VALUE.with(|v| v.set(7));
        let (seen, init) = thread::spawn(|| {
            let before = TLS_VALUE.with(|v| v.get());
            TLS_VALUE.with(|v| v.set(9));
            TLS_DROP.with(|_| {});
            (before, TLS_INIT_A.with(|v| v.get()))
        })
        .join()
        .unwrap_or((99, 0));
        let kept = TLS_VALUE.with(|v| v.get());
        check(
            "thread-tls",
            seen == 0 && kept == 7 && init == TLS_MAGIC_A,
            format!(
                "thread_local: new thread saw {seen} (want 0) and {} initial value, main kept {kept} (want 7)",
                if init == TLS_MAGIC_A { "the right" } else { "a WRONG" }
            ),
        );
        let drops = DROPPED.load(Ordering::SeqCst);
        check_warn(
            "thread-tls-dtor",
            drops == 1,
            format!(
                "thread_local destructor ran {drops} time(s) when its thread exited (want 1; 0 means TLS values leak)"
            ),
        );
    });
    report::section("thread-stacks", 30, || {
        match thread::spawn(nx::stack_here).join() {
            Ok(Some((_, size))) => info(
                "thread-default-stack",
                format!("std::thread::spawn stack: {} KiB mapped", size >> 10),
            ),
            _ => warn(
                "thread-default-stack",
                "couldn't query a spawned thread's stack",
            ),
        }
        let spawned = thread::Builder::new()
            .name("captest-stack".into())
            .stack_size(8 << 20)
            .spawn(|| {
                let size = nx::stack_here().map(|s| s.1).unwrap_or(0);
                (size, recurse(1000))
            });
        match spawned.map(|h| h.join()) {
            Ok(Ok((size, sum))) => {
                check(
                    "thread-big-stack",
                    sum > 0,
                    format!(
                        "8 MiB stack: {} KiB mapped, recursed 4 MiB deep",
                        size >> 10
                    ),
                );
            }
            Ok(Err(_)) => fail("thread-big-stack", "the 8 MiB-stack thread panicked"),
            Err(e) => fail(
                "thread-big-stack",
                format!("spawning with an 8 MiB stack failed: {e}"),
            ),
        };
    });
    report::section("thread-condvar", 30, condvar_ping_pong);
    report::section("thread-mpsc", 30, || {
        let (tx, rx) = mpsc::channel::<u64>();
        let producers: Vec<_> = (0..3u64)
            .map(|p| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for i in 0..10_000u64 {
                        let _ = tx.send(p * 1_000_000 + i);
                    }
                })
            })
            .collect();
        drop(tx);
        let t = Instant::now();
        let (mut count, mut sum) = (0u64, 0u64);
        while let Ok(v) = rx.recv_timeout(Duration::from_secs(5)) {
            count += 1;
            sum = sum.wrapping_add(v);
        }
        for h in producers {
            let _ = h.join();
        }
        let want: u64 = (0..3u64).map(|p| p * 1_000_000 * 10_000 + 49_995_000).sum();
        check(
            "thread-mpsc",
            count == 30_000 && sum == want,
            format!(
                "3 producers, {count}/30000 messages, sum ok: {}, {:.1} ms",
                sum == want,
                t.elapsed().as_secs_f64() * 1e3
            ),
        );
    });
    report::section("thread-rwlock-barrier", 30, || {
        let lock = Arc::new(RwLock::new(0u64));
        let barrier = Arc::new(Barrier::new(4));
        let workers: Vec<_> = (0..4)
            .map(|i| {
                let (lock, barrier) = (lock.clone(), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..1000 {
                        if i == 0 {
                            *lock.write().unwrap() += 1;
                        } else {
                            black_box(*lock.read().unwrap());
                        }
                    }
                    barrier.wait().is_leader()
                })
            })
            .collect();
        let leaders = workers
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|&l| l)
            .count();
        let total = *lock.read().unwrap();
        check(
            "thread-rwlock-barrier",
            total == 1000 && leaders == 1,
            format!("RwLock total {total}/1000, Barrier leaders {leaders}/1"),
        );
    });
    report::section("thread-park", 30, park_ping_pong);
    report::section("thread-spawn-cycles", 60, spawn_cycles);
    report::section("thread-names", 10, || {
        let name = thread::Builder::new()
            .name("captest-named".into())
            .spawn(|| thread::current().name().map(str::to_owned))
            .ok()
            .and_then(|h| h.join().ok())
            .flatten();
        check(
            "thread-names",
            name.as_deref() == Some("captest-named"),
            format!("name seen inside the thread: {name:?}"),
        );
        let mut parts = [0u64; 3];
        thread::scope(|s| {
            for (i, part) in parts.iter_mut().enumerate() {
                s.spawn(move || *part = i as u64 + 1);
            }
        });
        check(
            "thread-scoped",
            parts == [1, 2, 3],
            format!("thread::scope results {parts:?}"),
        );
    });
}

fn condvar_ping_pong() {
    let pair = Arc::new((Mutex::new(0u32), Condvar::new()));
    let rounds = 2000u32;
    let worker_pair = pair.clone();
    let t = Instant::now();
    let worker = thread::spawn(move || {
        let (m, cv) = &*worker_pair;
        for i in 0..rounds {
            let g = m.lock().unwrap();
            let (mut g, timeout) = cv
                .wait_timeout_while(g, Duration::from_secs(5), |v| *v != 2 * i + 1)
                .unwrap();
            if timeout.timed_out() {
                return false;
            }
            *g += 1;
            cv.notify_one();
        }
        true
    });
    let (m, cv) = &*pair;
    let mut ok = true;
    for i in 0..rounds {
        let mut g = m.lock().unwrap();
        *g += 1;
        cv.notify_one();
        let (_g, timeout) = cv
            .wait_timeout_while(g, Duration::from_secs(5), |v| *v != 2 * i + 2)
            .unwrap();
        if timeout.timed_out() {
            ok = false;
            break;
        }
    }
    let worker_ok = worker.join().unwrap_or(false);
    let per = t.elapsed().as_secs_f64() * 1e6 / rounds as f64;
    check(
        "thread-condvar",
        ok && worker_ok,
        format!("Mutex+Condvar ping-pong, {rounds} round trips, {per:.1} us each"),
    );
}

fn park_ping_pong() {
    let rounds = 500u32;
    let flag = Arc::new(AtomicU32::new(0));
    let main = thread::current();
    let worker_flag = flag.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    let worker = thread::spawn(move || {
        for i in 1..=rounds {
            while worker_flag.load(Ordering::SeqCst) != 2 * i - 1 {
                if Instant::now() > deadline {
                    return;
                }
                thread::park_timeout(Duration::from_millis(100));
            }
            worker_flag.store(2 * i, Ordering::SeqCst);
            main.unpark();
        }
    });
    let t = Instant::now();
    let mut done = 0;
    'rounds: for i in 1..=rounds {
        flag.store(2 * i - 1, Ordering::SeqCst);
        worker.thread().unpark();
        while flag.load(Ordering::SeqCst) != 2 * i {
            if Instant::now() > deadline {
                break 'rounds;
            }
            thread::park_timeout(Duration::from_millis(100));
        }
        done = i;
    }
    let _ = worker.join();
    let per = t.elapsed().as_secs_f64() * 1e6 / done.max(1) as f64;
    check(
        "thread-park",
        done == rounds,
        format!("park/unpark ping-pong {done}/{rounds} rounds, {per:.1} us each"),
    );
}

fn spawn_cycles() {
    let _ = thread::spawn(|| ()).join();
    let heap0 = nx::heap_in_use();
    let free0 = nx::info(24, 0);
    let t = Instant::now();
    let mut failed = None;
    for i in 0..200u32 {
        match thread::Builder::new().spawn(move || black_box(i)) {
            Ok(h) => {
                let _ = h.join();
            }
            Err(e) => {
                failed = Some(format!("spawn #{i} failed: {e}"));
                break;
            }
        }
    }
    let per = t.elapsed().as_secs_f64() * 1e6 / 200.0;
    match failed {
        Some(msg) => fail("thread-spawn-cycles", msg),
        None => {
            // Each new thread writes two [Thread] lines to log.txt on the SD card: time it without.
            bevy_horizon::set_thread_logging(false);
            let t = Instant::now();
            for i in 0..200u32 {
                let _ = thread::spawn(move || black_box(i)).join();
            }
            bevy_horizon::set_thread_logging(true);
            let quiet = t.elapsed().as_secs_f64() * 1e6 / 200.0;
            info(
                "thread-spawn-cycles",
                format!(
                    "200 spawn+join cycles, {per:.0} us each; {quiet:.0} us without the [Thread] log lines"
                ),
            )
        }
    }
    let heap1 = nx::heap_in_use();
    check_warn(
        "thread-spawn-leak",
        heap1 <= heap0 + (64 << 10),
        format!(
            "heap in use {} -> {} KiB over 400 threads",
            heap0 >> 10,
            heap1 >> 10
        ),
    );
    if let (Some(a), Some(b)) = (free0, nx::info(24, 0)) {
        check(
            "thread-spawn-slots",
            b >= a,
            format!("free thread slots {a} -> {b}"),
        );
    }
}

#[derive(Clone, Copy)]
struct PolicyResult {
    policy: ThreadCorePolicy,
    speedup: f64,
    starved: bool,
}

/// Runs `n` threads of `steps` each; returns the elapsed time and the cores they were seen on.
fn run_workers(n: usize, steps: u64) -> (f64, u8) {
    let t = Instant::now();
    let workers: Vec<_> = (0..n)
        .map(|_| {
            thread::spawn(move || {
                let mut cores = 0u8;
                for _ in 0..8 {
                    spin(steps / 8);
                    cores |= 1 << nx::core_now().min(7);
                }
                cores
            })
        })
        .collect();
    let cores = workers
        .into_iter()
        .filter_map(|h| h.join().ok())
        .fold(0, |a, b| a | b);
    (t.elapsed().as_secs_f64(), cores)
}

fn measure(policy: ThreadCorePolicy, steps: u64) -> PolicyResult {
    bevy_horizon::set_thread_core_policy(policy);
    let (one, _) = run_workers(1, steps);
    let (three, cores) = run_workers(3, steps);
    let speedup = 3.0 * one / three;

    // The main thread spins while a new thread tries to set a flag. If they share a core, the
    // lower-priority new thread never runs until main blocks: the starvation hello_switch hit.
    let flag = Arc::new(AtomicBool::new(false));
    let thread_flag = flag.clone();
    let t = Instant::now();
    let h = thread::spawn(move || thread_flag.store(true, Ordering::Release));
    let mut seen = None;
    while t.elapsed() < Duration::from_secs(1) {
        if flag.load(Ordering::Acquire) {
            seen = Some(t.elapsed());
            break;
        }
        std::hint::spin_loop();
    }
    let _ = h.join();
    bevy_horizon::set_thread_core_policy(ThreadCorePolicy::Default);

    let id = format!("cores-{}", policy.name());
    let start = match seen {
        Some(d) => format!(
            "a new thread ran {:.0} us after spawn while main spun",
            d.as_secs_f64() * 1e6
        ),
        None => "a new thread never ran while main spun (starved for 1 s)".into(),
    };
    info(
        &id,
        format!("3 threads {speedup:.2}x faster than 1, seen on cores {cores:#05b}; {start}"),
    );
    PolicyResult {
        policy,
        speedup,
        starved: seen.is_none(),
    }
}

/// Measures every policy and returns the one to use for the rest of the run.
pub fn core_policies() -> ThreadCorePolicy {
    report::heading("Cores");
    let steps = calibrate(250);
    let mut results = Vec::new();
    for policy in ThreadCorePolicy::ALL {
        let id = format!("cores-{}", policy.name());
        if report::begin(&id, 30) {
            results.push(measure(policy, steps));
            report::end();
        }
    }
    let best = results
        .iter()
        .filter(|r| !r.starved)
        .map(|r| r.speedup)
        .fold(0.0, f64::max);
    let order = [
        ThreadCorePolicy::Spread,
        ThreadCorePolicy::AnyCore,
        ThreadCorePolicy::Pinned,
        ThreadCorePolicy::Default,
    ];
    let chosen = order
        .into_iter()
        .find(|p| {
            results
                .iter()
                .any(|r| r.policy == *p && !r.starved && r.speedup >= best * 0.9)
        })
        .unwrap_or(ThreadCorePolicy::Spread);
    let default = results
        .iter()
        .find(|r| r.policy == ThreadCorePolicy::Default);
    if let Some(d) = default {
        check_warn(
            "cores-default",
            d.speedup > 1.5 && !d.starved,
            format!(
                "libnx's default placement: {:.2}x with 3 threads{}",
                d.speedup,
                if d.starved {
                    ", and a busy main thread starves new threads"
                } else {
                    ""
                }
            ),
        );
    }
    let chosen_speedup = results
        .iter()
        .find(|r| r.policy == chosen)
        .map(|r| r.speedup)
        .unwrap_or(0.0);
    check(
        "cores-chosen",
        chosen_speedup > 2.0,
        format!(
            "using ThreadCorePolicy::{} from here on ({chosen_speedup:.2}x with 3 threads)",
            chosen.name()
        ),
    );
    bevy_horizon::set_thread_core_policy(chosen);
    chosen
}
