//! A cache of Rust's small blocks in front of newlib's malloc.
//!
//! devkitA64's newlib malloc is Doug Lea's malloc 2.6.5 behind one lock (`__malloc_lock`), and it
//! sorts every freed block into its size bin there and then. In Skate 3's perf4 CPU profile (30 s,
//! a sample every 2 ms), a fifth of the game's running samples were in the allocator, most of them
//! in `_malloc_r` and `_free_r`, and the main and render threads spent about 9 and 6 ms a frame
//! waiting for it. Here a freed block of up to [`MAX_SIZE`] stays in one of [`CLASSES`] size
//! classes, and the next allocation of its class takes it back without going to malloc.
//!
//! The design is Bonwick and Adams's magazines (*Magazines and Vmem*, USENIX 2001). Threads share
//! [`SHARDS`] shards, picked by the address of the thread's local region on Horizon, so threads
//! made one after another get different ones. A shard keeps two magazines (lists of free blocks)
//! per class, and a depot behind the shards keeps a few full magazines per class, which moves
//! blocks one thread frees to another that allocates them. Its locks are flags that are only ever
//! tried, never waited for: a thread that finds its shard's held tries one other shard, then goes
//! to malloc. Horizon runs each core's highest-priority ready thread, so a thread spinning on a
//! flag held by a lower-priority thread on its own core would keep that thread from ever letting
//! go of it.
//!
//! A cached block is a malloc block of its class's size, so malloc counts it as in use, and it can
//! always go back to `free`. The most the cache holds is about 8.3 MiB in the shards and
//! [`DEPOT_BYTES`] (8 MiB) in the depot; [`Stats::cached`] says how much it does. When malloc runs out, the allocator
//! gives it the cache's blocks ([`Cache::drain`]) and asks again.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The size classes: 16 to 128 bytes in steps of 16, then four to each doubling, up to 32 KiB.
pub(crate) const CLASSES: usize = 40;
/// The largest block the cache keeps. In Skate 3's play on Ryujinx, 93 % of Rust's allocations
/// that still reached malloc with 8 KiB here were 8-32 KiB, about 146 a frame.
pub(crate) const MAX_SIZE: usize = 32 << 10;
/// The largest alignment the cache serves. malloc's blocks are aligned to two `size_t`s, 16 bytes
/// on AArch64 (newlib `libc/stdlib/_mallocr.c`, `MALLOC_ALIGNMENT`), and every class's size is a
/// multiple of 16.
pub(crate) const MAX_ALIGN: usize = 16;
/// Shards. Horizon gives a thread's local region 0x200 bytes, eight to a page, so eight threads
/// made one after another land in eight different shards.
pub(crate) const SHARDS: usize = 8;
/// About how many bytes a full magazine holds: 32 blocks of up to 256 bytes, two of 4 KiB and up.
const MAGAZINE_BYTES: usize = 8 << 10;
const MIN_MAGAZINE: usize = 2;
const MAX_MAGAZINE: usize = 32;
/// Bytes the depot keeps in full magazines, all classes together. Counts per class (32 magazines
/// up to 8 KiB, 4 above) overflowed on the console: in Skate 3's play, 801-1,277 full magazines a
/// second went back to malloc, and 5-11 % of the allocations the cache took found nothing, while
/// the cache held under 5 MiB. One budget lets the classes a frame frees most keep their blocks
/// for the next; 8 MiB is about what the counts allowed altogether.
const DEPOT_BYTES: usize = 8 << 20;
/// Tries at a depot slot before a magazine goes back to malloc or an allocation to malloc. Its
/// holder keeps it for a few instructions, so trying again finds it free unless the holder was
/// preempted, and a bounded number can't starve a holder on the same core.
const DEPOT_TRIES: usize = 4;

