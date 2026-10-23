//! How much memory the app has: how it was launched, what the kernel gives the process, and how
//! much of the malloc heap is in use. `log.txt` gets the first two at boot and the malloc figures
//! in every heartbeat.

use core::ffi::c_int;

mod ffi {
    use core::ffi::c_int;

    /// newlib `struct mallinfo` (devkitA64 `malloc.h`).
    #[repr(C)]
    pub struct MallInfo {
        /// Bytes malloc has taken from the heap with `sbrk`.
        pub arena: usize,
        pub ordblks: usize,
        pub smblks: usize,
        pub hblks: usize,
        pub hblkhd: usize,
        pub usmblks: usize,
        pub fsmblks: usize,
        /// Bytes in allocated blocks.
        pub uordblks: usize,
        /// Free bytes, the top chunk included.
        pub fordblks: usize,
        /// The top chunk: free space `free` could give back to the system.
        pub keepcost: usize,
    }

    unsafe extern "C" {
        pub fn appletGetAppletType() -> c_int;
        pub fn svcGetInfo(out: *mut u64, id0: u32, handle: u32, id1: u64) -> u32;
        pub fn envHasHeapOverride() -> bool;
        pub fn envGetHeapOverrideAddr() -> *mut core::ffi::c_void;
        pub fn envGetHeapOverrideSize() -> u64;
        pub fn mallinfo() -> MallInfo;
        /// The malloc heap's bounds, set by libnx's `__libnx_initheap`; `sbrk` hands out memory
        /// between them. NVK lowers `fake_heap_end` to take GPU memory from the top of the heap
        /// (see [`super::heap_end`]), so read it with [`super::heap_limit`].
        pub static fake_heap_start: *mut u8;
        pub static fake_heap_end: *mut u8;
    }

    // The top-down region's API (bevy-horizon's `patches/mesa-switch/`), in NVK.
    #[cfg(feature = "vulkan")]
    unsafe extern "C" {
        /// The region's extent and the bytes in its blocks, GPU and CPU, now and at the most,
        /// and how many GPU blocks it couldn't place. Takes the region's lock; doesn't allocate.
        pub fn nouveau_horizon_region_stats(
            size: *mut u64,
            used: *mut u64,
            peak: *mut u64,
            gpu_fallbacks: *mut u64,
        );
    }
}

/// `fake_heap_end` now: where `sbrk`, and with it malloc, has to stop. NVK lowers and raises it
/// while the app runs, so it's read with a volatile load.
pub(crate) fn heap_limit() -> usize {
    unsafe { core::ptr::read_volatile(&raw const ffi::fake_heap_end) as usize }
}

/// libnx `svc.h`.
const CUR_PROCESS_HANDLE: u32 = 0xFFFF_8001;
const INFO_TYPE_TOTAL_MEMORY_SIZE: u32 = 6;
const INFO_TYPE_USED_MEMORY_SIZE: u32 = 7;

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// libnx `AppletType` (switch/services/applet.h).
fn applet_type_name(applet_type: c_int) -> &'static str {
    match applet_type {
        0 => "Application: running as a game (a forwarder icon or title override), so it gets a game's memory",
        2 => "LibraryApplet: applet mode, e.g. started from the Album, so it gets an applet's memory",
        1 => "SystemApplet",
        3 => "OverlayApplet",
        4 => "SystemApplication",
        -1 => "Default: the loader didn't say",
        -2 => "None",
        _ => "unknown",
    }
}

fn process_info(info_type: u32) -> Option<u64> {
    let mut out = 0u64;
    let rc = unsafe { ffi::svcGetInfo(&mut out, info_type, CUR_PROCESS_HANDLE, 0) };
    (rc == 0).then_some(out)
}

/// libnx `MemType_Heap` (switch/kernel/svc.h).
const MEM_TYPE_HEAP: u32 = 5;

