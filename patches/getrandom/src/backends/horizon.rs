//! Implementation for Nintendo Switch (Horizon OS) using devkitPro libc arc4random_buf.
use crate::Error;
use core::mem::MaybeUninit;

pub use crate::util::{inner_u32, inner_u64};

unsafe extern "C" {
    fn arc4random_buf(buf: *mut core::ffi::c_void, nbytes: usize);
}

#[inline]
pub fn fill_inner(dest: &mut [MaybeUninit<u8>]) -> Result<(), Error> {
    unsafe {
        arc4random_buf(dest.as_mut_ptr().cast(), dest.len());
    }
    Ok(())
}