/// Where blocks come from and go back to: newlib's malloc on the Switch.
pub(crate) trait Backend {
    /// A block of `size` bytes aligned to [`MAX_ALIGN`], zeroed if `zeroed`, or null.
    unsafe fn alloc(size: usize, zeroed: bool) -> *mut u8;
    /// Frees a block from [`Backend::alloc`].
    unsafe fn free(ptr: *mut u8);
    /// The bytes a live block from the backend can hold.
    unsafe fn usable_size(ptr: *mut u8) -> usize;
    /// A number particular to the calling thread; its low bits pick the shard.
    fn shard_hint() -> usize;
}

/// The class a block of `size` bytes aligned to `align` goes in, or `None` if the cache doesn't
/// take it.
#[inline]
pub(crate) fn class_of(size: usize, align: usize) -> Option<usize> {
    if align > MAX_ALIGN || size > MAX_SIZE {
        return None;
    }
    if size <= 128 {
        return Some(size.max(1).div_ceil(16) - 1);
    }
    // 129..=8192: four classes from just above each power of two to the next, from 128 up.
    let lg = (usize::BITS - 1 - (size - 1).leading_zeros()) as usize;
    let sub = (size - 1) >> (lg - 2);
    Some(8 + (lg - 7) * 4 + (sub - 4))
}

/// The size of a class's blocks.
pub(crate) const fn class_size(class: usize) -> usize {
    if class < 8 {
        (class + 1) * 16
    } else {
        let lg = 7 + (class - 8) / 4;
        let sub = 4 + (class - 8) % 4;
        (sub + 1) << (lg - 2)
    }
}

/// The blocks in a class's full magazine.
pub(crate) const fn magazine_len(class: usize) -> usize {
    let n = MAGAZINE_BYTES / class_size(class);
    if n < MIN_MAGAZINE {
        MIN_MAGAZINE
    } else if n > MAX_MAGAZINE {
        MAX_MAGAZINE
    } else {
        n
    }
}

const SIZES: [usize; CLASSES] = {
    let mut sizes = [0; CLASSES];
    let mut class = 0;
    while class < CLASSES {
        sizes[class] = class_size(class);
        class += 1;
    }
    sizes
};

const LENS: [usize; CLASSES] = {
    let mut lens = [0; CLASSES];
    let mut class = 0;
    while class < CLASSES {
        lens[class] = magazine_len(class);
        class += 1;
    }
    lens
};

/// What the cache has done, for the `[Memory]` lines.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Stats {
    pub(crate) enabled: bool,
    /// Allocations the cache served.
    pub(crate) hits: usize,
    /// Allocations it had no block for, which malloc served.
    pub(crate) misses: usize,
    /// Allocations that found both shards they tried held, which malloc served.
    pub(crate) busy_allocs: usize,
    /// Frees that found both shards they tried held, which went to malloc.
    pub(crate) busy_frees: usize,
    /// Frees of blocks smaller than their class, which went to malloc: blocks allocated while the
    /// cache was off, or C's.
    pub(crate) rejected: usize,
    /// Magazines a shard took from the depot.
    pub(crate) refills: usize,
    /// Full magazines a shard gave the depot.
    pub(crate) flushes: usize,
    /// Full magazines given back to malloc because the depot was full or held.
    pub(crate) returns: usize,
    /// Tries at a depot slot, to give or take a magazine, that found it held [`DEPOT_TRIES`] times.
    pub(crate) depot_held: usize,
    /// Magazines the depot turned away for [`DEPOT_BYTES`].
    pub(crate) depot_full: usize,
    /// Times malloc ran out with blocks in the cache, which gave them back.
    pub(crate) drains: usize,
    /// Bytes in cached blocks.
    pub(crate) cached: usize,
}

/// A class's magazines in one shard.
#[derive(Clone, Copy)]
struct Magazines {
    /// Free blocks, each holding the next in its first word; null-terminated.
    current: *mut u8,
    /// Blocks in `current`, at most the class's [`magazine_len`].
    len: usize,
    /// A full magazine, or null.
    spare: *mut u8,
}

impl Magazines {
    const EMPTY: Self = Self { current: null_mut(), len: 0, spare: null_mut() };
}

