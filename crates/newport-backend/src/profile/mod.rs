//! Encrypted profiles and migration policy, independent of the desktop host.
pub mod account;
pub mod credentials;
mod instance;
pub mod legacy;
#[cfg(test)]
mod manager_tests;
mod migration;
mod snapshot;
mod store;
pub(crate) mod upgrade;
pub mod vault;
pub use instance::Instance;
pub use migration::{
    data_directory_override, import_profiles, profile_directory, remove_plaintext,
};
pub use store::Store;

/// Open the existing product profile with its OS credential adapter. Windowing
/// and UI frameworks are not involved in path selection or key access.
pub fn open() -> Result<Store, String> {
    let directory = match data_directory_override() {
        Some(path) => path,
        None => default_directory()?,
    };
    Store::open(directory, credentials::system_key)
}

pub(crate) fn default_directory() -> Result<std::path::PathBuf, String> {
    #[cfg(windows)]
    let base = dirs::data_local_dir();
    #[cfg(not(windows))]
    let base = dirs::data_dir();
    profile_directory(&base.ok_or("Cannot locate application data directory")?)
}
