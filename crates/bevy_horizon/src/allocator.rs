//! The `top_down_alloc` feature's global allocator. Rust's blocks of 256 KiB and up go in the
//! top-down region at the top of the heap, the one NVK takes GPU memory from (bevy-horizon's
//! `patches/mesa-switch/`); smaller ones go to newlib's malloc, as with Rust's `System`.
//!
//! On a desktop, malloc puts large blocks in their own `mmap` mappings, away from its small ones,
//! so freeing one gives its memory back whole. devkitA64's newlib malloc has no `mmap`: Doug Lea's
//! malloc 2.6.5 would map blocks of 128 KiB and up (`DEFAULT_MMAP_THRESHOLD`), but newlib's
//! `libc/stdlib/_mallocr.c` sets `HAVE_MMAP` to 0. So a game's large blocks sit between its small
//! ones in the one heap. Once loading frees them, malloc's free space is holes, and a GPU block
//! that needs one contiguous run finds none: in Skate 3 run sk8g, a 64 MiB block failed with 503
//! MiB free in holes. Here the region plays mmap's part. A large block takes whole 64 KiB granules
//! there, and freeing it leaves a run that GPU blocks and later large blocks can use; at the
//! region's bottom, the run goes back to malloc.
//!
//! A large block the region can't place comes from malloc, and a small block malloc can't place
//! goes to the region if one of its runs is free. The `[Memory]` lines count both.
//!
//! Blocks of up to 32 KiB, aligned to at most 16 bytes, go through a cache in front of malloc
//! (small_blocks.rs) unless `BH_ALLOC_CACHE=0`.

mod small_blocks;

use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::alloc::System;

use small_blocks::{Backend, Cache};

/// Blocks this big and up go to the region. A smaller one would waste more of its last granule.
pub(crate) const LARGE: usize = 256 << 10;
/// malloc's blocks from this size up to [`LARGE`] are counted, to show what a lower threshold
/// would move.
pub(crate) const MEDIUM: usize = 64 << 10;
/// The region's granule (`NOUVEAU_HORIZON_REGION_GRANULE_B`): its blocks are whole granules,
/// aligned to at least one.
const GRANULE: usize = 64 << 10;

unsafe extern "C" {
    /// A block in the region, or null when the region can't place it.
    fn nouveau_horizon_region_alloc(align: usize, size: usize) -> *mut u8;
    /// Frees a block, or the granules at the end of one, given the size asked for. Returns
    /// false, freeing nothing, when `ptr` isn't in the region.
    fn nouveau_horizon_region_free(ptr: *mut u8, size: usize) -> bool;
    // newlib's (libc/stdlib/_mallocr.c). `malloc_usable_size` reads the block's own header and the
    // next block's, without malloc's lock.
    fn malloc(size: usize) -> *mut c_void;
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn malloc_usable_size(ptr: *mut c_void) -> usize;
}

#[global_allocator]
static GLOBAL: HorizonAllocator = HorizonAllocator;

struct HorizonAllocator;

/// The small-block cache's malloc.
struct Newlib;

impl Backend for Newlib {
    #[inline]
    unsafe fn alloc(size: usize, zeroed: bool) -> *mut u8 {
        unsafe { if zeroed { calloc(1, size) } else { malloc(size) }.cast() }
    }

    #[inline]
    unsafe fn free(ptr: *mut u8) {
        unsafe { free(ptr.cast()) }
    }

    #[inline]
    unsafe fn usable_size(ptr: *mut u8) -> usize {
        unsafe { malloc_usable_size(ptr.cast()) }
    }

    /// The thread's local region (`tpidrro_el0`), which the kernel gives each thread, 0x200 bytes
    /// apart: always set, unlike `tpidr_el0` before compat.rs sets it on a new thread.
    #[inline]
    fn shard_hint() -> usize {
        let tlr: usize;
        unsafe {
            core::arch::asm!("mrs {}, tpidrro_el0", out(reg) tlr, options(nomem, nostack, preserves_flags))
        };
        tlr >> 9
    }
}

static CACHE: Cache<Newlib> = Cache::new();

/// Reads `BH_ALLOC_CACHE`: `0` turns the small-block cache off, and gives back the blocks it
/// holds. `HorizonPlugin` calls this once, after the game has set its environment; until then the
/// cache is on.
pub(crate) fn configure_from_env() {
    if std::env::var_os("BH_ALLOC_CACHE").is_some_and(|value| value == "0") {
        CACHE.set_enabled(false);
        tracing::info!("BH_ALLOC_CACHE=0: the small-block cache is off");
    }
}

