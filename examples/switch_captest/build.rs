fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "horizon" {
        return;
    }
    for dir in [
        "/opt/devkitpro/libnx/lib",
        "/opt/devkitpro/portlibs/switch/lib",
        "/opt/devkitpro/devkitA64/aarch64-none-elf/lib/pic",
        "/opt/devkitpro/devkitA64/lib/gcc/aarch64-none-elf/16.1.0/pic",
    ] {
        println!("cargo:rustc-link-search=native={dir}");
    }
    // The `gpu` feature's OpenGL ES probe needs devkitPro's Mesa (EGL, GLESv2, glapi, nouveau) and
    // the libraries it uses. rust-lld only pulls archive members that are referenced.
    for lib in [
        "EGL", "GLESv2", "glapi", "drm_nouveau", "stdc++", "pthread", "nx", "sysbase", "c", "m", "gcc",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=notext");
}