#[repr(align(64))]
struct Shard {
    locked: AtomicBool,
    classes: UnsafeCell<[Magazines; CLASSES]>,
    // Written only with the shard held; atomics so that `stats` can read them.
    hits: AtomicUsize,
    misses: AtomicUsize,
    cached: AtomicUsize,
}

impl Shard {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            classes: UnsafeCell::new([Magazines::EMPTY; CLASSES]),
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
            cached: AtomicUsize::new(0),
        }
    }
}

/// A class's full magazines, kept for whichever shard runs out first.
#[repr(align(64))]
struct DepotSlot {
    locked: AtomicBool,
    /// The first block of the newest magazine; each magazine's first block holds the next
    /// magazine's in its second word.
    head: UnsafeCell<*mut u8>,
    /// Written only with the slot held.
    len: AtomicUsize,
}

impl DepotSlot {
    const fn new() -> Self {
        Self { locked: AtomicBool::new(false), head: UnsafeCell::new(null_mut()), len: AtomicUsize::new(0) }
    }
}

/// A held flag, let go when dropped.
struct Held<'a>(&'a AtomicBool);

impl Drop for Held<'_> {
    #[inline]
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[inline]
fn try_hold(flag: &AtomicBool) -> Option<Held<'_>> {
    // Looking first leaves a held flag's cache line shared instead of taking it from the holder.
    if flag.load(Ordering::Relaxed) {
        return None;
    }
    flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).ok().map(|_| Held(flag))
}

/// [`try_hold`] up to [`DEPOT_TRIES`] times.
#[inline]
fn try_hold_soon(flag: &AtomicBool) -> Option<Held<'_>> {
    for _ in 1..DEPOT_TRIES {
        if let Some(held) = try_hold(flag) {
            return Some(held);
        }
        core::hint::spin_loop();
    }
    try_hold(flag)
}

/// For turning the cache off, where waiting is fine: sleeping, unlike yielding, lets a
/// lower-priority holder run. Gives up after about 10 ms.
fn hold_patiently(flag: &AtomicBool) -> Option<Held<'_>> {
    for _ in 0..100 {
        if let Some(held) = try_hold(flag) {
            return Some(held);
        }
        std::thread::sleep(core::time::Duration::from_micros(100));
    }
    None
}

/// Adds to a counter only ever written with its shard or slot held.
#[inline]
fn add(counter: &AtomicUsize, n: usize) {
    counter.store(counter.load(Ordering::Relaxed).wrapping_add(n), Ordering::Relaxed);
}

#[inline]
fn sub(counter: &AtomicUsize, n: usize) {
    counter.store(counter.load(Ordering::Relaxed).wrapping_sub(n), Ordering::Relaxed);
}

/// The block after `block` in its list.
#[inline]
unsafe fn next(block: *mut u8) -> *mut u8 {
    unsafe { block.cast::<*mut u8>().read() }
}

#[inline]
unsafe fn set_next(block: *mut u8, next: *mut u8) {
    unsafe { block.cast::<*mut u8>().write(next) }
}

/// The magazine after the one `head` starts in the depot.
#[inline]
unsafe fn next_magazine(head: *mut u8) -> *mut u8 {
    unsafe { head.cast::<*mut u8>().add(1).read() }
}

#[inline]
unsafe fn set_next_magazine(head: *mut u8, next: *mut u8) {
    unsafe { head.cast::<*mut u8>().add(1).write(next) }
}

pub(crate) struct Cache<B> {
    enabled: AtomicBool,
    shards: [Shard; SHARDS],
    depot: [DepotSlot; CLASSES],
    /// Bytes in the depot's magazines, against [`DEPOT_BYTES`].
    depot_bytes: AtomicUsize,
    depot_held: AtomicUsize,
    depot_full: AtomicUsize,
    busy_allocs: AtomicUsize,
    busy_frees: AtomicUsize,
    rejected: AtomicUsize,
    refills: AtomicUsize,
    flushes: AtomicUsize,
    returns: AtomicUsize,
    drains: AtomicUsize,
    backend: PhantomData<B>,
}

