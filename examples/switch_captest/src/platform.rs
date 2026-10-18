//! System facts and the self-contained std checks: randomness, time, CPU speed and clocks.

use std::hash::BuildHasher;
use std::hint::black_box;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::nx;
use crate::report::{self, check, check_warn, fail, info, warn};

mod c {
    unsafe extern "C" {
        // bevy_horizon's compat.rs defines getrandom (over libnx's randomGet); arc4random_buf is
        // newlib's, over getentropy -> bevy_horizon's _getentropy_r.
        pub fn getrandom(buf: *mut u8, len: usize, flags: u32) -> isize;
        pub fn arc4random_buf(buf: *mut core::ffi::c_void, len: usize);
    }
}

pub fn system_info() {
    report::heading("System");
    info(
        "build",
        format!(
            "switch_captest {} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("CAPTEST_BUILD").unwrap_or("dev")
        ),
    );
    info("horizon", nx::hos_version());
    let applet = unsafe { nx::appletGetAppletType() };
    let name = match applet {
        0 => "Application",
        1 => "SystemApplet",
        2 => "LibraryApplet",
        3 => "OverlayApplet",
        4 => "SystemApplication",
        _ => "unknown",
    };
    if applet == 0 || applet == 4 {
        info(
            "applet-type",
            format!("{name} ({applet}): full application memory"),
        );
    } else {
        warn(
            "applet-type",
            format!(
                "{name} ({applet}): running as an applet, so memory is limited. Launch from a forwarder or with R held over a game"
            ),
        );
    }
    let mode = unsafe { nx::appletGetOperationMode() };
    info(
        "operation-mode",
        if mode == 0 { "handheld" } else { "docked" },
    );
    let perf = unsafe { nx::appletGetPerformanceMode() };
    info(
        "performance-mode",
        match perf {
            0 => "normal (handheld clocks)",
            1 => "boost (docked clocks)",
            _ => "unknown",
        },
    );
    if let (Some(total), Some(used)) = (nx::info(6, 0), nx::info(7, 0)) {
        info(
            "memory",
            format!(
                "process memory {} MiB, {} MiB in use at start",
                total >> 20,
                used >> 20
            ),
        );
    }
    if let (Some(cores), Some(prios)) = (nx::info(0, 0), nx::info(1, 0)) {
        info(
            "process-masks",
            format!("core mask {cores:#06b}, priority mask {prios:#018x}"),
        );
    }
    match nx::info(24, 0) {
        Some(n) => info(
            "free-threads",
            format!("{n} more threads allowed by the resource limit"),
        ),
        None => info("free-threads", "not reported (needs HOS 11+)"),
    }
    let (core, mask) = nx::core_mask_here();
    info(
        "main-thread",
        format!(
            "main thread on core {} (ideal core {core}, mask {mask:#06b})",
            nx::core_now()
        ),
    );
    match nx::stack_here() {
        Some((free, size)) => info(
            "main-stack",
            format!(
                "main thread stack {} KiB, {} KiB free here",
                size >> 10,
                free >> 10
            ),
        ),
        None => warn("main-stack", "svcQueryMemory failed on the stack"),
    }
}