/// Where the heap ends, as libnx's `__libnx_initheap` set it. `fake_heap_end` itself no longer
/// says: NVK takes GPU memory from the top of the heap downward by lowering it, so that `sbrk`,
/// and with it malloc, stops below that memory (the top-down region bevy-horizon's
/// `patches/mesa-switch/` adds to mesa-switch's `nouveau_horizon_memory.c`).
///
/// An NRO's heap is the one the homebrew loader passed in, which libnx's `envGetHeapOverride*`
/// still give. Without one, `svcQueryMemory` walks the heap's blocks up from `fake_heap_start`
/// until the memory type isn't heap; device mapping and locking change a block's attributes, not
/// its type. Worked out once and cached, without allocating.
fn heap_end() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static HEAP_END: AtomicU64 = AtomicU64::new(0);

    let cached = HEAP_END.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let end = unsafe {
        if ffi::envHasHeapOverride() {
            (ffi::envGetHeapOverrideAddr() as u64).saturating_add(ffi::envGetHeapOverrideSize())
        } else {
            walk_heap_end()
        }
    };
    let end = end.max(heap_limit() as u64);
    HEAP_END.store(end, Ordering::Relaxed);
    end
}

/// The end of the run of heap-type blocks that starts at `fake_heap_start`, or 0 if a query fails.
fn walk_heap_end() -> u64 {
    use crate::logging::ffi as log_ffi;

    let mut addr = unsafe { ffi::fake_heap_start as u64 };
    loop {
        let mut info = log_ffi::MemoryInfo::default();
        let mut page_info = 0u32;
        let rc = unsafe { log_ffi::svcQueryMemory(&mut info, &mut page_info, addr) };
        let block_end = info.addr.saturating_add(info.size);
        if rc != 0 || block_end <= addr {
            return 0;
        }
        if info.kind != MEM_TYPE_HEAP {
            return addr;
        }
        addr = block_end;
    }
}

/// The whole heap, NVK's GPU region at the top included.
fn heap_size() -> u64 {
    heap_end().saturating_sub(unsafe { ffi::fake_heap_start as u64 })
}

/// How much of the top of the heap the top-down region has: everything above `fake_heap_end`.
fn top_down_region() -> u64 {
    heap_end().saturating_sub(heap_limit() as u64)
}

/// The boot log's `[Memory]` lines.
pub(crate) fn boot_report() -> String {
    let applet_type = unsafe { ffi::appletGetAppletType() };
    let process = match (
        process_info(INFO_TYPE_TOTAL_MEMORY_SIZE),
        process_info(INFO_TYPE_USED_MEMORY_SIZE),
    ) {
        (Some(total), Some(used)) => format!(
            "{:.1} MiB in total, {:.1} MiB used (svcGetInfo; \"used\" already counts the whole malloc heap below)",
            mib(total),
            mib(used)
        ),
        _ => "svcGetInfo failed".to_string(),
    };
    let heap_from = if unsafe { ffi::envHasHeapOverride() } {
        "set up by the homebrew loader"
    } else {
        "set up by libnx"
    };
    format!(
        "[Memory] applet type {applet_type} = {}\n\
         [Memory] process: {process}\n\
         [Memory] malloc heap: {:.1} MiB, {heap_from}; {}\n",
        applet_type_name(applet_type),
        mib(heap_size()),
        malloc_report(),
    )
}

/// How much of the malloc heap is used: `in use` is what Rust and C code have allocated; `taken`
/// also counts freed blocks malloc keeps for reuse.
pub(crate) fn malloc_report() -> String {
    MallocFigures::now().to_string()
}

/// The malloc heap at one moment, from newlib's `mallinfo`. `Display` writes the heartbeat's
/// `[Memory]` figures without allocating, so the panic hook can use it too.
///
/// devkitA64's newlib malloc is Doug Lea's malloc 2.6.5 (newlib `libc/stdlib/_mallocr.c`; its
/// `libc.a` has that malloc, not nano-malloc). Its `malloc_update_mallinfo` sets `arena` to the
/// bytes taken with `sbrk`, `fordblks` to all free space including the top chunk, and `keepcost`
/// to the top chunk alone. A large block needs one free run, and once loading has freed memory
/// between blocks still in use, only the top is sure to have one.
///
/// NVK takes GPU memory from the top of the heap downward (bevy-horizon's
/// `patches/mesa-switch/`), and so, with the `top_down_alloc` feature, do Rust's blocks of 256 KiB
/// and up (allocator.rs). malloc grows up toward that region, so the free space at the top is
/// shared: both draw on it. Only once it runs out do GPU blocks go to malloc's holes.
pub(crate) struct MallocFigures {
    in_use: usize,
    taken: usize,
    heap: u64,
    /// The free top chunk, plus the part of the heap between `sbrk`'s break and the region.
    top: u64,
    /// Free space below the top, split among the free blocks between those in use.
    holes: usize,
    /// The top-down region at the top of the heap, free runs in it included.
    region: u64,
    #[cfg(feature = "vulkan")]
    region_blocks: RegionBlocks,
    #[cfg(feature = "top_down_alloc")]
    rust: crate::allocator::Figures,
}