/// Bytes, in whole granules, in Rust's blocks in the region.
static REGION_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Bytes in Rust's malloc blocks of [`LARGE`] and up.
static MALLOC_LARGE_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Bytes in Rust's malloc blocks from [`MEDIUM`] up to [`LARGE`].
static MALLOC_MEDIUM_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Large blocks the region couldn't place, which malloc took.
static LARGE_FALLBACKS: AtomicUsize = AtomicUsize::new(0);
/// Small blocks malloc couldn't place, which the region took.
static SMALL_RESCUES: AtomicUsize = AtomicUsize::new(0);
/// Rust's calls that reach malloc, for the `[Memory]` lines: allocations under [`LARGE`] the
/// small-block cache doesn't take, by why, then reallocs and frees malloc does.
static MALLOC_CALLS: [AtomicUsize; MALLOC_KINDS] = [const { AtomicUsize::new(0) }; MALLOC_KINDS];
/// Allocations aligned to more than [`small_blocks::MAX_ALIGN`].
const MALLOC_ALIGNED: usize = 0;
/// Allocations over [`small_blocks::MAX_SIZE`]: one index a doubling from [`SIZED_BASE`], 8-16 KiB
/// here up to 128-256 KiB at `MALLOC_SIZED + 4`, so the lines compare with builds whose cache
/// stopped at 8 KiB.
const MALLOC_SIZED: usize = 1;
const SIZED_BASE: usize = 8 << 10;
const MALLOC_REALLOCS: usize = 6;
const MALLOC_FREES: usize = 7;
pub(crate) const MALLOC_KINDS: usize = 8;

/// [`crate::release_cached_memory`].
pub(crate) fn release_cache() -> bool {
    CACHE.release()
}

/// Counts an allocation the cache didn't take, unless it was only off.
fn count_malloc_call(layout: Layout) {
    let kind = if layout.align() > small_blocks::MAX_ALIGN {
        MALLOC_ALIGNED
    } else if layout.size() > small_blocks::MAX_SIZE {
        let doublings = usize::BITS - ((layout.size() - 1) / SIZED_BASE).leading_zeros();
        MALLOC_SIZED + (doublings as usize - 1).min(4)
    } else {
        return;
    };
    MALLOC_CALLS[kind].fetch_add(1, Relaxed);
}

/// Where Rust's memory is, for the `[Memory]` lines. Reading it doesn't allocate.
#[derive(Clone, Copy)]
pub(crate) struct Figures {
    pub(crate) region: u64,
    pub(crate) malloc_large: u64,
    pub(crate) malloc_medium: u64,
    pub(crate) large_fallbacks: usize,
    pub(crate) small_rescues: usize,
    pub(crate) malloc_calls: [usize; MALLOC_KINDS],
    pub(crate) cache: small_blocks::Stats,
}

pub(crate) fn figures() -> Figures {
    Figures {
        region: REGION_BYTES.load(Relaxed) as u64,
        malloc_large: MALLOC_LARGE_BYTES.load(Relaxed) as u64,
        malloc_medium: MALLOC_MEDIUM_BYTES.load(Relaxed) as u64,
        large_fallbacks: LARGE_FALLBACKS.load(Relaxed),
        small_rescues: SMALL_RESCUES.load(Relaxed),
        malloc_calls: MALLOC_CALLS.each_ref().map(|calls| calls.load(Relaxed)),
        cache: CACHE.stats(),
    }
}

/// The granules a block of `size` bytes takes in the region. Can't overflow: a `Layout`'s size is
/// at most `isize::MAX`.
fn rounded(size: usize) -> usize {
    size.div_ceil(GRANULE) * GRANULE
}

/// Whether a block is the region's. malloc's blocks lie below `sbrk`'s break, which never passes
/// `fake_heap_end`, and the region's lie at or above it. NVK moves `fake_heap_end` only past free
/// memory (it grows the region only 16 MiB or more above the break, and shrinks it only up to
/// its lowest block), so the answer for a live block never changes and needs no lock.
fn in_region(ptr: *mut u8) -> bool {
    ptr as usize >= crate::memory::heap_limit()
}

fn count_malloc(size: usize, added: bool) {
    let counter = if size >= LARGE {
        &MALLOC_LARGE_BYTES
    } else if size >= MEDIUM {
        &MALLOC_MEDIUM_BYTES
    } else {
        return;
    };
    if added {
        counter.fetch_add(size, Relaxed);
    } else {
        counter.fetch_sub(size, Relaxed);
    }
}

unsafe fn region_alloc(layout: Layout, zeroed: bool) -> *mut u8 {
    let ptr = unsafe { nouveau_horizon_region_alloc(layout.align(), layout.size()) };
    if !ptr.is_null() {
        REGION_BYTES.fetch_add(rounded(layout.size()), Relaxed);
        // A granule can come back from an earlier block, so it isn't zero.
        if zeroed {
            unsafe { ptr.write_bytes(0, layout.size()) };
        }
    }
    ptr
}

unsafe fn malloc_alloc(layout: Layout, zeroed: bool) -> *mut u8 {
    let ptr = unsafe {
        if zeroed {
            System.alloc_zeroed(layout)
        } else {
            System.alloc(layout)
        }
    };
    if !ptr.is_null() {
        count_malloc(layout.size(), true);
    }
    ptr
}

/// `ptr`, or if it's null because malloc ran out, what `again` gives once the cache has given
/// malloc its blocks.
#[inline]
fn or_after_drain(ptr: *mut u8, again: impl FnOnce() -> *mut u8) -> *mut u8 {
    if ptr.is_null() && CACHE.drain() { again() } else { ptr }
}

