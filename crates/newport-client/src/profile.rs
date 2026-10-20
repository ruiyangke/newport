//! The profile directory the window's own preferences live in: the same one
//! the backend opened, so released and development builds behave alike.
use std::{path::PathBuf, sync::OnceLock};

static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Called once at startup with the directory the backend opened.
pub fn set_directory(directory: PathBuf) {
    let _ = DIRECTORY.set(directory);
}

/// The opened profile, or `NEWPORT_DATA_DIR` where no host opened one (tests
/// and tools). None means preferences are kept for this session only.
pub fn directory() -> Option<PathBuf> {
    DIRECTORY.get().cloned().or_else(|| {
        std::env::var_os("NEWPORT_DATA_DIR")
            .filter(|base| !base.is_empty())
            .map(PathBuf::from)
    })
}