// SAFETY: a shard's magazines and a depot slot's list are only touched with its flag held, and
// the flags' Acquire and Release order those accesses between threads.
unsafe impl<B> Sync for Cache<B> {}

impl<B: Backend> Cache<B> {
    pub(crate) const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            shards: [const { Shard::new() }; SHARDS],
            depot: [const { DepotSlot::new() }; CLASSES],
            depot_bytes: AtomicUsize::new(0),
            depot_held: AtomicUsize::new(0),
            depot_full: AtomicUsize::new(0),
            busy_allocs: AtomicUsize::new(0),
            busy_frees: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
            refills: AtomicUsize::new(0),
            flushes: AtomicUsize::new(0),
            returns: AtomicUsize::new(0),
            drains: AtomicUsize::new(0),
            backend: PhantomData,
        }
    }

    /// The class for a block of `size` bytes aligned to `align`, or `None` if the cache is off or
    /// doesn't take it.
    #[inline]
    pub(crate) fn class_for(&self, size: usize, align: usize) -> Option<usize> {
        if !self.enabled.load(Ordering::Relaxed) {
            return None;
        }
        class_of(size, align)
    }

    /// The bytes a live block can hold.
    #[inline]
    pub(crate) unsafe fn usable_size(&self, ptr: *mut u8) -> usize {
        unsafe { B::usable_size(ptr) }
    }

    /// This thread's shard, or the other one it tries, held; `None` if both are held.
    #[inline]
    fn hold_shard(&self) -> Option<(&Shard, Held<'_>)> {
        let hint = B::shard_hint();
        for index in [hint % SHARDS, (hint ^ (SHARDS / 2)) % SHARDS] {
            let shard = &self.shards[index];
            if let Some(held) = try_hold(&shard.locked) {
                return Some((shard, held));
            }
        }
        None
    }

    /// A block of `class`, the class `class_for` gave for `size` bytes, with those bytes zeroed if
    /// `zeroed`; null if malloc has no room for one.
    ///
    /// # Safety
    /// `class` is below [`CLASSES`].
    #[inline]
    pub(crate) unsafe fn alloc(&self, class: usize, size: usize, zeroed: bool) -> *mut u8 {
        if let Some((shard, held)) = self.hold_shard() {
            // Turning the cache off empties each shard with it held; looking again here keeps
            // this from refilling one from the depot after that.
            if self.enabled.load(Ordering::Relaxed) {
                let block = unsafe { self.pop(shard, class) };
                if !block.is_null() {
                    add(&shard.hits, 1);
                    drop(held);
                    if zeroed {
                        unsafe { block.write_bytes(0, size) };
                    }
                    return block;
                }
                add(&shard.misses, 1);
            }
            drop(held);
        } else {
            self.busy_allocs.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { B::alloc(SIZES[class], zeroed) }
    }

    /// Takes a block of `class` from the shard, or null if it and the depot have none.
    #[inline]
    unsafe fn pop(&self, shard: &Shard, class: usize) -> *mut u8 {
        // SAFETY: the shard is held.
        let mags = unsafe { &mut (*shard.classes.get())[class] };
        if mags.len == 0 {
            if !mags.spare.is_null() {
                mags.current = core::mem::replace(&mut mags.spare, null_mut());
            } else {
                let magazine = self.depot_take(class);
                if magazine.is_null() {
                    return null_mut();
                }
                mags.current = magazine;
                add(&shard.cached, LENS[class] * SIZES[class]);
            }
            mags.len = LENS[class];
        }
        let block = mags.current;
        mags.current = unsafe { next(block) };
        mags.len -= 1;
        sub(&shard.cached, SIZES[class]);
        block
    }

    /// Keeps `ptr`, a block of `class`, or gives it back to malloc.
    ///
    /// # Safety
    /// `ptr` is a live block from the backend, `class` is the class of the size it was allocated
    /// for, and `ptr` isn't used again.
    #[inline]
    pub(crate) unsafe fn free(&self, ptr: *mut u8, class: usize) {
        // A block allocated while the cache was off, or by C, can be smaller than its class.
        if unsafe { B::usable_size(ptr) } < SIZES[class] {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            return unsafe { B::free(ptr) };
        }
        let Some((shard, held)) = self.hold_shard() else {
            self.busy_frees.fetch_add(1, Ordering::Relaxed);
            return unsafe { B::free(ptr) };
        };
        // Turning the cache off empties each shard with it held, so nothing goes in after that.
        if !self.enabled.load(Ordering::Relaxed) {
            drop(held);
            return unsafe { B::free(ptr) };
        }
        let returned = unsafe { self.push(shard, class, ptr) };
        drop(held);
        if !returned.is_null() {
            self.returns.fetch_add(1, Ordering::Relaxed);
            unsafe { Self::free_list(returned) };
        }
    }

    /// Puts a block in the shard. Returns a full magazine to give back to malloc, or null.
    #[inline]
    unsafe fn push(&self, shard: &Shard, class: usize, block: *mut u8) -> *mut u8 {
        // SAFETY: the shard is held.
        let mags = unsafe { &mut (*shard.classes.get())[class] };
        let mut returned = null_mut();
        if mags.len == LENS[class] {
            if !mags.spare.is_null() {
                sub(&shard.cached, LENS[class] * SIZES[class]);
                if unsafe { self.depot_put(class, mags.spare) } {
                    self.flushes.fetch_add(1, Ordering::Relaxed);
                } else {
                    returned = mags.spare;
                }
            }
            mags.spare = core::mem::replace(&mut mags.current, null_mut());
            mags.len = 0;
        }
        unsafe { set_next(block, mags.current) };
        mags.current = block;
        mags.len += 1;
        add(&shard.cached, SIZES[class]);
        returned
    }

    /// A full magazine of `class` from the depot, or null.
    fn depot_take(&self, class: usize) -> *mut u8 {
        let slot = &self.depot[class];
        if slot.len.load(Ordering::Relaxed) == 0 {
            return null_mut();
        }
        let Some(_held) = try_hold_soon(&slot.locked) else {
            self.depot_held.fetch_add(1, Ordering::Relaxed);
            return null_mut();
        };
        // SAFETY: the slot is held.
        let head = unsafe { &mut *slot.head.get() };
        let magazine = *head;
        if !magazine.is_null() {
            *head = unsafe { next_magazine(magazine) };
            sub(&slot.len, 1);
            self.depot_bytes.fetch_sub(LENS[class] * SIZES[class], Ordering::Relaxed);
            self.refills.fetch_add(1, Ordering::Relaxed);
        }
        magazine
    }

    /// Gives the depot a full magazine of `class`. False if it's full or held.
    unsafe fn depot_put(&self, class: usize, magazine: *mut u8) -> bool {
        let bytes = LENS[class] * SIZES[class];
        // Threads putting at once can pass this together, by a magazine each.
        if self.depot_bytes.load(Ordering::Relaxed) + bytes > DEPOT_BYTES {
            self.depot_full.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let slot = &self.depot[class];
        let Some(_held) = try_hold_soon(&slot.locked) else {
            self.depot_held.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        // SAFETY: the slot is held.
        let head = unsafe { &mut *slot.head.get() };
        unsafe { set_next_magazine(magazine, *head) };
        *head = magazine;
        add(&slot.len, 1);
        self.depot_bytes.fetch_add(bytes, Ordering::Relaxed);
        true
    }

    /// Gives every block in a list back to malloc.
    unsafe fn free_list(mut block: *mut u8) {
        while !block.is_null() {
            let following = unsafe { next(block) };
            unsafe { B::free(block) };
            block = following;
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Turns the cache on or off. Turning it off gives its blocks back to malloc, as
    /// [`Cache::drain`] does.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::SeqCst) && !enabled {
            self.empty();
        }
    }

    /// For when malloc has run out: gives it the cache's blocks, except any in a shard or depot
    /// slot that stays held for 10 ms. False if there were none to give, so asking malloc again
    /// is no use.
    pub(crate) fn drain(&self) -> bool {
        let gave = self.empty();
        if gave {
            self.drains.fetch_add(1, Ordering::Relaxed);
        }
        gave
    }

    /// Gives back the cached blocks it can hold the shards and depot slots of; true if any.
    fn empty(&self) -> bool {
        let mut gave = false;
        // Shards first: a shard gives the depot magazines only while held.
        for shard in &self.shards {
            let Some(_held) = hold_patiently(&shard.locked) else {
                continue;
            };
            // SAFETY: the shard is held.
            let classes = unsafe { &mut *shard.classes.get() };
            for mags in classes.iter_mut() {
                gave |= !mags.current.is_null() || !mags.spare.is_null();
                unsafe {
                    Self::free_list(mags.current);
                    Self::free_list(mags.spare);
                }
                *mags = Magazines::EMPTY;
            }
            shard.cached.store(0, Ordering::Relaxed);
        }
        for (class, slot) in self.depot.iter().enumerate() {
            let Some(_held) = hold_patiently(&slot.locked) else {
                continue;
            };
            // SAFETY: the slot is held.
            let head = unsafe { &mut *slot.head.get() };
            while !head.is_null() {
                gave = true;
                let magazine = *head;
                *head = unsafe { next_magazine(magazine) };
                unsafe { Self::free_list(magazine) };
            }
            let magazines = slot.len.swap(0, Ordering::Relaxed);
            self.depot_bytes.fetch_sub(magazines * LENS[class] * SIZES[class], Ordering::Relaxed);
        }
        gave
    }

    /// Doesn't allocate or wait.
    pub(crate) fn stats(&self) -> Stats {
        let relaxed = |counter: &AtomicUsize| counter.load(Ordering::Relaxed);
        let mut stats = Stats {
            enabled: self.enabled(),
            busy_allocs: relaxed(&self.busy_allocs),
            busy_frees: relaxed(&self.busy_frees),
            rejected: relaxed(&self.rejected),
            refills: relaxed(&self.refills),
            flushes: relaxed(&self.flushes),
            returns: relaxed(&self.returns),
            depot_held: relaxed(&self.depot_held),
            depot_full: relaxed(&self.depot_full),
            drains: relaxed(&self.drains),
            ..Stats::default()
        };
        for shard in &self.shards {
            stats.hits += relaxed(&shard.hits);
            stats.misses += relaxed(&shard.misses);
            stats.cached = stats.cached.wrapping_add(relaxed(&shard.cached));
        }
        for (class, slot) in self.depot.iter().enumerate() {
            stats.cached = stats.cached.wrapping_add(relaxed(&slot.len) * LENS[class] * SIZES[class]);
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_cover_every_size_once() {
        let mut last = 0;
        for size in 0..=MAX_SIZE {
            let class = class_of(size, 8).unwrap();
            assert!(size <= class_size(class), "{size} -> {class}");
            if class > 0 {
                assert!(size > class_size(class - 1), "{size} -> {class}");
            }
            assert!(class == last || class == last + 1);
            last = class;
        }
        assert_eq!(last, CLASSES - 1);
        assert_eq!(class_size(CLASSES - 1), MAX_SIZE);
        assert_eq!(class_of(MAX_SIZE + 1, 8), None);
        assert_eq!(class_of(64, 32), None);
        for class in 0..CLASSES {
            assert_eq!(class_size(class) % MAX_ALIGN, 0);
            assert!((MIN_MAGAZINE..=MAX_MAGAZINE).contains(&magazine_len(class)));
        }
    }

    #[test]
    fn most_it_holds() {
        let shards: usize = (0..CLASSES).map(|c| 2 * magazine_len(c) * class_size(c)).sum::<usize>() * SHARDS;
        let depot = DEPOT_BYTES;
        println!("shards {:.2} MiB, depot {:.2} MiB", shards as f64 / 1048576.0, depot as f64 / 1048576.0);
    }
}