pub fn random() {
    report::heading("Randomness");
    report::section("random-getrandom", 20, || {
        let (mut a, mut b) = ([0u8; 64], [0u8; 64]);
        match getrandom::fill(&mut a).and_then(|()| getrandom::fill(&mut b)) {
            Ok(()) => {
                check(
                    "random-getrandom",
                    a != b && a != [0; 64],
                    "getrandom 0.4: two 64-byte draws are non-zero and differ",
                );
            }
            Err(e) => fail("random-getrandom", format!("getrandom 0.4 failed: {e}")),
        }
    });
    report::section("random-libc", 20, || {
        let mut a = [0u8; 4096];
        let n = unsafe { c::getrandom(a.as_mut_ptr(), a.len(), 0) };
        let nonzero = a.iter().filter(|&&x| x != 0).count();
        check(
            "random-libc",
            n == 4096 && nonzero > 3900,
            format!(
                "libc getrandom (ring, getrandom 0.2) returned {n}; {nonzero}/4096 bytes non-zero"
            ),
        );
        let mut b = [0u8; 4096];
        unsafe { c::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
        let nonzero = b.iter().filter(|&&x| x != 0).count();
        check(
            "random-arc4random",
            nonzero > 3900 && a != b,
            format!("newlib arc4random_buf: {nonzero}/4096 bytes non-zero"),
        );
    });
    report::section("random-hashmap-keys", 20, || {
        let here = std::collections::hash_map::RandomState::new().hash_one(42u64);
        let there =
            std::thread::spawn(|| std::collections::hash_map::RandomState::new().hash_one(42u64))
                .join()
                .unwrap_or(here);
        check(
            "random-hashmap-keys",
            here != there,
            "HashMap seeds differ between threads",
        );
    });
}

pub fn time() {
    report::heading("Time");
    report::section("time-instant", 20, || {
        let mut min = Duration::MAX;
        let mut backwards = 0;
        let mut last = Instant::now();
        for _ in 0..100_000 {
            let now = Instant::now();
            if now < last {
                backwards += 1;
            }
            if now > last {
                min = min.min(now - last);
            }
            last = now;
        }
        check(
            "time-instant",
            min < Duration::from_micros(10),
            format!("Instant resolution {} ns", min.as_nanos()),
        );
        check(
            "time-monotonic",
            backwards == 0,
            format!("100000 Instant::now() calls, {backwards} went backwards"),
        );

        let freq = nx::tick_freq();
        let (t0, i0) = (nx::ticks(), Instant::now());
        std::thread::sleep(Duration::from_millis(200));
        let (t1, i1) = (nx::ticks(), Instant::now());
        let ratio = ((t1 - t0) as f64 / freq as f64) / (i1 - i0).as_secs_f64();
        check(
            "time-tick-vs-instant",
            (ratio - 1.0).abs() < 0.01,
            format!(
                "system tick ({freq} Hz) and Instant agree to {:.3}%",
                (ratio - 1.0).abs() * 100.0
            ),
        );
    });
    report::section("time-sleep", 30, || {
        for (ms, n) in [(1u64, 50u32), (16, 20)] {
            let want = Duration::from_millis(ms);
            let (mut early, mut total, mut worst) = (0, Duration::ZERO, Duration::ZERO);
            for _ in 0..n {
                let t = Instant::now();
                std::thread::sleep(want);
                let d = t.elapsed();
                early += (d < want) as u32;
                total += d;
                worst = worst.max(d);
            }
            check(
                &format!("time-sleep-{ms}ms"),
                early == 0 && worst < want + Duration::from_millis(5),
                format!(
                    "sleep({ms} ms) x{n}: average {:.3} ms, worst {:.3} ms, {early} woke early",
                    (total / n).as_secs_f64() * 1e3,
                    worst.as_secs_f64() * 1e3
                ),
            );
        }
        let t = Instant::now();
        for _ in 0..1000 {
            std::thread::yield_now();
        }
        info(
            "time-yield",
            format!("yield_now: {:.2} us each", t.elapsed().as_secs_f64() * 1e3),
        );
    });
    report::section("time-system", 20, || {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => {
                let year = 1970 + d.as_secs() / 31_556_952;
                check_warn(
                    "time-system",
                    year >= 2024,
                    format!("SystemTime is {} s after 1970 (year {year})", d.as_secs()),
                );
            }
            Err(e) => fail("time-system", format!("SystemTime is before 1970: {e}")),
        }
    });
}

/// A xorshift loop the compiler can't shorten: the unit of CPU work in these tests.
pub fn spin(iters: u64) -> u64 {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..black_box(iters) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
    }
    black_box(x)
}

/// Iterations of `spin` that take about `ms` milliseconds on this thread.
pub fn calibrate(ms: u64) -> u64 {
    let t = Instant::now();
    spin(2_000_000);
    let per_ms = 2_000_000.0 / (t.elapsed().as_secs_f64() * 1e3).max(0.001);
    (per_ms * ms as f64) as u64
}

fn time_spin(iters: u64) -> f64 {
    let t = Instant::now();
    spin(iters);
    t.elapsed().as_secs_f64()
}

struct Clocks;