/// The region's blocks, from `nouveau_horizon_region_stats`.
#[cfg(feature = "vulkan")]
#[derive(Default)]
struct RegionBlocks {
    /// Bytes in blocks, GPU and Rust's, now and at the most.
    used: u64,
    peak: u64,
    /// GPU blocks the region couldn't place, which came from malloc.
    gpu_fallbacks: u64,
}

impl MallocFigures {
    pub(crate) fn now() -> Self {
        let info = unsafe { ffi::mallinfo() };
        let heap = heap_size();
        let region = top_down_region();
        #[cfg(feature = "vulkan")]
        let region_blocks = {
            let (mut extent, mut blocks) = (0u64, RegionBlocks::default());
            unsafe {
                ffi::nouveau_horizon_region_stats(
                    &mut extent,
                    &mut blocks.used,
                    &mut blocks.peak,
                    &mut blocks.gpu_fallbacks,
                )
            };
            blocks
        };
        Self {
            in_use: info.uordblks,
            taken: info.arena,
            heap,
            top: heap.saturating_sub(region).saturating_sub(info.arena as u64)
                + info.keepcost as u64,
            holes: info.fordblks.saturating_sub(info.keepcost),
            region,
            #[cfg(feature = "vulkan")]
            region_blocks,
            #[cfg(feature = "top_down_alloc")]
            rust: crate::allocator::figures(),
        }
    }
}

impl core::fmt::Display for MallocFigures {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "malloc has {:.1} MiB in use, {:.1} MiB taken from the {:.1} MiB heap; {:.1} MiB free at the top, {:.1} MiB free in holes below it",
            mib(self.in_use as u64),
            mib(self.taken as u64),
            mib(self.heap),
            mib(self.top),
            mib(self.holes as u64),
        )?;
        if self.region != 0 {
            write!(f, "; the top-down region above the top: {:.1} MiB", mib(self.region))?;
            #[cfg(feature = "vulkan")]
            {
                let blocks = &self.region_blocks;
                #[cfg(not(feature = "top_down_alloc"))]
                write!(f, ", {:.1} MiB of it in GPU blocks", mib(blocks.used))?;
                #[cfg(feature = "top_down_alloc")]
                write!(
                    f,
                    ", {:.1} MiB of it in blocks ({:.1} MiB GPU, {:.1} MiB Rust)",
                    mib(blocks.used),
                    mib(blocks.used.saturating_sub(self.rust.region)),
                    mib(self.rust.region),
                )?;
                write!(f, ", {:.1} MiB at the most", mib(blocks.peak))?;
            }
        }
        #[cfg(feature = "vulkan")]
        if self.region_blocks.gpu_fallbacks != 0 {
            write!(
                f,
                "; {} GPU blocks the region couldn't place came from malloc",
                self.region_blocks.gpu_fallbacks
            )?;
        }
        #[cfg(feature = "top_down_alloc")]
        {
            let rust = &self.rust;
            write!(
                f,
                "; Rust's malloc blocks: {:.1} MiB of 256 KiB and up, {:.1} MiB of 64-256 KiB",
                mib(rust.malloc_large),
                mib(rust.malloc_medium),
            )?;
            if rust.large_fallbacks != 0 {
                write!(
                    f,
                    "; {} large blocks the region couldn't place went to malloc",
                    rust.large_fallbacks
                )?;
            }
            if rust.small_rescues != 0 {
                write!(
                    f,
                    "; {} small blocks malloc couldn't place went to the region",
                    rust.small_rescues
                )?;
            }
            let [aligned, s16, s32, s64, s128, s256, reallocs, frees] = rust.malloc_calls;
            write!(
                f,
                "; Rust's calls that reached malloc: {aligned} allocations aligned past 16 bytes, \
                 {s16} / {s32} / {s64} / {s128} / {s256} of 8-16 / 16-32 / 32-64 / 64-128 / 128-256 KiB, \
                 {reallocs} reallocs and {frees} frees"
            )?;
            let cache = &rust.cache;
            if cache.enabled {
                write!(
                    f,
                    "; the small-block cache holds {:.1} MiB; of the allocations it takes, it served \
                     {} and malloc {} ({} with the cache busy), and of the frees, {} went to malloc \
                     with the cache busy and {} for blocks under their class's size; its depot took \
                     {} magazines and gave out {}, and {} went back to malloc",
                    mib(cache.cached as u64),
                    cache.hits,
                    cache.misses + cache.busy_allocs,
                    cache.busy_allocs,
                    cache.busy_frees,
                    cache.rejected,
                    cache.flushes,
                    cache.refills,
                    cache.returns,
                )?;
                write!(
                    f,
                    "; {} tries found a depot slot held, and {} magazines found the depot full",
                    cache.depot_held, cache.depot_full
                )?;
                if cache.drains > 0 {
                    write!(f, "; malloc ran out {} times, and took back the cache's blocks", cache.drains)?;
                }
            } else {
                write!(f, "; the small-block cache is off (BH_ALLOC_CACHE=0)")?;
            }
        }
        Ok(())
    }
}

