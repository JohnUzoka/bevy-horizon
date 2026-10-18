use super::TaskPool;
use bevy_platform::sync::OnceLock;
use core::ops::Deref;

macro_rules! taskpool {
    ($(#[$attr:meta])* ($static:ident, $type:ident)) => {
        static $static: OnceLock<$type> = OnceLock::new();

        $(#[$attr])*
        #[derive(Debug)]
        pub struct $type(TaskPool);

        impl $type {
            #[doc = concat!(" Gets the global [`", stringify!($type), "`] instance, or initializes it with `f`.")]
            pub fn get_or_init(f: impl FnOnce() -> TaskPool) -> &'static Self {
                $static.get_or_init(|| Self(f()))
            }

            #[doc = concat!(" Attempts to get the global [`", stringify!($type), "`] instance, \
                or returns `None` if it is not initialized.")]
            pub fn try_get() -> Option<&'static Self> {
                $static.get()
            }

            #[doc = concat!(" Gets the global [`", stringify!($type), "`] instance.")]
            #[doc = ""]
            #[doc = " # Panics"]
            #[doc = " Panics if the global instance has not been initialized yet."]
            pub fn get() -> &'static Self {
                $static.get().expect(
                    concat!(
                        "The ",
                        stringify!($type),
                        " has not been initialized yet. Please call ",
                        stringify!($type),
                        "::get_or_init beforehand."
                    )
                )
            }
        }

        impl Deref for $type {
            type Target = TaskPool;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

taskpool! {
    /// A newtype for a task pool for CPU-intensive work that must be completed to
    /// deliver the next frame
    ///
    /// See [`TaskPool`] documentation for details on Bevy tasks.
    /// [`AsyncComputeTaskPool`] should be preferred if the work does not have to be
    /// completed before the next frame.
    (COMPUTE_TASK_POOL, ComputeTaskPool)
}

taskpool! {
    /// A newtype for a task pool for CPU-intensive work that may span across multiple frames
    ///
    /// See [`TaskPool`] documentation for details on Bevy tasks.
    /// Use [`ComputeTaskPool`] if the work must be complete before advancing to the next frame.
    (ASYNC_COMPUTE_TASK_POOL, AsyncComputeTaskPool)
}

taskpool! {
    /// A newtype for a task pool for IO-intensive work (i.e. tasks that spend very little time in a
    /// "woken" state)
    ///
    /// See [`TaskPool`] documentation for details on Bevy tasks.
    (IO_TASK_POOL, IoTaskPool)
}

/// Shuts down the global pools that have been initialized ([`ComputeTaskPool`],
/// [`AsyncComputeTaskPool`] and [`IoTaskPool`]) and joins their threads, waiting `timeout` at most
/// in all. Returns how many of their threads are still running. See [`TaskPool::shut_down`]: only
/// call this once the process is done with Bevy.
///
/// Added by bevy_horizon: the global pools are never dropped, so nothing else stops their threads
/// ([bevyengine/bevy#9477](https://github.com/bevyengine/bevy/issues/9477)).
#[cfg(all(not(target_arch = "wasm32"), feature = "multi_threaded"))]
pub fn shut_down_global_task_pools(timeout: core::time::Duration) -> usize {
    use std::time::Instant;

    let pools: [Option<&TaskPool>; 3] = [
        COMPUTE_TASK_POOL.get().map(|pool| &pool.0),
        ASYNC_COMPUTE_TASK_POOL.get().map(|pool| &pool.0),
        IO_TASK_POOL.get().map(|pool| &pool.0),
    ];
    // Close them all first, so every thread winds down at once.
    for pool in pools.iter().flatten() {
        pool.close();
    }
    let deadline = Instant::now() + timeout;
    pools
        .iter()
        .flatten()
        .map(|pool| pool.shut_down(deadline.saturating_duration_since(Instant::now())))
        .sum()
}

/// Shuts down the global pools and joins their threads. Without `multi_threaded`, or on the web,
/// the pools have no threads of their own, so there is nothing to do.
#[cfg(not(all(not(target_arch = "wasm32"), feature = "multi_threaded")))]
pub fn shut_down_global_task_pools(_timeout: core::time::Duration) -> usize {
    0
}

/// How many threads of their own the initialized global pools started: what
/// [`shut_down_global_task_pools`] has to join. Unlike [`TaskPool::thread_num`], this is 0 without
/// `multi_threaded` or on the web, where tasks run on the thread that ticks the pools.
///
/// Added by bevy_horizon, alongside [`shut_down_global_task_pools`].
pub fn global_task_pool_threads() -> usize {
    #[cfg(all(not(target_arch = "wasm32"), feature = "multi_threaded"))]
    {
        [
            COMPUTE_TASK_POOL.get().map(|pool| &pool.0),
            ASYNC_COMPUTE_TASK_POOL.get().map(|pool| &pool.0),
            IO_TASK_POOL.get().map(|pool| &pool.0),
        ]
        .iter()
        .flatten()
        .map(|pool| pool.thread_num())
        .sum()
    }
    #[cfg(not(all(not(target_arch = "wasm32"), feature = "multi_threaded")))]
    {
        0
    }
}

crate::cfg::web! {
    if {} else {
        /// A function used by `bevy_app` to tick the global tasks pools on the main thread.
        /// This will run a maximum of 100 local tasks per executor per call to this function.
        ///
        /// # Warning
        ///
        /// This function *must* be called on the main thread, or the task pools will not be updated appropriately.
        pub fn tick_global_task_pools_on_main_thread() {
            COMPUTE_TASK_POOL
                .get()
                .unwrap()
                .with_local_executor(|compute_local_executor| {
                    ASYNC_COMPUTE_TASK_POOL
                        .get()
                        .unwrap()
                        .with_local_executor(|async_local_executor| {
                            IO_TASK_POOL
                                .get()
                                .unwrap()
                                .with_local_executor(|io_local_executor| {
                                    for _ in 0..100 {
                                        compute_local_executor.try_tick();
                                        async_local_executor.try_tick();
                                        io_local_executor.try_tick();
                                    }
                                });
                        });
                });
        }
    }
}
