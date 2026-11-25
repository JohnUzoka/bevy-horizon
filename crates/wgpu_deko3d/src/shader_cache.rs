//! UAM's output kept on the SD card, so a game compiles a shader on the console only the first time
//! it runs it, as NVK keeps its programs in `sdmc:/.mesa`.
//!
//! A program is a file named by a 128-bit FNV-1a hash of everything that decides UAM's output: the
//! GLSL, the stage, the `libuam.a` this crate links (build.rs hashes it) and [`FORMAT`]. Files are
//! written under a temporary name and renamed into place, and one is used only if its header, key
//! and checksum match. A missing or damaged file, or an SD card error, compiles the shader as if
//! there were no cache, and the error goes to the log.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use super::ffi;

/// Where the cache lives; delete it to clear it. Each UAM build and [`FORMAT`] gets a directory,
/// so a game built with another UAM neither reads nor evicts this one's programs.
const ROOT: &str = "sdmc:/.deko3d/shader_cache";
/// Bump when the file layout or what goes into a key changes.
const FORMAT: u32 = 1;
/// The `libuam.a` this crate links, hashed by build.rs.
const UAM: &str = env!("WGPU_DEKO3D_UAM");
const MAGIC: [u8; 8] = *b"DKSHCACH";
/// Magic, format, stage, key, GLSL length, compile time, DKSH length, DKSH checksum.
const HEADER: usize = 64;
/// Not under `wgpu`, which Bevy's default log filter limits to warnings.
const LOG: &str = "deko3d::shader_cache";

static DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let dir = PathBuf::from(format!("{ROOT}/uam-{UAM}-v{FORMAT}"));
    match fs::create_dir_all(&dir) {
        Ok(()) => Some(dir),
        Err(e) => {
            log::warn!(target: LOG, "can't use {}: {e}; every shader is compiled", dir.display());
            None
        }
    }
});

#[derive(Default)]
struct Stats {
    compiled: u32,
    compiling: Duration,
    read: u32,
    saved: Duration,
}

/// What this launch compiled and read; also serializes writes, which share a temporary name.
static STATS: Mutex<Stats> = Mutex::new(Stats { compiled: 0, compiling: Duration::ZERO, read: 0, saved: Duration::ZERO });

/// The DKSH for `glsl`: read from the cache, or made by `compile` and then stored.
pub fn get_or_compile(stage: i32, glsl: &str, compile: impl FnOnce() -> Result<Arc<[u8]>, String>) -> Result<Arc<[u8]>, String> {
    let name = match stage {
        ffi::UAM_VERTEX => "vertex",
        ffi::UAM_FRAGMENT => "fragment",
        _ => "compute",
    };
    let key = key(stage, glsl);
    let path = DIR.as_ref().map(|dir| dir.join(format!("{key:032x}.dksh")));
    if let Some(path) = &path {
        let start = Instant::now();
        if let Some((dksh, compiled_in)) = load(path, key, stage, glsl.len()) {
            let took = start.elapsed();
            let mut stats = STATS.lock().unwrap();
            stats.read += 1;
            stats.saved += compiled_in.saturating_sub(took);
            log::info!(target: LOG, "{name} {key:032x}: read {} bytes in {:.1} ms instead of compiling for {:.0} ms; \
                       {} read and {} compiled this launch, {:.2} s saved",
                       dksh.len(), ms(took), ms(compiled_in), stats.read, stats.compiled, stats.saved.as_secs_f64());
            return Ok(dksh);
        }
    }

    let start = Instant::now();
    let dksh = compile()?;
    let took = start.elapsed();
    let mut stats = STATS.lock().unwrap();
    stats.compiled += 1;
    stats.compiling += took;
    let stored = path.as_ref().is_some_and(|path| {
        store(path, key, stage, glsl.len(), took, &dksh)
            .inspect_err(|e| log::warn!(target: LOG, "storing {}: {e}", path.display()))
            .is_ok()
    });
    log::info!(target: LOG, "{name} {key:032x}: compiled {} bytes of GLSL in {:.0} ms{}; \
               {} read and {} compiled this launch, {:.2} s compiling",
               glsl.len(), ms(took), if stored { " and stored it" } else { "" },
               stats.read, stats.compiled, stats.compiling.as_secs_f64());
    Ok(dksh)
}

fn load(path: &Path, key: u128, stage: i32, glsl_len: usize) -> Option<(Arc<[u8]>, Duration)> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!(target: LOG, "reading {}: {e}; compiling instead", path.display());
            return None;
        }
    };
    match parse(&bytes, key, stage, glsl_len) {
        Ok(entry) => Some(entry),
        Err(problem) => {
            log::warn!(target: LOG, "{} {problem}; compiling it again", path.display());
            let _ = fs::remove_file(path);
            None
        }
    }
}

fn parse(bytes: &[u8], key: u128, stage: i32, glsl_len: usize) -> Result<(Arc<[u8]>, Duration), &'static str> {
    let (header, dksh) = bytes.split_at_checked(HEADER).ok_or("is shorter than its header")?;
    let u64_at = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
    if header[..8] != MAGIC || header[8..12] != FORMAT.to_le_bytes() {
        return Err("is not a cache file of this format");
    }
    if header[12..16] != stage.to_le_bytes() || header[16..32] != key.to_le_bytes() || u64_at(32) != glsl_len as u64 {
        return Err("holds another program");
    }
    if u64_at(48) != dksh.len() as u64 {
        return Err("is truncated");
    }
    if u64_at(56) != fnv64(dksh) {
        return Err("fails its checksum");
    }
    Ok((Arc::from(dksh), Duration::from_nanos(u64_at(40))))
}

/// Writes a temporary file and renames it into place, so a reader never sees half a program.
/// The caller holds `STATS`, so no other thread writes the same temporary file.
fn store(path: &Path, key: u128, stage: i32, glsl_len: usize, took: Duration, dksh: &[u8]) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(HEADER + dksh.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&FORMAT.to_le_bytes());
    bytes.extend_from_slice(&stage.to_le_bytes());
    bytes.extend_from_slice(&key.to_le_bytes());
    bytes.extend_from_slice(&(glsl_len as u64).to_le_bytes());
    bytes.extend_from_slice(&(took.as_nanos() as u64).to_le_bytes());
    bytes.extend_from_slice(&(dksh.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&fnv64(dksh).to_le_bytes());
    bytes.extend_from_slice(dksh);
    let temp = path.with_extension("tmp");
    fs::write(&temp, &bytes)?;
    // libnx's rename (fsFsRenameFile) won't replace a file, such as one another thread stored.
    if fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(path);
        if let Err(e) = fs::rename(&temp, path) {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
    }
    Ok(())
}

/// The cache key of `glsl` compiled for `stage`, which names its file.
pub fn key(stage: i32, glsl: &str) -> u128 {
    fnv128(&[&FORMAT.to_le_bytes()[..], UAM.as_bytes(), &stage.to_le_bytes(), glsl.as_bytes()])
}

fn fnv128(parts: &[&[u8]]) -> u128 {
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    parts.iter().flat_map(|part| part.iter()).fold(0x6c62_272e_07bb_0142_62b8_2175_6295_c58d, |hash, &byte| (hash ^ byte as u128).wrapping_mul(PRIME))
}

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}
