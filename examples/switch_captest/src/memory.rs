//! The heap: how much one allocation can get, how fast it is, and how failure shows up.

use std::alloc::{Layout, alloc, dealloc};
use std::hint::black_box;
use std::time::Instant;

use crate::nx;
use crate::report::{self, check, check_warn, info};

pub fn memory() {
    report::heading("Memory");
    report::section("mem-largest", 60, || {
        let m = unsafe { nx::mallinfo() };
        info(
            "mem-heap",
            format!(
                "newlib heap: arena {} MiB, {} MiB in use, {} MiB free inside it",
                m.arena >> 20,
                m.uordblks >> 20,
                m.fordblks >> 20
            ),
        );
        // In MiB. libnx commits the whole heap at start, so a reservation that succeeds is real.
        let (mut lo, mut hi) = (0usize, 8192usize);
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            let mut v: Vec<u8> = Vec::new();
            if v.try_reserve_exact(mid << 20).is_ok() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        check_warn(
            "mem-largest",
            lo >= 1024,
            format!("largest single allocation: {lo} MiB"),
        );
        let n = ((lo << 20) / 2).min(256 << 20);
        let mut v: Vec<u8> = Vec::new();
        if n > 0 && v.try_reserve_exact(n).is_ok() {
            let t = Instant::now();
            v.resize(n, 1);
            let secs = t.elapsed().as_secs_f64();
            let touched = v.iter().step_by(4096).filter(|&&b| b == 1).count();
            check(
                "mem-fill",
                touched == n.div_ceil(4096),
                format!(
                    "filled {} MiB at {:.2} GB/s",
                    n >> 20,
                    n as f64 / secs / 1e9
                ),
            );
        }
    });
    report::section("mem-small-allocs", 60, || {
        let heap0 = nx::heap_in_use();
        let t = Instant::now();
        let boxes: Vec<Box<u64>> = (0..1_000_000u64).map(Box::new).collect();
        let alloc_s = t.elapsed().as_secs_f64();
        let sum: u64 = boxes.iter().map(|b| **b).sum();
        let t = Instant::now();
        drop(boxes);
        let free_s = t.elapsed().as_secs_f64();
        let heap1 = nx::heap_in_use();
        check(
            "mem-small-allocs",
            sum == 499_999_500_000,
            format!(
                "1M Box<u64>: {:.0} ns per alloc, {:.0} ns per free",
                alloc_s * 1e3,
                free_s * 1e3
            ),
        );
        check_warn(
            "mem-small-leak",
            heap1 <= heap0 + (64 << 10),
            format!(
                "heap in use {} -> {} KiB after freeing them",
                heap0 >> 10,
                heap1 >> 10
            ),
        );
    });
    report::section("mem-align", 20, || {
        for align in [4096usize, 65536, 2 << 20] {
            let layout = Layout::from_size_align(100, align).expect("valid layout");
            let p = unsafe { alloc(layout) };
            let ok = !p.is_null() && (p as usize) % align == 0;
            if !p.is_null() {
                unsafe {
                    p.write_bytes(0xA5, 100);
                    black_box(p.read());
                    dealloc(p, layout);
                }
            }
            check(
                &format!("mem-align-{}k", align >> 10),
                ok,
                format!("allocation with {align}-byte alignment at {p:p}"),
            );
        }
        let mut v: Vec<u8> = Vec::new();
        check(
            "mem-alloc-failure",
            v.try_reserve_exact(8 << 30).is_err(),
            "an 8 GiB try_reserve returns Err (no abort)",
        );
    });
}
