//! bevy-horizon: memory maps on Horizon (Nintendo Switch), held on the heap.
//!
//! Horizon has no `mmap`: homebrew can't map a file's pages into memory, and libnx's file
//! systems only copy data in and out. memmap2's `stub.rs`, which this backend replaces, failed
//! every map with `Unsupported`. Here a map is a heap allocation filled from the file when the
//! map is made:
//!
//! - `map` (`Mmap::map`, `MmapOptions::map`, `map_raw_read_only`) and `map_copy_read_only` read
//!   the range in. Unlike a real map, the whole range is read up front and takes that much
//!   memory, and later changes to the file don't show up in it. Bytes past the end of the file
//!   read as zero.
//! - `map_copy` reads the range into a writable copy of its own, which is what a private
//!   (copy-on-write) map is.
//! - `map_anon` allocates zeroed memory.
//! - `map_mut` (a writable map shared with the file, also behind `map_raw`) and `map_exec`
//!   return `ErrorKind::Unsupported`: writes would never reach the file, and homebrew can't make
//!   heap memory executable. `make_mut` on a map of a file returns it too, for the first reason.
//!
//! A map too big to allocate returns `ErrorKind::OutOfMemory`, so a caller can still fall back
//! to reading the file in pieces. The allocation is page-aligned, as `mmap`'s memory is.
//! newlib has no `pread`, so the range is read by seeking the file and seeking back after; don't
//! read the same `File` from another thread while a map of it is being made.

use std::alloc::{self, Layout};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ptr::NonNull;

/// The alignment `mmap` gives.
const PAGE_SIZE: usize = 0x1000;

pub struct MmapInner {
    ptr: NonNull<u8>,
    len: usize,
    /// Made by `map`. A real map of a file is shared with it, so `make_mut` would send writes to
    /// the file; this copy can't, and refuses.
    file_contents: bool,
}

// SAFETY: the allocation is owned by this map alone, as the Unix backend's mapping is.
unsafe impl Send for MmapInner {}
unsafe impl Sync for MmapInner {}

fn unsupported(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} isn't supported on Horizon, which has no mmap"),
    )
}

impl MmapInner {
    fn allocate(len: usize, file_contents: bool) -> io::Result<MmapInner> {
        let ptr = if len == 0 {
            NonNull::dangling()
        } else {
            let layout = Layout::from_size_align(len, PAGE_SIZE)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            // SAFETY: the layout's size isn't zero.
            NonNull::new(unsafe { alloc::alloc_zeroed(layout) }).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("can't allocate {len} bytes to hold a memory map"),
                )
            })?
        };
        Ok(MmapInner {
            ptr,
            len,
            file_contents,
        })
    }

    /// Reads `len` bytes of `file` from `offset` into a new map. Bytes past the end of the file
    /// stay zero.
    fn read(len: usize, file: &File, offset: u64, file_contents: bool) -> io::Result<MmapInner> {
        let mut inner = MmapInner::allocate(len, file_contents)?;
        if len == 0 {
            return Ok(inner);
        }
        // SAFETY: `allocate` made `len` zeroed bytes at `ptr`, which only `inner` refers to.
        let buf = unsafe { std::slice::from_raw_parts_mut(inner.mut_ptr(), len) };
        let mut file = file;
        let position = file.stream_position()?;
        let read = file.seek(SeekFrom::Start(offset)).and_then(|_| {
            let mut filled = 0;
            while filled < len {
                match file.read(&mut buf[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        });
        let restored = file.seek(SeekFrom::Start(position));
        read?;
        restored?;
        Ok(inner)
    }

    pub fn map(len: usize, file: &File, offset: u64, _: bool, _: bool) -> io::Result<MmapInner> {
        MmapInner::read(len, file, offset, true)
    }

    pub fn map_exec(_: usize, _: &File, _: u64, _: bool, _: bool) -> io::Result<MmapInner> {
        Err(unsupported("an executable map"))
    }

    pub fn map_mut(_: usize, _: &File, _: u64, _: bool, _: bool) -> io::Result<MmapInner> {
        Err(unsupported("a writable map shared with its file"))
    }

    pub fn map_copy(
        len: usize,
        file: &File,
        offset: u64,
        _: bool,
        _: bool,
    ) -> io::Result<MmapInner> {
        MmapInner::read(len, file, offset, false)
    }

    pub fn map_copy_read_only(
        len: usize,
        file: &File,
        offset: u64,
        _: bool,
        _: bool,
    ) -> io::Result<MmapInner> {
        MmapInner::read(len, file, offset, false)
    }

    pub fn map_anon(len: usize, _: bool, _: bool, _: Option<u8>, _: bool) -> io::Result<MmapInner> {
        MmapInner::allocate(len, false)
    }

    /// Nothing to write back: no map here is shared with a file.
    pub fn flush(&self, offset: usize, len: usize) -> io::Result<()> {
        if offset > self.len || len > self.len - offset {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }

    pub fn flush_async(&self, offset: usize, len: usize) -> io::Result<()> {
        self.flush(offset, len)
    }

    /// Heap memory has no protection to change.
    pub fn make_read_only(&mut self) -> io::Result<()> {
        Ok(())
    }

    pub fn make_exec(&mut self) -> io::Result<()> {
        Err(unsupported("an executable map"))
    }

    pub fn make_mut(&mut self) -> io::Result<()> {
        if self.file_contents {
            return Err(unsupported("making a map of a file writable"));
        }
        Ok(())
    }

    #[inline]
    pub fn ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    #[inline]
    pub fn mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }
}

impl Drop for MmapInner {
    fn drop(&mut self) {
        if self.len != 0 {
            // SAFETY: `allocate` made this allocation with this layout.
            unsafe {
                alloc::dealloc(
                    self.ptr.as_ptr(),
                    Layout::from_size_align_unchecked(self.len, PAGE_SIZE),
                )
            };
        }
    }
}

pub fn file_len(file: &File) -> io::Result<u64> {
    Ok(file.metadata()?.len())
}
