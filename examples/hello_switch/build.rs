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
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=notext");
    }
}
