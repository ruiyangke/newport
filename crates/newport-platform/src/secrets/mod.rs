//! OS credential storage for small secrets: protected macOS Keychain items and
//! Windows Credential Manager entries. Naming and policy belong to the backend;
//! these adapters preserve platform access and protection rules.
use zeroize::Zeroizing;

/// Secrets one service keeps, each under its own account name.
pub trait Secrets: Send + Sync {
    fn read(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    /// Saves the value, replacing any earlier one.
    fn write(&self, account: &str, value: &[u8]) -> Result<(), String>;
    /// Removes the item; an absent item is not an error.
    fn delete(&self, account: &str) -> Result<(), String>;
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::SystemStorage;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::SystemStorage;
