//! OS credential storage. Namespace selection and migration policy belong to
//! the backend; these adapters preserve platform access and protection rules.
use zeroize::Zeroizing;

pub trait KeyStorage {
    fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn create(&self, key: &[u8]) -> Result<(), String>;
    fn generate(&self) -> Result<Zeroizing<Vec<u8>>, String>;
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::SystemStorage;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::SystemStorage;