unsafe fn allocate(layout: Layout, zeroed: bool) -> *mut u8 {
    if layout.size() >= LARGE {
        let ptr = unsafe { region_alloc(layout, zeroed) };
        if !ptr.is_null() {
            return ptr;
        }
        LARGE_FALLBACKS.fetch_add(1, Relaxed);
        let ptr = unsafe { malloc_alloc(layout, zeroed) };
        return or_after_drain(ptr, || unsafe { malloc_alloc(layout, zeroed) });
    }
    let small = || match CACHE.class_for(layout.size(), layout.align()) {
        Some(class) => unsafe { CACHE.alloc(class, layout.size(), zeroed) },
        None => {
            count_malloc_call(layout);
            unsafe { malloc_alloc(layout, zeroed) }
        }
    };
    let ptr = or_after_drain(small(), small);
    if !ptr.is_null() {
        return ptr;
    }
    // malloc is full, but a free run in the region can still hold the block.
    let ptr = unsafe { region_alloc(layout, zeroed) };
    if !ptr.is_null() {
        SMALL_RESCUES.fetch_add(1, Relaxed);
    }
    ptr
}

unsafe fn deallocate(ptr: *mut u8, layout: Layout) {
    if in_region(ptr) {
        // False can't happen, by `in_region`'s reasoning; leaking the block is safer than giving
        // malloc memory that isn't its own.
        if unsafe { nouveau_horizon_region_free(ptr, layout.size()) } {
            REGION_BYTES.fetch_sub(rounded(layout.size()), Relaxed);
        }
        return;
    }
    if let Some(class) = CACHE.class_for(layout.size(), layout.align()) {
        return unsafe { CACHE.free(ptr, class) };
    }
    count_malloc(layout.size(), false);
    MALLOC_CALLS[MALLOC_FREES].fetch_add(1, Relaxed);
    unsafe { System.dealloc(ptr, layout) }
}

/// Shrinks a region block in place, freeing its granules past `new_rounded`.
unsafe fn release_tail(ptr: *mut u8, new_rounded: usize, old_rounded: usize) {
    let tail = old_rounded - new_rounded;
    if unsafe { nouveau_horizon_region_free(ptr.add(new_rounded), tail) } {
        REGION_BYTES.fetch_sub(tail, Relaxed);
    }
}

/// Copies a block to `new` and frees it.
unsafe fn moved(ptr: *mut u8, layout: Layout, new: *mut u8, new_size: usize) -> *mut u8 {
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
        deallocate(ptr, layout);
    }
    new
}

unsafe impl GlobalAlloc for HorizonAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { allocate(layout, false) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { allocate(layout, true) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { deallocate(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `GlobalAlloc::realloc`'s caller guarantees `new_size`, rounded up to the
        // alignment, doesn't overflow `isize`.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };

        if in_region(ptr) {
            let (old_rounded, new_rounded) = (rounded(layout.size()), rounded(new_size));
            if new_rounded == old_rounded {
                return ptr;
            }
            if new_rounded < old_rounded && new_size >= LARGE {
                unsafe { release_tail(ptr, new_rounded, old_rounded) };
                return ptr;
            }
            // Growing, or small enough for malloc now.
            let new = unsafe { allocate(new_layout, false) };
            if !new.is_null() {
                return unsafe { moved(ptr, layout, new, new_size) };
            }
            if new_rounded < old_rounded {
                unsafe { release_tail(ptr, new_rounded, old_rounded) };
                return ptr;
            }
            return new;
        }

        let (old_class, new_class) = (
            CACHE.class_for(layout.size(), layout.align()),
            CACHE.class_for(new_size, layout.align()),
        );
        if old_class.is_some() || new_class.is_some() {
            // A block that stays in its class stays put: it was allocated at the class's size,
            // unless the cache was off then.
            if old_class == new_class && unsafe { CACHE.usable_size(ptr) } >= new_size {
                return ptr;
            }
            // In or out of the cache's sizes, or to another class: a new block, so that every
            // block the cache keeps is its class's size.
            let new = unsafe { allocate(new_layout, false) };
            if new.is_null() {
                return new;
            }
            return unsafe { moved(ptr, layout, new, new_size) };
        }

        if new_size >= LARGE {
            let new = unsafe { region_alloc(new_layout, false) };
            if !new.is_null() {
                return unsafe { moved(ptr, layout, new, new_size) };
            }
            LARGE_FALLBACKS.fetch_add(1, Relaxed);
        }
        MALLOC_CALLS[MALLOC_REALLOCS].fetch_add(1, Relaxed);
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        // A failed realloc leaves the block as it was.
        let new = or_after_drain(new, || unsafe { System.realloc(ptr, layout, new_size) });
        if !new.is_null() {
            count_malloc(layout.size(), false);
            count_malloc(new_size, true);
            return new;
        }
        if new_size >= LARGE {
            return new;
        }
        // malloc is full, but a free run in the region can still hold the block.
        let new = unsafe { region_alloc(new_layout, false) };
        if new.is_null() {
            return new;
        }
        SMALL_RESCUES.fetch_add(1, Relaxed);
        unsafe { moved(ptr, layout, new, new_size) }
    }
}