/// libnx `Perm_Rw` (switch/kernel/svc.h).
const PERM_RW: u32 = 3;
/// libnx `MemAttr_IsBorrowed` (switch/kernel/svc.h), the kernel's `KMemoryAttribute_Locked`.
const MEM_ATTR_LOCKED: u32 = 1 << 0;
/// The attributes the kernel ignores when it checks a non-secure IPC buffer, such as the one the
/// loader reads the next NRO into: `MemAttr_IsIpcMapped`, `MemAttr_IsDeviceMapped` and
/// `MemAttr_IsPermissionLocked` (Atmosphère `KPageTableBase::SetupForIpcClient`). Any other
/// attribute, `MemAttr_IsBorrowed` (locked) in particular, fails the read with 2001-0106.
const MEM_ATTRS_IPC_IGNORES: u32 = (1 << 1) | (1 << 2) | (1 << 4);
/// libnx `MemAttr_IsDeviceMapped` and `MemAttr_IsUncached`: heap pages an nvmap object still holds.
/// nvservices device-maps every buffer the Vulkan driver hands it (`svcMapDeviceAddressSpace*`),
/// and libnx's `nvMapCreate` marks an uncached one Uncached, which `nvMapClose` clears (libnx
/// `nvidia/map.c`). NVK takes all its GPU memory from this heap, most of it from the top
/// (bevy-horizon's `patches/mesa-switch/`), so such pages mean the driver hasn't shut down.
/// Ryujinx sets only Uncached.
const MEM_ATTRS_NVMAP: u32 = (1 << 2) | (1 << 3);
/// How many blocks `check_heap_for_exit` lists.
const LISTED: usize = 8;

/// A heap block the loader can't read into.
#[derive(Clone, Copy, Default)]
struct BadBlock {
    start: u64,
    end: u64,
    kind: u32,
    perm: u32,
    attr: u32,
}

/// What `scan_heap` found: how many blocks the loader can't read into, and the first few, and
/// how much of the heap nvmap objects hold.
#[derive(Default)]
struct HeapScan {
    blocks: usize,
    bytes: u64,
    first: [BadBlock; LISTED],
    nvmap_blocks: usize,
    nvmap_bytes: u64,
}

/// Walks the malloc heap with `svcQueryMemory`, looking for blocks the loader's read would fail
/// on: ones the process can't read and write, or with an attribute the kernel won't accept in an
/// IPC buffer. Fails with the address and result of a query that failed.
///
/// Allocates nothing, so it is safe at exit even if the exit is an abort because malloc failed.
fn scan_heap() -> Result<HeapScan, (u64, u32)> {
    use crate::logging::ffi as log_ffi;

    let (start, end) = (unsafe { ffi::fake_heap_start as u64 }, heap_end());
    let mut scan = HeapScan::default();
    let mut addr = start;
    while addr < end {
        let mut info = log_ffi::MemoryInfo::default();
        let mut page_info = 0u32;
        let rc = unsafe { log_ffi::svcQueryMemory(&mut info, &mut page_info, addr) };
        let block_end = info.addr.saturating_add(info.size).min(end);
        if rc != 0 || block_end <= addr {
            return Err((addr, rc));
        }
        if (info.perm & PERM_RW) != PERM_RW || (info.attr & !MEM_ATTRS_IPC_IGNORES) != 0 {
            if scan.blocks < LISTED {
                scan.first[scan.blocks] = BadBlock {
                    start: addr,
                    end: block_end,
                    kind: info.kind,
                    perm: info.perm,
                    attr: info.attr,
                };
            }
            scan.blocks += 1;
            scan.bytes += block_end - addr;
        }
        if info.attr & MEM_ATTRS_NVMAP != 0 {
            scan.nvmap_blocks += 1;
            scan.nvmap_bytes += block_end - addr;
        }
        addr = block_end;
    }
    Ok(scan)
}