impl Clocks {
    fn open() -> Option<Clocks> {
        let rc = unsafe { nx::clkrstInitialize() };
        if rc != 0 {
            warn(
                "cpu-clocks",
                format!("clkrstInitialize failed: {}", nx::rc_text(rc)),
            );
            return None;
        }
        Some(Clocks)
    }

    fn rate(&self, module: u32) -> Option<u32> {
        let mut session = nx::ClkrstSession([0; 32]);
        if unsafe { nx::clkrstOpenSession(&mut session, module, 3) } != 0 {
            return None;
        }
        let mut hz = 0u32;
        let rc = unsafe { nx::clkrstGetClockRate(&mut session, &mut hz) };
        unsafe { nx::clkrstCloseSession(&mut session) };
        (rc == 0).then_some(hz)
    }

    fn describe(&self) -> String {
        let mhz = |m| {
            self.rate(m)
                .map(|hz| format!("{:.1}", hz as f64 / 1e6))
                .unwrap_or_else(|| "?".into())
        };
        format!(
            "CPU {} MHz, GPU {} MHz, memory {} MHz",
            mhz(0x4000_0001),
            mhz(0x4000_0002),
            mhz(0x4000_0039)
        )
    }
}

impl Drop for Clocks {
    fn drop(&mut self) {
        unsafe { nx::clkrstExit() };
    }
}

pub fn cpu() {
    report::heading("CPU and clocks");
    report::section("cpu-speed", 60, || {
        let steps = 20_000_000u64;
        let s = time_spin(steps);
        info(
            "cpu-int",
            format!(
                "xorshift: {:.0} M steps/s on one core",
                steps as f64 / s / 1e6
            ),
        );

        let t = Instant::now();
        let mut acc = 0f32;
        for i in 0..black_box(10_000_000u32) {
            acc += (i as f32).sqrt() * 0.5;
        }
        black_box(acc);
        info(
            "cpu-float",
            format!(
                "f32 sqrt+mul+add: {:.0} M/s",
                10.0 / t.elapsed().as_secs_f64()
            ),
        );

        let src = vec![7u8; 16 << 20];
        let mut dst = vec![0u8; 16 << 20];
        let t = Instant::now();
        for _ in 0..8 {
            dst.copy_from_slice(black_box(&src));
            black_box(&mut dst);
        }
        info(
            "cpu-memcpy",
            format!(
                "memcpy {:.2} GB/s (16 MiB blocks)",
                0.125 / t.elapsed().as_secs_f64()
            ),
        );
        let features = [
            ("neon", cfg!(target_feature = "neon")),
            ("aes", cfg!(target_feature = "aes")),
            ("sha2", cfg!(target_feature = "sha2")),
            ("crc", cfg!(target_feature = "crc")),
            ("lse", cfg!(target_feature = "lse")),
        ];
        let on: Vec<_> = features.iter().filter(|f| f.1).map(|f| f.0).collect();
        info(
            "cpu-target-features",
            format!("compiled with: {}", on.join(" ")),
        );
    });
    report::section("cpu-clocks", 60, || {
        let Some(clocks) = Clocks::open() else { return };
        info("cpu-clocks", clocks.describe());
        let steps = calibrate(300);
        let base = time_spin(steps);
        let rc = unsafe { nx::appletSetCpuBoostMode(1) };
        if rc != 0 {
            warn(
                "cpu-boost",
                format!(
                    "appletSetCpuBoostMode(FastLoad) failed: {}",
                    nx::rc_text(rc)
                ),
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
        let boosted = time_spin(steps);
        let during = clocks.describe();
        let rc = unsafe { nx::appletSetCpuBoostMode(0) };
        std::thread::sleep(Duration::from_millis(50));
        info("cpu-boost-clocks", format!("during boost: {during}"));
        check_warn(
            "cpu-boost",
            boosted < base * 0.8,
            format!(
                "boost mode ran the same work {:.2}x faster ({:.0} -> {:.0} ms)",
                base / boosted,
                base * 1e3,
                boosted * 1e3
            ),
        );
        check(
            "cpu-boost-restore",
            rc == 0,
            format!(
                "boost mode off again: {}; {}",
                nx::rc_text(rc),
                clocks.describe()
            ),
        );
    });
}
