//! Bevy Engine runtime support for Nintendo Switch (Horizon OS).
//!
//! Provides libnx lifecycle initialization, Joy-Con/Pro Controller gamepad integration,
//! dynamic handheld/docked resolution handling, and the frame loop.
//!
//! Built against Bevy 0.19 by default, or 0.18 with the `bevy_0_18` feature (and default
//! features off).
//!
//! The `render` feature (`render_0_18` on Bevy 0.18) adds `HorizonUploadPlugin`: it spreads the
//! GPU uploads of a big load over several frames and lets wgpu free their staging memory as it
//! goes, so the GPU's copies don't use up the heap they share with the game.
//!
//! The `top_down_alloc` feature (with `vulkan`) sets the global allocator: Rust's blocks of
//! 256 KiB and up go in the top-down region NVK takes GPU memory from, so that freeing them after
//! a load leaves runs the GPU can use instead of holes between malloc's small blocks.

// Proving wgpu's types `Send` and `Sync` (uploads.rs takes Bevy's `MeshAllocator`) goes deeper than
// the default 128 levels; nightly rustc warns that it will stop allowing that.
#![recursion_limit = "256"]

#[cfg(all(feature = "bevy_0_19", feature = "bevy_0_18"))]
compile_error!(
    "bevy_horizon: enable one of `bevy_0_19` and `bevy_0_18`; for Bevy 0.18 set `default-features = false`"
);
#[cfg(not(any(feature = "bevy_0_19", feature = "bevy_0_18")))]
compile_error!("bevy_horizon: enable `bevy_0_19` (the default) or `bevy_0_18`, for the game's Bevy");
#[cfg(all(feature = "fps_overlay", feature = "bevy_0_18"))]
compile_error!("bevy_horizon: `fps_overlay` needs Bevy 0.19's bevy_dev_tools");
#[cfg(all(feature = "render", feature = "bevy_0_18"))]
compile_error!("bevy_horizon: a Bevy 0.18 game turns on `render_0_18`; `render` is for Bevy 0.19");
#[cfg(all(feature = "render_0_18", feature = "bevy_0_19"))]
compile_error!("bevy_horizon: `render_0_18` is for Bevy 0.18 games; with Bevy 0.19, use `render`");

// With `bevy_0_18`, the 0.18 crates take the names the 0.19 ones have, so every `use bevy_app::...`
// below means whichever Bevy the game uses. An `extern crate` at the crate root adds its name to
// the extern prelude.
#[cfg(feature = "bevy_0_18")]
extern crate bevy_app_018 as bevy_app;
#[cfg(feature = "bevy_0_18")]
extern crate bevy_ecs_018 as bevy_ecs;
#[cfg(feature = "bevy_0_18")]
extern crate bevy_input_018 as bevy_input;
#[cfg(feature = "bevy_0_18")]
extern crate bevy_tasks_018 as bevy_tasks;
#[cfg(feature = "bevy_0_18")]
extern crate bevy_window_018 as bevy_window;
#[cfg(feature = "render_0_18")]
extern crate bevy_asset_018 as bevy_asset;
#[cfg(feature = "render_0_18")]
extern crate bevy_mesh_018 as bevy_mesh;
#[cfg(feature = "render_0_18")]
extern crate bevy_render_018 as bevy_render;

pub mod compat;
pub mod display;
#[cfg(feature = "fps_overlay")]
pub mod fps_overlay;
pub mod input;
pub mod lifecycle;
pub mod logging;
#[cfg(all(target_os = "horizon", feature = "top_down_alloc"))]
mod allocator;
#[cfg(target_os = "horizon")]
mod memory;
#[cfg(all(target_os = "horizon", feature = "vulkan"))]
pub mod nvk_tune;
#[cfg(target_os = "horizon")]
pub mod profiler;
#[cfg(any(feature = "render", feature = "render_0_18"))]
pub mod uploads;
#[cfg(target_os = "horizon")]
pub mod splash;
#[cfg(target_os = "horizon")]
pub mod window;

