use std::path::PathBuf;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "horizon" {
        println!("cargo:rustc-link-search=native=/opt/devkitpro/libnx/lib");
        println!("cargo:rustc-link-search=native=/opt/devkitpro/portlibs/switch/lib");
        println!("cargo:rustc-link-search=native=/opt/devkitpro/devkitA64/aarch64-none-elf/lib/pic");
        println!("cargo:rustc-link-search=native=/opt/devkitpro/devkitA64/lib/gcc/aarch64-none-elf/16.1.0/pic");
        println!("cargo:rustc-link-lib=nx");
        println!("cargo:rustc-link-lib=sysbase");
        println!("cargo:rustc-link-lib=c");
        println!("cargo:rustc-link-lib=m");
        println!("cargo:rustc-link-lib=gcc");
        if std::env::var_os("CARGO_FEATURE_VULKAN").is_some() {
            link_nvk();
        }
    }
}

/// Links NVK as scripts/localize_nvk.sh leaves it: one object whose only global definitions are
/// the vk* entry points, the top-down region's `nouveau_horizon_region_{alloc,free,stats}`
/// (memory.rs and the `top_down_alloc` allocator call these) and the measurement switches'
/// `nouveau_horizon_tune` (nvk_tune.rs), so its own copies of Rust's std and
/// of Mesa's helpers can't clash with the game's. What it still needs (nvk/lib/undefined_bh.txt)
/// comes from libnx, newlib, libstdc++, expat, zstd and zlib, all in devkitPro's toolchain and
/// portlibs.
fn link_nvk() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=BEVY_HORIZON_NVK_LIB");
    let dir = match std::env::var_os("BEVY_HORIZON_NVK_LIB") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../nvk/lib"),
    };
    let lib = dir.join("libvulkan_bh.a");
    if !lib.is_file() {
        panic!(
            "bevy_horizon's `vulkan` feature links {}, which is missing: run scripts/build_nvk.sh \
             and scripts/localize_nvk.sh, or set BEVY_HORIZON_NVK_LIB to the directory that has it",
            lib.display()
        );
    }
    println!("cargo:rerun-if-changed={}", lib.display());
    println!("cargo:rustc-link-search=native={}", dir.display());
    for name in ["vulkan_bh", "expat", "zstd", "z", "stdc++"] {
        println!("cargo:rustc-link-lib={name}");
    }
}
