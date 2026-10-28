//! Encrypted profiles.
pub mod account;
pub mod credentials;
mod instance;
pub(crate) mod leftovers;
#[cfg(test)]
mod manager_tests;
mod snapshot;
mod store;
pub(crate) mod upgrade;
pub mod vault;
pub use instance::Instance;
use std::path::PathBuf;
pub use store::Store;

/// An explicit profile directory for scripts, tests and development builds.
pub fn data_directory_override() -> Option<PathBuf> {
    std::env::var_os("NEWPORT_DATA_DIR").map(PathBuf::from)
}

/// The profile directory this process opens.
pub fn directory() -> Result<PathBuf, String> {
    match data_directory_override() {
        Some(path) => Ok(path),
        None => default_directory(),
    }
}

/// Open the product profile with its OS credential adapter.
pub fn open() -> Result<Store, String> {
    Store::open(directory()?, credentials::system_key)
}

fn default_directory() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let base = dirs::data_local_dir();
    #[cfg(not(windows))]
    let base = dirs::data_dir();
    Ok(base
        .ok_or("Cannot locate application data directory")?
        .join("Newport"))
}
