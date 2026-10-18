use std::env;

const CPAL_ASIO_DIR: &str = "CPAL_ASIO_DIR";

fn main() {
    println!("cargo:rerun-if-env-changed={CPAL_ASIO_DIR}");
    if env::var(CPAL_ASIO_DIR).is_ok() {
        println!("cargo:rustc-cfg=asio");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("horizon") {
        link_libnx();
    }
}

/// The Horizon host calls libnx's audio renderer (src/host/horizon/mod.rs), so a game that builds
/// cpal links it with no build script of its own.
fn link_libnx() {
    println!("cargo:rustc-link-search=native=/opt/devkitpro/libnx/lib");
    println!("cargo:rustc-link-lib=nx");
}