/// Checks at exit that the homebrew loader can read the next NRO into the malloc heap, and
/// writes the result to `log.txt`. Returns how many blocks it can't read into, or `None` if
/// `svcQueryMemory` failed.
///
/// The loader reads the next NRO into this heap, and the kernel only accepts a read buffer whose
/// pages the process can read and write and that aren't locked (Atmosphère
/// `KPageTableBase::SetupForIpcClient`). Transfer memory (`svcCreateTransferMemory`: sockets,
/// nv, audio) and thread stacks (`svcMapMemory`) leave their heap pages locked with no access
/// until they are closed.
pub(crate) fn check_heap_for_exit(when: &str) -> Option<usize> {
    use crate::logging::write_log_line;

    let scan = match scan_heap() {
        Ok(scan) => scan,
        Err((addr, rc)) => {
            write_log_line(format_args!(
                "[Exit] heap check, {when}: svcQueryMemory({addr:#x}) failed with {}",
                crate::logging::ResultCode(rc)
            ));
            return None;
        }
    };
    match scan.blocks {
        0 => write_log_line(format_args!(
            "[Exit] heap check, {when}: the loader can read into all {:.1} MiB of the malloc heap; every page is read/write and unlocked",
            mib(heap_size())
        )),
        1 => write_log_line(format_args!(
            "[Exit] heap check, {when}: 1 block, {:.2} MiB, would fail the loader's read:",
            mib(scan.bytes)
        )),
        blocks => write_log_line(format_args!(
            "[Exit] heap check, {when}: {blocks} blocks, {:.2} MiB in all, would fail the loader's read{}:",
            mib(scan.bytes),
            if blocks > LISTED { " (the first 8 are listed)" } else { "" }
        )),
    }
    for block in &scan.first[..scan.blocks.min(LISTED)] {
        let why = if block.attr & MEM_ATTR_LOCKED != 0 && block.perm == 0 {
            ": locked with no access, as an open transfer memory or a thread stack leaves it"
        } else {
            ""
        };
        write_log_line(format_args!(
            "[Exit]   {:#x}-{:#x}, {:.2} MiB: memory type {:#x}, permission {:#x}, attributes {:#x}{why}",
            block.start,
            block.end,
            mib(block.end - block.start),
            block.kind,
            block.perm,
            block.attr,
        ));
    }
    Some(scan.blocks)
}

/// How many heap blocks, and bytes, nvmap objects still hold: GPU memory the driver hasn't freed.
/// `None` if `svcQueryMemory` failed. Allocates nothing.
pub(crate) fn nvmap_heap() -> Option<(usize, u64)> {
    scan_heap()
        .ok()
        .map(|scan| (scan.nvmap_blocks, scan.nvmap_bytes))
}

/// Waits, checking every millisecond for up to `timeout`, until the loader can read into the
/// whole heap again, and returns how long that took. `None` means the heap was still blocked at
/// the timeout or `svcQueryMemory` failed.
///
/// Closing a service doesn't unlock its transfer memory at once: the pages stay locked until the
/// service's own process notices the closed session and lets go of its mapping. libnx's
/// `_nvCleanup` waits like this (`tmemWaitForPermission`); `_bsdCleanup`, behind `socketExit`,
/// doesn't.
pub(crate) fn wait_for_heap_unlock(timeout: std::time::Duration) -> Option<std::time::Duration> {
    let started = std::time::Instant::now();
    loop {
        match scan_heap() {
            Ok(scan) if scan.blocks == 0 => return Some(started.elapsed()),
            Ok(_) if started.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            _ => return None,
        }
    }
}
