//! NVK's measurement switches (`patches/mesa-switch/0008-nvk-switch-tune.patch`, `0009` and
//! `0012`): changes to how NVK drives the GPU that a game can make between frames, to time a frame
//! with and without each in one launch. They are for measuring: `EarlyZ` draws a wrong picture.
//!
//! One is also a default: at launch `configure_from_env`, which `HorizonPlugin` calls, sets
//! `ZcullDir` to 1, `GREATER`, for Bevy's reversed depth. `BEVY_HORIZON_ZCULL_DIR=less` keeps
//! NVK's own `LESS` instead.

/// One of NVK's switches. Each takes -1, NVK's own behaviour and the default, or the values given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Knob {
    /// `SET_OPPORTUNISTIC_EARLY_Z_HYSTERESIS`, set as each render pass begins: 0 (instantaneous),
    /// 1-19 (an accumulated primitive area of `16 << (n - 1)` pixels) or 31 (late Z always).
    /// NVK doesn't set it otherwise.
    EarlyZHysteresis = 0,
    /// 0 turns ZCULL off in every render pass.
    Zcull = 1,
    /// `SET_ZCULL_SERIALIZATION`, set as each render pass begins: `ENABLE` in bit 0, `APPLIED` in
    /// bits 4-5.
    ZcullSerialization = 2,
    /// `SET_API_MANDATED_EARLY_Z` after each fragment shader's own: 1 tests depth before every
    /// fragment shader runs, even one that can discard.
    EarlyZ = 3,
    /// ZCULL's direction, `SET_ZCULL_DIR_FORMAT`'s `ZDIR`, set as each render pass begins: 0
    /// `LESS`, which NVK always sets, or 1 `GREATER`. Bevy draws with reversed depth (cleared to
    /// 0.0 and tested `GreaterEqual`), and with `LESS` ZCULL rejects nothing in such a pass.
    /// Change it only between frames, before the render world records a frame's passes: a
    /// render pass that loads depth must see the direction the frame's clear saw (NVK clears
    /// ZCULL instead of loading data built with the other direction, so a frame that mixes the
    /// two loses ZCULL's culling but draws the same picture).
    ZcullDir = 4,
    /// 0 sets up every indirect draw, as NVK does without `patches/mesa-switch/0012`; -1, the
    /// default, skips the setup of an indirect draw with no instances. Bevy's GPU culling
    /// writes a culled batch's draw with an instance count of 0 and, on an NVK without
    /// `drawIndirectCount` (Maxwell), still issues it. Read as each indirect draw is recorded;
    /// the picture is the same either way.
    SkipEmptyDraws = 5,
}

/// Sets ZCULL's direction: `GREATER` (`ZcullDir` 1) by default, or NVK's own `LESS` (0) with
/// `BEVY_HORIZON_ZCULL_DIR=less`. `greater` is accepted too; anything else is reported and the
/// default kept. Bevy's own pipelines test depth with `Greater`, `GreaterEqual`, `Equal` or
/// `Always`, never `Less` (bevy_pbr, bevy_core_pipeline, bevy_sprite_render and
/// bevy_gizmos_render 0.19.1), and `GREATER` made Skate 3's frames 0.15 ms faster with the same
/// picture (sk8 findings 31). Once, before the first frame; `HorizonPlugin` calls it.
pub fn configure_from_env() {
    let value = std::env::var("BEVY_HORIZON_ZCULL_DIR").ok();
    let (dir, source) = match value.as_deref().map(str::trim) {
        None => (1, "default; BEVY_HORIZON_ZCULL_DIR=less for NVK's own"),
        Some("greater") => (1, "BEVY_HORIZON_ZCULL_DIR"),
        Some("less") => (0, "BEVY_HORIZON_ZCULL_DIR"),
        Some(other) => {
            tracing::warn!("BEVY_HORIZON_ZCULL_DIR={other}: not `greater` or `less`; keeping `greater`");
            (1, "default")
        }
    };
    if set(Knob::ZcullDir, dir) {
        let name = if dir == 1 { "greater" } else { "less" };
        tracing::info!("[NVK] ZCULL direction: {name} ({source})");
    } else {
        tracing::warn!("[NVK] this NVK has no ZCULL direction switch (patches/mesa-switch/0009)");
    }
}

unsafe extern "C" {
    fn nouveau_horizon_tune(knob: u32, value: i32) -> bool;
}

/// Sets `knob` from the next render pass or fragment shader on. The two methods set as a render
/// pass begins are GPU context state, so going back to -1 leaves the last value in place: set the
/// value to go back to instead. False, changing nothing, for a value the switch can't take.
pub fn set(knob: Knob, value: i32) -> bool {
    unsafe { nouveau_horizon_tune(knob as u32, value) }
}
