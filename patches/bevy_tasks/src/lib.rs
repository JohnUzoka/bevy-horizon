#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://bevy.org/assets/icon.png",
    html_favicon_url = "https://bevy.org/assets/icon.png"
)]
#![no_std]

/// Configuration information for this crate.
pub mod cfg {
    pub(crate) use bevy_platform::cfg::*;

    pub use bevy_platform::cfg::{alloc, std, web};

    define_alias! {
        #[cfg(feature = "async_executor")] => {
            /// Indicates `async_executor` is used as the future execution backend.
            async_executor
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "multi_threaded"))] => {
            /// Indicates multithreading support.
            multi_threaded
        }

        #[cfg(target_arch = "wasm32")] => {
            /// Indicates the current target requires additional `Send` bounds.
            conditional_send
        }

    }
}

cfg::std! {
    extern crate std;
}

extern crate alloc;

cfg::conditional_send! {
    if {
        /// Use [`ConditionalSend`] to mark an optional Send trait bound. Useful as on certain platforms (eg. Wasm),
        /// futures aren't Send.
        pub trait ConditionalSend {}
        impl<T> ConditionalSend for T {}
    } else {
        /// Use [`ConditionalSend`] to mark an optional Send trait bound. Useful as on certain platforms (eg. Wasm),
        /// futures aren't Send.
        pub trait ConditionalSend: Send {}
        impl<T: Send> ConditionalSend for T {}
    }
}

/// Use [`ConditionalSendFuture`] for a future with an optional Send trait bound, as on certain platforms (eg. Wasm),
/// futures aren't Send.
pub trait ConditionalSendFuture: Future + ConditionalSend {}

impl<T: Future + ConditionalSend> ConditionalSendFuture for T {}

use alloc::boxed::Box;

/// An owned and dynamically typed Future used when you can't statically type your result or need to add some indirection.
pub type BoxedFuture<'a, T> = core::pin::Pin<Box<dyn ConditionalSendFuture<Output = T> + 'a>>;

// Modules
mod executor;
pub mod futures;
mod iter;
mod slice;
mod usages;

cfg::async_executor! {
    if {} else {
        mod edge_executor;
    }
}

// Exports
pub use async_task::Task;
pub use iter::ParallelIterator;
pub use slice::{ParallelSlice, ParallelSliceMut};
pub use usages::{
    global_task_pool_threads, shut_down_global_task_pools, AsyncComputeTaskPool, ComputeTaskPool,
    IoTaskPool,
};

pub use futures_lite;
pub use futures_lite::future::poll_once;

cfg::web! {
    if {} else {
        pub use usages::tick_global_task_pools_on_main_thread;
    }
}

cfg::multi_threaded! {
    if {
        mod task_pool;
        mod thread_executor;

        pub use task_pool::{Scope, TaskPool, TaskPoolBuilder};
        pub use thread_executor::{ThreadExecutor, ThreadExecutorTicker};
    } else {
        mod single_threaded_task_pool;

        pub use single_threaded_task_pool::{Scope, TaskPool, TaskPoolBuilder, ThreadExecutor};
    }
}

pub use bevy_platform::future::block_on;

/// The tasks prelude.
///
/// This includes the most common types in this crate, re-exported for your convenience.
pub mod prelude {
    #[doc(hidden)]
    pub use crate::{
        block_on,
        iter::ParallelIterator,
        slice::{ParallelSlice, ParallelSliceMut},
        usages::{AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool},
    };
}

/// Gets the logical CPU core count available to the current process.
///
/// This is identical to `std::thread::available_parallelism`, except
/// it will return a default value of 1 if it internally errors out
/// (bevy_horizon: on Horizon, where std always errors, the process's core count).
///
/// This will always return at least 1.
pub fn available_parallelism() -> usize {
    cfg::switch! {{
        cfg::std => {
            std::thread::available_parallelism()
                .map(core::num::NonZero::<usize>::get)
                .unwrap_or_else(|_| fallback_parallelism())
        }
        _ => {
            1
        }
    }}
}

/// bevy_horizon: std's `available_parallelism` is `Unsupported` on Horizon, which gave each of
/// Bevy's task pools a single thread. Count the cores the process may run on instead: 4 for
/// homebrew under Atmosphère's hbloader (core mask 0b1111), giving compute 2, IO 1 and async
/// compute 1; Ryujinx gives homebrew 3 cores, so compute gets 1 there.
#[cfg(target_os = "horizon")]
#[allow(dead_code, reason = "only called when std is available")]
#[allow(unsafe_code, reason = "a supervisor call through libnx; nothing else counts the cores")]
fn fallback_parallelism() -> usize {
    unsafe extern "C" {
        /// libnx's wrapper for the `GetInfo` supervisor call.
        fn svcGetInfo(out: *mut u64, id0: u32, handle: u32, id1: u64) -> u32;
    }
    const INFO_TYPE_CORE_MASK: u32 = 0;
    const CUR_PROCESS_HANDLE: u32 = 0xFFFF_8001;
    let mut mask = 0u64;
    // SAFETY: GetInfo only writes `mask`.
    let rc = unsafe { svcGetInfo(&mut mask, INFO_TYPE_CORE_MASK, CUR_PROCESS_HANDLE, 0) };
    if rc == 0 {
        (mask.count_ones() as usize).max(1)
    } else {
        1
    }
}

#[cfg(not(target_os = "horizon"))]
#[allow(dead_code, reason = "only called when std is available")]
fn fallback_parallelism() -> usize {
    1
}