use bevy_app::{App, Plugin};
pub use compat::{
    ThreadCorePolicy, set_thread_core_policy, set_thread_logging, thread_core_policy, thread_logging,
};
pub use display::{HorizonDisplayMode, HorizonDisplayPlugin};
#[cfg(feature = "fps_overlay")]
pub use fps_overlay::{HorizonFpsOverlayPlugin, fps_chord_held};
pub use input::{HorizonGamepadState, HorizonInputPlugin, RawPad, npad_button, raw_pad};
pub use lifecycle::{HorizonLifecyclePlugin, register_release_hook};
pub use logging::{flush_switch_logs, init_switch_logging};
#[cfg(any(feature = "render", feature = "render_0_18"))]
pub use uploads::{HorizonUploadPlugin, HorizonUploadSettings};

/// One line on the malloc heap, for a game's own logging around heavy loading: what malloc has in
/// use and has taken from the heap (the heartbeat's `[Memory]` line), and how much of the heap
/// nvmap objects hold. On the Switch that one heap holds everything: the game's allocations and,
/// with the `vulkan` feature, every GPU buffer and image, which NVK takes from the top of the heap
/// (bevy-horizon's `patches/mesa-switch/`) and nvservices maps for the GPU. Walks the heap with
/// `svcQueryMemory`, so call it at load stages, not every frame.
#[cfg(target_os = "horizon")]
pub fn memory_report() -> String {
    match memory::nvmap_heap() {
        Some((blocks, bytes)) => format!(
            "{}; {:.1} MiB of the heap is mapped for the GPU (nvmap), in {blocks} blocks",
            memory::malloc_report(),
            bytes as f64 / (1024.0 * 1024.0),
        ),
        None => memory::malloc_report(),
    }
}

/// Gives the small-block cache's blocks back to malloc, for a game that has freed a world's worth
/// at once and is about to make the next, as at a map change. Rust's allocations get the cache's
/// blocks back when malloc runs out, but NVK falling back to malloc for a GPU block doesn't, and
/// the cache holds up to about 40 MiB. Waits up to about 10 ms for each shard or depot slot
/// another thread holds. True if there were blocks to give; without `top_down_alloc` there is no
/// cache.
#[cfg(target_os = "horizon")]
pub fn release_cached_memory() -> bool {
    #[cfg(feature = "top_down_alloc")]
    {
        allocator::release_cache()
    }
    #[cfg(not(feature = "top_down_alloc"))]
    {
        false
    }
}

/// Frame budget for the frame loop: 60 Hz. A frame that finishes early sleeps out the rest.
#[cfg(target_os = "horizon")]
const FRAME_TIME: core::time::Duration = core::time::Duration::from_nanos(1_000_000_000 / 60);

/// Unified plugin for running Bevy applications on Nintendo Switch.
#[derive(Default)]
pub struct HorizonPlugin;

impl Plugin for HorizonPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            HorizonLifecyclePlugin,
            HorizonInputPlugin,
            HorizonDisplayPlugin,
        ));
        #[cfg(any(feature = "render", feature = "render_0_18"))]
        app.add_plugins(HorizonUploadPlugin);

        // Something has to call `App::update` every frame. On desktop that is winit's event
        // loop (`WinitPlugin`), which has no Switch backend, and `DefaultPlugins` only adds
        // `ScheduleRunnerPlugin` when `bevy_window` is off. With neither, `App::run` falls back
        // to `run_once`: one update, then `main` returns and the NRO exits to the menu.
        #[cfg(target_os = "horizon")]
        {
            // Only with BEVY_HORIZON_CPU_PROFILE set (profiler.rs).
            profiler::start_from_env();
            // BH_ALLOC_CACHE=0 turns the allocator's small-block cache off.
            #[cfg(feature = "top_down_alloc")]
            allocator::configure_from_env();
            // ZCULL for Bevy's reversed depth, unless BEVY_HORIZON_ZCULL_DIR=less (nvk_tune.rs).
            #[cfg(feature = "vulkan")]
            nvk_tune::configure_from_env();
            app.add_plugins(window::HorizonWindowPlugin);
            if !app.is_plugin_added::<bevy_app::ScheduleRunnerPlugin>() {
                app.add_plugins(bevy_app::ScheduleRunnerPlugin::run_loop(FRAME_TIME));
            }
        }
    }
}
