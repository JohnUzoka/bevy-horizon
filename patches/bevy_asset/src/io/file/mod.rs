#[cfg(feature = "file_watcher")]
mod file_watcher;

// [bevy-horizon patch] Horizon reads and writes files with the synchronous implementation, even
// with `multi_threaded`. The async one goes through async-fs, whose `blocking::Unblock` allocates
// an 8 MiB pipe for each file being read (blocking 1.7.0 `src/lib.rs`) and runs the reads on
// blocking's own pool, which adds threads (2 MiB stacks, from the heap) while reads queue up, to
// 500 by default. Skate 3's first run (on Ryujinx) started 43 threads while loading, then failed an
// 8 MiB allocation, the size of one pipe.
#[cfg(all(feature = "multi_threaded", not(target_os = "horizon")))]
mod file_asset;
#[cfg(any(not(feature = "multi_threaded"), target_os = "horizon"))]
pub(crate) mod sync_file_asset;

#[cfg(feature = "file_watcher")]
pub use file_watcher::*;
use tracing::{debug, error};

use alloc::borrow::ToOwned;
use std::{
    env,
    path::{Path, PathBuf},
};

pub(crate) fn get_base_path() -> PathBuf {
    if let Ok(manifest_dir) = env::var("BEVY_ASSET_ROOT") {
        PathBuf::from(manifest_dir)
    } else if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        PathBuf::from(manifest_dir)
    } else {
        env::current_exe()
            .map(|path| path.parent().map(ToOwned::to_owned).unwrap())
            .unwrap()
    }
}

/// `get_base_path().join(path)`, except that on Horizon a path naming a device (`sdmc:/...`,
/// `romfs:/...`) is used as it is. Such a path is absolute there, but Rust's `Path` counts only a
/// leading `/` as absolute, so `join` appended it to the base instead
/// (`sdmc:/switch/game/sdmc:/switch/game/assets`). [bevy-horizon patch]
pub(crate) fn under_base_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "horizon")]
    if path
        .to_str()
        .and_then(|path| path.split_once(":/"))
        .is_some_and(|(device, _)| !device.is_empty() && !device.contains('/'))
    {
        return path.to_owned();
    }
    get_base_path().join(path)
}

/// I/O implementation for the local filesystem.
///
/// This asset I/O is fully featured but it's not available on `android` and `wasm` targets.
pub struct FileAssetReader {
    root_path: PathBuf,
}

impl FileAssetReader {
    /// Creates a new `FileAssetIo` at a path relative to the executable's directory, optionally
    /// watching for changes.
    ///
    /// See `get_base_path` below.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let root_path = under_base_path(path.as_ref());
        debug!(
            "Asset Server using {} as its base path.",
            root_path.display()
        );
        Self { root_path }
    }

    /// Returns the base path of the assets directory, which is normally the executable's parent
    /// directory.
    ///
    /// To change this, set [`AssetPlugin::file_path`][crate::AssetPlugin::file_path].
    pub fn get_base_path() -> PathBuf {
        get_base_path()
    }

    /// Returns the root directory where assets are loaded from.
    ///
    /// See `get_base_path`.
    pub fn root_path(&self) -> &PathBuf {
        &self.root_path
    }
}

/// A writer for the local filesystem.
pub struct FileAssetWriter {
    root_path: PathBuf,
}

impl FileAssetWriter {
    /// Creates a new [`FileAssetWriter`] at a path relative to the executable's directory, optionally
    /// watching for changes.
    pub fn new<P: AsRef<Path> + core::fmt::Debug>(path: P, create_root: bool) -> Self {
        let root_path = under_base_path(path.as_ref());
        if create_root && let Err(e) = std::fs::create_dir_all(&root_path) {
            error!(
                "Failed to create root directory {} for file asset writer: {}",
                root_path.display(),
                e
            );
        }
        Self { root_path }
    }
}
