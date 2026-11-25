use std::{env, path::PathBuf, process::Command};

/// Compiles native/bridge.c and links deko3d and UAM (scripts/build_uam.sh) on Horizon.
fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("horizon") {
        return;
    }
    println!("cargo:rerun-if-changed=native/bridge.c");
    println!("cargo:rerun-if-env-changed=BEVY_HORIZON_UAM_LIB");
    println!("cargo:rerun-if-env-changed=BEVY_HORIZON_DEKO3D_DEBUG");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("bridge.o");
    let status = Command::new("aarch64-none-elf-gcc")
        .args(["-march=armv8-a+crc+crypto", "-mtune=cortex-a57", "-mtp=soft", "-fPIE", "-D__SWITCH__", "-O2",
               "-Wall", "-Wextra", "-Werror", "-I/opt/devkitpro/libnx/include", "-c", "native/bridge.c", "-o"])
        .arg(&object)
        .status()
        .expect("run aarch64-none-elf-gcc (devkitA64)");
    assert!(status.success(), "compiling native/bridge.c failed");
    let archive = out.join("libdeko3d_bridge.a");
    let _ = std::fs::remove_file(&archive);
    let status = Command::new("aarch64-none-elf-ar").arg("crs").arg(&archive).arg(&object).status().expect("run aarch64-none-elf-ar");
    assert!(status.success(), "archiving the bridge failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=deko3d_bridge");

    let uam = match env::var_os("BEVY_HORIZON_UAM_LIB") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../deko3d/lib"),
    };
    let lib = uam.join("libuam.a");
    if !lib.is_file() {
        panic!("wgpu_deko3d links {}, which is missing: run scripts/build_uam.sh, or set BEVY_HORIZON_UAM_LIB \
                to the directory that has it", lib.display());
    }
    println!("cargo:rerun-if-changed={}", lib.display());
    println!("cargo:rustc-link-search=native={}", uam.display());
    println!("cargo:rustc-link-search=native=/opt/devkitpro/libnx/lib");
    println!("cargo:rustc-link-search=native=/opt/devkitpro/devkitA64/aarch64-none-elf/lib/pic");
    // BEVY_HORIZON_DEKO3D_DEBUG=1 links deko3d's debug library, which validates every call and
    // reports misuse through the bridge's log (sdmc:/switch/deko3d.log).
    let deko3d = if env::var_os("BEVY_HORIZON_DEKO3D_DEBUG").is_some() { "deko3dd" } else { "deko3d" };
    for name in ["uam", deko3d] {
        println!("cargo:rustc-link-lib=static={name}");
    }
    // UAM is C++; the final link resolves libstdc++ (as bevy_horizon's NVK link does).
    println!("cargo:rustc-link-lib=stdc++");
}
